//! # PBGC: the pulsar_std node catalogue for Graphy
//!
//! The Blueprint graph compiler used to live here, with a bytecode VM and a
//! graph-to-Rust backend. Both are gone: Blueprint graphs now compile to
//! language-neutral script modules (`blueprint_compiler`, in the Blueprint
//! plugin) that the engine's script VM runs, and the Rust export generates
//! its code from that same module (`pulsar_script_codegen`).
//!
//! What remains is the bridge between `pulsar_std`'s `#[blueprint]` registry
//! and Graphy's node metadata: palettes, pin shapes and graph validation read
//! the standard library's nodes through it.

pub mod metadata;

pub use metadata::{
    extract_node_metadata, find_node, get_categories, get_node_metadata, get_nodes_by_category,
    BlueprintMetadataProvider,
};
