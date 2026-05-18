use crate::bytecode::{BpProgram, Instruction, LabelId};
use std::collections::HashMap;

#[derive(Debug)]
pub enum VmError {
    SlotOutOfBounds(u32),
    LabelNotFound(LabelId),
    UnresolvedCall(String),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::SlotOutOfBounds(s)  => write!(f, "slot {} out of bounds", s),
            VmError::LabelNotFound(l)    => write!(f, "label {} not found", l),
            VmError::UnresolvedCall(n)   => write!(f, "unresolved call '{}' — forgot to prepare?", n),
        }
    }
}
impl std::error::Error for VmError {}

/// ABI of every `__bp_dispatch_<name>` symbol.
/// Not used as a Vec or table — only as a transmute target inside the hot loop.
pub type DispatchFn = unsafe extern "C" fn(inputs: *const u64, output: *mut u64);

/// Execute a prepared `BpProgram`.
///
/// `fn_ptr` in every `Call` instruction must have been patched by the executor
/// before calling this. The loop dereferences each pointer directly — one
/// transmute and one call per instruction, nothing else.
pub fn run(program: &BpProgram) -> Result<(), VmError> {
    let mut slots   = vec![0u64; program.slot_count as usize];
    let labels      = build_labels(&program.instructions);
    let mut scratch = [0u64; 32];
    let mut pc      = 0usize;

    loop {
        if pc >= program.instructions.len() { break; }

        match &program.instructions[pc] {
            Instruction::LoadI64 { slot, value } => {
                slots[*slot as usize] = *value as u64;
            }
            Instruction::LoadF64 { slot, value } => {
                slots[*slot as usize] = value.to_bits();
            }
            Instruction::LoadI32 { slot, value } => {
                slots[*slot as usize] = *value as u32 as u64;
            }
            Instruction::LoadF32 { slot, value } => {
                slots[*slot as usize] = value.to_bits() as u64;
            }

            Instruction::Call { fn_ptr, node_type, inputs, output } => {
                if *fn_ptr == 0 {
                    return Err(VmError::UnresolvedCall(node_type.clone()));
                }
                for (i, &s) in inputs.iter().enumerate() {
                    scratch[i] = slots[s as usize];
                }
                let mut result = 0u64;
                // SAFETY: fn_ptr was resolved from the native cdylib by the executor.
                unsafe {
                    let f: DispatchFn = std::mem::transmute(*fn_ptr);
                    f(scratch.as_ptr(), &mut result);
                }
                if let Some(out) = output {
                    slots[*out as usize] = result;
                }
            }

            Instruction::JumpIf { condition, true_label, false_label } => {
                let target = if slots[*condition as usize] != 0 { *true_label } else { *false_label };
                pc = labels[&target] + 1;
                continue;
            }
            Instruction::Jump(label) => {
                pc = labels[label] + 1;
                continue;
            }
            Instruction::Label(_) => {}
            Instruction::Return => break,
        }
        pc += 1;
    }
    Ok(())
}

fn build_labels(instructions: &[Instruction]) -> HashMap<LabelId, usize> {
    instructions.iter().enumerate()
        .filter_map(|(i, instr)| if let Instruction::Label(id) = instr { Some((*id, i)) } else { None })
        .collect()
}
