# PBGC - Pulsar Blueprint Graph Compiler

Compiler for Pulsar Blueprint visual scripts to Rust code.

## Features

- Compiles Blueprint node graphs to executable Rust code
- Integrates with pulsar_std node registry
- Thread-safe variable handling (Cell/RefCell + Arc)
- Drop-in replacement for existing compiler

## Quick Start

```rust
use pbgc::compile_graph;
use graphy::GraphDescription;

let graph = GraphDescription::new("my_blueprint");
// ... build graph with nodes and connections

match compile_graph(&graph) {
    Ok(rust_code) => std::fs::write("generated.rs", rust_code)?,
    Err(e) => eprintln!("Error: {}", e),
}
```

## Integration with Pulsar Engine

Replace existing compiler calls:

```rust
// OLD:
use pulsar_engine::compiler::compile_graph;

// NEW:
use pbgc::compile_graph;

// API is identical - drop-in replacement!
```

## Architecture

Built on [Graphy](https://github.com/Far-Beyond-Pulsar/Graphy) library with Blueprint-specific:
- pulsar_std integration
- Rust code generation
- Variable getter/setter handling
- Event node support
