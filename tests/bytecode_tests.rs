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
use pbgc::{
    compile_graph, compile_graph_to_bytecode, compile_graph_to_bytecode_with_variables, BpProgram,
    Instruction,
};
use std::collections::HashMap;

// ── Native dispatch shims (mirrors what pulsar_macros #[blueprint] generates) ─
//
// ABI: unsafe extern "C" fn(args: *const *const u8, ret: *mut u8)
//   args[i] → pointer into the byte arena at the i-th input's offset
//   ret     → pointer to the output region in the arena

unsafe extern "C" fn shim_add(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const i64);
    let b = std::ptr::read(*args.add(1) as *const i64);
    std::ptr::write(ret as *mut i64, a + b);
}
unsafe extern "C" fn shim_subtract(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const i64);
    let b = std::ptr::read(*args.add(1) as *const i64);
    std::ptr::write(ret as *mut i64, a - b);
}
unsafe extern "C" fn shim_multiply(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const i64);
    let b = std::ptr::read(*args.add(1) as *const i64);
    std::ptr::write(ret as *mut i64, a * b);
}
unsafe extern "C" fn shim_divide(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const i64);
    let b = std::ptr::read(*args.add(1) as *const i64);
    std::ptr::write(ret as *mut i64, if b == 0 { 0 } else { a / b });
}
unsafe extern "C" fn shim_modulo(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const i64);
    let b = std::ptr::read(*args.add(1) as *const i64);
    std::ptr::write(ret as *mut i64, if b == 0 { 0 } else { a % b });
}
unsafe extern "C" fn shim_greater_than(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const f64);
    let b = std::ptr::read(*args.add(1) as *const f64);
    std::ptr::write(ret as *mut bool, a > b);
}
unsafe extern "C" fn shim_less_than(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const f64);
    let b = std::ptr::read(*args.add(1) as *const f64);
    std::ptr::write(ret as *mut bool, a < b);
}
unsafe extern "C" fn shim_abs(args: *const *const u8, ret: *mut u8) {
    let v = std::ptr::read(*args as *const f64);
    std::ptr::write(ret as *mut f64, v.abs());
}
unsafe extern "C" fn shim_sqrt(args: *const *const u8, ret: *mut u8) {
    let v = std::ptr::read(*args as *const f64);
    std::ptr::write(ret as *mut f64, v.sqrt());
}
unsafe extern "C" fn shim_lerp(args: *const *const u8, ret: *mut u8) {
    let a = std::ptr::read(*args as *const f64);
    let b = std::ptr::read(*args.add(1) as *const f64);
    let t = std::ptr::read(*args.add(2) as *const f64);
    std::ptr::write(ret as *mut f64, a + (b - a) * t);
}
unsafe extern "C" fn shim_clamp(args: *const *const u8, ret: *mut u8) {
    let v = std::ptr::read(*args as *const f64);
    let min = std::ptr::read(*args.add(1) as *const f64);
    let max = std::ptr::read(*args.add(2) as *const f64);
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
    let actual = std::ptr::read(*args as *const i64);
    let expected = std::ptr::read(*args.add(1) as *const i64);
    assert_eq!(actual, expected, "assert_eq_int failed");
}
unsafe extern "C" fn shim_assert_eq_float(args: *const *const u8, _ret: *mut u8) {
    let actual = std::ptr::read(*args as *const f64);
    let expected = std::ptr::read(*args.add(1) as *const f64);
    let eps = std::ptr::read(*args.add(2) as *const f64);
    assert!(
        (actual - expected).abs() < eps,
        "assert_eq_float failed: |{} - {}| = {} >= eps {}",
        actual,
        expected,
        (actual - expected).abs(),
        eps
    );
}

// ── Dispatch table builder ────────────────────────────────────────────────────

fn prepare_with_shims(program: &mut BpProgram) {
    for instr in &mut program.instructions {
        if let Instruction::Call {
            fn_ptr, node_type, ..
        } = instr
        {
            *fn_ptr = match node_type.as_str() {
                "add" => shim_add as u64,
                "subtract" => shim_subtract as u64,
                "multiply" => shim_multiply as u64,
                "divide" => shim_divide as u64,
                "modulo" => shim_modulo as u64,
                "greater_than" => shim_greater_than as u64,
                "less_than" => shim_less_than as u64,
                "abs" => shim_abs as u64,
                "sqrt" => shim_sqrt as u64,
                "lerp" => shim_lerp as u64,
                "clamp" => shim_clamp as u64,
                "print_string" => shim_print_string as u64,
                "assert_true" => shim_assert_true as u64,
                "assert_false" => shim_assert_false as u64,
                "assert_eq_int" => shim_assert_eq_int as u64,
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
    for prog in &mut programs {
        run(prog);
    }
}

// ── Graph builders ────────────────────────────────────────────────────────────

fn begin(pin: &str) -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
    n.outputs.push(PinInstance::new(
        pin,
        Pin::new(pin, "Body", DataType::Exec, PinType::Output),
    ));
    n
}

fn add_node(id: &str, ca: Option<f64>, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    if let Some(v) = ca {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = cb {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    n
}

fn get_bit_node(id: &str, value: i64, bit_index: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "get_bit", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_value"),
        Pin::new(
            &format!("{id}_value"),
            "value",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_bit_index"),
        Pin::new(
            &format!("{id}_bit_index"),
            "bit_index",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_result"),
        Pin::new(
            &format!("{id}_result"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert(format!("{id}_value"), serde_json::json!(value));
    n.properties
        .insert(format!("{id}_bit_index"), serde_json::json!(bit_index));
    n
}

fn mul_node(id: &str, ca: Option<f64>, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "multiply", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    if let Some(v) = ca {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = cb {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    n
}

fn gt_node(id: &str, cb: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "greater_than", Position { x: 200.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("bool"),
            PinType::Output,
        ),
    ));
    if let Some(v) = cb {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    n
}

fn branch_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "branch", Position { x: 300.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_c"),
        Pin::new(
            &format!("{id}_c"),
            "condition",
            DataType::typed("bool"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_t"),
        Pin::new(&format!("{id}_t"), "True", DataType::Exec, PinType::Output),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_f"),
        Pin::new(&format!("{id}_f"), "False", DataType::Exec, PinType::Output),
    ));
    n
}

fn getter_node(id: &str, var_name: &str) -> NodeInstance {
    let mut n = NodeInstance::new(
        id,
        &format!("get_{}", var_name),
        Position { x: 100.0, y: 0.0 },
    );
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("i64"),
            PinType::Output,
        ),
    ));
    n
}

fn assert_eq_int_node(id: &str, expected: i64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_int", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "actual",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_x"),
        Pin::new(
            &format!("{id}_x"),
            "expected",
            DataType::typed("i64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output),
    ));
    n.properties
        .insert(format!("{id}_x"), serde_json::json!(expected as f64));
    n
}

fn assert_eq_float_node(id: &str, expected: f64, epsilon: f64) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_float", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "actual",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_x"),
        Pin::new(
            &format!("{id}_x"),
            "expected",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_ep"),
        Pin::new(
            &format!("{id}_ep"),
            "epsilon",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output),
    ));
    n.properties
        .insert(format!("{id}_x"), serde_json::json!(expected));
    n.properties
        .insert(format!("{id}_ep"), serde_json::json!(epsilon));
    n
}

fn assert_true_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_true", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_c"),
        Pin::new(
            &format!("{id}_c"),
            "condition",
            DataType::typed("bool"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output),
    ));
    n
}

fn lerp_node(id: &str, ca: Option<f64>, cb: Option<f64>, ct: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "lerp", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_a"),
        Pin::new(
            &format!("{id}_a"),
            "a",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_b"),
        Pin::new(
            &format!("{id}_b"),
            "b",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_t"),
        Pin::new(
            &format!("{id}_t"),
            "t",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "result",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    if let Some(v) = ca {
        n.properties.insert(format!("{id}_a"), serde_json::json!(v));
    }
    if let Some(v) = cb {
        n.properties.insert(format!("{id}_b"), serde_json::json!(v));
    }
    if let Some(v) = ct {
        n.properties.insert(format!("{id}_t"), serde_json::json!(v));
    }
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
    let has_init = programs[0]
        .instructions
        .iter()
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
    let has_init = programs[0]
        .instructions
        .iter()
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
    assert!(programs[0]
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::JumpIf { .. })));
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
    let has_add = programs[0]
        .instructions
        .iter()
        .any(|i| matches!(i, Instruction::Call { node_type, .. } if node_type == "add"));
    assert!(has_add, "should have a Call with node_type='add'");
}

#[test]
fn standard_nodes_with_get_prefix_are_not_variable_getters() {
    let mut g = GraphDescription::new("get_bit_is_standard_node");
    g.add_node(begin("be"));
    g.add_node(get_bit_node("bit", 10, 1));
    g.add_node(assert_eq_int_node("chk", 1));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("bit", "bit_result", "chk", "chk_a"));

    let programs = compile_graph_to_bytecode(&g).expect("compile get_bit bytecode");
    assert!(programs.iter().any(|program| {
        program.instructions.iter().any(|instruction| {
            matches!(instruction, Instruction::Call { node_type, .. } if node_type == "get_bit")
        })
    }));

    let rust = compile_graph(&g).expect("compile get_bit Rust");
    assert!(rust.contains("get_bit"));
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
    gt.properties
        .insert("gt_a".to_string(), serde_json::json!(10.0));
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
    gt.properties
        .insert("gt_a".to_string(), serde_json::json!(10.0));
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
fn deep_pure_chain_compiles_without_stack_growth() {
    let programs = compile_graph_to_bytecode(&deep_pure_graph(500_000))
        .expect("deep pure graph should compile without stack overflow");
    assert_eq!(programs.len(), 1);
    assert!(programs[0].instructions.len() >= 500_000);
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
    println!(
        "[timing] 1000 × 10-node VM: {:?} ({:.2}µs/run)",
        t.elapsed(),
        t.elapsed().as_micros() as f64 / 1000.0
    );
}

// ── Cycle detection ──────────────────────────────────────────────────────────

#[test]
fn pure_dependency_cycle_detected() {
    use graphy::GraphyError;
    let mut g = GraphDescription::new("cycle");
    g.add_node(begin("be"));
    // A -> B -> C -> A pure cycle
    g.add_node(add_node("a", Some(1.0), None));
    g.add_node(add_node("b", None, None));
    g.add_node(add_node("c", None, None));
    g.add_connection(data("a", "a_r", "b", "b_a"));
    g.add_connection(data("b", "b_r", "c", "c_a"));
    g.add_connection(data("c", "c_r", "a", "a_b"));
    // Trigger compilation by connecting a pure node to an exec path
    g.add_node(assert_eq_int_node("chk", 0));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    g.add_connection(data("a", "a_r", "chk", "chk_a"));

    let result = compile_graph_to_bytecode(&g);
    assert!(result.is_err(), "expected cyclic dependency error");
    let err = result.unwrap_err();
    assert!(
        matches!(err, GraphyError::CyclicDependency)
            || matches!(&err, GraphyError::Custom(msg) if msg.contains("Cyclic dependency")),
        "expected cycle error, got {:?}",
        err
    );
}

// ── Getter deduplication ────────────────────────────────────────────────────

#[test]
fn getter_with_multiple_consumers_emits_once() {
    let mut vars = HashMap::new();
    vars.insert("myvar".to_string(), "i64".to_string());
    let mut g = GraphDescription::new("getter_dedup");
    g.add_node(begin("be"));
    // Create a get_myvar node
    g.add_node(getter_node("g", "myvar"));
    // Two pure nodes that both read from get_myvar
    g.add_node(add_node("a", None, None));
    g.add_node(add_node("b", None, None));
    g.add_node(assert_eq_int_node("chk", 0));
    g.add_connection(exec("begin", "be", "chk", "chk_e"));
    // Connect getter to both pure nodes
    g.add_connection(data("g", "g_r", "a", "a_a"));
    g.add_connection(data("g", "g_r", "b", "b_a"));
    // Connect both pure nodes through chained add to assert
    g.add_connection(data("a", "a_r", "chk", "chk_a"));

    let programs =
        compile_graph_to_bytecode_with_variables(&g, vars).expect("compile with variables");
    let loadvar_count = programs[0]
        .instructions
        .iter()
        .filter(|i| matches!(i, Instruction::LoadVar { .. }))
        .count();
    assert_eq!(
        loadvar_count, 1,
        "expected exactly one LoadVar for getter shared by multiple consumers, got {}",
        loadvar_count
    );
}

// ── Component ops (comp_get_prop / comp_set_prop / comp_call) ────────────────

use pbgc::bytecode::comp_ops::{decode_name_blob, json_blob_len, CompOpKind, ComponentOpRef};
use pbgc::bytecode::TypeSlot;
use pbgc::compile_graph_to_bytecode_full;
use std::cell::RefCell;

fn comp_get_node(id: &str, class: &str, prop: &str) -> NodeInstance {
    let mut n = NodeInstance::new(
        id,
        &format!("comp_get_prop::{class}::{prop}"),
        Position { x: 100.0, y: 0.0 },
    );
    // Editor comp_* nodes always carry the (optional) component_ref target
    // pin (#654); unconnected means self-targeted.
    n.inputs.push(PinInstance::new(
        "component_ref",
        Pin::new(
            "component_ref",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "value",
            DataType::typed("f64"),
            PinType::Output,
        ),
    ));
    n
}

fn comp_set_node(id: &str, class: &str, prop: &str, value: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(
        id,
        &format!("comp_set_prop::{class}::{prop}"),
        Position { x: 200.0, y: 0.0 },
    );
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        "component_ref",
        Pin::new(
            "component_ref",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Input,
        ),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{id}_v"),
        Pin::new(
            &format!("{id}_v"),
            "value",
            DataType::typed("f64"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output),
    ));
    if let Some(v) = value {
        n.properties.insert(format!("{id}_v"), serde_json::json!(v));
    }
    n
}

fn comp_call_node(id: &str, class: &str, method: &str, with_return: bool) -> NodeInstance {
    let mut n = NodeInstance::new(
        id,
        &format!("comp_call::{class}::{method}"),
        Position { x: 300.0, y: 0.0 },
    );
    n.inputs.push(PinInstance::new(
        &format!("{id}_e"),
        Pin::new(&format!("{id}_e"), "exec", DataType::Exec, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        "component_ref",
        Pin::new(
            "component_ref",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_o"),
        Pin::new(&format!("{id}_o"), "exec", DataType::Exec, PinType::Output),
    ));
    if with_return {
        n.outputs.push(PinInstance::new(
            &format!("{id}_r"),
            Pin::new(
                &format!("{id}_r"),
                "result",
                DataType::typed("f64"),
                PinType::Output,
            ),
        ));
    }
    n
}

#[test]
fn comp_ops_compile_with_staged_operands_and_component_list() {
    let mut g = GraphDescription::new("comp_ops");
    g.add_node(begin("be"));
    g.add_node(comp_set_node("set", "Light", "intensity", Some(3.5)));
    g.add_connection(exec("begin", "be", "set", "set_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    assert_eq!(
        compiled.components,
        vec![ComponentOpRef {
            kind: CompOpKind::SetProp,
            class_name: "Light".into(),
            member: "intensity".into(),
        }]
    );

    // Name blob + JSON value blob are staged as InitBytes; the op is a Call
    // carrying the full node_type so the executor can route on the prefix.
    let prog = &compiled.programs[0];
    assert!(prog.instructions.iter().any(|i| matches!(
        i,
        Instruction::Call { node_type, input_offsets, has_output: false, .. }
            if node_type == "comp_set_prop::Light::intensity" && input_offsets.len() == 2
    )));

    // Serde round trip preserves everything byte-for-byte.
    let json = serde_json::to_string(prog).unwrap();
    let back: pbgc::BpProgram = serde_json::from_str(&json).unwrap();
    assert_eq!(back.instructions.len(), prog.instructions.len());
}

#[test]
fn comp_get_feeding_comp_set_compiles_as_blob_chain() {
    let mut g = GraphDescription::new("comp_chain");
    g.add_node(begin("be"));
    g.add_node(comp_get_node("get", "Light", "intensity"));
    g.add_node(comp_set_node("set", "Light", "intensity", None));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_connection(data("get", "get_r", "set", "set_v"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    assert_eq!(compiled.components.len(), 2);
    let prog = &compiled.programs[0];
    // The set's value input resolves to the get's reserved output slot.
    let get_out = prog.instructions.iter().find_map(|i| match i {
        Instruction::Call {
            node_type,
            output_offset,
            has_output: true,
            ..
        } if node_type == "comp_get_prop::Light::intensity" => Some(*output_offset),
        _ => None,
    });
    let set_in = prog.instructions.iter().find_map(|i| match i {
        Instruction::Call {
            node_type,
            input_offsets,
            ..
        } if node_type == "comp_set_prop::Light::intensity" => Some(input_offsets[1]),
        _ => None,
    });
    assert_eq!(get_out, set_in, "get output slot feeds set value input");
}

#[test]
fn comp_call_without_return_has_no_output_slot() {
    let mut g = GraphDescription::new("comp_call_void");
    g.add_node(begin("be"));
    g.add_node(comp_call_node("call", "Door", "open", false));
    g.add_connection(exec("begin", "be", "call", "call_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    let has_output = compiled.programs[0].instructions.iter().any(|i| {
        matches!(
            i,
            Instruction::Call { node_type, has_output: true, .. }
                if node_type == "comp_call::Door::open"
        )
    });
    assert!(!has_output, "void call must not reserve an output slot");
}

// Stub component-op handlers mirroring what the world-connected executor
// (#647) will do: parse the name blob, decode JSON values, act.

thread_local! {
    static COMP_OPS_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Read a `{class}\0{member}\0` name blob staged in the arena: scan past two
/// NUL terminators and hand back exactly that span.
unsafe fn arena_name_blob<'a>(mut ptr: *const u8) -> &'a [u8] {
    let start = ptr;
    let mut terminators = 0usize;
    while terminators < 2 {
        if *ptr == 0 {
            terminators += 1;
        }
        ptr = ptr.add(1);
    }
    std::slice::from_raw_parts(start, ptr.offset_from(start) as usize)
}

unsafe fn log_op(kind: &str, name_ptr: *const u8, value_ptr: Option<*const u8>) {
    let (class, member) = decode_name_blob(arena_name_blob(name_ptr)).unwrap();
    let value = value_ptr.map(|p| {
        let len = json_blob_len(p);
        let bytes = std::slice::from_raw_parts(p.add(8), len);
        String::from_utf8_lossy(bytes).into_owned()
    });
    COMP_OPS_LOG.with(|log| {
        log.borrow_mut().push(format!(
            "{kind}:{class}:{member}:{}",
            value.unwrap_or_default()
        ))
    });
}

unsafe extern "C" fn stub_comp_get(args: *const *const u8, ret: *mut u8, _ts: *const TypeSlot) {
    log_op("get", *args, None);
    pbgc::bytecode::comp_ops::write_json_blob(ret, "42.5");
}

unsafe extern "C" fn stub_comp_set(args: *const *const u8, _ret: *mut u8, _ts: *const TypeSlot) {
    log_op("set", *args, Some(*args.add(1)));
}

unsafe extern "C" fn stub_comp_call(args: *const *const u8, ret: *mut u8, _ts: *const TypeSlot) {
    log_op("call", *args, None);
    if !ret.is_null() {
        pbgc::bytecode::comp_ops::write_json_blob(ret, "1.0");
    }
}

fn prepare_comp_shims(program: &mut BpProgram) {
    for instr in &mut program.instructions {
        if let Instruction::Call {
            fn_ptr, node_type, ..
        } = instr
        {
            if node_type.starts_with("comp_get_prop::") {
                *fn_ptr = stub_comp_get as u64;
            } else if node_type.starts_with("comp_set_prop::") {
                *fn_ptr = stub_comp_set as u64;
            } else if node_type.starts_with("comp_call::") {
                *fn_ptr = stub_comp_call as u64;
            }
        }
    }
}

#[test]
fn comp_ops_execute_against_stub_handlers() {
    COMP_OPS_LOG.with(|log| log.borrow_mut().clear());

    let mut g = GraphDescription::new("comp_exec");
    g.add_node(begin("be"));
    g.add_node(comp_set_node("set", "Light", "intensity", Some(7.0)));
    g.add_node(comp_call_node("call", "Door", "open", true));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_connection(exec("set", "set_o", "call", "call_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    for mut prog in compiled.programs {
        prepare_comp_shims(&mut prog);
        pbgc::vm::run(&prog).unwrap();
    }

    let log = COMP_OPS_LOG.with(|log| log.borrow().clone());
    assert_eq!(log.len(), 2, "both component ops executed");
    assert_eq!(log[1], "call:Door:open:");
    assert!(log[0].starts_with("set:Light:intensity:"), "{}", log[0]);
    // The staged constant survives as JSON; number formatting may differ.
    let staged: f64 = log[0]["set:Light:intensity:".len()..]
        .parse()
        .unwrap_or_else(|_| panic!("staged value is not a JSON number: {}", log[0]));
    assert!((staged - 7.0).abs() < 1e-6);
}

#[test]
fn comp_get_result_flows_into_set_through_arena() {
    COMP_OPS_LOG.with(|log| log.borrow_mut().clear());

    let mut g = GraphDescription::new("comp_flow");
    g.add_node(begin("be"));
    g.add_node(comp_get_node("get", "Light", "intensity"));
    g.add_node(comp_set_node("set", "Light", "intensity", None));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_connection(data("get", "get_r", "set", "set_v"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    for mut prog in compiled.programs {
        prepare_comp_shims(&mut prog);
        pbgc::vm::run(&prog).unwrap();
    }

    let log = COMP_OPS_LOG.with(|log| log.borrow().clone());
    assert_eq!(
        log,
        vec![
            "get:Light:intensity:".to_string(),
            "set:Light:intensity:42.5".to_string(),
        ]
    );
}

// ── Rust-source emission (#651) ───────────────────────────────────────────────
//
// The same graphs also compile to generated actor source. That emission must
// route through `pulsar_world_registry`'s dispatcher against the live-world
// `(entity, world)` parameters and never re-introduce the retired
// baked-store routing (`__bp_with_comp` + private `ComponentStore`).

#[test]
fn rust_emission_routes_comp_ops_through_the_live_dispatcher() {
    let mut g = GraphDescription::new("rust_comp");
    g.add_node(begin("be"));
    g.add_node(comp_get_node("get", "Light", "intensity"));
    g.add_node(comp_set_node("set", "Light", "intensity", Some(7.0)));
    g.add_node(comp_call_node("call", "Door", "open", true));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_connection(data("get", "get_r", "set", "set_v"));
    g.add_connection(exec("set", "set_o", "call", "call_e"));

    let logic = pbgc::compile_graph(&g).expect("rust compilation");
    let actor = pbgc::generate_blueprint_actor_source_with_components(
        "rust_probe",
        &logic,
        vec![pbgc::CompiledComponent {
            class_name: "Light".to_string(),
            property_defaults: serde_json::json!({ "intensity": 1.0 }),
            enabled: true,
        }],
    );

    // Dispatcher calls addressed at the live-world parameters.
    assert!(
        actor.contains("pulsar_world_registry::dispatch::get_component_property("),
        "comp_get_prop must read through the live dispatcher:\n{actor}"
    );
    assert!(
        actor.contains("pulsar_world_registry::dispatch::set_component_property(\n"),
        "comp_set_prop must write through the live dispatcher:\n{actor}"
    );
    assert!(
        actor.contains("json_args_to_method_args(\"Door\", \"open\"")
            && actor.contains(
                "invoke_component_method(_world, __bp_target_entity, \"Door\", __bp_target_index, \"open\", __args)"
            ),
        "comp_call must dispatch through the shared JSON->typed conversion:\n{actor}"
    );
    // Generated event functions receive the live world from the Actor impl.
    assert!(actor.contains(
        "pub fn begin_play(_entity: pulsar_game::Entity, _world: &mut pulsar_game::World)"
    ));
    // Live-world hydration of prefab components.
    assert!(actor.contains("world_component_present_for_class(\"Light\""));
    assert!(actor.contains("hydrate_world_component_for_class("));

    // The retired baked-store routing must be gone outright (#651).
    assert!(!actor.contains("__bp_with_comp"));
    assert!(!actor.contains("__bp_set_comp_ctx"));
    assert!(!actor.contains("__bp_clear_comp_ctx"));
    assert!(!actor.contains("ComponentStore"));
    assert!(!actor.contains("gamma_core"));
}

// ── Identity references (#654) ────────────────────────────────────────────────

use pbgc::bytecode::comp_ops::{decode_targeted_call_name_blob, decode_targeted_name_blob};

/// A `get_component_ref::Class::N` node: ComponentRef-typed output, optional
/// `actor` redirect input.
fn get_ref_node(id: &str, class: &str, index: u32, with_actor_pin: bool) -> NodeInstance {
    let mut n = NodeInstance::new(
        id,
        &format!("get_component_ref::{class}::{index}"),
        Position { x: 50.0, y: 0.0 },
    );
    if with_actor_pin {
        n.inputs.push(PinInstance::new(
            "actor",
            Pin::new(
                "actor",
                "actor",
                DataType::typed("ActorRef"),
                PinType::Input,
            ),
        ));
    }
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "component",
            DataType::typed("ComponentRef"),
            PinType::Output,
        ),
    ));
    n
}

/// A `find_object_by_name` resolver node: String needle in, ActorRef out.
fn find_node(id: &str, needle: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "find_object_by_name", Position { x: 25.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{id}_n"),
        Pin::new(
            &format!("{id}_n"),
            "name",
            DataType::typed("String"),
            PinType::Input,
        ),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{id}_r"),
        Pin::new(
            &format!("{id}_r"),
            "actor",
            DataType::typed("ActorRef"),
            PinType::Output,
        ),
    ));
    n.properties
        .insert(format!("{id}_n"), serde_json::json!(needle));
    n
}

/// #654: a comp_set_prop with a CONNECTED component_ref pin stages the
/// targeted name blob plus the reference operand between name and value.
#[test]
fn pin_targeted_comp_set_stages_reference_operand() {
    let mut g = GraphDescription::new("pin_targeted");
    g.add_node(begin("be"));
    g.add_node(get_ref_node("ref", "Light", 0, false));
    g.add_node(comp_set_node("set", "Light", "intensity", Some(3.0)));
    g.add_connection(data("ref", "ref_r", "set", "component_ref"));
    g.add_connection(exec("begin", "be", "set", "set_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    let prog = &compiled.programs[0];
    let call = prog.instructions.iter().find_map(|i| match i {
        Instruction::Call {
            node_type,
            input_offsets,
            ..
        } if node_type == "comp_set_prop::Light::intensity" => Some(input_offsets.clone()),
        _ => None,
    });
    let input_offsets = call.expect("pin-targeted set must compile to a Call");
    // name + reference operand + value operand.
    assert_eq!(input_offsets.len(), 3);

    // The staged name blob carries the trailing `pin` target field.
    let blob = prog.instructions.iter().find_map(|i| match i {
        Instruction::InitBytes { offset, bytes } if *offset == input_offsets[0] => {
            Some(bytes.clone())
        }
        _ => None,
    });
    let fields = decode_targeted_name_blob(&blob.expect("name blob staged")).expect("decodes");
    assert_eq!(fields.target, pbgc::bytecode::comp_ops::RefTarget::RefPin);
    assert_eq!(
        compiled.components,
        vec![
            ComponentOpRef {
                kind: CompOpKind::GetRef,
                class_name: "Light".into(),
                member: "0".into()
            },
            ComponentOpRef {
                kind: CompOpKind::SetProp,
                class_name: "Light".into(),
                member: "intensity".into()
            },
        ]
    );
}

/// #654: find_object_by_name feeding get_component_ref's actor pin, feeding
/// a comp_get_prop's component_ref pin — the whole cross-object chain
/// compiles as pure blob producers with runtime resolution.
#[test]
fn cross_object_chain_compiles_as_identity_producers() {
    let mut g = GraphDescription::new("cross_object");
    g.add_node(begin("be"));
    g.add_node(find_node("find", "door_light"));
    g.add_node(get_ref_node("ref", "Light", 2, true));
    g.add_node(comp_get_node("get", "Light", "intensity"));
    g.add_connection(data("find", "find_r", "ref", "actor"));
    g.add_connection(data("ref", "ref_r", "get", "component_ref"));
    g.add_node(comp_set_node("set", "Light", "intensity", None));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_connection(data("get", "get_r", "set", "set_v"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    let prog = &compiled.programs[0];

    // All three identity/value producers emitted Calls with outputs.
    for node_type in [
        "find_object_by_name",
        "get_component_ref::Light::2",
        "comp_get_prop::Light::intensity",
        "comp_set_prop::Light::intensity",
    ] {
        assert!(
            prog.instructions.iter().any(|i| matches!(
                i,
                Instruction::Call { node_type: nt, .. } if nt == node_type
            )),
            "missing Call for {node_type}"
        );
    }

    // The get op is pin-targeted (ref wired into its component_ref pin):
    // name + reference operand.
    let get_inputs = prog.instructions.iter().find_map(|i| match i {
        Instruction::Call {
            node_type,
            input_offsets,
            ..
        } if node_type == "comp_get_prop::Light::intensity" => Some(input_offsets.len()),
        _ => None,
    });
    assert_eq!(get_inputs, Some(2));
    // The set stays self-targeted (its own component_ref pin unconnected):
    // legacy name + value shape.
    let set_inputs = prog.instructions.iter().find_map(|i| match i {
        Instruction::Call {
            node_type,
            input_offsets,
            ..
        } if node_type == "comp_set_prop::Light::intensity" => Some(input_offsets.len()),
        _ => None,
    });
    assert_eq!(set_inputs, Some(2));
}

/// #654: the Rust emission routes cross-object graphs through
/// `pulsar_game::script_refs` — the SAME helpers the VM trampolines call —
/// with the dispatcher still doing the property access.
#[test]
fn rust_emission_routes_references_through_script_refs() {
    let mut g = GraphDescription::new("rust_refs");
    g.add_node(begin("be"));
    g.add_node(find_node("find", "door_light"));
    g.add_node(get_ref_node("ref", "Light", 0, true));
    g.add_connection(data("find", "find_r", "ref", "actor"));
    g.add_node(comp_set_node("set", "Light", "color", None));
    g.add_connection(data("ref", "ref_r", "set", "component_ref"));
    g.add_connection(exec("begin", "be", "set", "set_e"));
    g.add_node(comp_get_node("get", "Light", "color"));
    g.add_connection(data("ref", "ref_r", "get", "component_ref"));

    let logic = pbgc::compile_graph(&g).expect("rust compilation");
    assert!(
        logic.contains("pulsar_game::script_refs::find_object_by_name("),
        "resolver nodes must resolve through script_refs:\n{logic}"
    );
    assert!(
        logic.contains("pulsar_game::script_refs::component_ref_json(")
            && logic.contains("\"Light\",\n                0,"),
        "get_component_ref must build its ref through script_refs:\n{logic}"
    );
    assert!(
        logic.contains("pulsar_game::script_refs::resolve_pin_target("),
        "pin-targeted ops must resolve their target through script_refs:\n{logic}"
    );
    assert!(
        logic.contains("__bp_target_index,"),
        "pin-targeted ops must carry the reference's component_index:\n{logic}"
    );

    // An object literal emits its save/load form for runtime resolution —
    // never baked entity bits (#639). It only materializes when consumed,
    // so wire it into a set op's component_ref pin.
    let mut lit = GraphDescription::new("rust_literal");
    lit.add_node(begin("be"));
    let mut literal = NodeInstance::new("lit", "object_ref_literal", Position { x: 10.0, y: 0.0 });
    literal.outputs.push(PinInstance::new(
        "lit_r",
        Pin::new(
            "lit_r",
            "component",
            DataType::typed("ComponentRef"),
            PinType::Output,
        ),
    ));
    literal
        .properties
        .insert("stable_id".to_string(), serde_json::json!("door"));
    literal
        .properties
        .insert("class_name".to_string(), serde_json::json!("Light"));
    literal
        .properties
        .insert("component_index".to_string(), serde_json::json!(1));
    lit.add_node(literal);
    lit.add_node(comp_set_node("set", "Light", "color", Some(9.0)));
    lit.add_connection(data("lit", "lit_r", "set", "component_ref"));
    lit.add_connection(exec("begin", "be", "set", "set_e"));
    let lit_logic = pbgc::compile_graph(&lit).expect("literal compilation");
    assert!(
        lit_logic.contains("pulsar_game::script_refs::object_literal_json(")
            && lit_logic.contains("\"door\",")
            && lit_logic.contains("\"Light\","),
        "literals must emit their serialized form:\n{lit_logic}"
    );
    assert!(
        lit_logic.contains("resolve_pin_target("),
        "the consuming set must resolve through the literal:\n{lit_logic}"
    );
}

/// #654: self-targeted ops carry the EXPLICIT `self` target field (ABI v2) —
/// the runtime reader scans a fixed field count, so no legacy shapes exist in
/// fresh compilations.
#[test]
fn unconnected_pins_carry_explicit_self_target() {
    let mut g = GraphDescription::new("self_shape");
    g.add_node(begin("be"));
    g.add_node(comp_set_node("set", "Light", "intensity", Some(1.0)));
    g.add_connection(exec("begin", "be", "set", "set_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    let prog = &compiled.programs[0];
    let inputs = prog
        .instructions
        .iter()
        .find_map(|i| match i {
            Instruction::Call {
                node_type,
                input_offsets,
                ..
            } if node_type == "comp_set_prop::Light::intensity" => Some(input_offsets.clone()),
            _ => None,
        })
        .expect("set compiles");
    assert_eq!(inputs.len(), 2, "self-targeted: name + value only");
    let blob = prog.instructions.iter().find_map(|i| match i {
        Instruction::InitBytes { offset, bytes } if *offset == inputs[0] => Some(bytes.clone()),
        _ => None,
    });
    let fields = decode_targeted_name_blob(blob.as_deref().unwrap()).expect("decodes");
    assert_eq!(
        fields.target,
        pbgc::bytecode::comp_ops::RefTarget::SelfActor
    );
    // The staged bytes really do end with the `self` field.
    let blob = blob.unwrap();
    assert!(blob.ends_with(b"self\0"));
}

/// #654: a comp_call can be pin-targeted; its name blob then carries argc
/// AND the target field, with the reference operand first among values.
#[test]
fn pin_targeted_call_stages_argc_and_target() {
    let mut g = GraphDescription::new("call_targeted");
    g.add_node(begin("be"));
    g.add_node(get_ref_node("ref", "Door", 0, false));
    g.add_node(comp_call_node("call", "Door", "open", true));
    g.add_connection(data("ref", "ref_r", "call", "component_ref"));
    g.add_connection(exec("begin", "be", "call", "call_e"));

    let compiled = compile_graph_to_bytecode_full(&g, Default::default()).unwrap();
    let prog = &compiled.programs[0];
    let inputs = prog
        .instructions
        .iter()
        .find_map(|i| match i {
            Instruction::Call {
                node_type,
                input_offsets,
                ..
            } if node_type == "comp_call::Door::open" => Some(input_offsets.clone()),
            _ => None,
        })
        .expect("call compiles");
    // name + reference operand (no method args on this node).
    assert_eq!(inputs.len(), 2);
    let blob = prog.instructions.iter().find_map(|i| match i {
        Instruction::InitBytes { offset, bytes } if *offset == inputs[0] => Some(bytes.clone()),
        _ => None,
    });
    let fields = decode_targeted_call_name_blob(&blob.unwrap()).expect("decodes");
    assert_eq!(fields.arg_count, 0);
    assert_eq!(fields.target, pbgc::bytecode::comp_ops::RefTarget::RefPin);
}
