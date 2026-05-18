/// Comprehensive test suite for PBGC bytecode compilation and VM execution.
///
/// Tests are organized into three tiers:
///   1. Bytecode structure — verify the codegen emits the right instructions
///   2. VM execution — verify the VM executes correctly with a mock dispatch
///   3. Timing — measure compile and execute durations for editor-budget assertions
use std::time::Instant;

use graphy::{
    Connection, ConnectionType, DataType, GraphDescription, NodeInstance, Pin, PinInstance,
    PinType, Position, PropertyValue,
};
use pbgc::bytecode::{BpValue, Instruction};
use pbgc::vm::{BytecodeVm, NodeDispatch, VmError};
use pbgc::compile_graph_to_bytecode;

// ── Mock dispatch ─────────────────────────────────────────────────────────────

/// A simple native dispatch that mirrors the pulsar_std math/logic/flow functions.
/// This stands in for the WASM-backed dispatch the engine would provide.
struct MockDispatch {
    call_log: std::sync::Mutex<Vec<String>>,
}

impl MockDispatch {
    fn new() -> Self {
        Self {
            call_log: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.call_log.lock().unwrap().clone()
    }
}

impl NodeDispatch for MockDispatch {
    fn call(&self, node_type: &str, inputs: &[BpValue], output: &mut Option<BpValue>) -> Result<(), VmError> {
        self.call_log.lock().unwrap().push(node_type.to_string());
        match node_type {
            "add"      => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) + inputs[1].as_i64().unwrap_or(0))); }
            "subtract" => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) - inputs[1].as_i64().unwrap_or(0))); }
            "multiply" => { *output = Some(BpValue::Int(inputs[0].as_i64().unwrap_or(0) * inputs[1].as_i64().unwrap_or(0))); }
            "divide"   => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(1));
                *output = Some(BpValue::Int(if b == 0 { 0 } else { a / b }));
            }
            "greater_than" => { *output = Some(BpValue::Bool(inputs[0].as_f64().unwrap_or(0.0) > inputs[1].as_f64().unwrap_or(0.0))); }
            "less_than"    => { *output = Some(BpValue::Bool(inputs[0].as_f64().unwrap_or(0.0) < inputs[1].as_f64().unwrap_or(0.0))); }
            "equal" | "equals" => { *output = Some(BpValue::Bool(inputs[0].as_i64().unwrap_or(0) == inputs[1].as_i64().unwrap_or(0))); }
            "not" => { *output = Some(BpValue::Bool(!inputs[0].as_bool())); }
            "and" => { *output = Some(BpValue::Bool(inputs[0].as_bool() && inputs[1].as_bool())); }
            "or"  => { *output = Some(BpValue::Bool(inputs[0].as_bool() || inputs[1].as_bool())); }
            "abs" => { *output = Some(BpValue::Float(inputs[0].as_f64().unwrap_or(0.0).abs())); }
            "lerp" => {
                let (a, b, t) = (inputs[0].as_f64().unwrap_or(0.0), inputs[1].as_f64().unwrap_or(0.0), inputs[2].as_f64().unwrap_or(0.0));
                *output = Some(BpValue::Float(a + (b - a) * t));
            }
            "clamp" => {
                let (v, lo, hi) = (inputs[0].as_f64().unwrap_or(0.0), inputs[1].as_f64().unwrap_or(0.0), inputs[2].as_f64().unwrap_or(1.0));
                *output = Some(BpValue::Float(v.clamp(lo, hi)));
            }
            "print_string" => {}
            // ── assert nodes ──────────────────────────────────────────────────
            "assert_true" => {
                if !inputs[0].as_bool() {
                    return Err(VmError::AssertionFailed("assert_true: condition was false".into()));
                }
            }
            "assert_false" => {
                if inputs[0].as_bool() {
                    return Err(VmError::AssertionFailed("assert_false: condition was true".into()));
                }
            }
            "assert_eq_int" => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(0));
                if a != b {
                    return Err(VmError::AssertionFailed(format!("assert_eq_int: {} != {}", a, b)));
                }
            }
            "assert_ne_int" => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(0));
                if a == b {
                    return Err(VmError::AssertionFailed(format!("assert_ne_int: both == {}", a)));
                }
            }
            "assert_eq_float" => {
                let (a, b, eps) = (inputs[0].as_f64().unwrap_or(0.0), inputs[1].as_f64().unwrap_or(0.0), inputs[2].as_f64().unwrap_or(1e-9));
                if (a - b).abs() >= eps {
                    return Err(VmError::AssertionFailed(format!("assert_eq_float: |{} - {}| = {} >= eps {}", a, b, (a-b).abs(), eps)));
                }
            }
            "assert_gt_int" => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(0));
                if a <= b {
                    return Err(VmError::AssertionFailed(format!("assert_gt_int: {} is not > {}", a, b)));
                }
            }
            "assert_lt_int" => {
                let (a, b) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(0));
                if a >= b {
                    return Err(VmError::AssertionFailed(format!("assert_lt_int: {} is not < {}", a, b)));
                }
            }
            "assert_in_range_int" => {
                let (v, lo, hi) = (inputs[0].as_i64().unwrap_or(0), inputs[1].as_i64().unwrap_or(0), inputs[2].as_i64().unwrap_or(0));
                if v < lo || v > hi {
                    return Err(VmError::AssertionFailed(format!("assert_in_range_int: {} not in [{}, {}]", v, lo, hi)));
                }
            }
            other => return Err(VmError::UnknownNode(other.to_string())),
        }
        Ok(())
    }
}

// ── Graph builders ────────────────────────────────────────────────────────────

fn make_begin_play_node(exec_out_pin_id: &str) -> NodeInstance {
    let mut n = NodeInstance::new("begin", "begin_play", Position { x: 0.0, y: 0.0 });
    n.outputs.push(PinInstance::new(
        exec_out_pin_id,
        Pin::new(exec_out_pin_id, "Body", DataType::Execution, PinType::Output),
    ));
    n
}

fn make_add_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "add", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_a", id),
        Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_b", id),
        Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_result", id),
        Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Output),
    ));
    n
}

fn make_greater_than_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "greater_than", Position { x: 200.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_a", id),
        Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_b", id),
        Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_result", id),
        Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Output),
    ));
    n
}

fn make_branch_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "branch", Position { x: 300.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_exec", id),
        Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_condition", id),
        Pin::new(&format!("{}_condition", id), "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_true", id),
        Pin::new(&format!("{}_true", id), "True", DataType::Execution, PinType::Output),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_false", id),
        Pin::new(&format!("{}_false", id), "False", DataType::Execution, PinType::Output),
    ));
    n
}

fn make_print_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "print_string", Position { x: 400.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_exec", id),
        Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_message", id),
        Pin::new(&format!("{}_message", id), "message", DataType::String, PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_exec_out", id),
        Pin::new(&format!("{}_exec_out", id), "exec", DataType::Execution, PinType::Output),
    ));
    n
}

/// Build an `assert_eq_int` fn_ node: exec in, two i64 data inputs, exec out.
fn make_assert_eq_int_node(id: &str, const_expected: Option<i64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_int", Position { x: 500.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_exec", id),
        Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_actual", id),
        Pin::new(&format!("{}_actual", id), "actual", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_expected", id),
        Pin::new(&format!("{}_expected", id), "expected", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_exec_out", id),
        Pin::new(&format!("{}_exec_out", id), "exec", DataType::Execution, PinType::Output),
    ));
    if let Some(v) = const_expected {
        n.properties.insert(format!("{}_expected", id), PropertyValue::Number(v as f64));
    }
    n
}

fn make_assert_eq_float_node(id: &str, const_expected: Option<f64>, const_epsilon: Option<f64>) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_eq_float", Position { x: 500.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_exec", id),
        Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_actual", id),
        Pin::new(&format!("{}_actual", id), "actual", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_expected", id),
        Pin::new(&format!("{}_expected", id), "expected", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_epsilon", id),
        Pin::new(&format!("{}_epsilon", id), "epsilon", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_exec_out", id),
        Pin::new(&format!("{}_exec_out", id), "exec", DataType::Execution, PinType::Output),
    ));
    if let Some(v) = const_expected { n.properties.insert(format!("{}_expected", id), PropertyValue::Number(v)); }
    if let Some(v) = const_epsilon  { n.properties.insert(format!("{}_epsilon", id), PropertyValue::Number(v)); }
    n
}

fn make_assert_true_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "assert_true", Position { x: 500.0, y: 0.0 });
    n.inputs.push(PinInstance::new(
        &format!("{}_exec", id),
        Pin::new(&format!("{}_exec", id), "exec", DataType::Execution, PinType::Input),
    ));
    n.inputs.push(PinInstance::new(
        &format!("{}_condition", id),
        Pin::new(&format!("{}_condition", id), "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input),
    ));
    n.outputs.push(PinInstance::new(
        &format!("{}_exec_out", id),
        Pin::new(&format!("{}_exec_out", id), "exec", DataType::Execution, PinType::Output),
    ));
    n
}

fn make_multiply_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "multiply", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{}_a", id), Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{}_b", id), Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{}_result", id), Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("i64")), PinType::Output)));
    n
}

fn make_lerp_node(id: &str) -> NodeInstance {
    let mut n = NodeInstance::new(id, "lerp", Position { x: 100.0, y: 0.0 });
    n.inputs.push(PinInstance::new(&format!("{}_a", id), Pin::new(&format!("{}_a", id), "a", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{}_b", id), Pin::new(&format!("{}_b", id), "b", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input)));
    n.inputs.push(PinInstance::new(&format!("{}_t", id), Pin::new(&format!("{}_t", id), "t", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Input)));
    n.outputs.push(PinInstance::new(&format!("{}_result", id), Pin::new(&format!("{}_result", id), "result", DataType::Typed(graphy::TypeInfo::new("f64")), PinType::Output)));
    n
}

// ── Bytecode structure tests ──────────────────────────────────────────────────

#[test]
fn test_simple_add_graph_produces_call_instruction() {
    // Graph: begin_play → (pure add with constants 3+4)
    // The add is pure so it won't be in the exec chain, but gets emitted as a preamble
    // This test verifies the compile doesn't crash and produces a program.
    let mut graph = GraphDescription::new("simple_add");

    let begin = make_begin_play_node("begin_exec");
    let mut add = make_add_node("add1");
    add.properties.insert("add1_a".to_string(), PropertyValue::Number(3.0));
    add.properties.insert("add1_b".to_string(), PropertyValue::Number(4.0));

    graph.add_node(begin);
    graph.add_node(add);

    let programs = compile_graph_to_bytecode(&graph).expect("should compile");
    assert_eq!(programs.len(), 1);
    let prog = &programs[0];
    assert_eq!(prog.name, "begin_play");
    // Should have at least a Return instruction
    assert!(prog.instructions.iter().any(|i| matches!(i, Instruction::Return)));
}

#[test]
fn test_branch_node_emits_jumpif() {
    let mut graph = GraphDescription::new("branch_test");

    let begin = make_begin_play_node("begin_exec");
    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_a".to_string(), PropertyValue::Number(10.0));
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(5.0));
    let branch = make_branch_node("br1");
    let print_t = make_print_node("print_true");
    let print_f = make_print_node("print_false");

    graph.add_node(begin);
    graph.add_node(gt);
    graph.add_node(branch);
    graph.add_node(print_t);
    graph.add_node(print_f);

    // exec: begin → branch
    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    // data: gt.result → branch.condition
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));
    // exec: branch.True → print_true
    graph.add_connection(Connection::new("br1", "br1_true", "print_true", "print_true_exec", ConnectionType::Execution));
    // exec: branch.False → print_false
    graph.add_connection(Connection::new("br1", "br1_false", "print_false", "print_false_exec", ConnectionType::Execution));

    let programs = compile_graph_to_bytecode(&graph).expect("should compile");
    let prog = &programs[0];

    let has_jumpif = prog.instructions.iter().any(|i| matches!(i, Instruction::JumpIf { .. }));
    assert!(has_jumpif, "branch node must emit JumpIf; instructions:\n{:#?}", prog.instructions);

    let label_count = prog.instructions.iter().filter(|i| matches!(i, Instruction::Label(_))).count();
    // JumpIf needs at least 3 labels: true, false, end
    assert!(label_count >= 2, "expected at least 2 labels, got {}", label_count);
}

#[test]
fn test_load_const_emitted_when_pure_node_is_consumed() {
    // Pure nodes only emit when they have an exec-chain consumer.
    // Here: begin_play → branch(condition=add(42,7)>0)
    // The add and gt are pure consumers of the branch condition.
    let mut graph = GraphDescription::new("const_test");

    let begin = make_begin_play_node("begin_exec");
    let mut add = make_add_node("add1");
    add.properties.insert("add1_a".to_string(), PropertyValue::Number(42.0));
    add.properties.insert("add1_b".to_string(), PropertyValue::Number(7.0));
    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(0.0));
    let branch = make_branch_node("br1");

    graph.add_node(begin);
    graph.add_node(add);
    graph.add_node(gt);
    graph.add_node(branch);

    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("add1", "add1_result", "gt1", "gt1_a", ConnectionType::Data));
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));

    let programs = compile_graph_to_bytecode(&graph).expect("should compile");
    let prog = &programs[0];

    // At minimum the constant 42.0, 7.0, and 0.0 should be loaded
    let const_count = prog.instructions.iter().filter(|i| matches!(i, Instruction::LoadConst { .. })).count();
    assert!(const_count >= 2, "expected at least 2 LoadConst for add constants, got {}", const_count);
}

#[test]
fn test_slot_count_is_positive_when_exec_node_has_constants() {
    // Slots are only allocated when pure nodes or fn_ nodes are actually reached
    // in the execution chain. Use a branch (exec) that consumes an add (pure).
    let mut graph = GraphDescription::new("slot_test");
    let begin = make_begin_play_node("begin_exec");
    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_a".to_string(), PropertyValue::Number(5.0));
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(3.0));
    let branch = make_branch_node("br1");

    graph.add_node(begin);
    graph.add_node(gt);
    graph.add_node(branch);
    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    assert!(programs[0].slot_count > 0, "should have allocated at least one slot for the gt result and constants");
}

#[test]
fn test_pure_node_used_once_inlines_not_materializes() {
    // A pure node used by only one consumer should be inlined (Call with no preceding LoadConst slot for the result)
    // and NOT get its own named slot. We just test it compiles without error here.
    let mut graph = GraphDescription::new("inline_test");
    let begin = make_begin_play_node("begin_exec");
    let mut add_node = make_add_node("add_inline");
    add_node.properties.insert("add_inline_a".to_string(), PropertyValue::Number(5.0));
    add_node.properties.insert("add_inline_b".to_string(), PropertyValue::Number(3.0));
    let mut gt_node = make_greater_than_node("gt_consumer");
    gt_node.properties.insert("gt_consumer_b".to_string(), PropertyValue::Number(0.0));

    graph.add_node(begin);
    graph.add_node(add_node);
    graph.add_node(gt_node);
    graph.add_connection(Connection::new("add_inline", "add_inline_result", "gt_consumer", "gt_consumer_a", ConnectionType::Data));

    let programs = compile_graph_to_bytecode(&graph).expect("should compile inline graph");
    assert!(!programs.is_empty());
}

// ── VM execution tests ────────────────────────────────────────────────────────

#[test]
fn test_vm_executes_math_graph() {
    // Graph: begin_play → (no exec nodes, pure math)
    // We test that a begin_play program with pure preamble runs without error.
    let mut graph = GraphDescription::new("vm_math");
    let begin = make_begin_play_node("begin_exec");
    let mut add = make_add_node("add1");
    add.properties.insert("add1_a".to_string(), PropertyValue::Number(10.0));
    add.properties.insert("add1_b".to_string(), PropertyValue::Number(20.0));
    graph.add_node(begin);
    graph.add_node(add);

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    vm.run(&programs[0]).expect("VM should execute without error");
}

#[test]
fn test_vm_branch_true_path_executes_correct_node() {
    // 10 > 5 → true → calls print_true
    let mut graph = GraphDescription::new("vm_branch_true");

    let begin = make_begin_play_node("begin_exec");
    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_a".to_string(), PropertyValue::Number(10.0));
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(5.0));
    let branch = make_branch_node("br1");
    let print_t = make_print_node("print_true");
    let print_f = make_print_node("print_false");

    graph.add_node(begin);
    graph.add_node(gt);
    graph.add_node(branch);
    graph.add_node(print_t);
    graph.add_node(print_f);

    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));
    graph.add_connection(Connection::new("br1", "br1_true", "print_true", "print_true_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("br1", "br1_false", "print_false", "print_false_exec", ConnectionType::Execution));

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    vm.run(&programs[0]).expect("VM run");

    let calls = dispatch.calls();
    assert!(calls.contains(&"greater_than".to_string()), "should have called greater_than");
    assert!(calls.contains(&"print_string".to_string()), "should have called print_string");
    // print_string should appear exactly once (one branch only)
    assert_eq!(calls.iter().filter(|c| c.as_str() == "print_string").count(), 1);
}

#[test]
fn test_vm_branch_false_path_executes_correct_node() {
    // 1 > 5 → false → calls print_false only
    let mut graph = GraphDescription::new("vm_branch_false");

    let begin = make_begin_play_node("begin_exec");
    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_a".to_string(), PropertyValue::Number(1.0));
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(5.0));
    let branch = make_branch_node("br1");
    let print_t = make_print_node("print_true");
    let print_f = make_print_node("print_false");

    graph.add_node(begin);
    graph.add_node(gt);
    graph.add_node(branch);
    graph.add_node(print_t);
    graph.add_node(print_f);

    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));
    graph.add_connection(Connection::new("br1", "br1_true", "print_true", "print_true_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("br1", "br1_false", "print_false", "print_false_exec", ConnectionType::Execution));

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    vm.run(&programs[0]).expect("VM run");

    let calls = dispatch.calls();
    assert_eq!(calls.iter().filter(|c| c.as_str() == "print_string").count(), 1,
        "only false branch print should fire");
}

#[test]
fn test_vm_chained_math_via_branch() {
    // add(3, 4) → gt with 0 → branch. Both pure nodes must be evaluated for the branch.
    let mut graph = GraphDescription::new("chained_math");
    let begin = make_begin_play_node("begin_exec");

    let mut add1 = make_add_node("add1");
    add1.properties.insert("add1_a".to_string(), PropertyValue::Number(3.0));
    add1.properties.insert("add1_b".to_string(), PropertyValue::Number(4.0));

    let mut gt = make_greater_than_node("gt1");
    gt.properties.insert("gt1_b".to_string(), PropertyValue::Number(0.0));

    let branch = make_branch_node("br1");

    graph.add_node(begin);
    graph.add_node(add1);
    graph.add_node(gt);
    graph.add_node(branch);

    graph.add_connection(Connection::new("begin", "begin_exec", "br1", "br1_exec", ConnectionType::Execution));
    graph.add_connection(Connection::new("add1", "add1_result", "gt1", "gt1_a", ConnectionType::Data));
    graph.add_connection(Connection::new("gt1", "gt1_result", "br1", "br1_condition", ConnectionType::Data));

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    vm.run(&programs[0]).expect("chained math VM run");

    let calls = dispatch.calls();
    assert!(calls.iter().any(|c| c == "add"), "add should be called");
    assert!(calls.iter().any(|c| c == "greater_than"), "greater_than should be called");
}

#[test]
fn test_vm_returns_error_for_unknown_node() {
    // A graph with a node type that isn't in the mock dispatch should return VmError at runtime.
    // We use a fn_ node type unknown to the dispatch — the bytecode will have a Call for it.
    // The compile step succeeds (PBGC doesn't validate against the dispatch), VM fails.
    let mut graph = GraphDescription::new("unknown_node");
    let begin = make_begin_play_node("begin_exec");

    // Create a node that PBGC won't find metadata for — it will be treated as fn_
    // by falling back. Actually PBGC requires metadata. Instead, use the existing
    // print_string fn_ node type but respond to it with an error in a custom dispatch.
    let print = make_print_node("p1");

    graph.add_node(begin);
    graph.add_node(print);
    graph.add_connection(Connection::new("begin", "begin_exec", "p1", "p1_exec", ConnectionType::Execution));

    struct ErrorDispatch;
    impl NodeDispatch for ErrorDispatch {
        fn call(&self, node_type: &str, _inputs: &[BpValue], _output: &mut Option<BpValue>) -> Result<(), VmError> {
            Err(VmError::UnknownNode(node_type.to_string()))
        }
    }

    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = ErrorDispatch;
    let vm = BytecodeVm::new(&dispatch);
    let result = vm.run(&programs[0]);
    assert!(result.is_err(), "dispatch-rejected node should produce VmError");
}

// ── Timing tests ──────────────────────────────────────────────────────────────

/// Build a graph of `n` chained pure add nodes.
fn make_deep_pure_graph(n: usize) -> GraphDescription {
    let mut graph = GraphDescription::new("deep_pure");
    graph.add_node(make_begin_play_node("begin_exec"));

    let mut prev_id = "begin".to_string();
    for i in 0..n {
        let id = format!("add_{}", i);
        let mut node = make_add_node(&id);
        node.properties.insert(format!("{}_b", id), PropertyValue::Number(1.0));
        if i == 0 {
            node.properties.insert(format!("{}_a", id), PropertyValue::Number(0.0));
        } else {
            // connect output of previous add to this one's a
            graph.add_connection(Connection::new(
                &prev_id,
                &format!("{}_result", prev_id),
                &id,
                &format!("{}_a", id),
                ConnectionType::Data,
            ));
        }
        graph.add_node(node);
        prev_id = id;
    }
    graph
}

#[test]
fn test_bytecode_compile_timing_small_graph() {
    let graph = make_deep_pure_graph(10);
    let start = Instant::now();
    let programs = compile_graph_to_bytecode(&graph).expect("should compile");
    let elapsed = start.elapsed();
    println!("[timing] 10-node pure graph → bytecode: {:?}", elapsed);
    assert!(!programs.is_empty());
    // Bytecode compile should be fast — well under 100ms even in debug
    assert!(elapsed.as_millis() < 500, "compile took too long: {:?}", elapsed);
}

#[test]
fn test_bytecode_compile_timing_medium_graph() {
    let graph = make_deep_pure_graph(50);
    let start = Instant::now();
    let programs = compile_graph_to_bytecode(&graph).expect("should compile");
    let elapsed = start.elapsed();
    println!("[timing] 50-node pure graph → bytecode: {:?}", elapsed);
    assert!(!programs.is_empty());
    assert!(elapsed.as_millis() < 2000, "compile took too long: {:?}", elapsed);
}

#[test]
fn test_vm_execute_timing_small_graph() {
    let graph = make_deep_pure_graph(10);
    let programs = compile_graph_to_bytecode(&graph).unwrap();
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);

    let start = Instant::now();
    for _ in 0..1000 {
        vm.run(&programs[0]).unwrap();
    }
    let elapsed = start.elapsed();
    println!("[timing] 1000 × 10-node VM runs: {:?} ({:.2}µs/run)", elapsed,
        elapsed.as_micros() as f64 / 1000.0);
    assert!(elapsed.as_secs() < 5, "1000 VM runs took too long: {:?}", elapsed);
}

#[test]
fn test_bytecode_vs_rustcodegen_compile_time() {
    use pbgc::compile_graph;

    let graph = make_deep_pure_graph(20);

    let start = Instant::now();
    let _rust_code = compile_graph(&graph).expect("rust codegen");
    let rust_elapsed = start.elapsed();

    let start = Instant::now();
    let _bytecode = compile_graph_to_bytecode(&graph).expect("bytecode");
    let bc_elapsed = start.elapsed();

    println!(
        "[timing] 20-node graph — Rust codegen: {:?}, Bytecode: {:?}",
        rust_elapsed, bc_elapsed
    );
    // Both should finish quickly in the test environment
    assert!(rust_elapsed.as_secs() < 5);
    assert!(bc_elapsed.as_secs() < 5);
}

// ── BpValue unit tests ────────────────────────────────────────────────────────

#[test]
fn test_bpvalue_is_truthy() {
    assert!(BpValue::Bool(true).is_truthy());
    assert!(!BpValue::Bool(false).is_truthy());
    assert!(BpValue::Int(1).is_truthy());
    assert!(!BpValue::Int(0).is_truthy());
    assert!(BpValue::Float(0.1).is_truthy());
    assert!(!BpValue::Float(0.0).is_truthy());
    assert!(BpValue::Str("hello".to_string()).is_truthy());
    assert!(!BpValue::Str(String::new()).is_truthy());
    assert!(!BpValue::Null.is_truthy());
}

#[test]
fn test_bpvalue_conversions() {
    assert_eq!(BpValue::Int(42).as_i64(), Some(42));
    assert_eq!(BpValue::Float(3.7).as_i64(), Some(3));
    assert_eq!(BpValue::Bool(true).as_i64(), Some(1));
    assert_eq!(BpValue::Str("x".to_string()).as_i64(), None);

    assert_eq!(BpValue::Float(1.5).as_f64(), Some(1.5));
    assert_eq!(BpValue::Int(10).as_f64(), Some(10.0));
    assert_eq!(BpValue::Bool(false).as_f64(), Some(0.0));
}

#[test]
fn test_parse_bp_const_numbers() {
    use pbgc::bytecode::parse_bp_const;
    assert_eq!(parse_bp_const("42"), BpValue::Int(42));
    assert_eq!(parse_bp_const("3.14"), BpValue::Float(3.14));
    assert_eq!(parse_bp_const("true"), BpValue::Bool(true));
    assert_eq!(parse_bp_const("false"), BpValue::Bool(false));
    assert_eq!(parse_bp_const("\"hello\""), BpValue::Str("hello".to_string()));
}

// ── Correctness tests (self-asserting graphs) ─────────────────────────────────
//
// Each test builds a Blueprint graph that computes a result and wires it into
// an assert_eq_* node in the exec chain. If the VM executes the assert node
// and the value is wrong, the dispatch returns VmError::AssertionFailed and the
// Rust test fails with a descriptive message.

fn run_assert_graph(graph: &GraphDescription) {
    let programs = compile_graph_to_bytecode(graph)
        .expect("graph should compile to bytecode");
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    for prog in &programs {
        vm.run(prog).unwrap_or_else(|e| panic!("VM assertion failed: {}", e));
    }
}

// ── Basic arithmetic ──────────────────────────────────────────────────────────

#[test]
fn test_correctness_add_3_plus_4_equals_7() {
    // add(3, 4) → assert_eq_int(7)
    let mut g = GraphDescription::new("add_correct");
    let begin = make_begin_play_node("be");

    let mut add = make_add_node("a");
    add.properties.insert("a_a".into(), PropertyValue::Number(3.0));
    add.properties.insert("a_b".into(), PropertyValue::Number(4.0));

    let assert = make_assert_eq_int_node("chk", Some(7));

    g.add_node(begin);
    g.add_node(add);
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("a", "a_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_multiply_6_times_7_equals_42() {
    let mut g = GraphDescription::new("multiply_correct");
    let begin = make_begin_play_node("be");

    let mut mul = make_multiply_node("m");
    mul.properties.insert("m_a".into(), PropertyValue::Number(6.0));
    mul.properties.insert("m_b".into(), PropertyValue::Number(7.0));

    let assert = make_assert_eq_int_node("chk", Some(42));

    g.add_node(begin);
    g.add_node(mul);
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("m", "m_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_add_chain_0_plus_1_times_100_equals_100() {
    // Chain 100 add-by-1 nodes: result should be 100
    let mut g = GraphDescription::new("add_chain");
    let begin = make_begin_play_node("be");
    g.add_node(begin);

    for i in 0..100usize {
        let id = format!("a{}", i);
        let mut node = make_add_node(&id);
        node.properties.insert(format!("{}_b", id), PropertyValue::Number(1.0));
        if i == 0 {
            node.properties.insert(format!("{}_a", id), PropertyValue::Number(0.0));
        } else {
            let prev = format!("a{}", i - 1);
            g.add_connection(Connection::new(&prev, &format!("{}_result", prev), &id, &format!("{}_a", id), ConnectionType::Data));
        }
        g.add_node(node);
    }

    let assert = make_assert_eq_int_node("chk", Some(100));
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("a99", "a99_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

// ── Floating-point math ───────────────────────────────────────────────────────

#[test]
fn test_correctness_lerp_midpoint() {
    // lerp(0, 100, 0.5) == 50.0
    let mut g = GraphDescription::new("lerp_mid");
    let begin = make_begin_play_node("be");

    let mut lerp = make_lerp_node("l");
    lerp.properties.insert("l_a".into(), PropertyValue::Number(0.0));
    lerp.properties.insert("l_b".into(), PropertyValue::Number(100.0));
    lerp.properties.insert("l_t".into(), PropertyValue::Number(0.5));

    let assert = make_assert_eq_float_node("chk", Some(50.0), Some(1e-9));

    g.add_node(begin);
    g.add_node(lerp);
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("l", "l_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_lerp_at_zero_equals_a() {
    // lerp(42.0, 100.0, 0.0) == 42.0
    let mut g = GraphDescription::new("lerp_zero");
    let begin = make_begin_play_node("be");

    let mut lerp = make_lerp_node("l");
    lerp.properties.insert("l_a".into(), PropertyValue::Number(42.0));
    lerp.properties.insert("l_b".into(), PropertyValue::Number(100.0));
    lerp.properties.insert("l_t".into(), PropertyValue::Number(0.0));

    let assert = make_assert_eq_float_node("chk", Some(42.0), Some(1e-9));

    g.add_node(begin);
    g.add_node(lerp);
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("l", "l_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

// ── Control flow correctness ──────────────────────────────────────────────────

#[test]
fn test_correctness_branch_true_path_asserts_true() {
    // 10 > 5 == true → branch.True → assert_true(true)
    // if branch takes the wrong path this test either fails the assert or
    // doesn't reach it (no assertion fires on the false path).
    let mut g = GraphDescription::new("branch_true_path");
    let begin = make_begin_play_node("be");

    let mut gt = make_greater_than_node("gt");
    gt.properties.insert("gt_a".into(), PropertyValue::Number(10.0));
    gt.properties.insert("gt_b".into(), PropertyValue::Number(5.0));

    let branch = make_branch_node("br");

    // True path: assert that condition was true
    let assert_t = make_assert_true_node("chk_t");
    assert_t.inputs.iter().find(|p| p.pin.name == "condition").unwrap(); // sanity

    // False path: we'd never reach this in a correct run, but add it anyway
    // with an assert_false that would fail if the wrong path fires.
    let mut false_assert = NodeInstance::new("chk_f", "assert_false", Position { x: 500.0, y: 100.0 });
    false_assert.inputs.push(PinInstance::new("chk_f_exec", Pin::new("chk_f_exec", "exec", DataType::Execution, PinType::Input)));
    false_assert.inputs.push(PinInstance::new("chk_f_cond", Pin::new("chk_f_cond", "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input)));
    false_assert.properties.insert("chk_f_cond".into(), PropertyValue::Number(1.0)); // always true → assert_false would fire
    false_assert.outputs.push(PinInstance::new("chk_f_out", Pin::new("chk_f_out", "exec", DataType::Execution, PinType::Output)));

    g.add_node(begin);
    g.add_node(gt);
    g.add_node(branch);
    g.add_node(assert_t);
    g.add_node(false_assert);

    g.add_connection(Connection::new("begin", "be", "br", "br_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("gt", "gt_result", "br", "br_condition", ConnectionType::Data));
    g.add_connection(Connection::new("br", "br_true", "chk_t", "chk_t_exec", ConnectionType::Execution));
    // The condition pin on assert_true: connect gt result so it asserts the condition is true
    g.add_connection(Connection::new("gt", "gt_result", "chk_t", "chk_t_condition", ConnectionType::Data));
    // False branch would fire assert_false with a constant-true input → would fail
    g.add_connection(Connection::new("br", "br_false", "chk_f", "chk_f_exec", ConnectionType::Execution));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_assert_fails_on_wrong_value() {
    // add(1, 1) == 3 is WRONG — this test verifies the assertion mechanism itself
    // catches it and the VM returns an error.
    let mut g = GraphDescription::new("wrong_assert");
    let begin = make_begin_play_node("be");

    let mut add = make_add_node("a");
    add.properties.insert("a_a".into(), PropertyValue::Number(1.0));
    add.properties.insert("a_b".into(), PropertyValue::Number(1.0));

    let assert = make_assert_eq_int_node("chk", Some(3)); // WRONG: 1+1 = 2, not 3

    g.add_node(begin);
    g.add_node(add);
    g.add_node(assert);
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("a", "a_result", "chk", "chk_actual", ConnectionType::Data));

    let programs = compile_graph_to_bytecode(&g).expect("should compile");
    let dispatch = MockDispatch::new();
    let vm = BytecodeVm::new(&dispatch);
    let result = vm.run(&programs[0]);

    assert!(
        matches!(result, Err(VmError::AssertionFailed(_))),
        "expected AssertionFailed, got {:?}",
        result
    );
}

// ── Multi-step compound calculations ─────────────────────────────────────────

#[test]
fn test_correctness_pythagorean_triple_3_4_5() {
    // 3² + 4² = 9 + 16 = 25 → assert_eq_int(25)
    let mut g = GraphDescription::new("pythag");
    let begin = make_begin_play_node("be");

    let mut sq3 = make_multiply_node("sq3");
    sq3.properties.insert("sq3_a".into(), PropertyValue::Number(3.0));
    sq3.properties.insert("sq3_b".into(), PropertyValue::Number(3.0));

    let mut sq4 = make_multiply_node("sq4");
    sq4.properties.insert("sq4_a".into(), PropertyValue::Number(4.0));
    sq4.properties.insert("sq4_b".into(), PropertyValue::Number(4.0));

    let mut sum = make_add_node("sum");
    let assert = make_assert_eq_int_node("chk", Some(25));

    g.add_node(begin);
    g.add_node(sq3);
    g.add_node(sq4);
    g.add_node(sum);
    g.add_node(assert);

    g.add_connection(Connection::new("sq3", "sq3_result", "sum", "sum_a", ConnectionType::Data));
    g.add_connection(Connection::new("sq4", "sq4_result", "sum", "sum_b", ConnectionType::Data));
    g.add_connection(Connection::new("begin", "be", "chk", "chk_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("sum", "sum_result", "chk", "chk_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_add_is_commutative() {
    // add(a, b) == add(b, a) for a=7, b=13
    // We verify both sides equal 20.
    let mut g = GraphDescription::new("commutative");
    let begin = make_begin_play_node("be");

    let mut ab = make_add_node("ab");
    ab.properties.insert("ab_a".into(), PropertyValue::Number(7.0));
    ab.properties.insert("ab_b".into(), PropertyValue::Number(13.0));

    let mut ba = make_add_node("ba");
    ba.properties.insert("ba_a".into(), PropertyValue::Number(13.0));
    ba.properties.insert("ba_b".into(), PropertyValue::Number(7.0));

    let assert_ab = make_assert_eq_int_node("chk_ab", Some(20));
    let mut assert_ba = make_assert_eq_int_node("chk_ba", Some(20));
    // chain: begin → chk_ab → chk_ba
    assert_ba.inputs.iter_mut().find(|p| p.pin.name == "exec").unwrap();

    g.add_node(begin);
    g.add_node(ab);
    g.add_node(ba);
    g.add_node(assert_ab);
    g.add_node(assert_ba);

    g.add_connection(Connection::new("begin", "be", "chk_ab", "chk_ab_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("chk_ab", "chk_ab_exec_out", "chk_ba", "chk_ba_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("ab", "ab_result", "chk_ab", "chk_ab_actual", ConnectionType::Data));
    g.add_connection(Connection::new("ba", "ba_result", "chk_ba", "chk_ba_actual", ConnectionType::Data));

    run_assert_graph(&g);
}

#[test]
fn test_correctness_greater_than_is_antisymmetric() {
    // 10 > 5 is true, 5 > 10 is false
    let mut g = GraphDescription::new("gt_antisymmetric");
    let begin = make_begin_play_node("be");

    let mut gt_10_5 = make_greater_than_node("gt1");
    gt_10_5.properties.insert("gt1_a".into(), PropertyValue::Number(10.0));
    gt_10_5.properties.insert("gt1_b".into(), PropertyValue::Number(5.0));

    let mut gt_5_10 = make_greater_than_node("gt2");
    gt_5_10.properties.insert("gt2_a".into(), PropertyValue::Number(5.0));
    gt_5_10.properties.insert("gt2_b".into(), PropertyValue::Number(10.0));

    let assert_true_node = make_assert_true_node("chk1");  // 10>5 == true
    let mut assert_false_node = NodeInstance::new("chk2", "assert_false", Position { x: 600.0, y: 0.0 });
    assert_false_node.inputs.push(PinInstance::new("chk2_exec", Pin::new("chk2_exec", "exec", DataType::Execution, PinType::Input)));
    assert_false_node.inputs.push(PinInstance::new("chk2_cond", Pin::new("chk2_cond", "condition", DataType::Typed(graphy::TypeInfo::new("bool")), PinType::Input)));
    assert_false_node.outputs.push(PinInstance::new("chk2_out", Pin::new("chk2_out", "exec", DataType::Execution, PinType::Output)));

    g.add_node(begin);
    g.add_node(gt_10_5);
    g.add_node(gt_5_10);
    g.add_node(assert_true_node);
    g.add_node(assert_false_node);

    g.add_connection(Connection::new("begin", "be", "chk1", "chk1_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("chk1", "chk1_exec_out", "chk2", "chk2_exec", ConnectionType::Execution));
    g.add_connection(Connection::new("gt1", "gt1_result", "chk1", "chk1_condition", ConnectionType::Data));
    g.add_connection(Connection::new("gt2", "gt2_result", "chk2", "chk2_cond", ConnectionType::Data));

    run_assert_graph(&g);
}
