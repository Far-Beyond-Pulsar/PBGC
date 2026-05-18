use crate::bytecode::{BpProgram, BpValue, Instruction, LabelId};

// ── Error ────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum VmError {
    UnknownNode(String),
    SlotOutOfBounds(u32),
    LabelNotFound(LabelId),
    DispatchError(String),
    TypeMismatch { expected: &'static str, got: &'static str },
    /// An assert_* node fired and its condition was not met.
    AssertionFailed(String),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::UnknownNode(n) => write!(f, "Unknown node type: {}", n),
            VmError::SlotOutOfBounds(s) => write!(f, "Slot {} out of bounds", s),
            VmError::LabelNotFound(l) => write!(f, "Label {} not found", l),
            VmError::DispatchError(e) => write!(f, "Dispatch error: {}", e),
            VmError::TypeMismatch { expected, got } => {
                write!(f, "Type mismatch: expected {}, got {}", expected, got)
            }
            VmError::AssertionFailed(msg) => write!(f, "Assertion failed: {}", msg),
        }
    }
}

impl std::error::Error for VmError {}

// ── NodeDispatch trait ────────────────────────────────────────────────────────────
//
// The engine implements this trait, backed by its pre-compiled WASM module.
// PBGC ships only the trait + interpreter loop; the WASM runtime lives in the engine.

/// Dispatches a node call to the underlying implementation (WASM or native).
///
/// The engine wires this to its pre-compiled `pulsar_std.wasm` exports.
/// `node_type` is the interned string looked up once from `BpProgram::node_types`
/// before the hot loop — no heap allocation per call.
/// Results are written into `output` (set to `None` for void functions).
pub trait NodeDispatch {
    fn call(
        &self,
        node_type: &str,
        inputs: &[BpValue],
        output: &mut Option<BpValue>,
    ) -> Result<(), VmError>;
}

// ── BytecodeVm ────────────────────────────────────────────────────────────────────

/// Interprets a `BpProgram` using the provided `NodeDispatch` for node calls.
///
/// Slot layout: a flat `Vec<BpValue>` of size `program.slot_count`.
/// The VM resolves `Label` instructions at startup into a PC lookup table,
/// then executes a simple fetch-decode-execute loop.
pub struct BytecodeVm<'d, D: NodeDispatch> {
    dispatch: &'d D,
}

impl<'d, D: NodeDispatch> BytecodeVm<'d, D> {
    pub fn new(dispatch: &'d D) -> Self {
        Self { dispatch }
    }

    /// Execute a compiled program, returning any error encountered.
    pub fn run(&self, program: &BpProgram) -> Result<(), VmError> {
        let mut slots = vec![BpValue::Null; program.slot_count as usize];
        let label_table = build_label_table(&program.instructions)?;
        // Reusable scratch buffer — avoids one Vec allocation per Call
        let mut args: Vec<BpValue> = Vec::new();
        let mut call_output: Option<BpValue> = None;
        let mut pc = 0usize;

        loop {
            if pc >= program.instructions.len() {
                break;
            }

            match &program.instructions[pc] {
                Instruction::LoadConst { slot, value } => {
                    set_slot(&mut slots, *slot, value.clone())?;
                    pc += 1;
                }

                Instruction::Call {
                    node_type_idx,
                    inputs,
                    output,
                } => {
                    // Intern table lookup — one array index, no heap allocation
                    let node_type = program
                        .node_types
                        .get(*node_type_idx as usize)
                        .map(String::as_str)
                        .ok_or_else(|| VmError::UnknownNode(format!("idx {}", node_type_idx)))?;

                    // Reuse scratch buffer instead of allocating per call
                    args.clear();
                    for &s in inputs {
                        args.push(get_slot(&slots, s)?.clone());
                    }

                    call_output = None;
                    self.dispatch
                        .call(node_type, &args, &mut call_output)
                        .map_err(|e| match e {
                            // Assertion failures surface as-is for clean error messages
                            VmError::AssertionFailed(_) => e,
                            other => VmError::DispatchError(format!("{}: {}", node_type, other)),
                        })?;

                    if let Some(out_slot) = output {
                        set_slot(&mut slots, *out_slot, call_output.take().unwrap_or(BpValue::Null))?;
                    }
                    pc += 1;
                }

                Instruction::JumpIf {
                    condition,
                    true_label,
                    false_label,
                } => {
                    let cond = get_slot(&slots, *condition)?.is_truthy();
                    let target = if cond { *true_label } else { *false_label };
                    pc = *label_table
                        .get(&target)
                        .ok_or(VmError::LabelNotFound(target))?;
                    // Don't increment; we're already at the label instruction.
                    pc += 1; // step past the Label opcode itself
                }

                Instruction::Jump(label) => {
                    pc = *label_table
                        .get(label)
                        .ok_or(VmError::LabelNotFound(*label))?;
                    pc += 1;
                }

                Instruction::Label(_) => {
                    pc += 1; // no-op at runtime
                }

                Instruction::Return => break,
            }
        }

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────────

fn build_label_table(
    instructions: &[Instruction],
) -> Result<std::collections::HashMap<LabelId, usize>, VmError> {
    let mut table = std::collections::HashMap::new();
    for (i, instr) in instructions.iter().enumerate() {
        if let Instruction::Label(id) = instr {
            table.insert(*id, i);
        }
    }
    Ok(table)
}

fn get_slot(slots: &[BpValue], slot: u32) -> Result<&BpValue, VmError> {
    slots.get(slot as usize).ok_or(VmError::SlotOutOfBounds(slot))
}

fn set_slot(slots: &mut Vec<BpValue>, slot: u32, value: BpValue) -> Result<(), VmError> {
    let idx = slot as usize;
    if idx >= slots.len() {
        return Err(VmError::SlotOutOfBounds(slot));
    }
    slots[idx] = value;
    Ok(())
}
