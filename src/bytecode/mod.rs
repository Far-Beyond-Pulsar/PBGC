use serde::{Deserialize, Serialize};

pub type SlotId = u32;
pub type LabelId = u32;

/// A runtime value in the bytecode VM slot table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BpValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl BpValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            BpValue::Null => false,
            BpValue::Bool(b) => *b,
            BpValue::Int(i) => *i != 0,
            BpValue::Float(f) => *f != 0.0,
            BpValue::Str(s) => !s.is_empty(),
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            BpValue::Int(i) => Some(*i),
            BpValue::Float(f) => Some(*f as i64),
            BpValue::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            BpValue::Float(f) => Some(*f),
            BpValue::Int(i) => Some(*i as f64),
            BpValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        self.is_truthy()
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            BpValue::Null => "null",
            BpValue::Bool(_) => "bool",
            BpValue::Int(_) => "int",
            BpValue::Float(_) => "float",
            BpValue::Str(_) => "str",
        }
    }
}

impl std::fmt::Display for BpValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BpValue::Null => write!(f, "null"),
            BpValue::Bool(b) => write!(f, "{}", b),
            BpValue::Int(i) => write!(f, "{}", i),
            BpValue::Float(v) => write!(f, "{}", v),
            BpValue::Str(s) => write!(f, "{}", s),
        }
    }
}

/// A single bytecode instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    /// Load a compile-time constant into a slot.
    LoadConst { slot: SlotId, value: BpValue },

    /// Call a node function by its registered name.
    /// `inputs` are slot IDs for each positional parameter.
    /// `output` is where the return value is stored (None for void functions).
    Call {
        node_type: String,
        inputs: Vec<SlotId>,
        output: Option<SlotId>,
    },

    /// Conditional branch: jump to `true_label` if slot is truthy, else `false_label`.
    JumpIf {
        condition: SlotId,
        true_label: LabelId,
        false_label: LabelId,
    },

    /// Unconditional jump.
    Jump(LabelId),

    /// Branch target marker — no-op at runtime, used by the VM to resolve labels.
    Label(LabelId),

    /// End execution of this program.
    Return,
}

/// A compiled blueprint program ready for the bytecode VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpProgram {
    /// Name of the event/entry-point (e.g. `"begin_play"`, `"tick"`).
    pub name: String,
    /// Total number of value slots required.
    pub slot_count: u32,
    pub instructions: Vec<Instruction>,
}

impl BpProgram {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            slot_count: 0,
            instructions: Vec::new(),
        }
    }
}

/// Parse a Rust-literal constant string (as produced by the DataResolver) into a BpValue.
pub fn parse_bp_const(s: &str) -> BpValue {
    let s = s.trim();
    if s == "true" {
        return BpValue::Bool(true);
    }
    if s == "false" {
        return BpValue::Bool(false);
    }
    // Strip type suffixes like 42i64, 3.14f64
    let stripped = s
        .trim_end_matches("i64")
        .trim_end_matches("i32")
        .trim_end_matches("u64")
        .trim_end_matches("u32")
        .trim_end_matches("f64")
        .trim_end_matches("f32")
        .trim_end_matches("usize")
        .trim_end_matches("isize");
    if let Ok(i) = stripped.parse::<i64>() {
        return BpValue::Int(i);
    }
    if let Ok(f) = stripped.parse::<f64>() {
        return BpValue::Float(f);
    }
    // Rust string literals: "hello"
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        return BpValue::Str(s[1..s.len() - 1].to_string());
    }
    BpValue::Str(s.to_string())
}
