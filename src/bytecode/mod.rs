use serde::{Deserialize, Serialize};
// Re-export from pulsar_std so that callers only need one import.
pub use pulsar_std::TypeSlot;

pub mod comp_ops;
pub use comp_ops::{parse_node_type, CompOpKind, ComponentOpRef};

pub type LabelId = usize;

/// A single bytecode instruction.
///
/// Values live in a flat byte arena. Every instruction references arena positions
/// by byte offset. The VM has zero knowledge of concrete Rust types: it only moves
/// pointers and calls pre-resolved function addresses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    /// Copy `bytes` into the arena at `offset`. Used to initialise constant inputs.
    InitBytes {
        offset: usize,
        bytes: Vec<u8>,
    },

    /// Store a `TypeSlot { size, align }` at `offset` inside the arena.
    ///
    /// Emitted by the codegen whenever a generic node's type parameter `T` has
    /// been resolved at graph-compile time.  The VM writes the struct into the
    /// arena; the dispatch function reads it via `type_slots[i]`.
    InitTypeSlot {
        offset: usize,
        size: usize,
        align: usize,
    },

    /// Direct function call — no dispatch table, no type lookup.
    ///
    /// `fn_ptr` is patched by the executor (via dlsym) before execution.
    /// `input_offsets` are byte offsets into the arena, one per argument.
    /// `output_offset` is the byte offset where the return value is written
    /// (ignored when `has_output` is false).
    /// `node_type` is retained only for the executor's dlsym resolution and
    /// debug output; the VM hot loop never reads it.
    /// `type_slot_offsets` are arena offsets of `TypeSlot` values, one per
    /// resolved generic type parameter.  Empty for fully-concrete functions.
    Call {
        fn_ptr: u64,
        node_type: String,
        input_offsets: Vec<usize>,
        output_offset: usize,
        has_output: bool,
        type_slot_offsets: Vec<usize>,
    },

    /// Copy bytes from one arena slot into another.
    LoadVar {
        source_offset: usize,
        output_offset: usize,
        size: usize,
    },

    /// Store bytes from an input slot into a variable slot.
    StoreVar {
        input_offset: usize,
        target_offset: usize,
        size: usize,
    },

    /// Branch on the bool byte at `condition_offset` (non-zero → true).
    JumpIf {
        condition_offset: usize,
        true_label: LabelId,
        false_label: LabelId,
    },
    Jump(LabelId),
    Label(LabelId),
    Return,
}

/// A compiled blueprint program ready to be prepared and run.
///
/// After compilation, all `fn_ptr` fields are zero.
/// Call `BpExecutor::prepare` to patch them from the native cdylib.
/// After preparation `pbgc::vm::run` takes only `&BpProgram`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpProgram {
    pub name: String,
    /// Total byte size of the arena required to execute this program.
    pub arena_size: usize,
    /// Peak number of input-pointer arguments across all Call instructions.
    /// Used to pre-allocate the arg_ptrs scratch buffer in the VM.
    pub max_args_count: usize,
    pub instructions: Vec<Instruction>,
}

impl BpProgram {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arena_size: 0,
            max_args_count: 0,
            instructions: Vec::new(),
        }
    }
}
