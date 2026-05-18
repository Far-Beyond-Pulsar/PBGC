use crate::bytecode::{BpProgram, Instruction, LabelId};
use std::collections::HashMap;

#[derive(Debug)]
pub enum VmError {
    LabelNotFound(LabelId),
    UnresolvedCall(String),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::LabelNotFound(l)  => write!(f, "label {} not found", l),
            VmError::UnresolvedCall(n) => write!(f, "unresolved call '{}' — forgot BpExecutor::prepare?", n),
        }
    }
}
impl std::error::Error for VmError {}

/// ABI of every `__bp_dispatch_<name>` symbol.
///
/// `args` is an array of pointers into the byte arena, one per input.
/// `ret` points to the output region in the arena (null for void functions).
pub type DispatchFn = unsafe extern "C" fn(args: *const *const u8, ret: *mut u8);

/// Execute a prepared `BpProgram`.
///
/// All `fn_ptr` fields in `Instruction::Call` must have been patched by
/// `BpExecutor::prepare` before calling this. The loop resolves labels once
/// at startup, then executes purely via pointer arithmetic and direct calls.
pub fn run(program: &BpProgram) -> Result<(), VmError> {
    // Allocate the byte arena. Vec<u64> guarantees 8-byte alignment of the base pointer.
    let mut arena = vec![0u64; (program.arena_size + 7) / 8];
    let base = arena.as_mut_ptr() as *mut u8;

    // Pre-allocate the argument-pointer scratch buffer once to avoid heap churn.
    let mut arg_ptrs: Vec<*const u8> = Vec::with_capacity(program.max_args_count.max(1));

    let labels = build_labels(&program.instructions);
    let mut pc = 0usize;

    loop {
        if pc >= program.instructions.len() {
            break;
        }
        match &program.instructions[pc] {
            Instruction::InitBytes { offset, bytes } => {
                // SAFETY: offset + bytes.len() <= arena_size (guaranteed by codegen).
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.add(*offset), bytes.len());
                }
                pc += 1;
            }

            Instruction::Call { fn_ptr, node_type, input_offsets, output_offset, has_output } => {
                if *fn_ptr == 0 {
                    return Err(VmError::UnresolvedCall(node_type.clone()));
                }
                arg_ptrs.clear();
                for &off in input_offsets {
                    // SAFETY: offset is within the arena (guaranteed by codegen).
                    unsafe { arg_ptrs.push(base.add(off)); }
                }
                unsafe {
                    let ret = if *has_output { base.add(*output_offset) } else { std::ptr::null_mut() };
                    // SAFETY: fn_ptr was resolved from the cdylib by the executor.
                    let f: DispatchFn = std::mem::transmute(*fn_ptr);
                    f(arg_ptrs.as_ptr(), ret);
                }
                pc += 1;
            }

            Instruction::JumpIf { condition_offset, true_label, false_label } => {
                // bool is 1 byte; non-zero == true.
                let cond = unsafe { *base.add(*condition_offset) != 0 };
                let target = if cond { *true_label } else { *false_label };
                pc = *labels.get(&target).ok_or(VmError::LabelNotFound(target))? + 1;
            }

            Instruction::Jump(label) => {
                pc = *labels.get(label).ok_or(VmError::LabelNotFound(*label))? + 1;
            }

            Instruction::Label(_) => { pc += 1; }

            Instruction::Return => break,
        }
    }
    Ok(())
}

fn build_labels(instructions: &[Instruction]) -> HashMap<LabelId, usize> {
    instructions.iter().enumerate()
        .filter_map(|(i, instr)| {
            if let Instruction::Label(id) = instr { Some((*id, i)) } else { None }
        })
        .collect()
}
