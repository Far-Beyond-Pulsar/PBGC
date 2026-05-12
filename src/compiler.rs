//! # Blueprint Compiler
//!
//! Main entry points for compiling Blueprint graphs to Rust code.

use crate::metadata::{BlueprintMetadataProvider, get_node_metadata};
use crate::codegen::BlueprintCodeGenerator;
use graphy::{GraphDescription, GraphyError, DataResolver, ExecutionRouting};
use std::collections::HashMap;

/// Compile a Blueprint graph to Rust source code
///
/// This is the main entry point for the Blueprint compiler. It takes a visual node graph
/// and generates equivalent Rust source code that can be compiled and executed.
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code
/// * `Err(GraphyError)` - A descriptive error if compilation fails
///
/// # Examples
///
/// ```rust,no_run
/// use pbgc::compile_graph;
/// use graphy::GraphDescription;
///
/// let graph = GraphDescription::new("test");
/// match compile_graph(&graph) {
///     Ok(code) => println!("Generated:\n{}", code),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn compile_graph(graph: &GraphDescription) -> Result<String, GraphyError> {
    compile_graph_with_library_manager(graph, None)
}

/// Compile a Blueprint graph with sub-graph expansion support
///
/// This extended version of `compile_graph` supports expanding sub-graph instances
/// before compilation. Sub-graphs are Blueprint macros that can be instantiated
/// multiple times within a graph.
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
/// * `library_manager` - Optional library manager providing sub-graph definitions
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code
/// * `Err(GraphyError)` - A descriptive error if compilation fails
pub fn compile_graph_with_library_manager(
    graph: &GraphDescription,
    _library_manager: Option<()>, // TODO: Define LibraryManager type
) -> Result<String, GraphyError> {
    tracing::info!("[PBGC] Starting Blueprint compilation");
    tracing::info!("[PBGC] Graph: {} ({} nodes, {} connections)",
        graph.metadata.name,
        graph.nodes.len(),
        graph.connections.len());

    // Create a mutable copy for expansion
    let expanded_graph = graph.clone();

    // Phase 0: Expand sub-graphs if library manager is provided
    // TODO: Implement sub-graph expansion
    // if let Some(lib_manager) = library_manager {
    //     tracing::info!("[PBGC] Phase 0: Expanding sub-graphs...");
    //     expander.expand_all(&mut expanded_graph)?;
    // }

    // Phase 1: Get node metadata
    tracing::info!("[PBGC] Phase 1: Loading node metadata...");
    let metadata_provider = BlueprintMetadataProvider::new();
    tracing::info!("[PBGC] Loaded {} node types", get_node_metadata().len());

    // Phase 2: Build data flow resolver
    tracing::info!("[PBGC] Phase 2: Analyzing data flow...");
    let data_resolver = DataResolver::build(&expanded_graph, &metadata_provider)?;
    tracing::info!("[PBGC] Data flow analysis complete");
    tracing::info!("[PBGC]   - {} pure nodes in evaluation order",
        data_resolver.get_pure_evaluation_order().len());

    // Phase 3: Build execution routing
    tracing::info!("[PBGC] Phase 3: Analyzing execution flow...");
    let exec_routing = ExecutionRouting::build_from_graph(&expanded_graph);
    tracing::info!("[PBGC] Execution flow analysis complete");

    // Phase 4: Generate code
    tracing::info!("[PBGC] Phase 4: Generating Rust code...");
    let variables = HashMap::new();
    let code_generator = BlueprintCodeGenerator::new(
        &expanded_graph,
        &metadata_provider,
        &data_resolver,
        &exec_routing,
        variables,
    );
    let code = code_generator.generate_program()?;

    tracing::info!("[PBGC] Code generation complete ({} bytes)", code.len());
    tracing::info!("[PBGC] Compilation successful!");

    Ok(code)
}

/// Compile a graph with class variables
///
/// This variant supports Blueprint classes with member variables. The variables
/// are generated with appropriate thread-safe wrappers (Cell/RefCell + Arc).
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
/// * `variables` - Map of variable names to their Rust types
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code including variable declarations
/// * `Err(GraphyError)` - A descriptive error if compilation fails
pub fn compile_graph_with_variables(
    graph: &GraphDescription,
    variables: HashMap<String, String>,
) -> Result<String, GraphyError> {
    tracing::info!("[PBGC] Compiling with {} class variables", variables.len());

    let metadata_provider = BlueprintMetadataProvider::new();
    let data_resolver = DataResolver::build(&graph, &metadata_provider)?;
    let exec_routing = ExecutionRouting::build_from_graph(&graph);

    let code_generator = BlueprintCodeGenerator::new(
        &graph,
        &metadata_provider,
        &data_resolver,
        &exec_routing,
        variables,
    );

    code_generator.generate_program()
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphy::{Connection, ConnectionType, DataType, Pin, PinInstance, PinType, Position};
    use std::collections::HashSet;

    #[test]
    fn branch_condition_from_pure_chain_inlines_single_use_pure_nodes() {
        let mut graph = GraphDescription::new("branch_pure_chain");

        let mut begin = graphy::NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
        begin.outputs.push(PinInstance::new(
            "begin_exec",
            Pin::new("begin_exec", "Body", DataType::Execution, PinType::Output),
        ));

        let mut add = graphy::NodeInstance::new("add_node", "add", Position { x: 100.0, y: 0.0 });
        add.inputs.push(PinInstance::new(
            "add_a",
            Pin::new("add_a", "a", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
        ));
        add.inputs.push(PinInstance::new(
            "add_b",
            Pin::new("add_b", "b", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
        ));
        add.outputs.push(PinInstance::new(
            "add_result",
            Pin::new("add_result", "result", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Output),
        ));
        add.properties.insert("add_a".to_string(), graphy::PropertyValue::Number(1.0));
        add.properties.insert("add_b".to_string(), graphy::PropertyValue::Number(3.0));

        let mut gt = graphy::NodeInstance::new("gt_node", "greater_than", Position { x: 200.0, y: 0.0 });
        gt.inputs.push(PinInstance::new(
            "gt_a",
            Pin::new("gt_a", "a", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
        ));
        gt.inputs.push(PinInstance::new(
            "gt_b",
            Pin::new("gt_b", "b", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
        ));
        gt.outputs.push(PinInstance::new(
            "gt_result",
            Pin::new("gt_result", "result", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Output),
        ));
        gt.properties.insert("gt_b".to_string(), graphy::PropertyValue::Number(3.0));

        let mut branch = graphy::NodeInstance::new("branch_node", "branch", Position { x: 300.0, y: 0.0 });
        branch.inputs.push(PinInstance::new(
            "branch_exec",
            Pin::new("branch_exec", "exec", DataType::Execution, PinType::Input),
        ));
        branch.inputs.push(PinInstance::new(
            "branch_condition",
            Pin::new("branch_condition", "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input),
        ));
        branch.outputs.push(PinInstance::new(
            "branch_true",
            Pin::new("branch_true", "True", DataType::Execution, PinType::Output),
        ));
        branch.outputs.push(PinInstance::new(
            "branch_false",
            Pin::new("branch_false", "False", DataType::Execution, PinType::Output),
        ));

        graph.add_node(begin);
        graph.add_node(add);
        graph.add_node(gt);
        graph.add_node(branch);

        graph.add_connection(Connection::new(
            "begin",
            "begin_exec",
            "branch_node",
            "branch_exec",
            ConnectionType::Execution,
        ));
        graph.add_connection(Connection::new(
            "add_node",
            "add_result",
            "gt_node",
            "gt_a",
            ConnectionType::Data,
        ));
        graph.add_connection(Connection::new(
            "gt_node",
            "gt_result",
            "branch_node",
            "branch_condition",
            ConnectionType::Data,
        ));

        let code = compile_graph(&graph).expect("branch graph should compile");

        assert!(!code.contains("let node_add_node_result"));
        assert!(!code.contains("let node_gt_node_result"));
        assert!(code.contains("if greater_than"));

        let mut declared = HashSet::new();
        for line in code.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("let ") {
                if let Some(ident) = rest.split_whitespace().next() {
                    let ident = ident.trim_end_matches('=');
                    declared.insert(ident.to_string());
                }
            }
        }

        for line in code.lines() {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("if ") {
                let cond_ident = rest
                    .split(|c: char| c.is_whitespace() || c == '{' || c == '(')
                    .next()
                    .unwrap_or("");

                if cond_ident.starts_with("node_") && cond_ident.ends_with("_result") {
                    assert!(
                        declared.contains(cond_ident),
                        "branch condition references undeclared temporary: {}\nGenerated code:\n{}",
                        cond_ident,
                        code
                    );
                }
            }
        }
    }
}
