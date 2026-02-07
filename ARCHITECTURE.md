# PBGC Architecture

## Overview

PBGC (Pulsar Blueprint Graph Compiler) is built in two layers:

```
┌─────────────────────────────────────┐
│      Pulsar-Native (Engine)         │
│  ┌───────────────────────────────┐  │
│  │   blueprint_compiler crate    │  │
│  │   (thin wrapper for engine)   │  │
│  └─────────────┬─────────────────┘  │
│                │                     │
│    ┌───────────▼────────────┐       │
│    │     pulsar_std         │       │
│    │  (node definitions)    │       │
│    └────────────────────────┘       │
└─────────────────┬───────────────────┘
                  │
┌─────────────────▼───────────────────┐
│           PBGC (Blueprint)          │
│  - pulsar_std integration           │
│  - Rust code generation             │
│  - Blueprint-specific nodes         │
└─────────────────┬───────────────────┘
                  │
┌─────────────────▼───────────────────┐
│         Graphy (General)            │
│  - Graph data structures            │
│  - Data flow analysis               │
│  - Execution flow analysis          │
│  - AST transformation               │
└─────────────────────────────────────┘
```

## Module Structure

### PBGC Modules

- **`lib.rs`** - Public API and re-exports
- **`metadata.rs`** - pulsar_std integration
- **`compiler.rs`** - Main compilation entry points
- **`codegen/`** - Rust code generation
  - `rust_codegen.rs` - Blueprint → Rust generator
  - `node_handlers.rs` - Special node handling

### Graphy Modules

- **`core/`** - Data structures
  - `graph.rs`, `node.rs`, `connection.rs`, `types.rs`, `metadata.rs`
- **`analysis/`** - Analysis passes
  - `data_flow.rs` - Dependency resolution
  - `exec_flow.rs` - Execution routing
- **`generation/`** - Code generation framework
  - `context.rs`, `strategies.rs`
- **`utils/`** - Utilities
  - `ast_transform.rs` - AST manipulation
  - `variable_gen.rs` - Variable naming
  - `subgraph_expander.rs` - Graph composition

## Compilation Pipeline

1. **Load Metadata** - Extract from pulsar_std
2. **Expand Sub-graphs** - Inline macros (optional)
3. **Data Flow Analysis** - Build dependency graph (Graphy)
4. **Execution Flow** - Map exec connections (Graphy)
5. **Code Generation** - Generate Rust code (PBGC)

## Key Design Decisions

### Repository Organization

**Pulsar-Native (Engine Repository)**:
- `crates/pulsar_std` - Standard library with node definitions
- `crates/pulsar_macros` - Macro support for node registration
- `crates/blueprint_compiler` - Thin wrapper that re-exports PBGC for engine use

**PBGC (Compiler Repository)**:
- Imports `pulsar_std` from Pulsar-Native via path dependency
- Provides Blueprint-specific code generation on top of Graphy
- Can be used standalone or through the engine's wrapper

**Graphy (Analysis Library Repository)**:
- General-purpose graph compilation library
- No Blueprint or engine-specific code
- Reusable for other graph compilation tasks (e.g., PSGC shader compiler)

### Why Separate Graphy?

- **Reusability**: PSGC shader compiler uses same infrastructure
- **Modularity**: Clear separation of general vs specific
- **Testing**: Can test graph analysis independently
- **Extensibility**: Easy to add new target languages

### What's General vs Blueprint-Specific?

**Graphy (General)**:
- Graph representation
- Analysis algorithms
- AST utilities

**PBGC (Blueprint-Specific)**:
- pulsar_std integration
- Rust code generation
- Variable Cell/RefCell wrappers
- Blueprint naming conventions
