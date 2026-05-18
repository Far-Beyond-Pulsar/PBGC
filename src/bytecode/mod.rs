use serde::{Deserialize, Serialize};

pub type SlotId  = u32;
pub type LabelId = u32;

/// A single bytecode instruction.
///
/// All values transit the system as raw `u64` bit-patterns in the slot table.
/// Typed `Load*` variants store the correct bit representation at compile time;
/// `Call` lets the native dispatch shim reinterpret each slot as the right type.
/// No value enum — no runtime type dispatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    /// Store a 64-bit integer constant into a slot.
    LoadI64 { slot: SlotId, value: i64 },
    /// Store a 64-bit float constant into a slot (stored as its bit pattern).
    LoadF64 { slot: SlotId, value: f64 },
    /// Store a 32-bit integer (or bool as 0/1) constant into a slot.
    LoadI32 { slot: SlotId, value: i32 },
    /// Store a 32-bit float constant into a slot (stored as bits in low 32).
    LoadF32 { slot: SlotId, value: f32 },

    /// Call a node function via its pre-resolved native dispatch shim.
    /// `node_type_idx` indexes `BpProgram::node_types`.
    /// `inputs` are slot IDs read as `u64` and passed to the shim.
    /// `output` is the slot the shim writes its result into (None for void).
    Call {
        node_type_idx: u32,
        inputs: Vec<SlotId>,
        output: Option<SlotId>,
    },

    /// Conditional branch: jumps to `true_label` if slot != 0, else `false_label`.
    JumpIf {
        condition: SlotId,
        true_label: LabelId,
        false_label: LabelId,
    },

    /// Unconditional jump.
    Jump(LabelId),

    /// Label target — no-op at runtime.
    Label(LabelId),

    /// End execution of this program.
    Return,
}

/// A compiled blueprint program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpProgram {
    /// Name of the event/entry-point (e.g. `"begin_play"`).
    pub name: String,
    /// Total number of `u64` slots required.
    pub slot_count: u32,
    pub instructions: Vec<Instruction>,
    /// Interned node type strings. `Instruction::Call::node_type_idx` indexes this.
    /// Used by the executor to look up `__bp_dispatch_<name>` from the native lib.
    pub node_types: Vec<String>,
}

impl BpProgram {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), slot_count: 0, instructions: Vec::new(), node_types: Vec::new() }
    }
}
