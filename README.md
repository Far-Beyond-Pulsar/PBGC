# PBGC: pulsar_std node catalogue for Graphy

PBGC bridges `pulsar_std`'s `#[blueprint]` node registry and
[Graphy](https://github.com/Far-Beyond-Pulsar/Graphy)'s node metadata. The
Blueprint editor reads the standard library's nodes through it for its palette,
pin shapes and graph validation.

```rust
let metadata = pbgc::extract_node_metadata()?;           // every pulsar_std node
let provider = pbgc::BlueprintMetadataProvider::new();   // for graphy::DataResolver
```

## What was removed (0.2.0)

PBGC used to compile Blueprint graphs itself. That is gone:

| Removed | Replacement |
|---|---|
| `compile_graph`, `compile_graph_with_variables`, `compile_graph_with_library_manager` (graph to Rust source) | Compile to a script module with `blueprint_compiler`, then export with `pulsar_script_codegen::actor::generate_actor` |
| `compile_graph_to_bytecode*`, `BpProgram`, `Instruction`, `vm::run`, `VmError` (the old bytecode and its VM) | `pulsar_script_vm`: `Module`, `Program`, `Vm` |
| `generate_project`, `generate_blueprint_actor_source*`, `ProjectSpec`, `CompiledBlueprint` | `pulsar_script_codegen::actor::{generate_actor, class_files}` |
| `compile_project`, `compile_project_generated` | compile each class with `blueprint_compiler`, export with `class_files` |
| `BlueprintMetadataProvider::param_layout` / `return_layout` (bytecode layout helpers) | none: the script VM is typed, not layout-based |
| Graphy re-exports (`pbgc::GraphDescription`, ...) | depend on `graphy` directly |

The Rust export no longer keeps class variables in thread-local statics shared
by every instance of a class, and no longer sleeps the thread for `delay`: each
actor owns its script state and waits are suspended calls.

Anything outside the Pulsar-Native repository that still calls the removed
APIs must move to the replacements above; this is a breaking release.
