//! Component-operation bytecode ABI.
//!
//! Blueprint graphs address engine components through three node kinds the
//! editor generates from reflection metadata:
//!
//! ```text
//! comp_get_prop::{Class}::{Member}   pure  — produces a value
//! comp_set_prop::{Class}::{Member}   exec  — consumes a value
//! comp_call::{Class}::{Method}       exec  — optional return
//! ```
//!
//! None of these have `pulsar_std` registry entries, so the generic
//! `__bp_dispatch_<name>` dlsym path cannot resolve them. Instead every kind
//! compiles to a plain [`Instruction::Call`] whose `node_type` retains the
//! full original string (e.g. `comp_set_prop::Light::Intensity`); the
//! executor routes on that prefix BEFORE dlsym lookup and hands the call to
//! a component-op handler instead of a native shim.
//!
//! # Arena calling convention
//!
//! All operands are staged in the arena by `InitBytes`, exactly like
//! constants:
//!
//! * **Name blob** — always `input_offsets[0]`:
//!   `{class}\0{member}\0` as UTF-8. Deduplicated per event by the codegen.
//! * **Value blobs** — JSON values length-prefixed with a little-endian
//!   `u64`: `[u64 len][len bytes of UTF-8 JSON]`. Constants stage their
//!   exact bytes; runtime-produced outputs reserve
//!   [`JSON_BLOB_CAPACITY`] bytes of arena.
//!
//! Per kind:
//!
//! | Kind      | inputs                                | output          |
//! |-----------|---------------------------------------|-----------------|
//! | GetProp   | `[name]`                              | value blob slot |
//! | SetProp   | `[name, value]`                       | —               |
//! | Call      | `[name, arg0, .. argN]`               | blob slot iff the graph uses the return |
//!
//! Values travel as JSON blobs at this layer so the format stays stable
//! regardless of the concrete property type; the live handler (#647) decodes
//! them through `pulsar_world_registry::marshal` against registry metadata.
//! Native (non-component) sources connected to component pins are rejected
//! at compile time until that handler exists.

use serde::{Deserialize, Serialize};

/// Reserved arena bytes for any component-op output/argument value blob
/// written at runtime (length prefix included). Compile-time constants use
/// only their exact size.
pub const JSON_BLOB_CAPACITY: usize = 4096;

/// Which reflected member a component node addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompOpKind {
    GetProp,
    SetProp,
    Call,
}

impl CompOpKind {
    /// The node-type prefix this kind is encoded under.
    pub fn prefix(self) -> &'static str {
        match self {
            CompOpKind::GetProp => "comp_get_prop::",
            CompOpKind::SetProp => "comp_set_prop::",
            CompOpKind::Call => "comp_call::",
        }
    }
}

/// One `(class, member)` pair referenced by a compiled program. The runtime
/// uses these to validate availability and to pre-resolve reflection
/// metadata before executing events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentOpRef {
    pub kind: CompOpKind,
    pub class_name: String,
    pub member: String,
}

/// Parse a full node-type string into its operation parts.
///
/// Returns `None` for anything without a `comp_*::` prefix — callers treat
/// that as "not a component op" and fall through to normal dispatch.
pub fn parse_node_type(node_type: &str) -> Option<(CompOpKind, &str, &str)> {
    let (kind, rest) = if let Some(r) = node_type.strip_prefix("comp_get_prop::") {
        (CompOpKind::GetProp, r)
    } else if let Some(r) = node_type.strip_prefix("comp_set_prop::") {
        (CompOpKind::SetProp, r)
    } else if let Some(r) = node_type.strip_prefix("comp_call::") {
        (CompOpKind::Call, r)
    } else {
        return None;
    };

    let (class_name, member) = rest.split_once("::")?;
    if class_name.is_empty() || member.is_empty() {
        return None;
    }
    Some((kind, class_name, member))
}

/// Encode a name blob: `{class}\0{member}\0`.
pub fn encode_name_blob(class_name: &str, member: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(class_name.len() + member.len() + 2);
    bytes.extend_from_slice(class_name.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(member.as_bytes());
    bytes.push(0);
    bytes
}

/// Decode a name blob previously written by [`encode_name_blob`].
///
/// `blob` spans exactly the staged region; interior NULs are separators.
pub fn decode_name_blob(blob: &[u8]) -> Option<(&str, &str)> {
    let text = std::str::from_utf8(blob).ok()?;
    let text = text.strip_suffix('\0')?;
    let (class_name, member) = text.split_once('\0')?;
    Some((class_name, member))
}

/// Encode a JSON value blob: little-endian `u64` length prefix + UTF-8 bytes.
pub fn encode_json_blob(json: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8 + json.len());
    bytes.extend_from_slice(&(json.len() as u64).to_le_bytes());
    bytes.extend_from_slice(json.as_bytes());
    bytes
}

/// Read the length prefix of a runtime-written JSON blob.
///
/// # Safety
/// `ptr` must point to at least 8 readable, 8-aligned bytes.
pub unsafe fn json_blob_len(ptr: *const u8) -> usize {
    usize::try_from(std::ptr::read(ptr as *const u64)).unwrap_or(0)
}

/// Write `json` into an arena-reserved blob slot.
///
/// # Safety
/// `ptr` must point to at least [`JSON_BLOB_CAPACITY`] writable bytes.
pub unsafe fn write_json_blob(ptr: *mut u8, json: &str) {
    debug_assert!(
        json.len() + 8 <= JSON_BLOB_CAPACITY,
        "component-op JSON blob exceeds reserved capacity"
    );
    std::ptr::write(ptr as *mut u64, json.len() as u64);
    std::ptr::copy_nonoverlapping(json.as_ptr(), ptr.add(8), json.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_three_kinds() {
        assert_eq!(
            parse_node_type("comp_get_prop::Light::intensity"),
            Some((CompOpKind::GetProp, "Light", "intensity"))
        );
        assert_eq!(
            parse_node_type("comp_set_prop::Light::color"),
            Some((CompOpKind::SetProp, "Light", "color"))
        );
        assert_eq!(
            parse_node_type("comp_call::Door::open"),
            Some((CompOpKind::Call, "Door", "open"))
        );
    }

    #[test]
    fn rejects_malformed_node_types() {
        assert_eq!(parse_node_type("add"), None);
        assert_eq!(parse_node_type("comp_get_prop::OnlyClass"), None);
        assert_eq!(parse_node_type("comp_set_prop::::prop"), None);
        assert_eq!(parse_node_type("comp_call::Class::"), None);
    }

    #[test]
    fn name_blobs_round_trip() {
        let bytes = encode_name_blob("Light", "intensity");
        assert_eq!(decode_name_blob(&bytes), Some(("Light", "intensity")));
        // Names containing NUL would corrupt the encoding; parse refuses.
        let evil = encode_name_blob("Li\0ght", "x");
        assert_ne!(decode_name_blob(&evil), Some(("Li\0ght", "x")));
    }

    #[test]
    fn json_blobs_round_trip() {
        let encoded = encode_json_blob("{\"r\":255}");
        assert_eq!(&encoded[8..], b"{\"r\":255}");
        let len = unsafe { json_blob_len(encoded.as_ptr()) };
        assert_eq!(len, 9);
    }
}
