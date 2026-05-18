use crate::bytecode::{BpProgram, Instruction, LabelId, SlotId};
use crate::metadata::BlueprintMetadataProvider;
use graphy::core::NodeMetadataProvider;
use graphy::{DataResolver, ExecutionRouting, GraphDescription, GraphyError, NodeInstance, NodeTypes, DataType};
use std::collections::{HashMap, HashSet};

pub struct BytecodeCodegen<'a> {
    graph: &'a GraphDescription,
    metadata_provider: &'a BlueprintMetadataProvider,
    data_resolver: &'a DataResolver,
    exec_routing: &'a ExecutionRouting,

    instructions: Vec<Instruction>,
    output_slots: HashMap<String, SlotId>,
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
            graph, metadata_provider, data_resolver, exec_routing,
            instructions: Vec::new(),
            output_slots: HashMap::new(),
            const_slots: HashMap::new(),
            next_slot: 0,
            next_label: 0,
            visited: HashSet::new(),
        }
    }


    fn alloc_slot(&mut self) -> SlotId {
        let s = self.next_slot; self.next_slot += 1; s
    }

    fn alloc_label(&mut self) -> LabelId {
        let l = self.next_label; self.next_label += 1; l
    }

    pub fn generate_programs(&mut self) -> Result<Vec<BpProgram>, GraphyError> {
        let event_nodes: Vec<NodeInstance> = self.graph.nodes.values()
            .filter(|n| self.metadata_provider.get_node_metadata(&n.node_type)
                .map(|m| m.node_type == NodeTypes::event).unwrap_or(false))
            .cloned().collect();

        if event_nodes.is_empty() {
            return Err(GraphyError::CodeGeneration("No event nodes found in graph".to_string()));
        }

        let mut programs = Vec::new();
        for event in event_nodes {
            programs.push(self.compile_event(&event)?);
        }
        Ok(programs)
    }

    fn compile_event(&mut self, event: &NodeInstance) -> Result<BpProgram, GraphyError> {
        let prev_instructions = std::mem::take(&mut self.instructions);
        let prev_visited = std::mem::take(&mut self.visited);

        self.emit_pure_preamble()?;

        for pin in &event.outputs {
            if matches!(pin.pin.data_type, DataType::Execution) {
                for nid in self.exec_routing.get_connected_nodes(&event.id, &pin.id).to_vec() {
                    if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                        self.emit_exec_node(&n)?;
                    }
                }
            }
        }
        self.instructions.push(Instruction::Return);

        let meta = self.metadata_provider.get_node_metadata(&event.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(event.node_type.clone()))?;

        let instrs = std::mem::replace(&mut self.instructions, prev_instructions);
        self.visited = prev_visited;

        let mut prog = BpProgram::new(meta.name.clone());
        prog.instructions = instrs;
        prog.slot_count = self.next_slot;
        Ok(prog)
    }

    // ── Pure preamble — O(n), no recursion, no enum ───────────────────────────

    fn emit_pure_preamble(&mut self) -> Result<(), GraphyError> {
        for node_id in self.data_resolver.get_pure_evaluation_order() {
            let node_id = node_id.to_string();
            if self.output_slots.contains_key(&node_id) { continue; }
            let node = match self.graph.nodes.get(&node_id).cloned() { Some(n) => n, None => continue };
            let meta = match self.metadata_provider.get_node_metadata(&node.node_type) {
                Some(m) => m.clone(), None => continue
            };
            if meta.node_type != NodeTypes::pure { continue; }

            let input_slots = self.collect_input_slots_for(&node, &meta)?;
            let output_slot = if meta.return_type.is_some() {
                let s = self.alloc_slot();
                self.output_slots.insert(node_id.clone(), s);
                Some(s)
            } else { None };

            self.instructions.push(Instruction::Call { fn_ptr: 0, node_type: meta.name.to_string(), inputs: input_slots, output: output_slot });
        }
        Ok(())
    }

    // ── Exec chain ────────────────────────────────────────────────────────────

    fn emit_exec_node(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        if self.visited.contains(&node.id) { return Ok(()); }
        self.visited.insert(node.id.clone());

        if node.node_type.starts_with("set_") { return self.emit_setter(node); }

        let meta = self.metadata_provider.get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?.clone();

        match meta.node_type {
            NodeTypes::pure | NodeTypes::event => Ok(()),
            NodeTypes::fn_ => self.emit_fn_node(node, &meta),
            NodeTypes::control_flow => self.emit_control_flow(node, &meta),
        }
    }

    fn emit_fn_node(&mut self, node: &NodeInstance, meta: &graphy::core::NodeMetadata) -> Result<(), GraphyError> {
        let inputs = self.collect_input_slots_for(node, meta)?;
        let output = if meta.return_type.is_some() {
            let s = self.alloc_slot();
            self.output_slots.insert(node.id.clone(), s);
            Some(s)
        } else { None };

        self.instructions.push(Instruction::Call { fn_ptr: 0, node_type: meta.name.to_string(), inputs, output });
        self.follow_exec_outputs(node)
    }

    fn emit_setter(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        let var_name = node.node_type.strip_prefix("set_")
            .ok_or_else(|| GraphyError::Custom(format!("Bad setter: {}", node.node_type)))?;

        let val_pin = node.inputs.iter().find(|p| p.pin.name == "value")
            .ok_or_else(|| GraphyError::Custom(format!("No value pin on setter '{}'", node.id)))?;
        let val_slot = self.resolve_input_slot(&node.id, &val_pin.id)?;

        let setter_name = format!("set_{}", var_name);
        self.instructions.push(Instruction::Call { fn_ptr: 0, node_type: setter_name, inputs: vec![val_slot], output: None });
        self.follow_exec_outputs(node)
    }

    fn emit_control_flow(&mut self, node: &NodeInstance, meta: &graphy::core::NodeMetadata) -> Result<(), GraphyError> {
        let exec_pins: Vec<_> = node.outputs.iter()
            .filter(|p| matches!(p.pin.data_type, DataType::Execution)).collect();
        let data_ins: Vec<_> = node.inputs.iter()
            .filter(|p| !matches!(p.pin.data_type, DataType::Execution)).collect();

        if exec_pins.len() == 2 && data_ins.len() == 1 {
            return self.emit_two_way_branch(node, &data_ins, &exec_pins);
        }
        if data_ins.is_empty() && exec_pins.len() > 1 {
            return self.emit_sequential(node, &exec_pins);
        }
        self.emit_fn_node(node, meta)
    }

    fn emit_two_way_branch(
        &mut self, node: &NodeInstance,
        data_ins: &[&graphy::PinInstance], exec_pins: &[&graphy::PinInstance],
    ) -> Result<(), GraphyError> {
        let cond_slot = self.resolve_input_slot(&node.id, &data_ins[0].id)?;
        let true_lbl  = self.alloc_label();
        let false_lbl = self.alloc_label();
        let end_lbl   = self.alloc_label();

        self.instructions.push(Instruction::JumpIf { condition: cond_slot, true_label: true_lbl, false_label: false_lbl });

        self.instructions.push(Instruction::Label(true_lbl));
        let saved = self.visited.clone();
        for nid in self.exec_routing.get_connected_nodes(&node.id, &exec_pins[0].id).to_vec() {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() { self.emit_exec_node(&n)?; }
        }
        self.instructions.push(Instruction::Jump(end_lbl));

        self.instructions.push(Instruction::Label(false_lbl));
        self.visited = saved;
        for nid in self.exec_routing.get_connected_nodes(&node.id, &exec_pins[1].id).to_vec() {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() { self.emit_exec_node(&n)?; }
        }

        self.instructions.push(Instruction::Label(end_lbl));
        Ok(())
    }

    fn emit_sequential(&mut self, node: &NodeInstance, exec_pins: &[&graphy::PinInstance]) -> Result<(), GraphyError> {
        for pin in exec_pins {
            for nid in self.exec_routing.get_connected_nodes(&node.id, &pin.id).to_vec() {
                if let Some(n) = self.graph.nodes.get(&nid).cloned() { self.emit_exec_node(&n)?; }
            }
        }
        Ok(())
    }

    fn follow_exec_outputs(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        for pin in &node.outputs {
            if matches!(pin.pin.data_type, DataType::Execution) {
                for nid in self.exec_routing.get_connected_nodes(&node.id, &pin.id).to_vec() {
                    if let Some(n) = self.graph.nodes.get(&nid).cloned() { self.emit_exec_node(&n)?; }
                }
            }
        }
        Ok(())
    }

    // ── Input resolution ──────────────────────────────────────────────────────

    fn collect_input_slots_for(
        &mut self, node: &NodeInstance, meta: &graphy::core::NodeMetadata,
    ) -> Result<Vec<SlotId>, GraphyError> {
        let mut slots = Vec::new();
        for param in &meta.params {
            let pin = node.inputs.iter().find(|p| p.pin.name == param.name)
                .ok_or_else(|| GraphyError::Custom(
                    format!("Input pin '{}' not found on node '{}'", param.name, node.id)))?;
            slots.push(self.resolve_input_slot(&node.id, &pin.id)?);
        }
        Ok(slots)
    }

    fn resolve_input_slot(&mut self, node_id: &str, pin_id: &str) -> Result<SlotId, GraphyError> {
        use graphy::analysis::DataSource;

        match self.data_resolver.get_input_source(node_id, pin_id) {
            Some(DataSource::Connection { source_node_id, .. }) => {
                let sid = source_node_id.to_string();
                self.output_slots.get(&sid).copied()
                    .ok_or_else(|| GraphyError::Custom(format!("No slot for node '{}'", sid)))
            }
            Some(DataSource::Constant(val)) => {
                let key = format!("{}:{}:{}", node_id, pin_id, val);
                if let Some(&s) = self.const_slots.get(&key) { return Ok(s); }

                let node = self.graph.nodes.get(node_id)
                    .ok_or_else(|| GraphyError::NodeNotFound(node_id.to_string()))?;
                let pin = node.inputs.iter().find(|p| p.id == pin_id)
                    .ok_or_else(|| GraphyError::PinNotFound { node: node_id.to_string(), pin: pin_id.to_string() })?;

                let slot = self.alloc_slot();
                self.emit_typed_const(&val, &pin.pin.data_type, slot);
                self.const_slots.insert(key, slot);
                Ok(slot)
            }
            Some(DataSource::Default) => {
                let node = self.graph.nodes.get(node_id)
                    .ok_or_else(|| GraphyError::NodeNotFound(node_id.to_string()))?;
                let pin = node.inputs.iter().find(|p| p.id == pin_id)
                    .ok_or_else(|| GraphyError::PinNotFound { node: node_id.to_string(), pin: pin_id.to_string() })?;
                let slot = self.alloc_slot();
                self.emit_default_const(&pin.pin.data_type, slot);
                Ok(slot)
            }
            None => Err(GraphyError::Custom(format!("No data source for {}.{}", node_id, pin_id))),
        }
    }

    // ── Typed constant emission — no BpValue ──────────────────────────────────

    fn emit_typed_const(&mut self, raw: &str, dt: &DataType, slot: SlotId) {
        let s = raw.trim()
            .trim_end_matches("i64").trim_end_matches("u64")
            .trim_end_matches("i32").trim_end_matches("u32")
            .trim_end_matches("f64").trim_end_matches("f32")
            .trim_end_matches("usize").trim_end_matches("isize")
            .trim();

        let instr = match dt {
            DataType::Typed(ti) => match ti.type_string.as_str() {
                "f64"                         => Instruction::LoadF64 { slot, value: s.parse().unwrap_or(0.0) },
                "f32"                         => Instruction::LoadF32 { slot, value: s.parse().unwrap_or(0.0) },
                "bool"                        => Instruction::LoadI32 { slot, value: if s == "true" || s.parse::<f64>().unwrap_or(0.0) != 0.0 { 1 } else { 0 } },
                "i32"|"u32"|"i16"|"u16"|"i8"|"u8" => Instruction::LoadI32 { slot, value: s.parse().unwrap_or(0) },
                _                             => Instruction::LoadI64 { slot, value: s.parse::<i64>().unwrap_or_else(|_| s.parse::<f64>().map(|f| f as i64).unwrap_or(0)) },
            },
            DataType::Number  => Instruction::LoadF64 { slot, value: s.parse().unwrap_or(0.0) },
            DataType::Boolean => Instruction::LoadI32 { slot, value: if s == "true" || s.parse::<f64>().unwrap_or(0.0) != 0.0 { 1 } else { 0 } },
            _                 => Instruction::LoadI64 { slot, value: s.parse::<i64>().unwrap_or(0) },
        };
        self.instructions.push(instr);
    }

    fn emit_default_const(&mut self, dt: &DataType, slot: SlotId) {
        let instr = match dt {
            DataType::Typed(ti) => match ti.type_string.as_str() {
                "f64"  => Instruction::LoadF64 { slot, value: 0.0 },
                "f32"  => Instruction::LoadF32 { slot, value: 0.0 },
                "bool" => Instruction::LoadI32 { slot, value: 0 },
                "i32"|"u32"|"i16"|"u16"|"i8"|"u8" => Instruction::LoadI32 { slot, value: 0 },
                _      => Instruction::LoadI64 { slot, value: 0 },
            },
            DataType::Number  => Instruction::LoadF64 { slot, value: 0.0 },
            DataType::Boolean => Instruction::LoadI32 { slot, value: 0 },
            _                 => Instruction::LoadI64 { slot, value: 0 },
        };
        self.instructions.push(instr);
    }
}
