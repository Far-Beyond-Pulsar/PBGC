use crate::bytecode::{parse_bp_const, BpProgram, BpValue, Instruction, LabelId, SlotId};
use crate::metadata::BlueprintMetadataProvider;
use graphy::core::NodeMetadataProvider;
use graphy::{DataResolver, ExecutionRouting, GraphDescription, GraphyError, NodeInstance, NodeTypes};
use std::collections::{HashMap, HashSet};

/// Compiles a Blueprint graph into a flat bytecode program for each event entry-point.
pub struct BytecodeCodegen<'a> {
    graph: &'a GraphDescription,
    metadata_provider: &'a BlueprintMetadataProvider,
    data_resolver: &'a DataResolver,
    exec_routing: &'a ExecutionRouting,

    instructions: Vec<Instruction>,
    /// node_id → slot holding that node's output value
    output_slots: HashMap<String, SlotId>,
    /// dedup cache for constants: (node_id, pin_id, raw_str) → slot
    const_slots: HashMap<String, SlotId>,

    next_slot: SlotId,
    next_label: LabelId,
    visited: HashSet<String>,
}

impl<'a> BytecodeCodegen<'a> {
    pub fn new(
        graph: &'a GraphDescription,
        metadata_provider: &'a BlueprintMetadataProvider,
        data_resolver: &'a DataResolver,
        exec_routing: &'a ExecutionRouting,
    ) -> Self {
        Self {
            graph,
            metadata_provider,
            data_resolver,
            exec_routing,
            instructions: Vec::new(),
            output_slots: HashMap::new(),
            const_slots: HashMap::new(),
            next_slot: 0,
            next_label: 0,
            visited: HashSet::new(),
        }
    }

    fn alloc_slot(&mut self) -> SlotId {
        let s = self.next_slot;
        self.next_slot += 1;
        s
    }

    fn alloc_label(&mut self) -> LabelId {
        let l = self.next_label;
        self.next_label += 1;
        l
    }

    /// Compile every event node in the graph into its own `BpProgram`.
    pub fn generate_programs(&mut self) -> Result<Vec<BpProgram>, GraphyError> {
        let event_nodes: Vec<NodeInstance> = self
            .graph
            .nodes
            .values()
            .filter(|n| {
                self.metadata_provider
                    .get_node_metadata(&n.node_type)
                    .map(|m| m.node_type == NodeTypes::event)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        if event_nodes.is_empty() {
            return Err(GraphyError::CodeGeneration(
                "No event nodes found in graph".to_string(),
            ));
        }

        let mut programs = Vec::new();
        for event in event_nodes {
            let prog = self.compile_event(&event)?;
            programs.push(prog);
        }
        Ok(programs)
    }

    fn compile_event(&mut self, event: &NodeInstance) -> Result<BpProgram, GraphyError> {
        // Each event gets a fresh instruction buffer and visited set; slots are shared
        // across events (they share the slot space for the whole graph compile session).
        let prev_instructions = std::mem::take(&mut self.instructions);
        let prev_visited = std::mem::take(&mut self.visited);

        // Preamble: emit pure nodes that need to be materialized (used more than once)
        self.emit_pure_preamble()?;

        // Walk exec chain from each exec output of the event node
        for output_pin in &event.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Execution) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&event.id, &output_pin.id)
                    .to_vec();
                for next_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(&next_id).cloned() {
                        self.emit_exec_node(&next_node)?;
                    }
                }
            }
        }

        self.instructions.push(Instruction::Return);

        let meta = self
            .metadata_provider
            .get_node_metadata(&event.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(event.node_type.clone()))?;

        let prog_instructions = std::mem::replace(&mut self.instructions, prev_instructions);
        self.visited = prev_visited;

        let mut prog = BpProgram::new(meta.name.clone());
        prog.instructions = prog_instructions;
        prog.slot_count = self.next_slot;
        Ok(prog)
    }

    // ── Pure preamble ────────────────────────────────────────────────────────────

    /// Emit ALL pure nodes in topological order into slots.
    ///
    /// Processing every node (not just multi-use ones) has two benefits:
    ///  1. Eliminates O(n²) use-count scanning — we never need `should_materialize`.
    ///  2. Eliminates recursive `emit_pure_node` chains — deep graphs (500k+ nodes)
    ///     no longer risk stack overflow because every dependency is already slotted
    ///     before its consumer is processed.
    ///
    /// The trade-off: single-use pure nodes get a dedicated slot instead of being
    /// inlined as sub-expressions. In bytecode mode this is always acceptable — the
    /// VM is slot-based anyway, and we avoid a second O(n) pass over the graph.
    fn emit_pure_preamble(&mut self) -> Result<(), GraphyError> {
        for node_id in self.data_resolver.get_pure_evaluation_order() {
            let node_id = node_id.to_string();
            if self.output_slots.contains_key(&node_id) {
                continue;
            }
            let node = match self.graph.nodes.get(&node_id).cloned() {
                Some(n) => n,
                None => continue,
            };
            let meta = match self.metadata_provider.get_node_metadata(&node.node_type) {
                Some(m) => m.clone(),
                None => continue,
            };
            if meta.node_type != NodeTypes::pure {
                continue;
            }
            // All inputs are guaranteed to be in output_slots already (topological order)
            let input_slots = self.collect_input_slots_for(&node, &meta)?;
            let output_slot = if meta.return_type.is_some() {
                let s = self.alloc_slot();
                self.output_slots.insert(node_id.clone(), s);
                Some(s)
            } else {
                None
            };
            self.instructions.push(Instruction::Call {
                node_type: meta.name.to_string(),
                inputs: input_slots,
                output: output_slot,
            });
        }
        Ok(())
    }

    // ── Exec chain ───────────────────────────────────────────────────────────────

    fn emit_exec_node(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        if self.visited.contains(&node.id) {
            return Ok(());
        }
        self.visited.insert(node.id.clone());

        // Variable getter/setter special handling
        if node.node_type.starts_with("get_") {
            return Ok(()); // pure, already in preamble
        }
        if node.node_type.starts_with("set_") {
            return self.emit_setter(node);
        }

        let meta = self
            .metadata_provider
            .get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?
            .clone();

        match meta.node_type {
            NodeTypes::pure | NodeTypes::event => Ok(()),
            NodeTypes::fn_ => self.emit_fn_node(node, &meta),
            NodeTypes::control_flow => self.emit_control_flow(node, &meta),
        }
    }

    fn emit_fn_node(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<(), GraphyError> {
        let input_slots = self.collect_input_slots_for(node, meta)?;

        let output_slot = if meta.return_type.is_some() {
            let s = self.alloc_slot();
            self.output_slots.insert(node.id.clone(), s);
            Some(s)
        } else {
            None
        };

        self.instructions.push(Instruction::Call {
            node_type: meta.name.to_string(),
            inputs: input_slots,
            output: output_slot,
        });

        self.follow_exec_outputs(node)
    }

    fn emit_setter(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        let var_name = node
            .node_type
            .strip_prefix("set_")
            .ok_or_else(|| GraphyError::Custom(format!("Bad setter: {}", node.node_type)))?;

        let value_pin_id = node
            .inputs
            .iter()
            .find(|p| p.pin.name == "value")
            .map(|p| p.id.clone())
            .ok_or_else(|| {
                GraphyError::Custom(format!("No value pin on setter '{}'", node.id))
            })?;

        let value_slot = self.resolve_input_slot(&node.id, &value_pin_id)?;

        // Emit as a call to the synthetic "set_<var>" node type; the engine's dispatch
        // handles it by writing to the variable store.
        self.instructions.push(Instruction::Call {
            node_type: format!("set_{}", var_name),
            inputs: vec![value_slot],
            output: None,
        });

        self.follow_exec_outputs(node)
    }

    fn emit_control_flow(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<(), GraphyError> {
        let exec_out_pins: Vec<_> = node
            .outputs
            .iter()
            .filter(|p| matches!(p.pin.data_type, graphy::DataType::Execution))
            .collect();

        // ── Branch pattern: exactly 2 exec outputs, 1 bool-ish data input ────────
        let data_inputs: Vec<_> = node
            .inputs
            .iter()
            .filter(|p| !matches!(p.pin.data_type, graphy::DataType::Execution))
            .collect();

        if exec_out_pins.len() == 2 && data_inputs.len() == 1 {
            return self.emit_two_way_branch(node, meta, &data_inputs, &exec_out_pins);
        }

        // ── Sequential pattern: N exec outputs, 0 data inputs (e.g. sequence) ───
        if data_inputs.is_empty() && exec_out_pins.len() > 1 {
            return self.emit_sequential(node, meta, &exec_out_pins);
        }

        // ── Generic fallback: treat as a regular fn call ──────────────────────────
        self.emit_fn_node(node, meta)
    }

    fn emit_two_way_branch(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
        data_inputs: &[&graphy::PinInstance],
        exec_out_pins: &[&graphy::PinInstance],
    ) -> Result<(), GraphyError> {
        // First, resolve any required data inputs for the node itself (e.g. multi_branch has 3 conditions).
        // For the simple branch case we only need the single condition.
        let cond_slot = self.resolve_input_slot(&node.id, &data_inputs[0].id)?;

        let true_label = self.alloc_label();
        let false_label = self.alloc_label();
        let end_label = self.alloc_label();

        self.instructions.push(Instruction::JumpIf {
            condition: cond_slot,
            true_label,
            false_label,
        });

        // True branch (first exec output)
        self.instructions.push(Instruction::Label(true_label));
        let true_connected = self
            .exec_routing
            .get_connected_nodes(&node.id, &exec_out_pins[0].id)
            .to_vec();
        let saved_visited = self.visited.clone();
        for nid in true_connected {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                self.emit_exec_node(&n)?;
            }
        }
        self.instructions.push(Instruction::Jump(end_label));

        // False branch (second exec output)
        self.instructions.push(Instruction::Label(false_label));
        self.visited = saved_visited; // allow false branch to visit same nodes
        let false_connected = self
            .exec_routing
            .get_connected_nodes(&node.id, &exec_out_pins[1].id)
            .to_vec();
        for nid in false_connected {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                self.emit_exec_node(&n)?;
            }
        }

        self.instructions.push(Instruction::Label(end_label));
        let _ = meta; // used for type dispatch above
        Ok(())
    }

    fn emit_sequential(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
        exec_out_pins: &[&graphy::PinInstance],
    ) -> Result<(), GraphyError> {
        for pin in exec_out_pins {
            let connected = self
                .exec_routing
                .get_connected_nodes(&node.id, &pin.id)
                .to_vec();
            for nid in connected {
                if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                    self.emit_exec_node(&n)?;
                }
            }
        }
        let _ = meta;
        Ok(())
    }

    fn follow_exec_outputs(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        for pin in &node.outputs {
            if matches!(pin.pin.data_type, graphy::DataType::Execution) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&node.id, &pin.id)
                    .to_vec();
                for nid in connected {
                    if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                        self.emit_exec_node(&n)?;
                    }
                }
            }
        }
        Ok(())
    }

    // ── Input resolution ─────────────────────────────────────────────────────────

    fn collect_input_slots_for(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<Vec<SlotId>, GraphyError> {
        let mut slots = Vec::new();
        for param in &meta.params {
            let pin_id = node
                .inputs
                .iter()
                .find(|p| p.pin.name == param.name)
                .map(|p| p.id.clone())
                .ok_or_else(|| {
                    GraphyError::Custom(format!(
                        "Input pin '{}' not found on node '{}'",
                        param.name, node.id
                    ))
                })?;
            slots.push(self.resolve_input_slot(&node.id, &pin_id)?);
        }
        Ok(slots)
    }

    fn resolve_input_slot(&mut self, node_id: &str, pin_id: &str) -> Result<SlotId, GraphyError> {
        use graphy::analysis::DataSource;

        match self.data_resolver.get_input_source(node_id, pin_id) {
            Some(DataSource::Connection { source_node_id, .. }) => {
                let sid = source_node_id.to_string();
                // All pure nodes are pre-slotted by emit_pure_preamble (topological order).
                // fn_/setter nodes are slotted when their Call is emitted in the exec chain.
                self.output_slots
                    .get(&sid)
                    .copied()
                    .ok_or_else(|| GraphyError::Custom(format!("No slot for node '{}' — pure preamble may not have reached it", sid)))
            }
            Some(DataSource::Constant(val)) => {
                let key = format!("{}:{}:{}", node_id, pin_id, val);
                if let Some(&s) = self.const_slots.get(&key) {
                    return Ok(s);
                }
                let bp_val = parse_bp_const(&val);
                let slot = self.alloc_slot();
                self.instructions
                    .push(Instruction::LoadConst { slot, value: bp_val });
                self.const_slots.insert(key, slot);
                Ok(slot)
            }
            Some(DataSource::Default) => {
                let node = self
                    .graph
                    .nodes
                    .get(node_id)
                    .ok_or_else(|| GraphyError::NodeNotFound(node_id.to_string()))?;
                let pin = node
                    .inputs
                    .iter()
                    .find(|p| p.id == pin_id)
                    .ok_or_else(|| GraphyError::PinNotFound {
                        node: node_id.to_string(),
                        pin: pin_id.to_string(),
                    })?;
                let default = default_bp_value(&pin.pin.data_type);
                let slot = self.alloc_slot();
                self.instructions
                    .push(Instruction::LoadConst { slot, value: default });
                Ok(slot)
            }
            None => Err(GraphyError::Custom(format!(
                "No data source for {}.{}",
                node_id, pin_id
            ))),
        }
    }
}

fn default_bp_value(dt: &graphy::DataType) -> BpValue {
    match dt {
        graphy::DataType::Boolean => BpValue::Bool(false),
        graphy::DataType::Number => BpValue::Float(0.0),
        graphy::DataType::String => BpValue::Str(String::new()),
        graphy::DataType::Typed(ti) => match ti.type_string.as_str() {
            "bool" => BpValue::Bool(false),
            "f32" | "f64" => BpValue::Float(0.0),
            "i32" | "i64" | "u32" | "u64" | "usize" | "isize" => BpValue::Int(0),
            "String" | "&str" => BpValue::Str(String::new()),
            _ => BpValue::Null,
        },
        _ => BpValue::Null,
    }
}
