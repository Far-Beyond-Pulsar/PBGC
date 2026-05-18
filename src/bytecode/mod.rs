use serde::{Deserialize, Serialize};

pub type SlotId  = u32;
pub type LabelId = u32;

/// A single bytecode instruction.
///
/// All values move through the system as raw `u64` bit-patterns in a flat slot table.
/// No value enum, no type dispatch in the VM loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    LoadI64 { slot: SlotId, value: i64 },
    LoadF64 { slot: SlotId, value: f64 },
    LoadI32 { slot: SlotId, value: i32 },
    LoadF32 { slot: SlotId, value: f32 },

    /// Direct function call — no dispatch table, no lookup.
    ///
    /// `fn_ptr` is the address of `__bp_dispatch_<name>` in the native cdylib,
    /// patched once into the program by the executor before execution.
    /// The VM transmutes it and calls it directly — one pointer dereference.
    ///
    /// `node_type` is kept only for human-readable debug output and
    /// serde roundtrip; the VM never reads it.
    Call {
        fn_ptr:    u64,
        node_type: String,
        inputs:    Vec<SlotId>,
        output:    Option<SlotId>,
    },

    JumpIf { condition: SlotId, true_label: LabelId, false_label: LabelId },
    Jump(LabelId),
    Label(LabelId),
    Return,
}

/// A compiled blueprint program.
///
/// After compilation, `fn_ptr` fields in every `Call` instruction are zero.
/// Call `BpExecutor::prepare` to resolve them from the native cdylib.
/// After preparation the program is self-contained: `pbgc::vm::run` takes
/// only `&BpProgram` — no dispatch table argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpProgram {
    pub name:        String,
    pub slot_count:  u32,
    pub instructions: Vec<Instruction>,
}

impl BpProgram {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), slot_count: 0, instructions: Vec::new() }
    }
}
