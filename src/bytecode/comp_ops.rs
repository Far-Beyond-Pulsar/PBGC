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
//!
//! # Identity operands (#654)
//!
//! Component ops may address an actor OTHER than the one the running
//! instance is bound to: a `component_ref` input pin connected to an
//! identity producer (`get_component_ref::`, `find_object_by_*`,
//! `object_ref_literal`) carries the target explicitly. The staged ABI
//! extends accordingly:
//!
//! * **Target field** — the name blob carries a trailing NUL-terminated
//!   field naming the addressing mode ([`encode_targeted_name_blob`],
//!   [`encode_targeted_call_name_blob`]): `self` (address the executing
//!   instance's own entity — the legacy behaviour) or `pin` (take the
//!   target from the reference operand). ABI v2 compilers emit the field
//!   UNCONDITIONALLY so the runtime reader can scan a fixed field count
//!   without any length side-channel; the decoders still accept the legacy
//!   shorter shapes ([`RefTarget::SelfActor`] implied) for hand-built
//!   programs and tests.
//! * **Reference operand** — a pin-targeted op stages ONE extra JSON blob
//!   immediately after the name blob (before the value arguments),
//!   holding the reference in its #642 marshalling shape: a ComponentRef
//!   object (`{"entity":bits,"class_name":…,"component_index":n}`) or a
//!   bare packed-bits number for an ActorRef.
//!
//! Identity producers themselves are ordinary pure ops routed by the same
//! node-type prefixes, with JSON-blob operands and no name blob:
//!
//! | Node type                  | inputs               | output              |
//! |----------------------------|----------------------|---------------------|
//! | `get_component_ref::C::N`  | `[name, (actor)]`    | ComponentRef blob   |
//! | `find_object_by_stable_id` | `[needle]`           | ActorRef bits blob  |
//! | `find_object_by_name`      | `[needle]`           | ActorRef bits blob  |
//! | `object_ref_literal`       | `[literal]`          | ComponentRef blob   |
//!
//! `get_component_ref` keeps the name-blob convention (its class/index ride
//! there); the others stage everything as their single JSON operand. All
//! resolution happens at RUNTIME against the live world (#639 lazy
//! resolution) — literals stage their `{stable_id, …}` save/load form, not
//! entity bits, so references survive reloads.

use serde::{Deserialize, Serialize};

/// Reserved arena bytes for any component-op output/argument value blob
/// written at runtime (length prefix included). Compile-time constants use
/// only their exact size.
pub const JSON_BLOB_CAPACITY: usize = 4096;

/// How a component op addresses the actor it operates on (#654).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefTarget {
    /// The executing instance's own bound entity (the pre-#654 behaviour).
    SelfActor,
    /// The reference carried by the op's staged reference operand.
    RefPin,
}

impl RefTarget {
    /// The name-blob field spelling of this mode.
    pub fn as_field(self) -> &'static str {
        match self {
            RefTarget::SelfActor => "self",
            RefTarget::RefPin => "pin",
        }
    }

    /// Parse a name-blob target field. `None` for anything else — callers
    /// refuse rather than guess.
    pub fn from_field(field: &str) -> Option<Self> {
        match field {
            "self" => Some(RefTarget::SelfActor),
            "pin" => Some(RefTarget::RefPin),
            _ => None,
        }
    }
}

/// Which reflected member a component node addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompOpKind {
    GetProp,
    SetProp,
    Call,
    /// Pure producer of a `ComponentRef` for `(actor, class, index)`
    /// (`get_component_ref::{Class}::{Index}`).
    GetRef,
    /// Pure resolver: StableId needle → `ActorRef` (#654).
    FindByStableId,
    /// Pure resolver: display-name needle → `ActorRef` (#654).
    FindByName,
    /// Pure resolver: staged serialized reference → resolved `ComponentRef`
    /// (`object_ref_literal`, the drag-a-scene-object literal node).
    ObjectLiteral,
}

impl CompOpKind {
    /// The node-type prefix this kind is encoded under.
    pub fn prefix(self) -> &'static str {
        match self {
            CompOpKind::GetProp => "comp_get_prop::",
            CompOpKind::SetProp => "comp_set_prop::",
            CompOpKind::Call => "comp_call::",
            CompOpKind::GetRef => "get_component_ref::",
            CompOpKind::FindByStableId => "find_object_by_stable_id",
            CompOpKind::FindByName => "find_object_by_name",
            CompOpKind::ObjectLiteral => "object_ref_literal",
        }
    }

    /// Whether ops of this kind stage a class/member name blob.
    ///
    /// The identity resolvers (`find_*` / `object_ref_literal`) carry all
    /// their data in plain JSON operands instead.
    pub fn uses_name_blob(self) -> bool {
        !matches!(
            self,
            CompOpKind::FindByStableId | CompOpKind::FindByName | CompOpKind::ObjectLiteral
        )
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
/// Returns `None` for anything without a `comp_*::` / identity-op prefix —
/// callers treat that as "not a component op" and fall through to normal
/// dispatch. Identity resolvers carry no class/member in the node type, so
/// their class/member fields come back empty.
pub fn parse_node_type(node_type: &str) -> Option<(CompOpKind, &str, &str)> {
    let (kind, rest) = if let Some(r) = node_type.strip_prefix("comp_get_prop::") {
        (CompOpKind::GetProp, r)
    } else if let Some(r) = node_type.strip_prefix("comp_set_prop::") {
        (CompOpKind::SetProp, r)
    } else if let Some(r) = node_type.strip_prefix("comp_call::") {
        (CompOpKind::Call, r)
    } else if let Some(r) = node_type.strip_prefix("get_component_ref::") {
        (CompOpKind::GetRef, r)
    } else if node_type == "find_object_by_stable_id" {
        (CompOpKind::FindByStableId, "")
    } else if node_type == "find_object_by_name" {
        (CompOpKind::FindByName, "")
    } else if node_type == "object_ref_literal" {
        (CompOpKind::ObjectLiteral, "")
    } else {
        return None;
    };

    let (class_name, member) = match rest.split_once("::") {
        Some(pair) => pair,
        None => (rest, ""),
    };
    if kind.uses_name_blob() && (class_name.is_empty() || member.is_empty()) {
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

/// Encode a `comp_call` name blob, staging the argument count as a third
/// NUL-terminated field so the runtime handler knows how many value
/// operands follow without any out-of-band length: `{class}\0{method}\
/// {arg_count}\0`.
pub fn encode_call_name_blob(class_name: &str, method_name: &str, arg_count: usize) -> Vec<u8> {
    let mut bytes = encode_name_blob(class_name, method_name);
    bytes.extend_from_slice(arg_count.to_string().as_bytes());
    bytes.push(0);
    bytes
}

/// Encode a name blob with a trailing target-mode field (#654 ABI):
/// `{class}\0{member}\0{target}\0`.
pub fn encode_targeted_name_blob(class_name: &str, member: &str, target: RefTarget) -> Vec<u8> {
    let mut bytes = encode_name_blob(class_name, member);
    bytes.extend_from_slice(target.as_field().as_bytes());
    bytes.push(0);
    bytes
}

/// Encode a `comp_call` name blob with argument count AND target mode
/// (#654 ABI): `{class}\0{method}\0{arg_count}\0{target}\0`.
pub fn encode_targeted_call_name_blob(
    class_name: &str,
    method_name: &str,
    arg_count: usize,
    target: RefTarget,
) -> Vec<u8> {
    let mut bytes = encode_call_name_blob(class_name, method_name, arg_count);
    bytes.extend_from_slice(target.as_field().as_bytes());
    bytes.push(0);
    bytes
}

/// A decoded get/set name blob: the addressed member plus how the target
/// actor is chosen (#654).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameFields {
    pub class_name: String,
    pub member: String,
    pub target: RefTarget,
}

/// Decode a get/set name blob written by [`encode_name_blob`] (legacy,
/// target = [`RefTarget::SelfActor`]) or [`encode_targeted_name_blob`].
///
/// Discrimination is by NUL-field count, which is authoritative because
/// names cannot contain NULs.
pub fn decode_targeted_name_blob(blob: &[u8]) -> Option<NameFields> {
    let parts = split_nul_fields(blob)?;
    match parts.len() {
        2 => Some(NameFields {
            class_name: parts[0].to_string(),
            member: parts[1].to_string(),
            target: RefTarget::SelfActor,
        }),
        3 => Some(NameFields {
            class_name: parts[0].to_string(),
            member: parts[1].to_string(),
            target: RefTarget::from_field(parts[2])?,
        }),
        _ => None,
    }
}

/// A decoded call name blob: the addressed method, its declared argument
/// count, and how the target actor is chosen (#654).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallNameFields {
    pub class_name: String,
    pub method: String,
    pub arg_count: usize,
    pub target: RefTarget,
}

/// Decode a `comp_call` name blob written by [`encode_call_name_blob`]
/// (legacy, target = [`RefTarget::SelfActor`]) or
/// [`encode_targeted_call_name_blob`]. Field-count discriminated, like the
/// get/set decoder.
pub fn decode_targeted_call_name_blob(blob: &[u8]) -> Option<CallNameFields> {
    let parts = split_nul_fields(blob)?;
    match parts.len() {
        3 => Some(CallNameFields {
            class_name: parts[0].to_string(),
            method: parts[1].to_string(),
            arg_count: parts[2].parse().ok()?,
            target: RefTarget::SelfActor,
        }),
        4 => Some(CallNameFields {
            class_name: parts[0].to_string(),
            method: parts[1].to_string(),
            arg_count: parts[2].parse().ok()?,
            target: RefTarget::from_field(parts[3])?,
        }),
        _ => None,
    }
}

/// Split a staged NUL-separated blob into its UTF-8 fields (trailing
/// terminator required, interior NULs are separators).
fn split_nul_fields(blob: &[u8]) -> Option<Vec<&str>> {
    let text = std::str::from_utf8(blob).ok()?;
    let text = text.strip_suffix('\0')?;
    Some(text.split('\0').collect())
}

/// Decode a legacy 2-field name blob previously written by
/// [`encode_name_blob`].
///
/// `blob` spans exactly the staged region; interior NULs are separators.
pub fn decode_name_blob(blob: &[u8]) -> Option<(&str, &str)> {
    let parts = split_nul_fields(blob)?;
    match parts.as_slice() {
        [class_name, member] => Some((class_name, member)),
        _ => None,
    }
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

    /// #654: targeted get/set blobs round-trip; legacy 2-field blobs decode
    /// as self-targeted so pre-#654 programs keep running.
    #[test]
    fn targeted_name_blobs_round_trip_and_accept_legacy() {
        for target in [RefTarget::SelfActor, RefTarget::RefPin] {
            let bytes = encode_targeted_name_blob("Light", "intensity", target);
            assert_eq!(
                decode_targeted_name_blob(&bytes),
                Some(NameFields {
                    class_name: "Light".into(),
                    member: "intensity".into(),
                    target,
                })
            );
        }
        let legacy = encode_name_blob("Light", "intensity");
        assert_eq!(
            decode_targeted_name_blob(&legacy).map(|f| f.target),
            Some(RefTarget::SelfActor)
        );
        // Unknown mode field is refused, never guessed.
        let bad = encode_targeted_name_blob("Light", "intensity", RefTarget::SelfActor);
        let mut tampered = bad.clone();
        *tampered.last_mut().unwrap() = b'x';
        assert!(decode_targeted_name_blob(&tampered).is_none());
    }

    /// #654: targeted call blobs round-trip with argc preserved; the
    /// legacy 3-field form decodes as self-targeted. A member literally
    /// named "self" cannot fool the count-based discrimination.
    #[test]
    fn targeted_call_blobs_round_trip_count_is_authoritative() {
        for target in [RefTarget::SelfActor, RefTarget::RefPin] {
            let bytes = encode_targeted_call_name_blob("Door", "open", 2, target);
            assert_eq!(
                decode_targeted_call_name_blob(&bytes),
                Some(CallNameFields {
                    class_name: "Door".into(),
                    method: "open".into(),
                    arg_count: 2,
                    target,
                })
            );
        }
        let legacy = encode_call_name_blob("Door", "open", 1);
        assert_eq!(
            decode_targeted_call_name_blob(&legacy),
            Some(CallNameFields {
                class_name: "Door".into(),
                method: "open".into(),
                arg_count: 1,
                target: RefTarget::SelfActor,
            })
        );
    }

    /// #654: identity-op node types parse with empty class/member; the
    /// class-carrying kinds still demand both fields.
    #[test]
    fn identity_op_node_types_parse() {
        assert_eq!(
            parse_node_type("get_component_ref::Light::2"),
            Some((CompOpKind::GetRef, "Light", "2"))
        );
        assert_eq!(
            parse_node_type("find_object_by_stable_id"),
            Some((CompOpKind::FindByStableId, "", ""))
        );
        assert_eq!(
            parse_node_type("find_object_by_name"),
            Some((CompOpKind::FindByName, "", ""))
        );
        assert_eq!(
            parse_node_type("object_ref_literal"),
            Some((CompOpKind::ObjectLiteral, "", ""))
        );
        assert_eq!(parse_node_type("get_component_ref::NoIndex"), None);
        assert!(!CompOpKind::FindByStableId.uses_name_blob());
        assert!(CompOpKind::GetRef.uses_name_blob());
    }
}
