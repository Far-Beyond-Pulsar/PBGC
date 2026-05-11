//! Blueprint output generator.
//!
//! Turns a collection of compiled blueprints into blueprint-owned Rust source
//! files only.  Core project/bootstrap files (`Cargo.toml`, `main.rs`) are
//! intentionally out of scope and must be handled by the core build system.

use std::collections::BTreeMap;
use std::path::Path;

use graphy::{GraphDescription, GraphyError};

use crate::compile_graph;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev_upper = false;
    for (i, ch) in name.char_indices() {
        if ch.is_uppercase() {
            if i != 0 && !prev_upper {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_upper = true;
        } else if ch == '-' || ch == ' ' {
            out.push('_');
            prev_upper = false;
        } else {
            out.push(ch);
            prev_upper = false;
        }
    }
    out
}

fn to_pascal_case(name: &str) -> String {
    name.split(|c: char| c == '_' || c == '-' || c == ' ')
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.push_str(chars.as_str());
                    s
                }
            }
        })
        .collect()
}

// ── CompiledBlueprint ─────────────────────────────────────────────────────────

/// A blueprint that has already been compiled to Rust source by PBGC.
#[derive(Debug, Clone)]
pub struct CompiledBlueprint {
    /// Original name as given to the blueprint graph.
    pub name: String,
    /// Rust source emitted by the blueprint compiler.
    pub source: String,
    /// Whether this blueprint has a `tick` event entry point in its source.
    pub has_tick: bool,
    /// Whether this blueprint has a `begin_play` event entry point in its source.
    pub has_begin_play: bool,
}

impl CompiledBlueprint {
    /// Create from a name and compiled source, auto-detecting event entry points.
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        let has_tick = source.contains("fn tick") || source.contains("fn on_tick");
        let has_begin_play =
            source.contains("fn begin_play") || source.contains("fn on_begin_play");
        Self {
            name: name.into(),
            source,
            has_tick,
            has_begin_play,
        }
    }

    pub fn with_tick(mut self, has_tick: bool) -> Self {
        self.has_tick = has_tick;
        self
    }

    pub fn with_begin_play(mut self, has_begin_play: bool) -> Self {
        self.has_begin_play = has_begin_play;
        self
    }
}

// ── ProjectSpec ───────────────────────────────────────────────────────────────

/// Everything needed to generate blueprint output files.
pub struct ProjectSpec {
    pub name: String,
    pub version: String,
    pub description: String,
    pub blueprints: Vec<CompiledBlueprint>,
}

impl ProjectSpec {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: "0.1.0".into(),
            description: String::new(),
            blueprints: Vec::new(),
        }
    }

    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = v.into();
        self
    }

    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    pub fn add_blueprint(mut self, bp: CompiledBlueprint) -> Self {
        self.blueprints.push(bp);
        self
    }
}

// ── GeneratedProject ──────────────────────────────────────────────────────────

/// The output of [`generate_project`] — blueprint files ready to write to disk.
pub struct GeneratedProject {
    /// Map of relative file path → file content.
    pub files: BTreeMap<String, String>,
}

impl GeneratedProject {
    /// Write every file to `<dir>/<relative_path>`, creating directories as needed.
    pub fn write_to_dir(&self, dir: impl AsRef<Path>) -> std::io::Result<()> {
        let base = dir.as_ref();
        for (rel_path, content) in &self.files {
            let full = base.join(rel_path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, content)?;
        }
        Ok(())
    }

    pub fn file_paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }
}

// ── generate_project ─────────────────────────────────────────────────────────

/// Generate blueprint-owned source files (`src/blueprints/*.rs`) from a [`ProjectSpec`].
pub fn generate_project(spec: &ProjectSpec) -> GeneratedProject {
    let mut files = BTreeMap::new();
    files.insert("src/blueprints/mod.rs".into(), gen_blueprints_mod(spec));

    for bp in &spec.blueprints {
        let ident = to_snake_case(&bp.name);
        files.insert(
            format!("src/blueprints/{ident}.rs"),
            gen_blueprint_actor(bp),
        );
    }

    GeneratedProject { files }
}

/// Compile a graph and wrap it as a generated actor source file.
///
/// This is the safest high-level API for callers that need runnable class code
/// (struct + `#[derive(EngineClass)]` + `impl Actor`) instead of raw logic.
pub fn compile_graph_to_actor_source(
    blueprint_name: &str,
    graph: &GraphDescription,
) -> Result<String, GraphyError> {
    let source = compile_graph(graph)?;
    Ok(generate_blueprint_actor_source(blueprint_name, &source))
}

/// Wrap already compiled raw PBGC logic into a generated actor source file.
pub fn generate_blueprint_actor_source(blueprint_name: &str, compiled_source: &str) -> String {
    let bp = CompiledBlueprint::new(blueprint_name, compiled_source);
    gen_blueprint_actor(&bp)
}

// ── File generators ───────────────────────────────────────────────────────────

fn gen_blueprints_mod(spec: &ProjectSpec) -> String {
    let mod_decls: String = spec
        .blueprints
        .iter()
        .map(|bp| format!("pub mod {};\n", to_snake_case(&bp.name)))
        .collect();

    let use_decls: String = spec
        .blueprints
        .iter()
        .map(|bp| {
            let ident = to_snake_case(&bp.name);
            let ty = to_pascal_case(&bp.name);
            format!("pub use {ident}::{ty};\n")
        })
        .collect();

    let class_name_matches: String = spec
        .blueprints
        .iter()
        .map(|bp| {
            let ty = to_pascal_case(&bp.name);
            format!("        \"{ty}\" => Some(actors.register({ty}::new(), world)),\n")
        })
        .collect();

    let class_names: String = spec
        .blueprints
        .iter()
        .map(|bp| format!("\"{}\"", to_pascal_case(&bp.name)))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        r#"//! Blueprint actor registry — generated by PBGC.
//!
//! Blueprint class structs auto-register with `pulsar_reflection` via
//! `#[derive(EngineClass)]` in each generated file.

{mod_decls}
{use_decls}
use pulsar_game::{{ActorRegistry, Entity, World}};

/// List all compiled blueprint class names.
pub fn compiled_class_names() -> &'static [&'static str] {{
    &[{class_names}]
}}

/// Spawn a compiled blueprint class by name.
pub fn spawn_compiled_class(
    class_name: &str,
    world: &mut World,
    actors: &mut ActorRegistry,
) -> Option<Entity> {{
    match class_name {{
{class_name_matches}        _ => None,
    }}
}}
"#
    )
}

fn gen_blueprint_actor(bp: &CompiledBlueprint) -> String {
    let ident = to_snake_case(&bp.name);
    let ty = to_pascal_case(&bp.name);

    let begin_play_body = if bp.has_begin_play {
        "        logic::begin_play();\n".to_string()
    } else {
        "        // No begin_play event in this blueprint.\n".to_string()
    };

    let tick_body = if bp.has_tick {
        "        logic::tick();\n".to_string()
    } else {
        "        // No tick event in this blueprint.\n".to_string()
    };

    let indented_source: String = bp
        .source
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                "\n".into()
            } else {
                format!("    {line}\n")
            }
        })
        .collect();

    format!(
        r#"//! Blueprint actor: `{ident}`
//! Generated by PBGC. Do not hand-edit — changes will be overwritten.

use pulsar_game::prelude::*;
use engine_class_derive::EngineClass;

#[derive(Clone, EngineClass)]
pub struct {ty} {{}}

impl {ty} {{
    pub fn new() -> Self {{ Self {{}} }}
}}

impl Default for {ty} {{
    fn default() -> Self {{ Self::new() }}
}}

impl Actor for {ty} {{
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {{
{begin_play_body}    }}

    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {{
{tick_body}    }}
}}

mod logic {{
{indented_source}}}
"#
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_spec() -> ProjectSpec {
        let source = "pub fn begin_play() { let x = add(1.0, 2.0); print_number(x); }";
        ProjectSpec::new("my_game")
            .version("0.1.0")
            .description("A test Pulsar game")
            .add_blueprint(CompiledBlueprint::new("player_controller", source))
            .add_blueprint(
                CompiledBlueprint::new("enemy_ai", "pub fn tick() { }").with_tick(true),
            )
    }

    #[test]
    fn generates_expected_files() {
        let project = generate_project(&sample_spec());
        let paths: Vec<&str> = project.file_paths().collect();
        assert!(paths.contains(&"src/blueprints/mod.rs"));
        assert!(paths.contains(&"src/blueprints/player_controller.rs"));
        assert!(paths.contains(&"src/blueprints/enemy_ai.rs"));
    }

    #[test]
    fn actor_file_contains_struct_and_impl() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/blueprints/player_controller.rs"];
        assert!(actor.contains("pub struct PlayerController"));
        assert!(actor.contains("#[derive(Clone, EngineClass)]"));
        assert!(actor.contains("impl Actor for PlayerController"));
        assert!(actor.contains("logic::begin_play()"));
        assert!(actor.contains("No tick event in this blueprint"));
    }

    #[test]
    fn generate_actor_source_includes_engineclass_derive() {
        let actor = generate_blueprint_actor_source("player_controller", "pub fn begin_play() {}\n");
        assert!(actor.contains("pub struct PlayerController"));
        assert!(actor.contains("#[derive(Clone, EngineClass)]"));
    }

    #[test]
    fn enemy_actor_wires_tick() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/blueprints/enemy_ai.rs"];
        assert!(actor.contains("logic::tick()"));
    }

    #[test]
    fn mod_file_exports_all_actors() {
        let project = generate_project(&sample_spec());
        let modfile = &project.files["src/blueprints/mod.rs"];
        assert!(modfile.contains("pub mod player_controller"));
        assert!(modfile.contains("pub mod enemy_ai"));
        assert!(modfile.contains("pub use player_controller::PlayerController"));
        assert!(modfile.contains("pub use enemy_ai::EnemyAi"));
        assert!(modfile.contains("fn compiled_class_names"));
        assert!(modfile.contains("fn spawn_compiled_class"));
    }

    #[test]
    fn snake_to_pascal() {
        assert_eq!(to_pascal_case("player_controller"), "PlayerController");
        assert_eq!(to_pascal_case("enemy_ai"), "EnemyAi");
    }

    #[test]
    fn pascal_to_snake() {
        assert_eq!(to_snake_case("PlayerController"), "player_controller");
        assert_eq!(to_snake_case("my_cool_actor"), "my_cool_actor");
    }

    #[test]
    fn write_to_dir() {
        let project = generate_project(&sample_spec());
        let dir = std::env::temp_dir().join("pbgc_project_gen_test");
        project.write_to_dir(&dir).unwrap();
        assert!(dir.join("src/blueprints/mod.rs").exists());
        assert!(dir.join("src/blueprints/player_controller.rs").exists());
        assert!(dir.join("src/blueprints/enemy_ai.rs").exists());
    }
}
