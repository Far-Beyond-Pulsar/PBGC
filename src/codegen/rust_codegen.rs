//! # Rust Code Generator for Blueprints
//!
//! Generates Rust source code from Blueprint graphs.

use crate::metadata::BlueprintMetadataProvider;
use graphy::{
    GraphDescription, GraphyError, NodeTypes, NodeInstance,
    DataResolver, ExecutionRouting,
};
use graphy::core::NodeMetadataProvider;
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

        // Add imports
        code.push_str("// NOTE: Replace with actual pulsar_std import in production\n");
        code.push_str("// use pulsar_std::*;\n\n");

        // Collect node-specific imports
        let node_imports = self.collect_node_imports();
        for import_stmt in node_imports {
            code.push_str(&import_stmt);
            code.push_str("\n");
        }
        code.push_str("\n");

        // Find event nodes
        let event_nodes: Vec<_> = self.graph
            .nodes
            .values()
            .filter(|node| {
                self.metadata_provider
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

    /// Generate an event function
    fn generate_event_function(&self, event_node: &NodeInstance) -> Result<String, GraphyError> {
        let mut code = String::new();

        // Get event metadata
        let metadata = self.metadata_provider
            .get_node_metadata(&event_node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(event_node.node_type.clone()))?;

        // Generate function signature
        code.push_str(&format!("pub fn {}() {{\n", metadata.name));

        // Find execution output pins and follow them
        // We need to look up by pin ID (from the node instance), not pin name (from metadata)
        for output_pin in &event_node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Execution) {
                tracing::debug!("[CODEGEN] Looking up exec connections for node {} pin ID: {}", 
                    event_node.id, output_pin.id);
                
                let connected = self.exec_routing.get_connected_nodes(&event_node.id, &output_pin.id);
                
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

    /// Generate execution chain starting from a node
    fn generate_exec_chain(&mut self, node: &NodeInstance, indent_level: usize) -> Result<String, GraphyError> {
        let mut code = String::new();

        // Prevent infinite loops
        if self.visited.contains(&node.id) {
            return Ok(code);
        }
        self.visited.insert(node.id.clone());

        // Check if this is a variable getter or setter
        if node.node_type.starts_with("get_") {
            // Getter nodes are pure (no exec chain), skip
            return Ok(code);
        } else if node.node_type.starts_with("set_") {
            // Setter nodes have exec chain
            return self.generate_setter_node(node, indent_level);
        }

        let node_meta = self.metadata_provider
            .get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?;

        match node_meta.node_type {
            NodeTypes::pure => {
                // Pure nodes are pre-evaluated, skip in exec chain
                Ok(code)
            }
            NodeTypes::fn_ => {
                self.generate_function_node(node, node_meta, indent_level)
            }
            NodeTypes::control_flow => {
                self.generate_control_flow_node(node, node_meta, indent_level)
            }
            NodeTypes::event => {
                // Event nodes define the outer function, skip in exec chain
                Ok(code)
            }
        }
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

        // Check if this function returns a value
        let has_return = node_meta.return_type.is_some();

        if has_return {
            // Store result in variable
            let result_var = self.data_resolver
                .get_result_variable(&node.id)
                .ok_or_else(|| GraphyError::Custom(format!("No result variable for node: {}", node.id)))?;

            code.push_str(&format!(
                "{}let {} = {}({});\n",
                indent,
                result_var,
                node_meta.name,
                args.join(", ")
            ));
        } else {
            // Just call the function
            code.push_str(&format!(
                "{}{}({});\n",
                indent,
                node_meta.name,
                args.join(", ")
            ));
        }

        // Follow execution chain - look up by actual pin IDs from node instance
        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Execution) {
                let connected = self.exec_routing.get_connected_nodes(&node.id, &output_pin.id);
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
            if matches!(output_pin.pin.data_type, graphy::DataType::Execution) {
                let connected = self.exec_routing.get_connected_nodes(&node.id, &output_pin.id);

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
            let pin_id = node.inputs.iter()
                .find(|input| input.pin.name == param.name)
                .map(|input| input.id.clone())
                .ok_or_else(|| GraphyError::Custom(
                    format!("Input pin not found for parameter '{}' on node '{}'", param.name, node.id)
                ))?;

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
    fn generate_setter_node(&mut self, node: &NodeInstance, indent_level: usize) -> Result<String, GraphyError> {
        let mut code = String::new();
        let indent = "    ".repeat(indent_level);

        // Extract variable name from node type (remove "set_" prefix)
        let var_name = node.node_type
            .strip_prefix("set_")
            .ok_or_else(|| GraphyError::Custom(format!("Invalid setter node type: {}", node.node_type)))?;

        // Find the "value" input pin ID
        let value_pin_id = node.inputs.iter()
            .find(|input| input.pin.name == "value")
            .map(|input| input.id.clone())
            .ok_or_else(|| GraphyError::Custom(format!("Value input not found on setter node: {}", node.id)))?;

        // Get the value to set
        let value_expr = self.generate_input_expression(&node.id, &value_pin_id)?;

        // Get variable type to determine Cell vs RefCell
        let var_type = self.variables
            .get(var_name)
            .ok_or_else(|| GraphyError::Custom(format!("Variable '{}' not found", var_name)))?;

        // Generate setter code
        let is_copy_type = is_copy_type(var_type);
        if is_copy_type {
            code.push_str(&format!(
                "{}{}.with(|v| v.set({}));\n",
                indent,
                var_name.to_uppercase(),
                value_expr
            ));
        } else {
            code.push_str(&format!(
                "{}{}.with(|v| *v.borrow_mut() = {});\n",
                indent,
                var_name.to_uppercase(),
                value_expr
            ));
        }

        // Follow execution chain - use actual pin IDs from node instance
        for output_pin in &node.outputs {
            if matches!(output_pin.pin.data_type, graphy::DataType::Execution) {
                let connected = self.exec_routing.get_connected_nodes(&node.id, &output_pin.id);
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
    fn collect_arguments(&self, node: &NodeInstance, node_meta: &graphy::core::NodeMetadata) -> Result<Vec<String>, GraphyError> {
        let mut args = Vec::new();

        for param in &node_meta.params {
            // Find the actual pin ID from the node instance
            // Pin IDs are typically "{node_id}_{param_name}"
            let pin_id = node.inputs.iter()
                .find(|input| {
                    // Match by name - the pin's name should match the param name
                    input.pin.name == param.name
                })
                .map(|input| input.id.clone())
                .ok_or_else(|| GraphyError::Custom(
                    format!("Input pin not found for parameter '{}' on node '{}'", param.name, node.id)
                ))?;

            let value = self.generate_input_expression(&node.id, &pin_id)?;
            args.push(value);
        }

        Ok(args)
    }

    /// Generate expression for an input value
    /// pin_id should be the actual pin ID from the node instance (e.g., "print_1_value")
    fn generate_input_expression(&self, node_id: &str, pin_id: &str) -> Result<String, GraphyError> {
        use graphy::analysis::DataSource;

        match self.data_resolver.get_input_source(node_id, pin_id) {
            Some(DataSource::Connection { source_node_id, source_pin }) => {
                let source_node = self.graph.nodes.get(source_node_id)
                    .ok_or_else(|| GraphyError::NodeNotFound(source_node_id.clone()))?;

                // Check if source is a variable getter
                if source_node.node_type.starts_with("get_") {
                    let var_name = source_node.node_type.strip_prefix("get_").unwrap();
                    let var_type = self.variables.get(var_name)
                        .ok_or_else(|| GraphyError::Custom(format!("Variable '{}' not found", var_name)))?;

                    let is_copy = is_copy_type(var_type);
                    return if is_copy {
                        Ok(format!("{}.with(|v| v.get())", var_name.to_uppercase()))
                    } else {
                        Ok(format!("{}.with(|v| v.borrow().clone())", var_name.to_uppercase()))
                    };
                }

                // Check if source is pure - if so, inline it
                if let Some(node_meta) = self.metadata_provider.get_node_metadata(&source_node.node_type) {
                    if node_meta.node_type == NodeTypes::pure {
                        return self.generate_pure_node_expression(source_node);
                    }
                }

                // Non-pure: use result variable
                if let Some(var_name) = self.data_resolver.get_result_variable(source_node_id) {
                    Ok(var_name.clone())
                } else {
                    Err(GraphyError::Custom(format!("No variable for source node: {}", source_node_id)))
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
            None => Err(GraphyError::Custom(format!("No data source for input: {}.{}", node_id, pin_id))),
        }
    }

    /// Generate inlined expression for a pure node
    fn generate_pure_node_expression(&self, node: &NodeInstance) -> Result<String, GraphyError> {
        let node_meta = self.metadata_provider
            .get_node_metadata(&node.node_type)
            .ok_or_else(|| GraphyError::NodeNotFound(node.node_type.clone()))?;

        // Recursively generate arguments
        let mut args = Vec::new();
        for param in &node_meta.params {
            let arg_expr = self.generate_input_expression(&node.id, &param.name)?;
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
        "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "bool" | "char" |
        "usize" | "isize" | "i8" | "i16" | "u8" | "u16"
    )
}

/// Get default value for a data type
fn get_default_value(data_type: &graphy::DataType) -> String {
    use graphy::DataType;

    match data_type {
        DataType::Execution => "()".to_string(),
        DataType::Typed(type_info) => {
            graphy::utils::get_default_value_for_type(&type_info.type_string)
        }
        DataType::Number => "0.0".to_string(),
        DataType::String => "String::new()".to_string(),
        DataType::Boolean => "false".to_string(),
        DataType::Vector2 => "(0.0, 0.0)".to_string(),
        DataType::Vector3 => "(0.0, 0.0, 0.0)".to_string(),
        DataType::Color => "(0.0, 0.0, 0.0, 1.0)".to_string(),
        DataType::Any => "Default::default()".to_string(),
    }
}
