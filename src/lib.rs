pub mod bytecode;
pub mod codegen;
pub mod compiler;
pub mod disk;
pub mod metadata;
pub mod project;
pub mod vm;

// Rust source codegen
pub use compiler::{
    compile_graph,
    compile_graph_with_library_manager,
    compile_graph_with_variables,
    compile_graph_to_bytecode,
};

// Bytecode types
pub use bytecode::{BpProgram, Instruction, SlotId, LabelId};

// VM — run takes only &BpProgram, fn_ptrs embedded in instructions
pub use vm::{run, DispatchFn, VmError};

// Disk / project
pub use disk::{compile_project, compile_project_generated};
pub use project::{
    compile_graph_to_actor_source,
    CompiledBlueprint,
    GeneratedProject,
    ProjectSpec,
    generate_blueprint_actor_source,
    generate_project,
};

// Graphy re-exports
pub use graphy::{
    GraphDescription, NodeInstance, Connection, Pin, PinInstance,
    DataType, NodeTypes, Position, ConnectionType, PropertyValue,
    GraphMetadata, Result, GraphyError, PinType,
    NodeMetadata, NodeMetadataProvider,
};
pub use graphy::core::TypeInfo;

pub use metadata::{BlueprintMetadataProvider, extract_node_metadata};
