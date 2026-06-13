/// PBGC bytecode compiler + VM tests.
///
/// All dispatch is via raw `unsafe extern "C" fn(*const *const u8, *mut u8)` pointers —
/// the same ABI the `#[blueprint]` macro generates in pulsar_std.
/// No BpValue, no NodeDispatch trait, no type matching in the VM.
use std::time::Instant;

use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position,
};
use pbgc::{compile_graph, compile_graph_to_bytecode, BpProgram, Instruction};

// ── Native dispatch shims (mirrors what pulsar_macros #[blueprint] generates) ─
//
// ABI: unsafe extern "C" fn(args: *const *const u8, ret: *mut u8)
//   args[i] → pointer into the byte arena at the i-th input's offset
//   ret     → pointer to the output region in the arena

unsafe extern "C" fn shim_add(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const i64);
    let b = std::ptr::read(*args.add(1)    as *const i64);
    std::ptr::write(ret as *mut i64, a + b);
}
unsafe extern "C" fn shim_subtract(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const i64);
    let b = std::ptr::read(*args.add(1)    as *const i64);
    std::ptr::write(ret as *mut i64, a - b);
}
unsafe extern "C" fn shim_multiply(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const i64);
    let b = std::ptr::read(*args.add(1)    as *const i64);
    std::ptr::write(ret as *mut i64, a * b);
}
unsafe extern "C" fn shim_divide(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const i64);
    let b = std::ptr::read(*args.add(1)    as *const i64);
    std::ptr::write(ret as *mut i64, if b == 0 { 0 } else { a / b });
}
unsafe extern "C" fn shim_modulo(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const i64);
    let b = std::ptr::read(*args.add(1)    as *const i64);
    std::ptr::write(ret as *mut i64, if b == 0 { 0 } else { a % b });
}
unsafe extern "C" fn shim_greater_than(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const f64);
    let b = std::ptr::read(*args.add(1)    as *const f64);
    std::ptr::write(ret as *mut bool, a > b);
}
unsafe extern "C" fn shim_less_than(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const f64);
    let b = std::ptr::read(*args.add(1)    as *const f64);
    std::ptr::write(ret as *mut bool, a < b);
}
unsafe extern "C" fn shim_abs(args: *const *const u8, ret: *mut u8) {
    let v = std::ptr::read(*args           as *const f64);
    std::ptr::write(ret as *mut f64, v.abs());
}
unsafe extern "C" fn shim_sqrt(args: *const *const u8, ret: *mut u8) {
    let v = std::ptr::read(*args           as *const f64);
    std::ptr::write(ret as *mut f64, v.sqrt());
}
unsafe extern "C" fn shim_lerp(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args           as *const f64);
    let b = std::ptr::read(*args.add(1)    as *const f64);
    let t = std::ptr::read(*args.add(2)    as *const f64);
    std::ptr::write(ret as *mut f64, a + (b - a) * t);
}
unsafe extern "C" fn shim_clamp(args: *const *const u8, ret: *mut u8) {
    let v   = std::ptr::read(*args         as *const f64);
    let min = std::ptr::read(*args.add(1)  as *const f64);
    let max = std::ptr::read(*args.add(2)  as *const f64);
    std::ptr::write(ret as *mut f64, v.clamp(min, max));
}
unsafe extern "C" fn shim_print_string(_args: *const *const u8, _ret: *mut u8) {}

// ── Assert shims ──────────────────────────────────────────────────────────────

unsafe extern "C" fn shim_assert_true(args: *const *const u8, _ret: *mut u8) {
    let cond = std::ptr::read(*args as *const bool);
    assert!(cond, "assert_true failed: condition was false");
}
unsafe extern "C" fn shim_assert_false(args: *const *const u8, _ret: *mut u8) {
    let cond = std::ptr::read(*args as *const bool);
    assert!(!cond, "assert_false failed: condition was true");
}
unsafe extern "C" fn shim_assert_eq_int(args: *const *const u8, _ret: *mut u8) {
    let actual   = std::ptr::read(*args        as *const i64);
    let expected = std::ptr::read(*args.add(1) as *const i64);
    assert_eq!(actual, expected, "assert_eq_int failed");
}
unsafe extern "C" fn shim_assert_eq_float(args: *const *const u8, _ret: *mut u8) {
    let actual   = std::ptr::read(*args        as *const f64);
    let expected = std::ptr::read(*args.add(1) as *const f64);
    let eps      = std::ptr::read(*args.add(2) as *const f64);
    assert!(
        (actual - expected).abs() < eps,
        "assert_eq_float failed: |{} - {}| = {} >= eps {}",
        actual, expected, (actual - expected).abs(), eps
    );
}

// ── Dispatch table builder ────────────────────────────────────────────────────

fn prepare_with_shims(program: &mut BpProgram) {
    for instr in &mut program.instructions {
        if let Instruction::Call { fn_ptr, node_type, .. } = instr {
            *fn_ptr = match node_type.as_str() {
                "add"             => shim_add             as u64,
                "subtract"        => shim_subtract        as u64,
                "multiply"        => shim_multiply        as u64,
                "divide"          => shim_divide          as u64,
                "modulo"          => shim_modulo          as u64,
                "greater_than"    => shim_greater_than    as u64,
                "less_than"       => shim_less_than       as u64,
                "abs"             => shim_abs             as u64,
                "sqrt"            => shim_sqrt            as u64,
                "lerp"            => shim_lerp            as u64,
                "clamp"           => shim_clamp           as u64,
                "print_string"    => shim_print_string    as u64,
                "assert_true"     => shim_assert_true     as u64,
                "assert_false"    => shim_assert_false    as u64,
                "assert_eq_int"   => shim_assert_eq_int   as u64,
                "assert_eq_float" => shim_assert_eq_float as u64,
                other => panic!("no shim for: {}", other),
            };
        }
    }
}

fn run(program: &mut BpProgram) {
    prepare_with_shims(program);
    pbgc::vm::run(program).unwrap();
}

fn compile_and_run(graph: &GraphDescription) {
    let mut programs = compile_graph_to_bytecode(graph).expect("compile failed");
    for prog in &mut programs { run(prog); }
}

// ── Graph builders ────────────────────────────────────────────────────────────

fn begin(pin: &str) -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
    n.outputs.push(PinInstance::new(pin, Pin::new(pin, "Body", DataType::Exec, PinType::Output)));
    n
}

fn add_node(id: &str, ca: Option<f64>, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_a"), Pin::new(&format!("{id}_a"), "a", DataType::typed("i64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_b"), Pin::new(&format!("{id}_b"), "b", DataType::typed("i64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_r"), Pin::new(&format!("{id}_r"), "result", DataType::typed("i64"), PinType::Output)));
    if let Some(v) = ca { n.properties.insert(format!("{id}_a"), serde_json::json!(v)); }
    if let Some(v) = cb { n.properties.insert(format!("{id}_b"), serde_json::json!(v)); }
    n
}

fn mul_node(id: &str, ca: Option<f64>, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "multiply", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_a"), Pin::new(&format!("{id}_a"), "a", DataType::typed("i64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_b"), Pin::new(&format!("{id}_b"), "b", DataType::typed("i64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_r"), Pin::new(&format!("{id}_r"), "result", DataType::typed("i64"), PinType::Output)));
    if let Some(v) = ca { n.properties.insert(format!("{id}_a"), serde_json::json!(v)); }
    if let Some(v) = cb { n.properties.insert(format!("{id}_b"), serde_json::json!(v)); }
    n
}

fn gt_node(id: &str, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "greater_than", Position { x: 200.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_a"), Pin::new(&format!("{id}_a"), "a", DataType::typed("f64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_b"), Pin::new(&format!("{id}_b"), "b", DataType::typed("f64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_r"), Pin::new(&format!("{id}_r"), "result", DataType::typed("bool"), PinType::Output)));
    if let Some(v) = cb { n.properties.insert(format!("{id}_b"), serde_json::json!(v)); }
    n
}

fn branch_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "branch", Position { x: 300.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_e"), Pin::new(&format!("{id}_e"), "exec",      DataType::Exec, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_c"), Pin::new(&format!("{id}_c"), "condition", DataType::typed("bool"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_t"), Pin::new(&format!("{id}_t"), "True",  DataType::Exec, PinType::Output)));
    n.outputs.push(PinInstance::new(&format!("{id}_f"), Pin::new(&format!("{id}_f"), "False", DataType::Exec, PinType::Output)));
    n
}

fn assert_eq_int_node(id: &str, expected: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_int", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_e"), Pin::new(&format!("{id}_e"), "exec",     DataType::Exec, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_a"), Pin::new(&format!("{id}_a"), "actual",   DataType::typed("i64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_x"), Pin::new(&format!("{id}_x"), "expected", DataType::typed("i64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_o"), Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output)));
    n.properties.insert(format!("{id}_x"), serde_json::json!(expected as f64));
    n
}

fn assert_eq_float_node(id: &str, expected: f64, epsilon: f64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_float", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_e"),  Pin::new(&format!("{id}_e"),  "exec",     DataType::Exec, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_a"),  Pin::new(&format!("{id}_a"),  "actual",   DataType::typed("f64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_x"),  Pin::new(&format!("{id}_x"),  "expected", DataType::typed("f64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_ep"), Pin::new(&format!("{id}_ep"), "epsilon",  DataType::typed("f64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_o"),  Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output)));
    n.properties.insert(format!("{id}_x"),  serde_json::json!(expected));
    n.properties.insert(format!("{id}_ep"), serde_json::json!(epsilon));
    n
}

fn assert_true_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_true", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_e"), Pin::new(&format!("{id}_e"), "exec",      DataType::Exec, PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_c"), Pin::new(&format!("{id}_c"), "condition", DataType::typed("bool"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_o"), Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output)));
    n
}

fn lerp_node(id: &str, ca: Option<f64>, cb: Option<f64>, ct: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "lerp", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{id}_a"), Pin::new(&format!("{id}_a"), "a", DataType::typed("f64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_b"), Pin::new(&format!("{id}_b"), "b", DataType::typed("f64"), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{id}_t"), Pin::new(&format!("{id}_t"), "t", DataType::typed("f64"), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{id}_r"), Pin::new(&format!("{id}_r"), "result", DataType::typed("f64"), PinType::Output)));
    if let Some(v) = ca { n.properties.insert(format!("{id}_a"), serde_json::json!(v)); }
    if let Some(v) = cb { n.properties.insert(format!("{id}_b"), serde_json::json!(v)); }
    if let Some(v) = ct { n.properties.insert(format!("{id}_t"), serde_json::json!(v)); }
    n
}

fn conn(from: &str, from_pin: &str, to: &str, to_pin: &str, ct: ConnectionType) -> Connection {
    Connection::new(from, from_pin, to, to_pin, ct)
}
fn exec(from: &str, fp: &str, to: &str, tp: &str) -> Connection {
    conn(from, fp, to, tp, ConnectionType::Execution)
}
fn data(from: &str, fp: &str, to: &str, tp: &str) -> Connection {
    conn(from, fp, to, tp, ConnectionType::Data)
}

// ── Instruction structure tests ───────────────────────────────────────────────

#[test]
fn test_init_bytes_emitted_for_integer_constant() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(3.0), Some(4.0)));
    g.add_node(assert_eq_int_node("chk", 7));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a", "a_r", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let has_init = programs[0].instructions.iter()
        .any(|i| matches!(i, Instruction::InitBytes { .. }));
    assert!(has_init, "should emit InitBytes for integer constants");
}

#[test]
fn test_init_bytes_emitted_for_float_constant() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_node("l", Some(0.0), Some(1.0), Some(0.5)));
    g.add_node(assert_eq_float_node("chk", 0.5, 1e-9));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("l", "l_r", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let has_init = programs[0].instructions.iter()
        .any(|i| matches!(i, Instruction::InitBytes { .. }));
    assert!(has_init, "should emit InitBytes for float constants");
}

#[test]
fn test_branch_emits_jumpif() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(gt_node("gt", Some(0.0)));
    g.add_node(branch_node("br"));
    g.add_connection(exec("begin", "be", "br", "br_e"));
    g.add_connection(data("gt", "gt_r", "br", "br_c"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    assert!(programs[0].instructions.iter().any(|i| matches!(i, Instruction::JumpIf { .. })));
}

#[test]
fn test_node_type_embedded_in_call_instruction() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(1.0), Some(2.0)));
    g.add_node(assert_eq_int_node("chk", 3));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a", "a_r", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let has_add = programs[0].instructions.iter().any(|i| {
        matches!(i, Instruction::Call { node_type, .. } if node_type == "add")
    });
    assert!(has_add, "should have a Call with node_type='add'");
}

#[test]
fn test_arena_size_positive_when_nodes_produce_values() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(gt_node("gt", Some(0.0)));
    let mut gt2 = gt_node("gt2", Some(0.0));
    g.add_node(gt2);
    g.add_node(branch_node("br"));
    g.add_connection(exec("begin", "be", "br", "br_e"));
    g.add_connection(data("gt", "gt_r", "br", "br_c"));
    let programs = compile_graph_to_bytecode(&g).unwrap();
    assert!(programs[0].arena_size > 0, "arena_size should be > 0");
}

// ── Correctness: arithmetic ───────────────────────────────────────────────────

#[test]
fn test_correct_add_3_4_eq_7() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(3.0), Some(4.0)));
    g.add_node(assert_eq_int_node("chk", 7));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a", "a_r", "chk", "chk_a"));
    compile_and_run(&g);
}

#[test]
fn test_correct_multiply_6_7_eq_42() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(mul_node("m", Some(6.0), Some(7.0)));
    g.add_node(assert_eq_int_node("chk", 42));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("m", "m_r", "chk", "chk_a"));
    compile_and_run(&g);
}

#[test]
fn test_correct_pythagorean_3_4_5() {
    // 3²+4² = 25
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(mul_node("s3", Some(3.0), Some(3.0)));
    g.add_node(mul_node("s4", Some(4.0), Some(4.0)));
    g.add_node(add_node("sum", None, None));
    g.add_node(assert_eq_int_node("chk", 25));
    g.add_connection(data("s3", "s3_r", "sum", "sum_a"));
    g.add_connection(data("s4", "s4_r", "sum", "sum_b"));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("sum", "sum_r", "chk", "chk_a"));
    compile_and_run(&g);
}

#[test]
fn test_correct_add_chain_100_nodes_eq_100() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    for i in 0..100usize {
        let id = format!("a{i}");
        let ca = if i == 0 { Some(0.0) } else { None };
        g.add_node(add_node(&id, ca, Some(1.0)));
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(data(&prev, &format!("{prev}_r"), &id, &format!("{id}_a")));
        }
    }
    g.add_node(assert_eq_int_node("chk", 100));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a99", "a99_r", "chk", "chk_a"));
    compile_and_run(&g);
}

// ── Correctness: float math ───────────────────────────────────────────────────

#[test]
fn test_correct_lerp_midpoint() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_node("l", Some(0.0), Some(100.0), Some(0.5)));
    g.add_node(assert_eq_float_node("chk", 50.0, 1e-9));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("l", "l_r", "chk", "chk_a"));
    compile_and_run(&g);
}

#[test]
fn test_correct_lerp_at_zero_is_a() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_node("l", Some(42.0), Some(100.0), Some(0.0)));
    g.add_node(assert_eq_float_node("chk", 42.0, 1e-9));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("l", "l_r", "chk", "chk_a"));
    compile_and_run(&g);
}

#[test]
fn test_correct_lerp_at_one_is_b() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(lerp_node("l", Some(0.0), Some(77.0), Some(1.0)));
    g.add_node(assert_eq_float_node("chk", 77.0, 1e-9));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("l", "l_r", "chk", "chk_a"));
    compile_and_run(&g);
}

// ── Correctness: control flow ─────────────────────────────────────────────────

#[test]
fn test_correct_branch_true_fires_assert_true() {
    // 10 > 5 → true → assert_true
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    let mut gt = gt_node("gt", Some(5.0));
    gt.properties.insert("gt_a".to_string(), serde_json::json!(10.0));
    g.add_node(gt);
    g.add_node(branch_node("br"));
    g.add_node(assert_true_node("at"));
    g.add_connection(exec("begin", "be", "br", "br_e"));
    g.add_connection(data("gt", "gt_r", "br", "br_c"));
    g.add_connection(exec("br", "br_t", "at", "at_e"));
    g.add_connection(data("gt", "gt_r", "at", "at_c"));
    compile_and_run(&g);
}

#[test]
fn test_correct_branch_false_path_not_taken_for_true_condition() {
    // 10 > 0 is true — false branch should NOT execute.
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    let mut gt = gt_node("gt", Some(0.0));
    gt.properties.insert("gt_a".to_string(), serde_json::json!(10.0));
    g.add_node(gt);
    g.add_node(branch_node("br"));
    g.add_node(assert_true_node("at"));
    g.add_node(assert_true_node("canary")); // would fail if reached (condition defaults to 0/false)
    g.add_connection(exec("begin", "be", "br", "br_e"));
    g.add_connection(data("gt", "gt_r", "br", "br_c"));
    g.add_connection(exec("br", "br_t", "at", "at_e"));
    g.add_connection(data("gt", "gt_r", "at", "at_c"));
    g.add_connection(exec("br", "br_f", "canary", "canary_e"));
    compile_and_run(&g);
}

// ── Bytecode serialisation ────────────────────────────────────────────────────

#[test]
fn test_serde_roundtrip_preserves_instructions() {
    let mut g = GraphDescription::new("t");
    g.add_node(begin("be"));
    g.add_node(add_node("a", Some(5.0), Some(3.0)));
    g.add_node(assert_eq_int_node("chk", 8));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a", "a_r", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).unwrap();
    let json = serde_json::to_string(&programs[0]).unwrap();
    let mut restored: BpProgram = serde_json::from_str(&json).unwrap();
    assert_eq!(programs[0].instructions.len(), restored.instructions.len());
    assert_eq!(programs[0].arena_size, restored.arena_size);
    // fn_ptrs are 0 after deserialization — re-prepare before running
    run(&mut restored);
}

// ── Timing ───────────────────────────────────────────────────────────────────

fn deep_pure_graph(n: usize) -> GraphDescription {
    let mut g = GraphDescription::new("deep");
    g.add_node(begin("be"));
    for i in 0..n {
        let id = format!("a{i}");
        let ca = if i == 0 { Some(0.0) } else { None };
        g.add_node(add_node(&id, ca, Some(1.0)));
        if i > 0 {
            let prev = format!("a{}", i - 1);
            g.add_connection(data(&prev, &format!("{prev}_r"), &id, &format!("{id}_a")));
        }
    }
    let last = format!("a{}", n - 1);
    g.add_node(gt_node("gt", Some(0.0)));
    g.add_node(branch_node("br"));
    g.add_connection(data(&last, &format!("{last}_r"), "gt", "gt_a"));
    g.add_connection(exec("begin", "be", "br", "br_e"));
    g.add_connection(data("gt", "gt_r", "br", "br_c"));
    g
}

#[test]
fn test_timing_compile_10_nodes() {
    let g = deep_pure_graph(10);
    let t = Instant::now();
    let p = compile_graph_to_bytecode(&g).unwrap();
    println!("[timing] 10-node compile: {:?}", t.elapsed());
    assert!(!p.is_empty());
}

#[test]
fn test_timing_compile_50_nodes_vs_rust_codegen() {
    let g = deep_pure_graph(50);
    let t0 = Instant::now();
    let _ = compile_graph(&g).unwrap();
    let rust_t = t0.elapsed();
    let t1 = Instant::now();
    let _ = compile_graph_to_bytecode(&g).unwrap();
    let bc_t = t1.elapsed();
    println!("[timing] 50-node Rust={:?}  Bytecode={:?}", rust_t, bc_t);
}

#[test]
fn test_timing_vm_execute_1000_times() {
    let g = deep_pure_graph(10);
    let mut programs = compile_graph_to_bytecode(&g).unwrap();
    prepare_with_shims(&mut programs[0]);
    let t = Instant::now();
    for _ in 0..1_000 {
        pbgc::vm::run(&programs[0]).unwrap();
    }
    println!("[timing] 1000 × 10-node VM: {:?} ({:.2}µs/run)",
        t.elapsed(), t.elapsed().as_micros() as f64 / 1000.0);
}

