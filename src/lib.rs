pub mod bytecode;
pub mod codegen;
pub mod compiler;
pub mod disk;
pub mod metadata;
pub mod project;
pub mod vm;

// Rust source codegen
pub use compiler::{
    compile_graph, compile_graph_to_bytecode, compile_graph_to_bytecode_full,
    compile_graph_to_bytecode_with_variables, compile_graph_with_library_manager,
    compile_graph_with_variables, BytecodeCompilation,
};

// Bytecode types
pub use bytecode::comp_ops::{parse_node_type, CompOpKind, ComponentOpRef};
pub use bytecode::{BpProgram, Instruction, LabelId};

// VM execution APIs
pub use vm::{run, run_with_external_arena, DispatchFn, VmError};

// Disk / project
pub use disk::{compile_project, compile_project_generated};
pub use project::{
    compile_graph_to_actor_source, generate_blueprint_actor_source,
    generate_blueprint_actor_source_with_components, generate_project, CompiledBlueprint,
    CompiledComponent, CompiledVariable, GeneratedProject, ProjectSpec,
};

// Graphy re-exports
pub use graphy::core::TypeInfo;
pub use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, GraphMetadata, GraphyError, JsonValue,
    NodeInstance, NodeMetadata, NodeMetadataProvider, NodeTypes, Pin, PinInstance, PinType,
    Position, Result,
};

pub use metadata::{extract_node_metadata, BlueprintMetadataProvider};
