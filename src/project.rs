//! Blueprint output generator.
//!
//! Turns a collection of compiled blueprints into blueprint-owned Rust source
//! files only.  Core project/bootstrap files (`Cargo.toml`, `main.rs`) are
//! intentionally out of scope and must be handled by the core build system.

use std::collections::BTreeMap;
use std::path::Path;

use graphy::{GraphDescription, GraphyError};

use crate::compile_graph;

const VARIABLE_STORAGE_BEGIN: &str = "// PBGC_VARIABLE_STORAGE_BEGIN";
const VARIABLE_STORAGE_END: &str = "// PBGC_VARIABLE_STORAGE_END";

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

/// Represents a single component instance baked into a compiled blueprint.
///
/// The `class_name` must match a class in the reflection registry.
/// `property_defaults` is raw JSON mirroring the prefab asset's component data.
#[derive(Debug, Clone)]
pub struct CompiledComponent {
    /// Class name as registered in `pulsar_reflection::REGISTRY`.
    pub class_name: String,
    /// JSON property overrides from the prefab sidecar.
    pub property_defaults: serde_json::Value,
    /// Whether this component is enabled.
    pub enabled: bool,
}

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
    /// Class variables declared by the blueprint asset.
    pub variables: Vec<CompiledVariable>,
    /// Component instances attached to this prefab (from the prefab sidecar).
    pub components: Vec<CompiledComponent>,
}

#[derive(Debug, Clone)]
pub struct CompiledVariable {
    pub name: String,
    pub rust_type: String,
    pub default_value: Option<String>,
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
            variables: Vec::new(),
            components: Vec::new(),
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

    pub fn with_variables(mut self, variables: Vec<CompiledVariable>) -> Self {
        self.variables = variables;
        self
    }

    pub fn with_components(mut self, components: Vec<CompiledComponent>) -> Self {
        self.components = components;
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

/// Generate a complete Pulsar class module tree from a [`ProjectSpec`].
///
/// Actor implementations live in `events/events.rs`, matching the layout used
/// by the Blueprint Editor:
///
/// ```text
/// src/classes/<class>/
/// ├── mod.rs
/// ├── events/
/// │   ├── mod.rs
/// │   └── events.rs
/// └── vars/
///     └── mod.rs
/// ```
pub fn generate_project(spec: &ProjectSpec) -> GeneratedProject {
    let mut files = BTreeMap::new();
    files.insert("src/classes/mod.rs".into(), gen_blueprints_mod(spec));

    for bp in &spec.blueprints {
        let ident = to_snake_case(&bp.name);
        files.insert(
            format!("src/classes/{ident}/mod.rs"),
            gen_blueprint_mod(&ident),
        );
        files.insert(
            format!("src/classes/{ident}/events/mod.rs"),
            gen_blueprint_events_mod(),
        );
        files.insert(
            format!("src/classes/{ident}/events/events.rs"),
            gen_blueprint_actor(bp),
        );
        files.insert(
            format!("src/classes/{ident}/vars/mod.rs"),
            gen_blueprint_vars(bp),
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

/// Wrap compiled logic + component data into a generated actor source file.
pub fn generate_blueprint_actor_source_with_components(
    blueprint_name: &str,
    compiled_source: &str,
    components: Vec<CompiledComponent>,
) -> String {
    let bp = CompiledBlueprint::new(blueprint_name, compiled_source)
        .with_components(components);
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

    format!(
        r#"//! Blueprint class module — generated by PBGC.
//!
//! Blueprint class structs auto-register with `pulsar_reflection` via
//! `#[derive(EngineClass)]` in each generated file.

{mod_decls}
{use_decls}
"#
    )
}

fn gen_blueprint_mod(ident: &str) -> String {
    format!(
        r#"//! Blueprint class module: `{ident}`
//! Generated by PBGC. Do not hand-edit — changes will be overwritten.

pub mod vars;
pub mod events;
pub use events::*;
"#
    )
}

fn gen_blueprint_events_mod() -> String {
    r#"//! Blueprint event module — generated by PBGC.

pub mod events;
pub use events::*;
"#
    .to_string()
}

fn gen_blueprint_actor(bp: &CompiledBlueprint) -> String {
    let ident = to_snake_case(&bp.name);
    let ty = to_pascal_case(&bp.name);

    let has_components = !bp.components.is_empty();

    // ── Component init block ──────────────────────────────────────────────────
    // Generates the `__init_components` method body that constructs each
    // component from the prefab defaults baked in at compile time.
    let init_components_body: String = if has_components {
        let mut lines = String::new();
        for comp in &bp.components {
            if !comp.enabled {
                continue;
            }
            // Serialize the JSON defaults so they can be embedded as a string literal.
            let json_str = serde_json::to_string(&comp.property_defaults)
                .unwrap_or_else(|_| "{}".to_string())
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            let class = &comp.class_name;
            lines.push_str(&format!(
                "        std::sync::Arc::get_mut(&mut self.components).expect(\"exclusive Arc\").add_from_registry(\"{class}\", &serde_json::from_str(\"{json_str}\").unwrap_or(serde_json::json!({{}})));\n"
            ));
        }
        lines
    } else {
        "        // No components on this prefab.\n".to_string()
    };

    // ── Set up custom events ──────────────────────────────────────────────────
    let has_custom_events = bp.source.contains("on_player_died")
        || bp.source.contains("emit_event");
    let init_events_body = if has_custom_events {
        "\
        // Register custom event handlers (added by PBGC for custom blueprint events).\n\
        // Subscriptions are set up here so they are active before begin_play runs.\n".to_string()
    } else {
        String::new()
    };

    // ── begin_play body ───────────────────────────────────────────────────────
    let begin_play_body = {
        let mut body = String::new();
        // Always initialise components first (idempotent after first call).
        if has_components {
            body.push_str("        self.__init_components();\n");
            // Run any component begin_plays via reflection.
            body.push_str("        self.__run_component_begin_plays();\n");
        }
        // Set the thread-local execution context so logic fns can access components.
        if has_components {
            body.push_str(
                "        pulsar_game::__bp_set_comp_ctx(std::sync::Arc::get_mut(&mut self.components).expect(\"exclusive Arc\"));\n",
            );
        }
        if has_custom_events && !init_events_body.is_empty() {
            body.push_str("        self.__init_events();\n");
        }
        if bp.has_begin_play {
            body.push_str("        logic::begin_play();\n");
        } else {
            body.push_str("        // No begin_play event in this blueprint.\n");
        }
        if has_components {
            body.push_str("        pulsar_game::__bp_clear_comp_ctx();\n");
        }
        body
    };

    // ── tick body ─────────────────────────────────────────────────────────────
    let tick_body = {
        let mut body = String::new();
        if has_components {
            body.push_str(
                "        pulsar_game::__bp_set_comp_ctx(std::sync::Arc::get_mut(&mut self.components).expect(\"exclusive Arc\"));\n",
            );
        }
        if bp.has_tick {
            body.push_str("        logic::tick();\n");
        } else {
            body.push_str("        // No tick event in this blueprint.\n");
        }
        if has_components {
            body.push_str("        pulsar_game::__bp_clear_comp_ctx();\n");
        }
        body
    };

    // ── Set up custom events ──────────────────────────────────────────────────
    let has_custom_events = bp.source.contains("on_player_died")
        || bp.source.contains("emit_event");
    let init_events_body = if has_custom_events {
        "\
        // Register custom event handlers (added by PBGC for custom blueprint events).\n\
        // Subscriptions are set up here so they are active before begin_play runs.\n".to_string()
    } else {
        String::new()
    };

    // ── Struct definition ─────────────────────────────────────────────────────
    let struct_def = if has_components {
        format!(
            r#"pub struct {ty} {{
    /// Runtime component store — populated lazily in `begin_play`.
    pub components: std::sync::Arc<pulsar_game::ComponentStore>,
    /// Per-instance event bus for custom blueprint events.
    pub events: gamma_core::EventBus,
}}"#
        )
    } else {
        format!(
            r#"pub struct {ty} {{
    /// Per-instance event bus for custom blueprint events.
    pub events: gamma_core::EventBus,
}}"#
        )
    };

    let new_body = if has_components {
        format!(
            "Self {{ components: std::sync::Arc::new(pulsar_game::ComponentStore::new()), events: gamma_core::EventBus::new() }}"
        )
    } else {
        "Self { events: gamma_core::EventBus::new() }".to_string()
    };

    // ── Component helper impls ────────────────────────────────────────────────
    let event_helpers = if has_custom_events {
        format!(
            r#"
impl {ty} {{
    /// Initialise custom event subscriptions.
    /// Called at the start of `begin_play`.
    pub fn __init_events(&mut self) {{
{init_events_body}    }}
}}
"#
        )
    } else {
        String::new()
    };

    let component_helpers = if has_components {
        format!(
            r#"
impl {ty} {{
    /// Initialise components from the prefab defaults baked in at compile time.
    ///
    /// Idempotent — does nothing if components are already present.
    pub fn __init_components(&mut self) {{
        if !self.components.is_empty() {{ return; }}
{init_components_body}    }}

    /// Call `begin_play` on each component that implements it.
    pub fn __run_component_begin_plays(&mut self) {{
        let class_names: Vec<String> = self.components.iter().map(|(name, _)| name.to_string()).collect();
        for class_name in &class_names {{
            if let Some(methods) = pulsar_reflection::REGISTRY.get_methods(class_name) {{
                if let Some(bp_method) = methods.into_iter().find(|m| m.name == "begin_play") {{
                    if let Some(comp) = std::sync::Arc::get_mut(&mut self.components).expect("exclusive Arc").get_by_name_mut(class_name) {{
                        (bp_method.caller)(comp, vec![]);
                    }}
                }}
            }}
        }}
    }}
}}
"#
        )
    } else {
        String::new()
    };

    // ── Logic source ──────────────────────────────────────────────────────────
    let indented_source: String = logic_source_for_class(bp)
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                "\n".into()
            } else {
                format!("    {line}\n")
            }
        })
        .collect();

    let extra_imports = if has_components {
        "use pulsar_reflection;\n#[allow(unused_imports)]\nuse serde_json;\n"
    } else {
        ""
    };

    // events.rs lives at <Class>/events/events.rs and is declared as a sub-module
    // of <Class>/events/mod.rs. The module path is therefore:
    //   crate::classes::<Class>::events::events          (outer struct/impls)
    //   crate::classes::<Class>::events::events::logic   (inner mod)
    //
    // vars/ lives at <Class>/vars/mod.rs and is declared by <Class>/mod.rs.
    // Reaching it from the outer scope:  super::super::vars
    // Reaching it from inside mod logic: super::super::super::vars
    //
    // We do NOT emit `pub mod vars;` here — the parent mod.rs owns that declaration.
    format!(
        r#"//! Blueprint actor: `{ident}`
//! Generated by PBGC. Do not hand-edit — changes will be overwritten.

use pulsar_game::prelude::*;
use engine_class_derive::EngineClass;
#[allow(unused_imports)]
use pulsar_std::*;
{extra_imports}
// vars lives two levels up: <Class>::events::events → <Class>::events → <Class>
#[allow(unused_imports)]
use super::super::vars::*;

#[derive(EngineClass, Clone)]
{struct_def}

impl {ty} {{
    pub fn new() -> Self {{ {new_body} }}
}}

impl Default for {ty} {{
    fn default() -> Self {{ Self::new() }}
}}
{component_helpers}
{event_helpers}
impl Actor for {ty} {{
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {{
{begin_play_body}    }}

    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {{
{tick_body}    }}
}}

mod logic {{
    // vars is three levels up inside this inline module:
    // logic → events::events → events → <Class> → vars
    #[allow(unused_imports)]
    use super::super::super::vars::*;
    #[allow(unused_imports)]
    use pulsar_std::*;

{indented_source}}}
"#
    )
}

fn gen_blueprint_vars(bp: &CompiledBlueprint) -> String {
    let body = extract_variable_storage_block(&bp.source)
        .unwrap_or_else(|| default_variable_storage_block(&bp.variables));

    format!(
        "//! Blueprint class vars for `{}`\n//! Generated by PBGC. Do not hand-edit.\n\n{}\n",
        to_snake_case(&bp.name),
        body.trim()
    )
}

fn logic_source_for_class(bp: &CompiledBlueprint) -> String {
    if bp.variables.is_empty() {
        return bp.source.clone();
    }

    strip_variable_storage_block(&bp.source)
}

fn extract_variable_storage_block(source: &str) -> Option<String> {
    let start = source.find(VARIABLE_STORAGE_BEGIN)? + VARIABLE_STORAGE_BEGIN.len();
    let end = source.find(VARIABLE_STORAGE_END)?;
    Some(source[start..end].trim().to_string())
}

fn strip_variable_storage_block(source: &str) -> String {
    let Some(start) = source.find(VARIABLE_STORAGE_BEGIN) else {
        return source.to_string();
    };
    let Some(end_marker_start) = source.find(VARIABLE_STORAGE_END) else {
        return source.to_string();
    };
    let end = end_marker_start + VARIABLE_STORAGE_END.len();

    let before = source[..start].trim_end();
    let after = source[end..].trim_start();

    if before.is_empty() {
        after.to_string()
    } else if after.is_empty() {
        before.to_string()
    } else {
        format!("{}\n\n{}", before, after)
    }
}

fn default_variable_storage_block(variables: &[CompiledVariable]) -> String {
    if variables.is_empty() {
        return "// This blueprint declares no class variables.".to_string();
    }

    let mut body = String::from("use std::cell::{Cell, RefCell};\n\nthread_local! {\n");
    let mut vars: Vec<_> = variables.iter().collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));

    for var in vars {
        let static_name = to_static_var_name(&var.name);
        if is_copy_type(&var.rust_type) {
            body.push_str(&format!(
                "    pub(super) static {}: Cell<Option<{}>> = Cell::new(None);\n",
                static_name, var.rust_type
            ));
        } else {
            body.push_str(&format!(
                "    pub(super) static {}: RefCell<Option<{}>> = RefCell::new(None);\n",
                static_name, var.rust_type
            ));
        }
    }

    body.push_str("}\n\n");
    body.push_str("#[inline]\npub(super) fn __pbgc_set_copy<T: Copy>(slot: &Cell<Option<T>>, value: T) {\n    slot.set(Some(value));\n}\n\n");
    body.push_str("#[inline]\npub(super) fn __pbgc_get_copy<T: Copy>(slot: &Cell<Option<T>>, var_name: &str) -> T {\n    slot.get().unwrap_or_else(|| panic!(\"PBGC variable '{}' read before assignment\", var_name))\n}\n\n");
    body.push_str("#[inline]\npub(super) fn __pbgc_set_clone<T>(slot: &RefCell<Option<T>>, value: T) {\n    *slot.borrow_mut() = Some(value);\n}\n\n");
    body.push_str("#[inline]\npub(super) fn __pbgc_get_clone<T: Clone>(slot: &RefCell<Option<T>>, var_name: &str) -> T {\n    slot.borrow().as_ref().cloned().unwrap_or_else(|| panic!(\"PBGC variable '{}' read before assignment\", var_name))\n}\n");
    body
}

fn is_copy_type(type_str: &str) -> bool {
    matches!(
        type_str,
        "i32" | "i64" | "u32" | "u64" | "f32" | "f64" | "bool" | "char" |
        "usize" | "isize" | "i8" | "i16" | "u8" | "u16"
    )
}

fn to_static_var_name(var_name: &str) -> String {
    let mut out = String::from("PBGC_VAR_");
    for ch in var_name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    if out.ends_with('_') {
        out.push('X');
    }
    out
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
        assert!(paths.contains(&"src/classes/mod.rs"));
        assert!(paths.contains(&"src/classes/player_controller/mod.rs"));
        assert!(paths.contains(&"src/classes/player_controller/events/mod.rs"));
        assert!(paths.contains(&"src/classes/player_controller/events/events.rs"));
        assert!(paths.contains(&"src/classes/player_controller/vars/mod.rs"));
        assert!(paths.contains(&"src/classes/enemy_ai/mod.rs"));
        assert!(paths.contains(&"src/classes/enemy_ai/events/mod.rs"));
        assert!(paths.contains(&"src/classes/enemy_ai/events/events.rs"));
        assert!(paths.contains(&"src/classes/enemy_ai/vars/mod.rs"));
    }

    #[test]
    fn actor_file_contains_struct_and_impl() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/classes/player_controller/events/events.rs"];
        assert!(actor.contains("pub struct PlayerController"));
        assert!(actor.contains("#[derive(EngineClass, Clone)]"));
        assert!(actor.contains("impl Actor for PlayerController"));
        assert!(actor.contains("logic::begin_play()"));
        assert!(actor.contains("No tick event in this blueprint"));
    }

    #[test]
    fn generate_actor_source_includes_engineclass_derive() {
        let actor = generate_blueprint_actor_source("player_controller", "pub fn begin_play() {}\n");
        assert!(actor.contains("pub struct PlayerController"));
        assert!(actor.contains("#[derive(EngineClass, Clone)]"));
    }

    #[test]
    fn enemy_actor_wires_tick() {
        let project = generate_project(&sample_spec());
        let actor = &project.files["src/classes/enemy_ai/events/events.rs"];
        assert!(actor.contains("logic::tick()"));
    }

    #[test]
    fn class_modules_own_vars_and_reexport_events() {
        let project = generate_project(&sample_spec());
        let class_mod = &project.files["src/classes/player_controller/mod.rs"];
        let events_mod = &project.files["src/classes/player_controller/events/mod.rs"];

        assert!(class_mod.contains("pub mod vars;"));
        assert!(class_mod.contains("pub mod events;"));
        assert!(class_mod.contains("pub use events::*;"));
        assert!(events_mod.contains("pub mod events;"));
        assert!(events_mod.contains("pub use events::*;"));
    }

    #[test]
    fn mod_file_exports_all_actors() {
        let project = generate_project(&sample_spec());
        let modfile = &project.files["src/classes/mod.rs"];
        assert!(modfile.contains("pub mod player_controller"));
        assert!(modfile.contains("pub mod enemy_ai"));
        assert!(modfile.contains("pub use player_controller::PlayerController"));
        assert!(modfile.contains("pub use enemy_ai::EnemyAi"));
        assert!(!modfile.contains("fn compiled_class_names"));
        assert!(!modfile.contains("fn spawn_compiled_class"));
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
        assert!(dir.join("src/classes/mod.rs").exists());
        assert!(dir.join("src/classes/player_controller/mod.rs").exists());
        assert!(dir.join("src/classes/player_controller/events/mod.rs").exists());
        assert!(dir.join("src/classes/player_controller/events/events.rs").exists());
        assert!(dir.join("src/classes/player_controller/vars/mod.rs").exists());
        assert!(dir.join("src/classes/enemy_ai/mod.rs").exists());
        assert!(dir.join("src/classes/enemy_ai/events/mod.rs").exists());
        assert!(dir.join("src/classes/enemy_ai/events/events.rs").exists());
        assert!(dir.join("src/classes/enemy_ai/vars/mod.rs").exists());
    }

    #[test]
    fn variables_are_extracted_into_class_vars_module() {
        let source = r#"
// PBGC_VARIABLE_STORAGE_BEGIN
thread_local! {
    pub(super) static PBGC_VAR_HEALTH: std::cell::Cell<Option<f64>> = std::cell::Cell::new(None);
}
// PBGC_VARIABLE_STORAGE_END

pub fn begin_play() { }
"#;

        let bp = CompiledBlueprint::new("player", source).with_variables(vec![CompiledVariable {
            name: "health".to_string(),
            rust_type: "f64".to_string(),
            default_value: Some("100.0".to_string()),
        }]);

        let spec = ProjectSpec::new("game").add_blueprint(bp);
        let project = generate_project(&spec);

        let class_mod = &project.files["src/classes/player/mod.rs"];
        let actor = &project.files["src/classes/player/events/events.rs"];
        let vars = &project.files["src/classes/player/vars/mod.rs"];

        assert!(class_mod.contains("pub mod vars;"));
        assert!(actor.contains("use super::super::vars::*;"));
        assert!(!actor.contains("PBGC_VARIABLE_STORAGE_BEGIN"));
        assert!(!actor.contains("thread_local!"));
        assert!(vars.contains("thread_local!"));
        assert!(vars.contains("PBGC_VAR_HEALTH"));
    }
}
