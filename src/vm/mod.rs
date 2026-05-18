use crate::bytecode::{BpProgram, Instruction, LabelId};
use std::collections::HashMap;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum VmError {
    SlotOutOfBounds(u32),
    LabelNotFound(LabelId),
    /// A dispatch function index had no corresponding shim in the table.
    MissingDispatch(usize),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::SlotOutOfBounds(s)   => write!(f, "slot {} out of bounds", s),
            VmError::LabelNotFound(l)     => write!(f, "label {} not found", l),
            VmError::MissingDispatch(idx) => write!(f, "no dispatch shim at index {}", idx),
        }
    }
}

impl std::error::Error for VmError {}

// ── Dispatch function type ────────────────────────────────────────────────────

/// Signature of every `__bp_dispatch_<name>` shim in the native lib.
///
/// - `inputs`:  pointer to contiguous `u64` slot values (one per parameter)
/// - `output`:  pointer to a single `u64` that receives the return value
///              (ignored for void functions)
///
/// The shim knows its own parameter types and performs the cast itself —
/// no type information is needed by the VM.
pub type DispatchFn = unsafe extern "C" fn(inputs: *const u64, output: *mut u64);

// ── VM execution ──────────────────────────────────────────────────────────────

/// Execute a compiled blueprint program.
///
/// `dispatch` is a slice of function pointers indexed by `node_type_idx` in
/// every `Call` instruction. The executor (in `pulsar_bp_executor`) builds this
/// slice by resolving `__bp_dispatch_<name>` symbols from the native lib.
///
/// Slot table: `Vec<u64>` — raw 64-bit words, typed by the shim at each call.
/// No allocation in the hot loop beyond the initial slot vec.
pub fn run(program: &BpProgram, dispatch: &[DispatchFn]) -> Result<(), VmError> {
    let mut slots = vec![0u64; program.slot_count as usize];
    let label_table = build_label_table(&program.instructions);
    // Scratch buffer for passing inputs to shims — no per-call allocation.
    let mut scratch = [0u64; 32];
    let mut pc = 0usize;

    loop {
        if pc >= program.instructions.len() {
            break;
        }

        match &program.instructions[pc] {
            Instruction::LoadI64 { slot, value } => {
                *slot_mut(&mut slots, *slot)? = *value as u64;
                pc += 1;
            }
            Instruction::LoadF64 { slot, value } => {
                *slot_mut(&mut slots, *slot)? = value.to_bits();
                pc += 1;
            }
            Instruction::LoadI32 { slot, value } => {
                *slot_mut(&mut slots, *slot)? = *value as u32 as u64;
                pc += 1;
            }
            Instruction::LoadF32 { slot, value } => {
                *slot_mut(&mut slots, *slot)? = value.to_bits() as u64;
                pc += 1;
            }

            Instruction::Call { node_type_idx, inputs, output } => {
                let func = dispatch
                    .get(*node_type_idx as usize)
                    .copied()
                    .ok_or(VmError::MissingDispatch(*node_type_idx as usize))?;

                // Fill scratch from input slots — no heap allocation.
                for (i, &sid) in inputs.iter().enumerate() {
                    scratch[i] = *slot_ref(&slots, sid)?;
                }

                let mut result = 0u64;
                // SAFETY: the shim is a valid function pointer from the native lib.
                unsafe { func(scratch.as_ptr(), &mut result); }

                if let Some(out) = output {
                    *slot_mut(&mut slots, *out)? = result;
                }
                pc += 1;
            }

            Instruction::JumpIf { condition, true_label, false_label } => {
                let cond = *slot_ref(&slots, *condition)? != 0;
                let target = if cond { *true_label } else { *false_label };
                pc = resolve_label(&label_table, target)? + 1;
            }

            Instruction::Jump(label) => {
                pc = resolve_label(&label_table, *label)? + 1;
            }

            Instruction::Label(_) => { pc += 1; }

            Instruction::Return => break,
        }
    }

    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_label_table(instructions: &[Instruction]) -> HashMap<LabelId, usize> {
    instructions
        .iter()
        .enumerate()
        .filter_map(|(i, instr)| {
            if let Instruction::Label(id) = instr { Some((*id, i)) } else { None }
        })
        .collect()
}

fn resolve_label(table: &HashMap<LabelId, usize>, id: LabelId) -> Result<usize, VmError> {
    table.get(&id).copied().ok_or(VmError::LabelNotFound(id))
}

fn slot_ref(slots: &[u64], id: u32) -> Result<&u64, VmError> {
    slots.get(id as usize).ok_or(VmError::SlotOutOfBounds(id))
}

fn slot_mut(slots: &mut Vec<u64>, id: u32) -> Result<&mut u64, VmError> {
    let idx = id as usize;
    if idx >= slots.len() { return Err(VmError::SlotOutOfBounds(id)); }
    Ok(&mut slots[idx])
}
