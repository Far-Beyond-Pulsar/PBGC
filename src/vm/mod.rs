use crate::bytecode::{BpProgram, Instruction, LabelId, TypeSlot};
use std::collections::HashMap;

#[derive(Debug)]
pub enum VmError {
    LabelNotFound(LabelId),
    UnresolvedCall(String),
    InsufficientArena { required: usize, provided: usize },
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::LabelNotFound(l)  => write!(f, "label {} not found", l),
            VmError::UnresolvedCall(n) => write!(f, "unresolved call '{}' — forgot BpExecutor::prepare?", n),
            VmError::InsufficientArena { required, provided } => {
                write!(f, "insufficient arena size: required {}, provided {}", required, provided)
            }
        }
    }
}
impl std::error::Error for VmError {}

/// ABI of every `__bp_dispatch_<name>` symbol.
///
/// `args`       — array of pointers into the byte arena, one per input.
/// `ret`        — pointer to the output region in the arena (null for void).
/// `type_slots` — array of `TypeSlot` values resolved at graph-compile time,
///                one per generic type parameter `T`.  Concrete functions
///                receive this pointer but must not read it (pass null/empty).
pub type DispatchFn = unsafe extern "C" fn(
    args:       *const *const u8,
    ret:        *mut u8,
    type_slots: *const TypeSlot,
);

/// Execute a prepared `BpProgram`.
///
/// All `fn_ptr` fields in `Instruction::Call` must have been patched by
/// `BpExecutor::prepare` before calling this. The loop resolves labels once
/// at startup, then executes purely via pointer arithmetic and direct calls.
pub fn run(program: &BpProgram) -> Result<(), VmError> {
    // Allocate the byte arena. Vec<u64> guarantees 8-byte alignment of the base pointer.
    let mut arena = vec![0u64; (program.arena_size + 7) / 8];
    let arena_ptr = arena.as_mut_ptr() as *mut u8;

    // SAFETY: arena_ptr comes from Vec<u64>, so it is 8-byte aligned and valid for
    // program.arena_size bytes.
    unsafe { run_with_external_arena(program, arena_ptr, program.arena_size) }
}

/// Execute a prepared `BpProgram` with an external arena.
///
/// This allows persistent variable state across multiple event executions.
/// The caller provides a mutable arena that will be used instead of allocating
/// a fresh one for each run.
///
/// # Safety
/// - `arena` must be valid for reads/writes of at least `arena_size` bytes
/// - `arena` must be 8-byte aligned
/// - variable regions in the arena must be initialized before execution
pub unsafe fn run_with_external_arena(
    program: &BpProgram,
    arena: *mut u8,
    arena_size: usize,
) -> Result<(), VmError> {
    if arena_size < program.arena_size {
        return Err(VmError::InsufficientArena {
            required: program.arena_size,
            provided: arena_size,
        });
    }

    let base = arena;

    // Pre-allocate the argument-pointer scratch buffer once to avoid heap churn.
    let mut arg_ptrs:      Vec<*const u8>  = Vec::with_capacity(program.max_args_count.max(1));
    // Scratch buffer: TypeSlot values collected contiguously for each generic Call.
    // Generic dispatch functions receive `*const TypeSlot` into this buffer.
    let mut type_slots_buf: Vec<TypeSlot>  = Vec::new();

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

            Instruction::InitTypeSlot { offset, size, align } => {
                // Write a TypeSlot { size, align } into the arena at *offset*.
                // SAFETY: the codegen guarantees offset + sizeof(TypeSlot) <= arena_size
                // and that the slot is 8-byte aligned.
                unsafe {
                    let slot_ptr = base.add(*offset) as *mut TypeSlot;
                    std::ptr::write(slot_ptr, TypeSlot { size: *size, align: *align });
                }
                pc += 1;
            }

            Instruction::Call { fn_ptr, node_type, input_offsets, output_offset, has_output, type_slot_offsets } => {
                if *fn_ptr == 0 {
                    return Err(VmError::UnresolvedCall(node_type.clone()));
                }
                arg_ptrs.clear();
                for &off in input_offsets {
                    // SAFETY: offset is within the arena (guaranteed by codegen).
                    unsafe { arg_ptrs.push(base.add(off)); }
                }
                // Collect TypeSlot values into a contiguous scratch buffer so that
                // generic dispatch functions can index them as type_slots[i].
                type_slots_buf.clear();
                for &off in type_slot_offsets {
                    unsafe {
                        type_slots_buf.push(*(base.add(off) as *const TypeSlot));
                    }
                }
                unsafe {
                    let ret = if *has_output { base.add(*output_offset) } else { std::ptr::null_mut() };
                    // Null when there are no type slots (concrete functions).
                    let ts_ptr = if type_slots_buf.is_empty() {
                        std::ptr::null()
                    } else {
                        type_slots_buf.as_ptr()
                    };
                    // SAFETY: fn_ptr was resolved from the cdylib by the executor.
                    let f: DispatchFn = std::mem::transmute(*fn_ptr);
                    f(arg_ptrs.as_ptr(), ret, ts_ptr);
                }
                pc += 1;
            }

            Instruction::LoadVar { source_offset, output_offset, size } => {
                unsafe {
                    std::ptr::copy(
                        base.add(*source_offset),
                        base.add(*output_offset),
                        *size,
                    );
                }
                pc += 1;
            }

            Instruction::StoreVar { input_offset, target_offset, size } => {
                unsafe {
                    std::ptr::copy(
                        base.add(*input_offset),
                        base.add(*target_offset),
                        *size,
                    );
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
