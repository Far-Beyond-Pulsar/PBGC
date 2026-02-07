//! # Pulsar Blueprint Graph Compiler (PBGC)
//!
//! Production-ready compiler for transforming Pulsar Blueprint visual node graphs
//! into executable Rust source code.
//!
//! PBGC builds on the [graphy](https://github.com/yourusername/graphy) library,
//! providing Blueprint-specific functionality including:
//! - Integration with `pulsar_std` node registry
//! - Rust code generation optimized for Blueprints
//! - Blueprint class variables with thread-safe wrappers
//! - Support for event nodes, getters/setters, and control flow
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use pbgc::compile_graph;
//! use graphy::GraphDescription;
//!
//! let graph = GraphDescription::new("my_blueprint");
//! // ... build graph with nodes and connections
//!
//! match compile_graph(&graph) {
//!     Ok(rust_code) => {
//!         std::fs::write("generated.rs", rust_code)?;
//!     }
//!     Err(e) => eprintln!("Compilation failed: {}", e),
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## Architecture
//!
//! PBGC follows a multi-phase compilation pipeline:
//!
//! 1. **Metadata Loading** - Extract node definitions from pulsar_std
//! 2. **Graph Expansion** - Inline sub-graphs (optional)
//! 3. **Data Flow Analysis** - Build dependency graph (via Graphy)
//! 4. **Execution Flow Analysis** - Map execution connections (via Graphy)
//! 5. **Code Generation** - Generate Rust code with Blueprint-specific logic

pub mod metadata;
pub mod codegen;
pub mod compiler;

// Re-export the main compilation API
pub use compiler::{
    compile_graph,
    compile_graph_with_library_manager,
    compile_graph_with_variables,
};

// Re-export Graphy types for convenience
pub use graphy::{
    GraphDescription, NodeInstance, Connection, Pin, PinInstance,
    DataType, NodeTypes, Position, ConnectionType, PropertyValue,
    GraphMetadata, Result, GraphyError,
};

// Re-export metadata types
pub use metadata::{
    BlueprintMetadataProvider,
    extract_node_metadata,
};
