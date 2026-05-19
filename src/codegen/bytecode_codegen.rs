use crate::bytecode::{BpProgram, Instruction, LabelId};
use crate::metadata::BlueprintMetadataProvider;
use graphy::core::NodeMetadataProvider;
use graphy::{DataResolver, DataType, ExecutionRouting, GraphDescription, GraphyError, NodeInstance, NodeTypes};
use std::collections::{HashMap, HashSet};

// ── Layout allocator ─────────────────────────────────────────────────────────

struct LayoutAllocator {
    next_offset: usize,
}

impl LayoutAllocator {
    fn new() -> Self { Self { next_offset: 0 } }

    /// Align `next_offset` up to `align`, allocate `size` bytes, return the offset.
    fn alloc(&mut self, size: usize, align: usize) -> usize {
        let offset = (self.next_offset + align - 1) & !(align - 1);
        self.next_offset = offset + size;
        offset
    }
}

// ── Constant serialization ────────────────────────────────────────────────────
//
// Converts a constant string (from the graph) into the exact little-endian bytes
// that represent the value in memory, matching what `ptr::read::<T>` expects.
//
// Only primitive types that can appear as blueprint pin constants are handled.
// Any other type_string is an error — there is no silent fallback.

fn serialize_const(raw: &str, ty: &str) -> Result<Vec<u8>, GraphyError> {
    // Strip Rust numeric suffixes like `3i64`, `2.0f32`.
    let s = raw.trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .trim();

    let bytes = match ty.trim() {
        "f64" => s.parse::<f64>()
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as f64", s)))?
            .to_le_bytes().to_vec(),
        "f32" => s.parse::<f32>()
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as f32", s)))?
            .to_le_bytes().to_vec(),
        "i64" => s.parse::<i64>()
            .or_else(|_| s.parse::<f64>().map(|f| f as i64))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as i64", s)))?
            .to_le_bytes().to_vec(),
        "u64" => s.parse::<u64>()
            .or_else(|_| s.parse::<f64>().map(|f| f as u64))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as u64", s)))?
            .to_le_bytes().to_vec(),
        "i32" => s.parse::<i32>()
            .or_else(|_| s.parse::<f64>().map(|f| f as i32))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as i32", s)))?
            .to_le_bytes().to_vec(),
        "u32" => s.parse::<u32>()
            .or_else(|_| s.parse::<f64>().map(|f| f as u32))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as u32", s)))?
            .to_le_bytes().to_vec(),
        "i16" => s.parse::<i16>()
            .or_else(|_| s.parse::<f64>().map(|f| f as i16))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as i16", s)))?
            .to_le_bytes().to_vec(),
        "u16" => s.parse::<u16>()
            .or_else(|_| s.parse::<f64>().map(|f| f as u16))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as u16", s)))?
            .to_le_bytes().to_vec(),
        "i8" => s.parse::<i8>()
            .or_else(|_| s.parse::<f64>().map(|f| f as i8))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as i8", s)))?
            .to_le_bytes().to_vec(),
        "u8" => s.parse::<u8>()
            .or_else(|_| s.parse::<f64>().map(|f| f as u8))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as u8", s)))?
            .to_le_bytes().to_vec(),
        "bool" => {
            let v = s == "true" || s.parse::<f64>().unwrap_or(0.0) != 0.0;
            vec![v as u8]
        }
        "isize" => s.parse::<isize>()
            .or_else(|_| s.parse::<f64>().map(|f| f as isize))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as isize", s)))?
            .to_le_bytes().to_vec(),
        "usize" => s.parse::<usize>()
            .or_else(|_| s.parse::<f64>().map(|f| f as usize))
            .map_err(|_| GraphyError::Custom(format!("Cannot parse {:?} as usize", s)))?
            .to_le_bytes().to_vec(),
        other => return Err(GraphyError::Custom(
            format!("Constant serialization is not supported for type '{}'. \
                     Only primitive numeric types and bool can be graph constants.", other)
        )),
    };
    Ok(bytes)
}

// ── BytecodeCodegen ───────────────────────────────────────────────────────────

pub struct BytecodeCodegen<'a> {
    graph:             &'a GraphDescription,
    metadata_provider: &'a BlueprintMetadataProvider,
    data_resolver:     &'a DataResolver,
    exec_routing:      &'a ExecutionRouting,
    variables:         HashMap<String, String>,

    instructions:   Vec<Instruction>,
    /// node_id → byte offset of that node's output value in the arena
    output_offsets: HashMap<String, usize>,
    /// canonical key → byte offset of a previously emitted constant
    const_offsets:  HashMap<String, usize>,
    /// variable name → (arena offset, size, align)
    variable_slots: HashMap<String, (usize, usize, usize)>,

    layout:         LayoutAllocator,
    next_label:     LabelId,
    visited:        HashSet<String>,
    max_args_count: usize,
}

impl<'a> BytecodeCodegen<'a> {
    pub fn new(
        graph:             &'a GraphDescription,
        metadata_provider: &'a BlueprintMetadataProvider,
        data_resolver:     &'a DataResolver,
        exec_routing:      &'a ExecutionRouting,
        variables:         HashMap<String, String>,
    ) -> Self {
        Self {
            graph, metadata_provider, data_resolver, exec_routing, variables,
            instructions:   Vec::new(),
            output_offsets: HashMap::new(),
            const_offsets:  HashMap::new(),
            variable_slots: HashMap::new(),
            layout:         LayoutAllocator::new(),
            next_label:     0,
            visited:        HashSet::new(),
            max_args_count: 0,
        }
    }

    fn alloc_label(&mut self) -> LabelId {
        let l = self.next_label;
        self.next_label += 1;
        l
    }

    pub fn generate_programs(&mut self) -> Result<Vec<BpProgram>, GraphyError> {
        self.preallocate_variable_slots()?;

        let event_nodes: Vec<NodeInstance> = self.graph.nodes.values()
            .filter(|n| self.metadata_provider.get_node_metadata(&n.node_type)
                .map(|m| m.node_type == NodeTypes::event)
                .unwrap_or(false))
            .cloned()
            .collect();

        if event_nodes.is_empty() {
            return Err(GraphyError::CodeGeneration("No event nodes found in graph".to_string()));
        }

        let mut programs = Vec::new();
        for event in event_nodes {
            programs.push(self.compile_event(&event)?);
        }
        Ok(programs)
    }

    fn preallocate_variable_slots(&mut self) -> Result<(), GraphyError> {
        let mut vars: Vec<(String, String)> = self
            .variables
            .iter()
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();
        vars.sort_by(|a, b| a.0.cmp(&b.0));

        for (var_name, type_str) in vars {
            self.ensure_variable_slot(&var_name, &type_str)?;
        }
        Ok(())
    }

    fn compile_event(&mut self, event: &NodeInstance) -> Result<BpProgram, GraphyError> {
        // Reset per-event state while preserving the shared layout allocator.
        let prev_instructions   = std::mem::take(&mut self.instructions);
        let prev_visited        = std::mem::take(&mut self.visited);
        let prev_output_offsets = std::mem::take(&mut self.output_offsets);
        let prev_const_offsets  = std::mem::take(&mut self.const_offsets);
        let prev_max_args       = self.max_args_count;
        self.max_args_count     = 0;

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

        let instrs         = std::mem::replace(&mut self.instructions, prev_instructions);
        let arena_size     = self.layout.next_offset;
        let max_args_count = self.max_args_count;

        self.visited        = prev_visited;
        self.output_offsets = prev_output_offsets;
        self.const_offsets  = prev_const_offsets;
        self.max_args_count = prev_max_args;

        let mut prog = BpProgram::new(meta.name.clone());
        prog.instructions   = instrs;
        prog.arena_size     = arena_size;
        prog.max_args_count = max_args_count;
        Ok(prog)
    }

    // ── Pure preamble ─────────────────────────────────────────────────────────

    fn emit_pure_preamble(&mut self) -> Result<(), GraphyError> {
        for node_id in self.data_resolver.get_pure_evaluation_order() {
            let node_id = node_id.to_string();
            if self.output_offsets.contains_key(&node_id) { continue; }
            let node = match self.graph.nodes.get(&node_id).cloned() {
                Some(n) => n, None => continue,
            };

            if node.node_type.starts_with("get_") {
                self.emit_getter(&node)?;
                continue;
            }

            let meta = match self.metadata_provider.get_node_metadata(&node.node_type) {
                Some(m) => m.clone(), None => continue,
            };
            if meta.node_type != NodeTypes::pure { continue; }

            let input_offsets = self.collect_input_offsets_for(&node, &meta)?;
            self.max_args_count = self.max_args_count.max(input_offsets.len());
            let (output_offset, has_output) = self.alloc_output_for_meta(&node.id, &node.node_type, &meta);
            let type_slot_offsets = self.collect_type_slot_offsets_for(&node)?;

            self.instructions.push(Instruction::Call {
                fn_ptr:            0,
                node_type:         meta.name.to_string(),
                input_offsets,
                output_offset,
                has_output,
                type_slot_offsets,
            });
        }
        Ok(())
    }

    // ── Exec chain ────────────────────────────────────────────────────────────

    fn emit_exec_node(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        if self.visited.contains(&node.id) { return Ok(()); }
        self.visited.insert(node.id.clone());

        if node.node_type.starts_with("set_") {
            return self.emit_setter(node);
        }

        let meta = self.metadata_provider.get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?.clone();

        match meta.node_type {
            NodeTypes::pure | NodeTypes::event => Ok(()),
            NodeTypes::fn_          => self.emit_fn_node(node, &meta),
            NodeTypes::control_flow => self.emit_control_flow(node, &meta),
        }
    }

    fn emit_fn_node(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<(), GraphyError> {
        let input_offsets = self.collect_input_offsets_for(node, meta)?;
        self.max_args_count = self.max_args_count.max(input_offsets.len());
        let (output_offset, has_output) = self.alloc_output_for_meta(&node.id, &node.node_type, meta);
        let type_slot_offsets = self.collect_type_slot_offsets_for(node)?;

        self.instructions.push(Instruction::Call {
            fn_ptr:            0,
            node_type:         meta.name.to_string(),
            input_offsets,
            output_offset,
            has_output,
            type_slot_offsets,
        });
        self.follow_exec_outputs(node)
    }

    fn emit_setter(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        let var_name = node.node_type.strip_prefix("set_")
            .ok_or_else(|| GraphyError::Custom(format!("Bad setter: {}", node.node_type)))?;
        let var_type = self.variable_type(var_name)?;
        let (var_offset, var_size, _) = self.ensure_variable_slot(var_name, &var_type)?;

        let val_pin = node.inputs.iter().find(|p| p.pin.name == "value")
            .ok_or_else(|| GraphyError::Custom(
                format!("No value pin on setter '{}'", node.id)))?;
        let val_offset = self.resolve_input_offset(
            &node.id, &node.node_type, &val_pin.id, &val_pin.pin.name)?;
        self.max_args_count = self.max_args_count.max(1);

        self.instructions.push(Instruction::StoreVar {
            input_offset: val_offset,
            target_offset: var_offset,
            size: var_size,
        });
        self.follow_exec_outputs(node)
    }

    fn emit_getter(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        let var_name = node.node_type.strip_prefix("get_")
            .ok_or_else(|| GraphyError::Custom(format!("Bad getter: {}", node.node_type)))?;
        let var_type = self.variable_type(var_name)?;
        let (source_offset, var_size, align) = self.ensure_variable_slot(var_name, &var_type)?;
        let output_offset = self.layout.alloc(var_size, align);
        self.output_offsets.insert(node.id.clone(), output_offset);

        self.instructions.push(Instruction::LoadVar {
            source_offset,
            output_offset,
            size: var_size,
        });
        Ok(())
    }

    fn emit_control_flow(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<(), GraphyError> {
        let exec_pins: Vec<_> = node.outputs.iter()
            .filter(|p| matches!(p.pin.data_type, DataType::Execution))
            .collect();
        let data_ins: Vec<_> = node.inputs.iter()
            .filter(|p| !matches!(p.pin.data_type, DataType::Execution))
            .collect();

        if exec_pins.len() == 2 && data_ins.len() == 1 {
            return self.emit_two_way_branch(node, &data_ins, &exec_pins);
        }
        if data_ins.is_empty() && exec_pins.len() > 1 {
            return self.emit_sequential(node, &exec_pins);
        }
        self.emit_fn_node(node, meta)
    }

    fn emit_two_way_branch(
        &mut self,
        node:      &NodeInstance,
        data_ins:  &[&graphy::PinInstance],
        exec_pins: &[&graphy::PinInstance],
    ) -> Result<(), GraphyError> {
        let cond_offset = self.resolve_input_offset(
            &node.id, &node.node_type, &data_ins[0].id, &data_ins[0].pin.name)?;
        let true_lbl    = self.alloc_label();
        let false_lbl   = self.alloc_label();
        let end_lbl     = self.alloc_label();

        self.instructions.push(Instruction::JumpIf {
            condition_offset: cond_offset,
            true_label:       true_lbl,
            false_label:      false_lbl,
        });

        self.instructions.push(Instruction::Label(true_lbl));
        let saved = self.visited.clone();
        for nid in self.exec_routing.get_connected_nodes(&node.id, &exec_pins[0].id).to_vec() {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                self.emit_exec_node(&n)?;
            }
        }
        self.instructions.push(Instruction::Jump(end_lbl));

        self.instructions.push(Instruction::Label(false_lbl));
        self.visited = saved;
        for nid in self.exec_routing.get_connected_nodes(&node.id, &exec_pins[1].id).to_vec() {
            if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                self.emit_exec_node(&n)?;
            }
        }

        self.instructions.push(Instruction::Label(end_lbl));
        Ok(())
    }

    fn emit_sequential(
        &mut self,
        node:      &NodeInstance,
        exec_pins: &[&graphy::PinInstance],
    ) -> Result<(), GraphyError> {
        for pin in exec_pins {
            for nid in self.exec_routing.get_connected_nodes(&node.id, &pin.id).to_vec() {
                if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                    self.emit_exec_node(&n)?;
                }
            }
        }
        Ok(())
    }

    fn follow_exec_outputs(&mut self, node: &NodeInstance) -> Result<(), GraphyError> {
        for pin in &node.outputs {
            if matches!(pin.pin.data_type, DataType::Execution) {
                for nid in self.exec_routing.get_connected_nodes(&node.id, &pin.id).to_vec() {
                    if let Some(n) = self.graph.nodes.get(&nid).cloned() {
                        self.emit_exec_node(&n)?;
                    }
                }
            }
        }
        Ok(())
    }

    // ── TypeSlot allocation ───────────────────────────────────────────────────
    //
    // For a generic node call, bare-T parameters (those whose compile-time size
    // is 0 from the T→() substitution) need a runtime TypeSlot in the arena so
    // the dispatch shim can read element size/align from type_slots[i].
    //
    // Algorithm:
    //   For each param with metadata size == 0:
    //     • Follow its data connection to the source node.
    //     • Read that node's return_size / return_align from the pulsar_std registry.
    //     • Deduplicate: if a TypeSlot for that (size, align) was already emitted
    //       for this call, reuse its offset.
    //     • Otherwise allocate space in the arena and emit InitTypeSlot.
    //   Return the slot offsets in the order they are needed by the shim
    //   (first-unique-bare-T-param first).

    fn collect_type_slot_offsets_for(
        &mut self,
        node: &NodeInstance,
    ) -> Result<Vec<usize>, GraphyError> {
        use graphy::analysis::DataSource;

        let raw_meta = match pulsar_std::get_all_nodes()
            .iter()
            .find(|m| m.name == node.node_type)
        {
            Some(m) => m,
            None    => return Ok(vec![]),  // not in registry, no type slots
        };

        // Size/align of a TypeSlot value in the arena.
        let slot_sz  = std::mem::size_of::<pulsar_std::TypeSlot>();
        let slot_al  = std::mem::align_of::<pulsar_std::TypeSlot>();

        // Deduplicate within this call: (size, align) → arena offset
        let mut seen: std::collections::HashMap<(usize, usize), usize> = std::collections::HashMap::new();
        let mut offsets: Vec<usize> = Vec::new();

        for raw_param in raw_meta.params {
            // Only bare-T params signal size == 0 via the T→() macro substitution.
            if raw_param.size != 0 {
                continue;
            }

            // Find the matching pin on the node instance.
            let pin = match node.inputs.iter().find(|p| p.pin.name == raw_param.name) {
                Some(p) => p,
                None    => continue,
            };

            // Follow the connection to the source node.
            let (t_size, t_align) = match self.data_resolver.get_input_source(&node.id, &pin.id) {
                Some(DataSource::Connection { source_node_id, .. }) => {
                    let src_type = match self.graph.nodes.get(source_node_id) {
                        Some(n) => n.node_type.clone(),
                        None    => continue,
                    };
                    self.metadata_provider
                        .return_layout(&src_type)
                        .unwrap_or((8, 8))  // safe fallback: 8-byte primitive
                }
                // Default / constant bare-T params: use 8-byte fallback.
                // A zero-size default would produce an empty TypeSlot, so 8/8 is safer.
                _ => (8, 8),
            };

            // Deduplicate: reuse an existing slot for the same (size, align).
            let slot_offset = match seen.entry((t_size, t_align)) {
                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    let off = self.layout.alloc(slot_sz, slot_al);
                    self.instructions.push(Instruction::InitTypeSlot {
                        offset: off,
                        size:   t_size,
                        align:  t_align,
                    });
                    *e.insert(off)
                }
            };
            offsets.push(slot_offset);
        }

        Ok(offsets)
    }

    // ── Output allocation ─────────────────────────────────────────────────────
    //
    // Layout comes from the compile-time return_size/return_align values baked
    // into pulsar_std::NodeMetadata by the #[blueprint] macro.  No type-string matching.

    fn alloc_output_for_meta(
        &mut self,
        node_id:   &str,
        node_type: &str,
        meta:      &graphy::core::NodeMetadata,
    ) -> (usize, bool) {
        let is_void = meta.return_type.as_ref()
            .map(|rt| rt.type_string == "()")
            .unwrap_or(true);
        if is_void {
            return (0, false);
        }
        let (size, align) = self.metadata_provider
            .return_layout(node_type)
            .unwrap_or((8, 8)); // only fallback: node not in registry (e.g. intrinsics)
        if size == 0 {
            return (0, false); // macro flagged it as void
        }
        let offset = self.layout.alloc(size, align);
        self.output_offsets.insert(node_id.to_string(), offset);
        (offset, true)
    }

    // ── Input resolution ──────────────────────────────────────────────────────

    fn collect_input_offsets_for(
        &mut self,
        node: &NodeInstance,
        meta: &graphy::core::NodeMetadata,
    ) -> Result<Vec<usize>, GraphyError> {
        let mut offsets = Vec::new();
        for param in &meta.params {
            let pin = node.inputs.iter().find(|p| p.pin.name == param.name)
                .ok_or_else(|| GraphyError::Custom(
                    format!("Input pin '{}' not found on node '{}'", param.name, node.id)))?;
            offsets.push(self.resolve_input_offset(&node.id, &node.node_type, &pin.id, &pin.pin.name)?);
        }
        Ok(offsets)
    }

    fn resolve_input_offset(
        &mut self,
        node_id:   &str,
        node_type: &str,
        pin_id:    &str,
        pin_name:  &str,
    ) -> Result<usize, GraphyError> {
        use graphy::analysis::DataSource;

        match self.data_resolver.get_input_source(node_id, pin_id) {
            Some(DataSource::Connection { source_node_id, .. }) => {
                let sid = source_node_id.to_string();
                self.output_offsets.get(&sid).copied()
                    .ok_or_else(|| GraphyError::Custom(
                        format!("No arena offset for node '{}'", sid)))
            }

            Some(DataSource::Constant(val)) => {
                let key = format!("{}:{}:{}", node_id, pin_id, val);
                if let Some(&off) = self.const_offsets.get(&key) {
                    return Ok(off);
                }
                let (size, align, ty_str) = self.param_layout_and_type(node_type, pin_name)?;
                let off = self.emit_const(&val, &ty_str, size, align)?;
                self.const_offsets.insert(key, off);
                Ok(off)
            }

            Some(DataSource::Default) => {
                // Zero-initialised slot — arena is already zeroed at VM startup,
                // so we only need to reserve the space.
                let (size, align, _) = self.param_layout_and_type(node_type, pin_name)?;
                Ok(self.layout.alloc(size, align))
            }

            None => Err(GraphyError::Custom(
                format!("No data source for {}.{}", node_id, pin_id))),
        }
    }

    fn variable_type(&self, var_name: &str) -> Result<String, GraphyError> {
        self.variables
            .get(var_name)
            .cloned()
            .ok_or_else(|| GraphyError::Custom(format!("Variable '{}' not found", var_name)))
    }

    fn ensure_variable_slot(
        &mut self,
        var_name: &str,
        type_str: &str,
    ) -> Result<(usize, usize, usize), GraphyError> {
        if let Some(slot) = self.variable_slots.get(var_name) {
            return Ok(*slot);
        }

        let (size, align) = variable_layout_for_type(type_str).ok_or_else(|| {
            GraphyError::Custom(format!("Unsupported variable type '{}' for variable '{}'", type_str, var_name))
        })?;

        let offset = self.layout.alloc(size, align);
        let slot = (offset, size, align);
        self.variable_slots.insert(var_name.to_string(), slot);
        Ok(slot)
    }

    /// Returns (size, align, type_string) for a named input parameter, sourced
    /// exclusively from the compile-time `pulsar_std` registry.
    fn param_layout_and_type(
        &self,
        node_type: &str,
        param_name: &str,
    ) -> Result<(usize, usize, String), GraphyError> {
        let (size, align) = self.metadata_provider
            .param_layout(node_type, param_name)
            .ok_or_else(|| GraphyError::Custom(
                format!("No layout metadata for param '{}' on node '{}'.\
                         Ensure the node is registered with #[blueprint].",
                         param_name, node_type)))?;

        // The type string is only needed for constant serialization.
        // Retrieve it from the raw pulsar_std NodeParameter.
        let ty_str = pulsar_std::get_all_nodes()
            .iter()
            .find(|m| m.name == node_type)
            .and_then(|m| m.params.iter().find(|p| p.name == param_name))
            .map(|p| p.ty.to_string())
            .ok_or_else(|| GraphyError::Custom(
                format!("No type info for param '{}' on node '{}'", param_name, node_type)))?;

        Ok((size, align, ty_str))
    }

    /// Serialise a constant string, allocate aligned arena space, emit InitBytes.
    /// Errors if the type is not a supported primitive — no silent fallbacks.
    fn emit_const(
        &mut self,
        raw:   &str,
        ty:    &str,
        size:  usize,
        align: usize,
    ) -> Result<usize, GraphyError> {
        let bytes     = serialize_const(raw, ty)?;
        // Use the serialised byte count as the authoritative size when it differs
        // (e.g., bool serialises to 1 byte but metadata size is also 1).
        let real_size = bytes.len().max(size);
        let offset    = self.layout.alloc(real_size, align);
        self.instructions.push(Instruction::InitBytes { offset, bytes });
        Ok(offset)
    }
}

fn variable_layout_for_type(type_str: &str) -> Option<(usize, usize)> {
    match type_str.trim() {
        "bool" | "i8" | "u8" => Some((1, 1)),
        "i16" | "u16" => Some((2, 2)),
        "i32" | "u32" | "f32" | "char" => Some((4, 4)),
        "i64" | "u64" | "f64" | "isize" | "usize" => Some((8, 8)),
        "String" | "Vec" | "Vec<()>" => Some((std::mem::size_of::<String>(), std::mem::align_of::<String>())),
        other if other.starts_with("Vec<") => Some((std::mem::size_of::<Vec<u8>>(), std::mem::align_of::<Vec<u8>>())),
        other if other.starts_with('[') && other.ends_with(']') => parse_array_layout(other),
        _ => None,
    }
}

fn parse_array_layout(type_str: &str) -> Option<(usize, usize)> {
    // Very small parser for `[T; N]` used by generic shims and array variables.
    let body = type_str.strip_prefix('[')?.strip_suffix(']')?;
    let (inner, len_str) = body.rsplit_once(';')?;
    let len: usize = len_str.trim().parse().ok()?;
    let (elem_size, elem_align) = variable_layout_for_type(inner.trim())?;
    let size = elem_size.saturating_mul(len);
    Some((size, elem_align))
}
