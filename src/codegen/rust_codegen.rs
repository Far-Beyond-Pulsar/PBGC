//! # Rust Code Generator for Blueprints
//!
//! Generates Rust source code from Blueprint graphs.

use crate::metadata::BlueprintMetadataProvider;
use graphy::core::NodeMetadataProvider;
use graphy::{
    DataResolver, ExecutionRouting, GraphDescription, GraphyError, NodeInstance, NodeTypes,
};
use std::collections::{HashMap, HashSet};

/// Blueprint-specific Rust code generator
pub struct BlueprintCodeGenerator<'a> {
    graph: &'a GraphDescription,
    metadata_provider: &'a BlueprintMetadataProvider,
    data_resolver: &'a DataResolver,
    exec_routing: &'a ExecutionRouting,
    variables: HashMap<String, String>,
    visited: HashSet<String>,
}

impl<'a> BlueprintCodeGenerator<'a> {
    pub fn new(
        graph: &'a GraphDescription,
        metadata_provider: &'a BlueprintMetadataProvider,
        data_resolver: &'a DataResolver,
        exec_routing: &'a ExecutionRouting,
        variables: HashMap<String, String>,
    ) -> Self {
        Self {
            graph,
            metadata_provider,
            data_resolver,
            exec_routing,
            variables,
            visited: HashSet::new(),
        }
    }

    /// Generate complete Rust program from the graph
    pub fn generate_program(&self) -> Result<String, GraphyError> {
        let mut code = String::new();

        // Add header
        code.push_str("// Auto-generated code from Pulsar Blueprint\n");
        code.push_str("// DO NOT EDIT - Changes will be overwritten\n");
        code.push_str("// Compiled with PBGC (Pulsar Blueprint Graph Compiler)\n\n");

        // pulsar_std is imported by the enclosing `mod logic` template in project.rs;
        // no additional imports needed here for the generated function body.

        if !self.variables.is_empty() {
            code.push_str("use std::cell::{Cell, RefCell};\n\n");
        }

        // Collect node-specific imports
        let node_imports = self.collect_node_imports();
        for import_stmt in node_imports {
            code.push_str(&import_stmt);
            code.push_str("\n");
        }
        code.push_str("\n");

        if !self.variables.is_empty() {
            code.push_str("// PBGC_VARIABLE_STORAGE_BEGIN\n");
            code.push_str(&self.generate_variable_storage_block()?);
            code.push_str("// PBGC_VARIABLE_STORAGE_END\n");
            code.push_str("\n");
        }

        // Find event nodes
        let event_nodes: Vec<_> = self
            .graph
            .nodes
            .values()
            .filter(|node| {
                // Custom event On nodes (node_type starts with "on_") are synthetic —
                // no static metadata entry, but still treated as event entry points.
                node.node_type.starts_with("on_")
                    || self
                        .metadata_provider
                        .get_node_metadata(&node.node_type)
                        .map(|meta| meta.node_type == NodeTypes::event)
                        .unwrap_or(false)
            })
            .collect();

        if event_nodes.is_empty() {
            return Err(GraphyError::CodeGeneration(
                "No event nodes found in graph - add a 'main' or 'begin_play' event".to_string(),
            ));
        }

        // Generate `#[pulsar_event]` structs for custom events
        for event_node in &event_nodes {
            if event_node.node_type.starts_with("on_") {
                let event_name =
                    pascal_case(&event_node.node_type.strip_prefix("on_").unwrap_or("custom"));
                code.push_str(&format!("#[pulsar_event]\npub struct {} {{\n", event_name));
                // Emit fields from output data pins (skip execution pins)
                for pin in &event_node.outputs {
                    if !matches!(pin.pin.data_type, graphy::DataType::Exec) {
                        let rust_type = match &pin.pin.data_type {
                            graphy::DataType::Data(ti) => ti.type_string.clone(),
                            graphy::DataType::Exec => "()".to_string(),
                        };
                        code.push_str(&format!("    pub {}: {},\n", pin.id, rust_type));
                    }
                }
                code.push_str("}\n\n");
            }
        }

        // Also collect custom event node types referenced by emit_custom_event nodes
        let custom_event_structs: std::collections::HashSet<String> = self
            .graph
            .nodes
            .values()
            .filter(|n| n.node_type == "emit_custom_event")
            .filter_map(|n| n.properties.get("event_uid").and_then(|v| v.as_str()))
            .map(pascal_case)
            .collect();
        for struct_name in custom_event_structs {
            // Ensure the struct is defined — will be generated above from the On node.
            // If no On node exists in this graph, the graph structure is incomplete
            // (the On node lives in a different blueprint actor).
        }

        // Generate each event function
        for event_node in event_nodes {
            let event_code = self.generate_event_function(event_node)?;
            code.push_str(&event_code);
            code.push_str("\n");
        }

        Ok(code)
    }

    /// Collect imports from all nodes
    fn collect_node_imports(&self) -> Vec<String> {
        let mut imports: HashSet<String> = HashSet::new();

        for node in self.graph.nodes.values() {
            if let Some(metadata) = self.metadata_provider.get_node_metadata(&node.node_type) {
                for import in &metadata.imports {
                    imports.insert(import.clone());
                }
            }
        }

        let mut import_vec: Vec<_> = imports.into_iter().collect();
        import_vec.sort();
        import_vec
    }

    /// Generate thread-local storage and helper functions for class variables.
    fn generate_variable_storage_block(&self) -> Result<String, GraphyError> {
        let mut code = String::new();
        code.push_str("thread_local! {\n");

        let mut vars: Vec<_> = self.variables.iter().collect();
        vars.sort_by(|a, b| a.0.cmp(b.0));

        for (var_name, var_type) in vars {
            let static_name = to_static_var_name(var_name);
            if is_copy_type(var_type) {
                code.push_str(&format!(
                    "    static {}: Cell<Option<{}>> = Cell::new(None);\n",
                    static_name, var_type
                ));
            } else {
                code.push_str(&format!(
                    "    static {}: RefCell<Option<{}>> = RefCell::new(None);\n",
                    static_name, var_type
                ));
            }
        }

        code.push_str("}\n\n");
        code.push_str("#[inline]\n");
        code.push_str("fn __pbgc_set_copy<T: Copy>(slot: &Cell<Option<T>>, value: T) {\n");
        code.push_str("    slot.set(Some(value));\n");
        code.push_str("}\n\n");
        code.push_str("#[inline]\n");
        code.push_str(
            "fn __pbgc_get_copy<T: Copy>(slot: &Cell<Option<T>>, var_name: &str) -> T {\n",
        );
        code.push_str("    slot.get().unwrap_or_else(|| panic!(\"PBGC variable '{}' read before assignment\", var_name))\n");
        code.push_str("}\n\n");
        code.push_str("#[inline]\n");
        code.push_str("fn __pbgc_set_clone<T>(slot: &RefCell<Option<T>>, value: T) {\n");
        code.push_str("    *slot.borrow_mut() = Some(value);\n");
        code.push_str("}\n\n");
        code.push_str("#[inline]\n");
        code.push_str(
            "fn __pbgc_get_clone<T: Clone>(slot: &RefCell<Option<T>>, var_name: &str) -> T {\n",
        );
        code.push_str("    slot.borrow()\n");
        code.push_str("        .as_ref()\n");
        code.push_str("        .cloned()\n");
        code.push_str("        .unwrap_or_else(|| panic!(\"PBGC variable '{}' read before assignment\", var_name))\n");
        code.push_str("}\n");

        Ok(code)
    }

    /// Generate an event function
    fn generate_event_function(&self, event_node: &NodeInstance) -> Result<String, GraphyError> {
        let mut code = String::new();

        // Every generated logic function receives the live-world slice
        // (#651): the `(entity, world)` pair the enclosing `impl Actor`
        // callback got from the engine. Component nodes compile to
        // dispatcher calls against exactly these parameters. Types are
        // fully qualified because this module only glob-imports
        // `pulsar_std`/vars, which must never shadow them.
        const LIVE_WORLD_PARAMS: &str =
            "_entity: pulsar_game::Entity, _world: &mut pulsar_game::World";

        // Custom event nodes don't have static metadata — infer from the node itself.
        if event_node.node_type.starts_with("on_") {
            let func_name = event_node.node_type.clone();
            let mut params: Vec<String> = event_node
                .outputs
                .iter()
                .filter(|p| !matches!(p.pin.data_type, graphy::DataType::Exec))
                .map(|p| {
                    let type_str = match &p.pin.data_type {
                        graphy::DataType::Data(ti) => ti.type_string.clone(),
                        graphy::DataType::Exec => "()".to_string(),
                    };
                    format!("{}: {}", p.id, type_str)
                })
                .collect();
            params.push(LIVE_WORLD_PARAMS.to_string());
            code.push_str(&format!("pub fn {}({}) {{\n", func_name, params.join(", ")));
            let indent = "    ";
            // Generate pure node preamble
            let pure_preamble = self.generate_pure_node_preamble(1)?;
            if !pure_preamble.is_empty() {
                code.push_str(&pure_preamble);
            }
            // Follow execution chain
            for output_pin in &event_node.outputs {
                if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                    let connected = self
                        .exec_routing
                        .get_connected_nodes(&event_node.id, &output_pin.id);
                    for next_node_id in connected {
                        if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                            let mut generator = self.clone_with_new_visited();
                            let node_code = generator.generate_exec_chain(next_node, 1)?;
                            code.push_str(&node_code);
                        }
                    }
                }
            }
            code.push_str("}\n");
            return Ok(code);
        }

        // Get event metadata (for standard event nodes)
        let metadata = self
            .metadata_provider
            .get_node_metadata(&event_node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(event_node.node_type.clone()))?;

        // Generate function signature — include any event parameters (e.g. delta_time for on_tick),
        // then the live-world slice every generated function receives (#651).
        let mut params: Vec<String> = metadata
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, p.param_type))
            .collect();
        params.push(LIVE_WORLD_PARAMS.to_string());
        code.push_str(&format!(
            "pub fn {}({}) {{\n",
            metadata.name,
            params.join(", ")
        ));

        let pure_preamble = self.generate_pure_node_preamble(1)?;
        if !pure_preamble.is_empty() {
            code.push_str(&pure_preamble);
        }

        // Find execution output pins and follow them
        // We need to look up by pin ID (from the node instance), not pin name (from metadata)
        for output_pin in &event_node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                tracing::debug!(
                    "[CODEGEN] Looking up exec connections for node {} pin ID: {}",
                    event_node.id,
                    output_pin.id
                );

                let connected = self
                    .exec_routing
                    .get_connected_nodes(&event_node.id, &output_pin.id);

                tracing::debug!("[CODEGEN] Found {} connected nodes", connected.len());

                for next_node_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                        let mut generator = self.clone_with_new_visited();
                        let node_code = generator.generate_exec_chain(next_node, 1)?;
                        code.push_str(&node_code);
                    }
                }
            }
        }

        code.push_str("}\n");

        Ok(code)
    }

    fn generate_pure_node_preamble(&self, indent_level: usize) -> Result<String, GraphyError> {
        let mut code = String::new();
        let indent = "    ".repeat(indent_level);

        for node_id in self.data_resolver.get_pure_evaluation_order() {
            if !self.should_materialize_pure_result(node_id) {
                continue;
            }

            let Some(result_var) = self.data_resolver.get_result_variable(node_id) else {
                continue;
            };

            let node = self
                .graph
                .nodes
                .get(node_id)
                .ok_or_else(|| GraphyError::NodeNotFound(node_id.clone()))?;

            let expr = if let Some(var_name) = node
                .node_type
                .strip_prefix("get_")
                .filter(|name| self.variables.contains_key(*name))
            {
                let var_type = self.variables.get(var_name).ok_or_else(|| {
                    GraphyError::Custom(format!("Variable '{}' not found", var_name))
                })?;
                let static_name = to_static_var_name(var_name);

                if is_copy_type(var_type) {
                    format!(
                        "{}.with(|v| __pbgc_get_copy(v, \"{}\"))",
                        static_name, var_name
                    )
                } else {
                    format!(
                        "{}.with(|v| __pbgc_get_clone(v, \"{}\"))",
                        static_name, var_name
                    )
                }
            } else {
                self.generate_pure_node_expression(node)?
            };

            code.push_str(&format!("{}let {} = {};\n", indent, result_var, expr));
        }

        Ok(code)
    }

    fn should_materialize_pure_result(&self, source_node_id: &str) -> bool {
        false
    }

    fn pure_result_use_count(&self, source_node_id: &str) -> usize {
        use graphy::analysis::DataSource;

        let mut count = 0usize;
        for (consumer_id, node) in &self.graph.nodes {
            for input in &node.inputs {
                if let Some(DataSource::Connection {
                    source_node_id: sid,
                    ..
                }) = self.data_resolver.get_input_source(consumer_id, &input.id)
                {
                    if sid == source_node_id {
                        count += 1;
                    }
                }
            }
        }

        count
    }

    /// Generate execution chain starting from a node
    fn generate_exec_chain(
        &mut self,
        node: &NodeInstance,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let code = String::new();

        // Prevent infinite loops
        if self.visited.contains(&node.id) {
            return Ok(code);
        }
        self.visited.insert(node.id.clone());

        // ── Component nodes ───────────────────────────────────────────────────
        // Node types follow the convention:
        //   comp_get_prop::{ClassName}::{PropName}  — pure, value producer
        //   comp_set_prop::{ClassName}::{PropName}  — exec, value consumer
        //   comp_call::{ClassName}::{MethodName}    — exec, optional return

        if node.node_type.starts_with("comp_get_prop::") {
            // Pure node — no exec chain contribution.
            return Ok(code);
        }
        if node.node_type.starts_with("comp_set_prop::") {
            return self.generate_comp_set_prop_node(node, indent_level);
        }
        if node.node_type.starts_with("comp_call::") {
            return self.generate_comp_call_node(node, indent_level);
        }

        // ── Variable nodes ────────────────────────────────────────────────────
        if node
            .node_type
            .strip_prefix("get_")
            .is_some_and(|name| self.variables.contains_key(name))
        {
            // Getter nodes are pure (no exec chain), skip
            return Ok(code);
        } else if node
            .node_type
            .strip_prefix("set_")
            .is_some_and(|name| self.variables.contains_key(name))
        {
            // Setter nodes have exec chain
            return self.generate_setter_node(node, indent_level);
        }

        let node_meta = self
            .metadata_provider
            .get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?;

        match node_meta.node_type {
            NodeTypes::pure => {
                // Pure nodes are pre-evaluated, skip in exec chain
                Ok(code)
            }
            NodeTypes::fn_ => self.generate_function_node(node, node_meta, indent_level),
            NodeTypes::control_flow => {
                self.generate_control_flow_node(node, node_meta, indent_level)
            }
            NodeTypes::event => {
                // Event nodes define the outer function, skip in exec chain
                Ok(code)
            }
        }
    }

    // ── Component node codegen ────────────────────────────────────────────────
    //
    // All three kinds compile to `pulsar_world_registry::dispatch` calls
    // against the live-world parameters of the enclosing generated function
    // (#651) — C's #643 dispatcher is THE one dispatch layer, with the VM's
    // arena trampolines as the other adapter. Values travel in the graph's
    // JSON domain exactly like the VM path: method arguments are converted
    // to their declared parameter types by `json_args_to_method_args`
    // before dispatch, and every failure logs and degrades to JSON null
    // instead of aborting the event.

    /// Generate a component property getter expression (pure, inline).
    ///
    /// Node type format: `comp_get_prop::{ClassName}::{PropName}`.
    ///
    /// #654: when the node's `component_ref` input pin is wired to an
    /// identity producer, the read targets THAT reference's actor/instance;
    /// unconnected pins keep addressing the executing instance itself
    /// (index 0).
    fn generate_comp_get_prop_expr(&self, node: &NodeInstance) -> Result<String, GraphyError> {
        let rest = node
            .node_type
            .strip_prefix("comp_get_prop::")
            .ok_or_else(|| {
                GraphyError::Custom(format!("Bad comp_get_prop node: {}", node.node_type))
            })?;
        let mut parts = rest.splitn(2, "::");
        let class_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing class name in comp_get_prop".into()))?;
        let prop_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing prop name in comp_get_prop".into()))?;

        let target = self.generate_pin_target_expr(node, class_name)?;
        Ok(format!(
            r#"match {target} {{
                Some((__bp_target_entity, __bp_target_index)) => pulsar_world_registry::dispatch::get_component_property(
                    _world,
                    __bp_target_entity,
                    "{class_name}",
                    __bp_target_index,
                    "{prop_name}",
                )
                .unwrap_or(serde_json::Value::Null),
                None => serde_json::Value::Null,
            }}"#
        ))
    }

    /// Generate the target-resolution expression for a component op's
    /// optional `component_ref` input (#654).
    ///
    /// Unconnected pin → constant `(self entity, index 0)` pair shaped like
    /// the resolved form so every op site shares one code path. Connected →
    /// `resolve_pin_target(..)` against the wired reference, which logs its
    /// typed failure and yields `None` on stale/lost targets (#641
    /// degrade-to-null semantics, no panics).
    fn generate_pin_target_expr(
        &self,
        node: &NodeInstance,
        class_name: &str,
    ) -> Result<String, GraphyError> {
        use graphy::analysis::DataSource;

        match self
            .data_resolver
            .get_input_source(&node.id, "component_ref")
        {
            Some(DataSource::Connection { source_node_id, .. }) => {
                let ref_expr = self.identity_producer_expr(&source_node_id)?;
                Ok(format!(
                    "pulsar_game::script_refs::resolve_pin_target(\n\
                     \x20               _world,\n\
                     \x20               &({ref_expr}),\n\
                     \x20               \"{class_name}\",\n\
                     \x20               \"{}\",\n\
                     \x20           )",
                    node.node_type
                ))
            }
            _ => Ok("Some((_entity, 0))".to_string()),
        }
    }

    /// Generate the producing expression for an identity-reference node
    /// (`get_component_ref`, `find_object_by_*`, `object_ref_literal`),
    /// shared by pure-input inlining and component_ref pin resolution (#654).
    fn identity_producer_expr(&self, source_node_id: &str) -> Result<String, GraphyError> {
        let node = self
            .graph
            .nodes
            .get(source_node_id)
            .ok_or_else(|| GraphyError::NodeNotFound(source_node_id.to_string()))?;

        if let Some(rest) = node.node_type.strip_prefix("get_component_ref::") {
            let mut parts = rest.splitn(2, "::");
            let class_name = parts.next().unwrap_or_default();
            let index: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            // Optional actor redirect: another object's actor ref.
            let actor_expr = match node.inputs.iter().find(|p| p.id == "actor") {
                Some(pin) => self.generate_input_expression(&node.id, &pin.id)?,
                None => "_entity".to_string(),
            };
            return Ok(format!(
                "pulsar_game::script_refs::component_ref_json(\n\
                 \x20               _world,\n\
                 \x20               {actor_expr},\n\
                 \x20               \"{class_name}\",\n\
                 \x20               {index},\n\
                 \x20               \"{}\",\n\
                 \x20           )",
                node.node_type
            ));
        }

        if node.node_type == "find_object_by_stable_id" || node.node_type == "find_object_by_name" {
            let needle_pin = node
                .inputs
                .iter()
                .find(|p| !matches!(p.pin.data_type, graphy::DataType::Exec))
                .ok_or_else(|| {
                    GraphyError::Custom(format!(
                        "identity resolver '{}' has no operand input",
                        node.id
                    ))
                })?;
            let needle = self.generate_input_expression(&node.id, &needle_pin.id)?;
            let func = if node.node_type == "find_object_by_stable_id" {
                "find_object_by_stable_id"
            } else {
                "find_object_by_name"
            };
            return Ok(format!(
                "pulsar_game::script_refs::{func}(\n\
                 \x20               _world,\n\
                 \x20               serde_json::to_value({needle}).unwrap_or(serde_json::Value::Null),\n\
                 \x20               \"{}\",\n\
                 \x20           )",
                node.node_type
            ));
        }

        if node.node_type == "object_ref_literal" {
            let stable_id = node
                .properties
                .get("stable_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let class_name = node
                .properties
                .get("class_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let component_index = node
                .properties
                .get("component_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            return Ok(format!(
                "pulsar_game::script_refs::object_literal_json(\n\
                 \x20               _world,\n\
                 \x20               \"{stable_id}\",\n\
                 \x20               \"{class_name}\",\n\
                 \x20               {component_index},\n\
                 \x20               \"{}\",\n\
                 \x20           )",
                node.node_type
            ));
        }

        Err(GraphyError::Custom(format!(
            "Node '{}' is not an identity reference producer",
            node.node_type
        )))
    }

    /// Generate code for `comp_set_prop::{ClassName}::{PropName}` (exec node).
    ///
    /// #654: a connected `component_ref` input redirects the write to that
    /// reference's `(entity, component_index)`; unconnected keeps the
    /// executing instance's live-typed instance (index 0).
    fn generate_comp_set_prop_node(
        &mut self,
        node: &NodeInstance,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let rest = node
            .node_type
            .strip_prefix("comp_set_prop::")
            .ok_or_else(|| {
                GraphyError::Custom(format!("Bad comp_set_prop node: {}", node.node_type))
            })?;
        let mut parts = rest.splitn(2, "::");
        let class_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing class name in comp_set_prop".into()))?;
        let prop_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing prop name in comp_set_prop".into()))?;

        let indent = "    ".repeat(indent_level);

        // Find the value input pin (never the component_ref pin).
        let value_pin_id = node
            .inputs
            .iter()
            .filter(|p| p.id != "component_ref" && p.pin.name != "component")
            .find(|p| p.pin.name == "value" || !matches!(p.pin.data_type, graphy::DataType::Exec))
            .map(|p| p.id.clone())
            .ok_or_else(|| {
                GraphyError::Custom(format!("No value pin on comp_set_prop node: {}", node.id))
            })?;

        let value_expr = self.generate_input_expression(&node.id, &value_pin_id)?;
        let target_expr = self.generate_pin_target_expr(node, class_name)?;

        let code = format!(
            r#"{indent}match {target_expr} {{
{indent}    Some((__bp_target_entity, __bp_target_index)) => {{
{indent}        if let Err(__e) = pulsar_world_registry::dispatch::set_component_property(
{indent}            _world,
{indent}            __bp_target_entity,
{indent}            "{class_name}",
{indent}            __bp_target_index,
{indent}            "{prop_name}",
{indent}            serde_json::to_value({value_expr}).unwrap_or(serde_json::Value::Null),
{indent}        ) {{
{indent}            tracing::error!("comp_set_prop::{class_name}::{prop_name} failed: {{__e}}");
{indent}        }}
{indent}    }}
{indent}    None => {{}}
{indent}}}
"#
        );

        // Follow exec chain.
        self.follow_exec_outputs(node, indent_level)
            .map(|chain| format!("{code}{chain}"))
    }

    /// Generate code for `comp_call::{ClassName}::{MethodName}` (exec node).
    ///
    /// #654: a connected `component_ref` input dispatches the method on that
    /// reference's actor instead of the executing instance.
    fn generate_comp_call_node(
        &mut self,
        node: &NodeInstance,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let rest = node.node_type.strip_prefix("comp_call::").ok_or_else(|| {
            GraphyError::Custom(format!("Bad comp_call node: {}", node.node_type))
        })?;
        let mut parts = rest.splitn(2, "::");
        let class_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing class name in comp_call".into()))?;
        let method_name = parts
            .next()
            .ok_or_else(|| GraphyError::Custom("Missing method name in comp_call".into()))?;

        let indent = "    ".repeat(indent_level);

        // Collect data input arguments (skip exec AND component_ref pins),
        // staged as JSON just like the VM path stages its arena blobs.
        let arg_values: Vec<String> = {
            let mut exprs = Vec::new();
            for input_pin in &node.inputs {
                if matches!(input_pin.pin.data_type, graphy::DataType::Exec) {
                    continue;
                }
                if input_pin.id == "component_ref" || input_pin.pin.name == "component" {
                    continue;
                }
                let expr = self.generate_input_expression(&node.id, &input_pin.id)?;
                exprs.push(format!(
                    "serde_json::to_value({expr}).unwrap_or(serde_json::Value::Null)"
                ));
            }
            exprs
        };

        let args_vec = if arg_values.is_empty() {
            "vec![]".to_string()
        } else {
            format!("vec![{}]", arg_values.join(", "))
        };
        let target_expr = self.generate_pin_target_expr(node, class_name)?;
        let invoke = format!(
            "pulsar_world_registry::dispatch::invoke_component_method(\
             _world, __bp_target_entity, \"{class_name}\", __bp_target_index, \"{method_name}\", __args)"
        );

        let mut code = String::new();
        if has_returns_used(node) {
            let result_var = format!("__comp_result_{}", &node.id[..8.min(node.id.len())]);
            code.push_str(&format!(
r#"{indent}let {result_var} = match {target_expr} {{
{indent}    Some((__bp_target_entity, __bp_target_index)) => {{
{indent}        match pulsar_world_registry::dispatch::json_args_to_method_args("{class_name}", "{method_name}", {args_vec}) {{
{indent}            Ok(__args) => match {invoke} {{
{indent}                Ok(__returned) => __returned
{indent}                    .and_then(|__v| pulsar_world_registry::marshal::any_to_json(
{indent}                        "comp_call::{class_name}::{method_name} return",
{indent}                        __v.as_ref(),
{indent}                    ).ok())
{indent}                    .unwrap_or(serde_json::Value::Null),
{indent}                Err(__e) => {{
{indent}                    tracing::error!("comp_call::{class_name}::{method_name} failed: {{__e}}");
{indent}                    serde_json::Value::Null
{indent}                }}
{indent}            }},
{indent}            Err(__e) => {{
{indent}                tracing::error!("comp_call::{class_name}::{method_name} arguments: {{__e}}");
{indent}                serde_json::Value::Null
{indent}            }}
{indent}        }}
{indent}    }}
{indent}    None => serde_json::Value::Null,
{indent}}};
"#
            ));
        } else {
            code.push_str(&format!(
r#"{indent}match {target_expr} {{
{indent}    Some((__bp_target_entity, __bp_target_index)) => {{
{indent}        match pulsar_world_registry::dispatch::json_args_to_method_args("{class_name}", "{method_name}", {args_vec}) {{
{indent}            Ok(__args) => {{
{indent}                if let Err(__e) = {invoke} {{
{indent}                    tracing::error!("comp_call::{class_name}::{method_name} failed: {{__e}}");
{indent}                }}
{indent}            }}
{indent}            Err(__e) => tracing::error!("comp_call::{class_name}::{method_name} arguments: {{__e}}"),
{indent}        }}
{indent}    }}
{indent}    None => {{}}
{indent}}}
"#
            ));
        }

        // Follow exec chain.
        self.follow_exec_outputs(node, indent_level)
            .map(|chain| format!("{code}{chain}"))
    }

    /// Emit every exec-chain successor of `node` at the same indent level.
    ///
    /// Shared tail of the exec-node generators (function/setter/component
    /// nodes all continue their chains identically).
    fn follow_exec_outputs(
        &mut self,
        node: &NodeInstance,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let mut code = String::new();
        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&node.id, &output_pin.id);
                for next_node_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                        let next_code = self.generate_exec_chain(next_node, indent_level)?;
                        code.push_str(&next_code);
                    }
                }
            }
        }
        Ok(code)
    }

    /// Generate code for a function node
    fn generate_function_node(
        &mut self,
        node: &NodeInstance,
        node_meta: &graphy::core::NodeMetadata,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let mut code = String::new();
        let indent = "    ".repeat(indent_level);

        // Collect arguments
        let args = self.collect_arguments(node, node_meta)?;

        // Use effective_outputs() which handles both multi-output (output_params)
        // and single-output (synthesized "result" pin from return_type).
        let outputs = node_meta.effective_outputs();
        let is_multi = outputs.len() > 1;

        if is_multi {
            // Multi-output: store the whole tuple in a temp variable.
            // Consumers reference fields via accessor (.0, .1, ...).
            let tmp_var = format!("__tmp_{}", &node.id[..8.min(node.id.len())]);
            code.push_str(&format!(
                "{}let {} = {}({});\n",
                indent,
                tmp_var,
                node_meta.name,
                args.join(", ")
            ));
        } else if !outputs.is_empty() {
            // Single output (the default "result" pin)
            let result_var = self
                .data_resolver
                .get_result_variable(&node.id)
                .ok_or_else(|| {
                    GraphyError::Custom(format!("No result variable for node: {}", node.id))
                })?;

            code.push_str(&format!(
                "{}let {} = {}({});\n",
                indent,
                result_var,
                node_meta.name,
                args.join(", ")
            ));
        } else {
            // Void return
            code.push_str(&format!(
                "{}{}({});\n",
                indent,
                node_meta.name,
                args.join(", ")
            ));
        }

        // Follow execution chain - look up by actual pin IDs from node instance
        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&node.id, &output_pin.id);
                for next_node_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                        let next_code = self.generate_exec_chain(next_node, indent_level)?;
                        code.push_str(&next_code);
                    }
                }
            }
        }

        Ok(code)
    }

    /// Generate code for a control flow node
    fn generate_control_flow_node(
        &mut self,
        node: &NodeInstance,
        node_meta: &graphy::core::NodeMetadata,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let mut code = String::new();
        let indent = "    ".repeat(indent_level);

        // Build exec_output replacements - need to map pin names to pin IDs
        let mut exec_replacements = HashMap::new();

        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&node.id, &output_pin.id);

                let mut exec_code = String::new();
                let local_visited = self.visited.clone();

                for next_node_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                        let mut sub_gen = BlueprintCodeGenerator {
                            graph: self.graph,
                            metadata_provider: self.metadata_provider,
                            data_resolver: self.data_resolver,
                            exec_routing: self.exec_routing,
                            variables: self.variables.clone(),
                            visited: local_visited.clone(),
                        };

                        let next_code = sub_gen.generate_exec_chain(next_node, 0)?;
                        exec_code.push_str(&next_code);
                    }
                }

                // Use the pin NAME for the template substitution (e.g., "then", "else")
                exec_replacements.insert(output_pin.pin.name.clone(), exec_code.trim().to_string());
            }
        }

        // Build parameter substitutions - need to look up by pin ID
        let mut param_substitutions = HashMap::new();
        for param in &node_meta.params {
            // Find the actual pin ID from the node instance
            let pin_id = node
                .inputs
                .iter()
                .find(|input| input.pin.name == param.name)
                .map(|input| input.id.clone())
                .ok_or_else(|| {
                    GraphyError::Custom(format!(
                        "Input pin not found for parameter '{}' on node '{}'",
                        param.name, node.id
                    ))
                })?;

            let value = self.generate_input_expression(&node.id, &pin_id)?;
            param_substitutions.insert(param.name.clone(), value);
        }

        // Inline the function with substitutions
        let inlined_body = graphy::utils::inline_control_flow_function(
            &node_meta.function_source,
            exec_replacements,
            param_substitutions,
        )?;

        // Add inlined code with proper indentation
        for line in inlined_body.lines() {
            if !line.trim().is_empty() {
                code.push_str(&format!("{}{}\n", indent, line));
            }
        }

        Ok(code)
    }

    /// Generate code for a setter node
    fn generate_setter_node(
        &mut self,
        node: &NodeInstance,
        indent_level: usize,
    ) -> Result<String, GraphyError> {
        let mut code = String::new();
        let indent = "    ".repeat(indent_level);

        // Extract variable name from node type (remove "set_" prefix)
        let var_name = node.node_type.strip_prefix("set_").ok_or_else(|| {
            GraphyError::Custom(format!("Invalid setter node type: {}", node.node_type))
        })?;

        // Find the "value" input pin ID
        let value_pin_id = node
            .inputs
            .iter()
            .find(|input| input.pin.name == "value")
            .map(|input| input.id.clone())
            .ok_or_else(|| {
                GraphyError::Custom(format!("Value input not found on setter node: {}", node.id))
            })?;

        // Get the value to set
        let value_expr = self.generate_input_expression(&node.id, &value_pin_id)?;

        // Get variable type to determine Cell vs RefCell
        let var_type = self
            .variables
            .get(var_name)
            .ok_or_else(|| GraphyError::Custom(format!("Variable '{}' not found", var_name)))?;

        let static_name = to_static_var_name(var_name);

        // Generate setter code
        let is_copy_type = is_copy_type(var_type);
        if is_copy_type {
            code.push_str(&format!(
                "{}{}.with(|v| __pbgc_set_copy(v, {}));\n",
                indent, static_name, value_expr
            ));
        } else {
            code.push_str(&format!(
                "{}{}.with(|v| __pbgc_set_clone(v, {}));\n",
                indent, static_name, value_expr
            ));
        }

        // Follow execution chain - use actual pin IDs from node instance
        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Exec) {
                let connected = self
                    .exec_routing
                    .get_connected_nodes(&node.id, &output_pin.id);
                for next_node_id in connected {
                    if let Some(next_node) = self.graph.nodes.get(next_node_id) {
                        let next_code = self.generate_exec_chain(next_node, indent_level)?;
                        code.push_str(&next_code);
                    }
                }
            }
        }

        Ok(code)
    }

    /// Collect arguments for a function call
    fn collect_arguments(
        &self,
        node: &NodeInstance,
        node_meta: &graphy::core::NodeMetadata,
    ) -> Result<Vec<String>, GraphyError> {
        let mut args = Vec::new();

        for param in &node_meta.params {
            // Find the actual pin ID from the node instance
            // Pin IDs are typically "{node_id}_{param_name}"
            let pin_id = node
                .inputs
                .iter()
                .find(|input| {
                    // Match by name - the pin's name should match the param name
                    input.pin.name == param.name
                })
                .map(|input| input.id.clone())
                .ok_or_else(|| {
                    GraphyError::Custom(format!(
                        "Input pin not found for parameter '{}' on node '{}'",
                        param.name, node.id
                    ))
                })?;

            let value = self.generate_input_expression(&node.id, &pin_id)?;
            args.push(value);
        }

        Ok(args)
    }

    /// Generate expression for an input value
    /// pin_id should be the actual pin ID from the node instance (e.g., "print_1_value")
    fn generate_input_expression(
        &self,
        node_id: &str,
        pin_id: &str,
    ) -> Result<String, GraphyError> {
        use graphy::analysis::DataSource;

        /// Resolve the output pin name on the source node for a multi-output accessor.
        fn get_multi_output_accessor(
            metadata_provider: &BlueprintMetadataProvider,
            graph: &GraphDescription,
            source_node_id: &str,
            source_pin_id: &str,
        ) -> Option<String> {
            let src_node = graph.nodes.get(source_node_id)?;
            let src_meta = metadata_provider.get_node_metadata(&src_node.node_type)?;
            // Find the pin name by matching pin instance ID
            let pin_name = src_node
                .outputs
                .iter()
                .find(|p| p.id == source_pin_id)
                .map(|p| p.pin.name.as_str())?;
            let outputs = src_meta.effective_outputs();
            if outputs.len() <= 1 {
                return None;
            }
            outputs.iter().find(|o| o.name == pin_name).and_then(|o| {
                if o.accessor.is_empty() {
                    None
                } else {
                    Some(o.accessor.clone())
                }
            })
        }

        match self.data_resolver.get_input_source(node_id, pin_id) {
            Some(DataSource::Connection {
                source_node_id,
                source_pin,
            }) => {
                let source_node = self
                    .graph
                    .nodes
                    .get(source_node_id)
                    .ok_or_else(|| GraphyError::NodeNotFound(source_node_id.clone()))?;

                // Check if source is a component property getter (pure)
                if source_node.node_type.starts_with("comp_get_prop::") {
                    return self.generate_comp_get_prop_expr(source_node);
                }

                // Check if source is an identity reference producer (#654):
                // get_component_ref / find_object_by_* / object_ref_literal.
                if source_node
                    .node_type
                    .strip_prefix("get_component_ref::")
                    .is_some()
                    || source_node.node_type == "find_object_by_stable_id"
                    || source_node.node_type == "find_object_by_name"
                    || source_node.node_type == "object_ref_literal"
                {
                    return self.identity_producer_expr(source_node_id);
                }

                // Check if source is a variable getter
                if let Some(var_name) = source_node
                    .node_type
                    .strip_prefix("get_")
                    .filter(|name| self.variables.contains_key(*name))
                {
                    let var_type = self.variables.get(var_name).ok_or_else(|| {
                        GraphyError::Custom(format!("Variable '{}' not found", var_name))
                    })?;

                    let static_name = to_static_var_name(var_name);

                    let is_copy = is_copy_type(var_type);
                    return if is_copy {
                        Ok(format!(
                            "{}.with(|v| __pbgc_get_copy(v, \"{}\"))",
                            static_name, var_name
                        ))
                    } else {
                        Ok(format!(
                            "{}.with(|v| __pbgc_get_clone(v, \"{}\"))",
                            static_name, var_name
                        ))
                    };
                }

                // Check if source is pure. Prefer precomputed temporary results
                // to avoid duplicating pure function calls across downstream uses.
                if let Some(node_meta) = self
                    .metadata_provider
                    .get_node_metadata(&source_node.node_type)
                {
                    if node_meta.node_type == NodeTypes::pure {
                        if self.should_materialize_pure_result(source_node_id) {
                            if let Some(var_name) =
                                self.data_resolver.get_result_variable(source_node_id)
                            {
                                // Check for multi-output accessor
                                if let Some(acc) = get_multi_output_accessor(
                                    self.metadata_provider,
                                    self.graph,
                                    source_node_id,
                                    &source_pin,
                                ) {
                                    return Ok(format!("{}{}", var_name, acc));
                                }
                                return Ok(var_name.clone());
                            }
                        }
                        let expr = self.generate_pure_node_expression(source_node)?;
                        // Check for multi-output accessor on the inline expression
                        if let Some(acc) = get_multi_output_accessor(
                            self.metadata_provider,
                            self.graph,
                            source_node_id,
                            &source_pin,
                        ) {
                            return Ok(format!("({}){}", expr, acc));
                        }
                        return Ok(expr);
                    }
                }

                // Non-pure: use result variable
                if let Some(var_name) = self.data_resolver.get_result_variable(source_node_id) {
                    // Check for multi-output accessor
                    if let Some(acc) = get_multi_output_accessor(
                        self.metadata_provider,
                        self.graph,
                        source_node_id,
                        &source_pin,
                    ) {
                        Ok(format!("{}{}", var_name, acc))
                    } else {
                        Ok(var_name.clone())
                    }
                } else {
                    Err(GraphyError::Custom(format!(
                        "No variable for source node: {}",
                        source_node_id
                    )))
                }
            }
            Some(DataSource::Constant(value)) => Ok(value.clone()),
            Some(DataSource::Default) => {
                // Use default value for the type
                if let Some(node) = self.graph.nodes.get(node_id) {
                    if let Some(pin) = node.inputs.iter().find(|p| p.id == pin_id) {
                        Ok(get_default_value(&pin.pin.data_type))
                    } else {
                        Err(GraphyError::PinNotFound {
                            node: node_id.to_string(),
                            pin: pin_id.to_string(),
                        })
                    }
                } else {
                    Err(GraphyError::NodeNotFound(node_id.to_string()))
                }
            }
            None => Err(GraphyError::Custom(format!(
                "No data source for input: {}.{}",
                node_id, pin_id
            ))),
        }
    }

    /// Generate inlined expression for a pure node
    fn generate_pure_node_expression(&self, node: &NodeInstance) -> Result<String, GraphyError> {
        let node_meta = self
            .metadata_provider
            .get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?;

        // Recursively generate arguments
        let mut args = Vec::new();
        for param in &node_meta.params {
            // Find the actual pin ID from the node instance
            let pin_id = node
                .inputs
                .iter()
                .find(|input| input.pin.name == param.name)
                .map(|input| input.id.clone())
                .ok_or_else(|| {
                    GraphyError::Custom(format!(
                        "Input pin not found for parameter '{}' on node '{}'",
                        param.name, node.id
                    ))
                })?;

            let arg_expr = self.generate_input_expression(&node.id, &pin_id)?;
            args.push(arg_expr);
        }

        Ok(format!("{}({})", node_meta.name, args.join(", ")))
    }

    /// Clone with new visited set
    fn clone_with_new_visited(&self) -> Self {
        Self {
            graph: self.graph,
            metadata_provider: self.metadata_provider,
            data_resolver: self.data_resolver,
            exec_routing: self.exec_routing,
            variables: self.variables.clone(),
            visited: HashSet::new(),
        }
    }
}

/// Check if a type is Copy (uses Cell) or not (uses RefCell)
fn is_copy_type(type_str: &str) -> bool {
    matches!(
        type_str,
        "i32"
            | "i64"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
            | "bool"
            | "char"
            | "usize"
            | "isize"
            | "i8"
            | "i16"
            | "u8"
            | "u16"
    )
}

fn to_static_var_name(var_name: &str) -> String {
    let mut out = String::from("PBGC_VAR_");

    for ch in var_name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }

    if out.ends_with('_') {
        out.push('X');
    }

    out
}

/// Check whether any downstream data consumer reads the result of this node.
///
/// Used to decide whether to emit a `let` binding for a component call result.
fn has_returns_used(node: &NodeInstance) -> bool {
    node.outputs
        .iter()
        .any(|p| !matches!(p.pin.data_type, graphy::DataType::Exec))
}

/// Get default value for a data type
fn get_default_value(data_type: &graphy::DataType) -> String {
    use graphy::DataType;

    match data_type {
        DataType::Exec => "()".to_string(),
        DataType::Data(ti) => graphy::utils::get_default_value_for_type(&ti.type_string),
    }
}

/// Convert a kebab-case or snake_case string to PascalCase.
fn pascal_case(name: &str) -> String {
    name.split(|c: char| c == '_' || c == '-')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.push_str(chars.as_str());
                    s
                }
            }
        })
        .collect()
}
