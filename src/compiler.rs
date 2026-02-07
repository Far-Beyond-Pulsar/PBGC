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
