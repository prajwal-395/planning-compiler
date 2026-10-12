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
        tasks_axi: None,
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
fn cap_range(id: &str, low: f64, likely: f64, high: f64) -> Capability {
    Capability {
        id: id.into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "duration_minutes".into(),
            low,
            likely,
            high,
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
            tasks_axi_sections: Vec::new(),
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
fn makespan_reports_low_likely_high_range() {
    // Two sequential tasks with wide triples: the makespan range must span
    // the CPM over each bound, not collapse to the single expected value.
    // Fails on the old single-value output (no `makespan_range` to assert).
    let plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["a", "b"], &[("a", "b")]),
            invoke("ia", "a", "ca"),
            invoke("ib", "b", "cb"),
        ],
        vec![
            cap_range("ca", 1.0, 2.0, 9.0),
            cap_range("cb", 3.0, 4.0, 5.0),
        ],
    );
    let compiled = compile(&plan).unwrap();
    let range = compiled.makespan_range;
    assert_eq!(range.low, 4.0, "critical-path low span");
    assert_eq!(range.likely, 6.0, "critical-path likely span");
    assert_eq!(range.high, 14.0, "critical-path high span");
    assert_eq!(
        compiled.makespan_minutes, range.expected,
        "legacy single value stays the (optimistic) expected span"
    );
    assert_eq!(
        compiled.totals["duration_minutes"], range,
        "makespan range matches the duration total, which already carried a range"
    );
    assert!(
        compiled.trace.iter().any(|row| row.subject == "plan"
            && row.decision == "analyzed"
            && row.reason.contains("low 4.000")
            && row.reason.contains("likely 6.000")
            && row.reason.contains("high 14.000")
            && row.reason.contains("(optimistic)")),
        "trace names the range and the optimistic PERT bias"
    );
}

#[test]
fn p6_selects_cheapest_feasible_alternative_and_reports_optimality() {
    let mut plan = doc(
        vec![task("root", TaskState::Resolved)],
        vec![
            invoke("slow", "root", "slow-cap"),
            invoke("fast", "root", "fast-cap"),
        ],
        vec![cap("slow-cap", 20.0), cap("fast-cap", 5.0)],
    );
    for capability in &mut plan.capability {
        capability.estimates.push(Estimate {
            metric: "money_usd".into(),
            low: if capability.id == "fast-cap" {
                3.0
            } else {
                8.0
            },
            likely: if capability.id == "fast-cap" {
                3.0
            } else {
                8.0
            },
            high: if capability.id == "fast-cap" {
                3.0
            } else {
                8.0
            },
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        });
    }
    plan.objective.weights.insert("money_usd".into(), 1.0);
    plan.objective.budget_usd = Some(5.0);
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.selection_status, "optimal");
    assert_eq!(compiled.selection_time_budget_ms, 2000);
    assert!(
        compiled
            .trace
            .iter()
            .any(|row| row.subject == "task:root" && row.reason == "uses method:fast")
    );
    assert!(
        compiled
            .trace
            .iter()
            .any(|row| row.subject == "method:slow" && row.decision == "not selected")
    );
    assert_eq!(compiled.makespan_minutes, 5.0);
}

fn money(capability: &mut Capability, usd: f64) {
    capability.estimates.push(Estimate {
        metric: "money_usd".into(),
        low: usd,
        likely: usd,
        high: usd,
        provenance: EstimateProvenance::Declared,
        observations: None,
        window: None,
    });
}

#[test]
fn p6_ignores_done_subtask_costs_and_missing_estimates_under_budget() {
    let mut a_cap = cap("a-cap", 1.0);
    money(&mut a_cap, 100.0);
    let mut b_cap = cap("b-cap", 2.0);
    money(&mut b_cap, 10.0);
    let mut plan = doc(
        vec![
            task("root", TaskState::Resolved),
            task("a", TaskState::Done),
            task("b", TaskState::Resolved),
            task("c", TaskState::Dropped),
        ],
        vec![
            decompose("root-method", "root", &["a", "b", "c"], &[]),
            invoke("a-method", "a", "a-cap"),
            invoke("b-method", "b", "b-cap"),
            invoke("c-method", "c", "c-cap"),
        ],
        vec![a_cap, b_cap, cap("c-cap", 3.0)],
    );
    plan.objective.budget_usd = Some(50.0);
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.totals["money_usd"].expected, 10.0);
    assert!(
        compiled
            .trace
            .iter()
            .all(|row| row.decision != "not selected")
    );
}

#[test]
fn p6_reports_ineligible_alternatives_and_omits_unneeded_tasks() {
    let mut priced = cap("priced-cap", 5.0);
    money(&mut priced, 3.0);
    let mut pricier = cap("pricier-cap", 5.0);
    money(&mut pricier, 4.0);
    let mut plan = doc(
        vec![
            task("root", TaskState::Resolved),
            task("unused", TaskState::Resolved),
        ],
        vec![
            invoke("priced", "root", "priced-cap"),
            invoke("unpriced", "root", "unpriced-cap"),
            decompose("detour", "root", &["unused"], &[]),
            invoke("unused-a", "unused", "unpriced-cap"),
            invoke("unused-b", "unused", "pricier-cap"),
        ],
        vec![priced, pricier, cap("unpriced-cap", 1.0)],
    );
    plan.objective.weights.insert("money_usd".into(), 1.0);
    plan.objective.budget_usd = Some(5.0);
    let compiled = compile(&plan).unwrap();
    let not_selected: Vec<_> = compiled
        .trace
        .iter()
        .filter(|row| row.decision == "not selected")
        .map(|row| (row.subject.as_str(), row.reason.as_str()))
        .collect();
    assert_eq!(
        not_selected,
        vec![
            (
                "method:unpriced",
                "ineligible: missing money_usd estimate under budget_usd"
            ),
            ("method:detour", "P6 selected method:priced for task:root"),
        ]
    );
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

#[test]
fn compile_surfaces_unattributed_task_evidence() {
    let mut plan = doc(
        vec![task("root", TaskState::Resolved)],
        vec![invoke("via-a", "root", "a"), invoke("via-b", "root", "b")],
        vec![cap("a", 1.0), cap("b", 2.0)],
    );
    for (subject, time) in [
        ("task:root", "2026-10-01T00:00:00Z"),
        ("task:root", "2026-10-02T00:00:00Z"),
    ] {
        plan.evidence.push(pc_ir::Evidence {
            subject: subject.into(),
            metric: "duration_minutes".into(),
            value: 30.0,
            unit: "minutes".into(),
            time: time.into(),
            source: "test".into(),
            basis: None,
            artifact_hash: None,
        });
    }
    let compiled = compile(&plan).unwrap();
    assert_eq!(
        compiled.unattributed_tasks,
        [pc_core::UnattributedTaskEvidence {
            task: "root".into(),
            metric: "duration_minutes".into(),
            observations: 2,
            capabilities: vec!["a".into(), "b".into()],
            methods: vec!["via-a".into(), "via-b".into()],
        }]
    );
    assert!(compiled.provenance.iter().all(|row| row.measured.is_none()));
    assert_eq!(compiled.totals["duration_minutes"].expected, 1.0);
}
