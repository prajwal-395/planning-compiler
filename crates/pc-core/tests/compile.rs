//! Acceptance tests for three-point roll-ups and P7 critical-path analysis.

use pc_core::compile;
use pc_ir::{
    Capability, CapabilityKind, Estimate, EstimateProvenance, Method, MethodKind, MethodStatus,
    Objective, OrderEdge, PlanDoc, PlanMeta, Task, TaskState,
};

fn task(id: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        state,
        consumes: vec![],
        produces: vec![],
    }
}
fn invoke(id: &str, task: &str, capability: &str) -> Method {
    Method {
        id: id.into(),
        task: task.into(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some(capability.into()),
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    }
}
fn decompose(id: &str, task: &str, children: &[&str], edges: &[(&str, &str)]) -> Method {
    Method {
        id: id.into(),
        task: task.into(),
        kind: MethodKind::Decompose,
        subtasks: children.iter().map(|s| (*s).into()).collect(),
        order_edges: edges
            .iter()
            .map(|(from, to)| OrderEdge {
                from: (*from).into(),
                to: (*to).into(),
            })
            .collect(),
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    }
}
fn cap(id: &str, minutes: f64) -> Capability {
    Capability {
        id: id.into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "duration_minutes".into(),
            low: minutes,
            likely: minutes,
            high: minutes,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        resources: vec![],
    }
}
fn doc(tasks: Vec<Task>, methods: Vec<Method>, caps: Vec<Capability>) -> PlanDoc {
    PlanDoc {
        ir_version: 0,
        plan: PlanMeta {
            id: "p".into(),
            title: "test".into(),
            description: String::new(),
            external_inputs: vec![],
        },
        objective: Objective {
            root_task: "root".into(),
            weights: Default::default(),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            acceptance: vec![],
        },
        artifact: vec![],
        resource: vec![],
        task: tasks,
        method: methods,
        capability: caps,
        evidence: vec![],
    }
}

#[test]
fn textbook_cpm_forward_backward_and_parallel_width() {
    // A(3) -> B(2) -> D(4), while A -> C(5) -> D. Critical path is 12.
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Resolved),
            task("c", TaskState::Resolved),
            task("d", TaskState::Resolved),
        ],
        vec![
            decompose(
                "root-method",
                "root",
                &["a", "b", "c", "d"],
                &[("a", "b"), ("a", "c"), ("b", "d"), ("c", "d")],
            ),
            invoke("ia", "a", "ca"),
            invoke("ib", "b", "cb"),
            invoke("ic", "c", "cc"),
            invoke("id", "d", "cd"),
        ],
        vec![
            cap("ca", 3.0),
            cap("cb", 2.0),
            cap("cc", 5.0),
            cap("cd", 4.0),
        ],
    );
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 12.0);
    let path_lengths = [3.0 + 2.0 + 4.0, 3.0 + 5.0 + 4.0];
    assert!(
        path_lengths
            .iter()
            .all(|path| compiled.makespan_minutes >= *path)
    );
    assert_eq!(compiled.parallel_width, 2);
    assert_eq!(compiled.timings["b"].slack, 3.0);
    assert!(compiled.timings["c"].critical);
    assert_eq!(compiled.totals["duration_minutes"].expected, 12.0);
}

#[test]
fn trace_explains_assumptions_and_per_task_slack() {
    let mut root = decompose("root-method", "root", &["a", "b"], &[]);
    root.assumptions = vec!["a and b run in parallel".into()];
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Resolved),
        ],
        vec![root, invoke("ia", "a", "ca"), invoke("ib", "b", "cb")],
        vec![cap("ca", 3.0), cap("cb", 5.0)],
    );
    let compiled = compile(&plan).unwrap();
    let rows = |subject: &str| -> Vec<(String, String)> {
        compiled
            .trace
            .iter()
            .filter(|row| row.subject == subject)
            .map(|row| (row.decision.clone(), row.reason.clone()))
            .collect()
    };
    assert!(
        rows("method:root-method").contains(&("assumes".into(), "a and b run in parallel".into()))
    );
    assert!(rows("task:a").contains(&(
        "has slack".into(),
        "slack 2.000 minutes; earliest 0.000-3.000, latest 2.000-5.000".into()
    )));
    assert!(rows("task:b").contains(&(
        "critical".into(),
        "slack 0.000 minutes; earliest 0.000-5.000, latest 0.000-5.000".into()
    )));
}

#[test]
fn order_edges_relay_through_leafless_siblings() {
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Open),
            task("c", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["a", "b", "c"], &[("a", "b"), ("b", "c")]),
            invoke("ia", "a", "ca"),
            invoke("ic", "c", "cc"),
        ],
        vec![cap("ca", 10.0), cap("cc", 10.0)],
    );
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 20.0);
    assert_eq!(compiled.parallel_width, 1);
    assert_eq!(compiled.timings["c"].earliest_start, 10.0);
    assert_eq!(compiled.unresolved_tasks, vec!["b".to_string()]);
}

#[test]
fn shared_and_repeated_subtasks_compile_once() {
    let mut money = cap("cs", 10.0);
    money.estimates.push(Estimate {
        metric: "money_usd".into(),
        low: 5.0,
        likely: 5.0,
        high: 5.0,
        provenance: EstimateProvenance::Declared,
        observations: None,
        window: None,
    });
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("x", TaskState::Decomposed),
            task("y", TaskState::Decomposed),
            task("s", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["x", "y"], &[]),
            decompose("dx", "x", &["s"], &[]),
            decompose("dy", "y", &["s", "s"], &[]),
            invoke("is", "s", "cs"),
        ],
        vec![money],
    );
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.totals["money_usd"].expected, 5.0);
    assert_eq!(compiled.makespan_minutes, 10.0);
    assert_eq!(compiled.parallel_width, 1);
    assert_eq!(compiled.timings.len(), 1);
    let explained = compiled
        .trace
        .iter()
        .filter(|row| row.subject == "task:s")
        .count();
    assert_eq!(explained, 2, "one accepted row and one timing row");
}

#[test]
fn ordered_siblings_sharing_a_subtask_keep_valid_timings() {
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("p", TaskState::Decomposed),
            task("q", TaskState::Decomposed),
            task("x", TaskState::Resolved),
            task("s", TaskState::Resolved),
            task("y", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["p", "q"], &[("p", "q")]),
            decompose("dp", "p", &["x", "s"], &[]),
            decompose("dq", "q", &["s", "y"], &[]),
            invoke("ix", "x", "c"),
            invoke("is", "s", "c"),
            invoke("iy", "y", "c"),
        ],
        vec![cap("c", 4.0)],
    );
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 8.0);
    assert_eq!(compiled.parallel_width, 2);
    for task in ["x", "s", "y"] {
        let timing = &compiled.timings[task];
        assert!(timing.latest_start >= timing.earliest_start, "{task}");
        assert!(timing.critical, "{task}");
    }
    assert_eq!(compiled.timings["x"].earliest_start, 0.0);
    assert_eq!(compiled.timings["s"].earliest_start, 0.0);
    assert_eq!(compiled.timings["y"].earliest_start, 4.0);
}

#[test]
fn contradictory_orders_across_methods_fail_loudly() {
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("p", TaskState::Decomposed),
            task("q", TaskState::Decomposed),
            task("s", TaskState::Resolved),
            task("t", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["p", "q"], &[("p", "q")]),
            decompose("dp", "p", &["s"], &[]),
            decompose("dq", "q", &["s", "t"], &[("t", "s")]),
            invoke("is", "s", "c"),
            invoke("it", "t", "c"),
        ],
        vec![cap("c", 4.0)],
    );
    assert_eq!(
        compile(&plan),
        Err(pc_core::CompileError::PrecedenceCycle {
            tasks: vec!["s".into(), "t".into()]
        })
    );
}

#[test]
fn adding_a_task_never_shortens_makespan() {
    let base = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
        ],
        vec![decompose("r", "root", &["a"], &[]), invoke("ia", "a", "ca")],
        vec![cap("ca", 4.0)],
    );
    let mut extended = base.clone();
    extended.task.push(task("b", TaskState::Resolved));
    extended.method[0].subtasks.push("b".into());
    extended.method.push(invoke("ib", "b", "cb"));
    extended.capability.push(cap("cb", 7.0));
    assert!(
        compile(&extended).unwrap().makespan_minutes >= compile(&base).unwrap().makespan_minutes
    );
}

#[test]
fn compilation_is_independent_of_input_order_and_ignores_proposals() {
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["a", "b"], &[]),
            invoke("ia", "a", "ca"),
            invoke("ib", "b", "cb"),
        ],
        vec![cap("ca", 3.0), cap("cb", 5.0)],
    );
    plan.method.push(Method {
        status: MethodStatus::Proposed,
        ..decompose("proposal", "root", &["missing"], &[])
    });
    let first = serde_json::to_vec(&compile(&plan).unwrap()).unwrap();
    assert_eq!(first, serde_json::to_vec(&compile(&plan).unwrap()).unwrap());
    let mut shuffled = plan.clone();
    shuffled.task.reverse();
    shuffled.method.reverse();
    shuffled.capability.reverse();
    let second = serde_json::to_vec(&compile(&shuffled).unwrap()).unwrap();
    assert_eq!(first, second);
}
