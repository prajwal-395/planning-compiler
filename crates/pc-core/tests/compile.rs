//! Acceptance tests for three-point roll-ups and P7 critical-path analysis.

use pc_core::compile;
use pc_ir::{
    Capability, CapabilityKind, Estimate, EstimateProvenance, Method, MethodKind, MethodStatus,
    Objective, OrderEdge, PlanDoc, PlanMeta, ResourceDecl, Task, TaskState,
};

fn task(id: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        state,
        tasks_axi: None,
        beads_jsonl: None,
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
        resources: vec![],
    }
}
fn holding_cap(id: &str, minutes: f64, resources: &[&str]) -> Capability {
    let mut capability = cap(id, minutes);
    capability.resources = resources.iter().map(|r| (*r).into()).collect();
    capability
}
fn resource(name: &str, capacity: f64) -> ResourceDecl {
    ResourceDecl {
        name: name.into(),
        capacity,
        unit: "instances".into(),
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
            platforms: Vec::new(),
            licenses: Vec::new(),
            acceptance: vec![],
        },
        artifact: vec![],
        resource: vec![],
        task: tasks,
        method: methods,
        capability: caps,
        evidence: vec![],
        investigation: vec![],
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

#[test]
fn exclusive_resource_holders_serialize_in_compiled_makespan() {
    // Two parallel 20-minute tasks on one Resolve instance take 40 minutes
    // elapsed, and the trace names the bottleneck. Fails on a
    // precedence-only P7, which reports 20 with no resource-limited row.
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
        vec![
            holding_cap("ca", 20.0, &["resolve"]),
            holding_cap("cb", 20.0, &["resolve"]),
        ],
    );
    plan.resource = vec![resource("resolve", 1.0)];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 40.0);
    assert_eq!(compiled.makespan_range.expected, 40.0);
    assert_eq!(compiled.totals["duration_minutes"].expected, 40.0);
    assert!(
        compiled.trace.iter().any(|row| row.subject == "plan"
            && row.decision == "resource-limited"
            && row.reason.contains("`resolve`")
            && row.reason.contains("capacity 1")
            && row.reason.contains("20.000 to 40.000")),
        "trace names the binding resource and the stretch: {:?}",
        compiled.trace
    );
}

#[test]
fn makespan_selection_prefers_the_plan_that_reduces_contention() {
    // Plan-level acceptance case 3: with two Resolve instances the
    // resolve-heavy plan finishes in 20 minutes against 25 for the offload
    // plan; with one, the resolve-heavy plan serializes to 40 while
    // offloading one task to a GPU worker stays at 25, so P6 flips. Fails
    // when P6 scores candidates without capacities.
    fn contention_plan(resolve_capacity: f64) -> PlanDoc {
        let mut plan = doc(
            vec![
                task("root", TaskState::Decomposed),
                task("r1", TaskState::Resolved),
                task("r2", TaskState::Resolved),
                task("g1", TaskState::Resolved),
                task("g2", TaskState::Resolved),
            ],
            vec![
                decompose("resolve-heavy", "root", &["r1", "r2"], &[]),
                decompose("offload", "root", &["g1", "g2"], &[]),
                invoke("ir1", "r1", "cr1"),
                invoke("ir2", "r2", "cr2"),
                invoke("ig1", "g1", "cg1"),
                invoke("ig2", "g2", "cg2"),
            ],
            vec![
                holding_cap("cr1", 20.0, &["resolve"]),
                holding_cap("cr2", 20.0, &["resolve"]),
                holding_cap("cg1", 20.0, &["resolve"]),
                holding_cap("cg2", 25.0, &["gpu"]),
            ],
        );
        plan.resource = vec![resource("resolve", resolve_capacity), resource("gpu", 2.0)];
        plan
    }
    let roomy = pc_core::compile_with_objective(
        &contention_plan(2.0),
        std::time::Duration::from_secs(2),
        pc_core::Objective::Makespan,
    )
    .unwrap();
    assert_eq!(roomy.selection["root"], "resolve-heavy");
    assert_eq!(roomy.makespan_minutes, 20.0);
    let tight = pc_core::compile_with_objective(
        &contention_plan(1.0),
        std::time::Duration::from_secs(2),
        pc_core::Objective::Makespan,
    )
    .unwrap();
    assert_eq!(tight.selection["root"], "offload");
    assert_eq!(tight.makespan_minutes, 25.0);
    assert_eq!(tight.selection_status, "optimal");
}

#[test]
fn contended_holders_are_critical_in_the_resource_contention_example() {
    // The single Resolve instance serializes grade-a and grade-b (0-30,
    // 30-60) while analysis overlaps on CI. Both Resolve holders form the
    // critical chain; analysis has 30 minutes of slack, and at most two
    // tasks ever run at once. Fails when P7 mixes precedence earliest
    // times with the contended makespan, which gives every task 30 minutes
    // of slack, no critical task and width 3.
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/resource-contention/plan.toml"
    ))
    .unwrap();
    let plan = pc_ir::from_toml(&text).unwrap();
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 60.0);
    let timing = |task: &str| compiled.timings[task].clone();
    assert!(timing("grade-a").critical && timing("grade-b").critical);
    assert_eq!(timing("grade-b").earliest_start, 30.0);
    assert!(!timing("analyze").critical);
    assert_eq!(timing("analyze").slack, 30.0);
    assert_eq!(compiled.parallel_width, 2);
}

#[test]
fn resource_limited_row_names_contention_below_the_load_bound() {
    // a(resolve) -> x and b(resolve) -> y, ten minutes each, one Resolve
    // instance: a 0-10, b 10-20, y 20-30 against a 20-minute precedence
    // span, though the resolve load (20) never exceeds that span. Fails
    // when the row only fires on the pure load bound.
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("b", TaskState::Resolved),
            task("x", TaskState::Resolved),
            task("y", TaskState::Resolved),
        ],
        vec![
            decompose(
                "r",
                "root",
                &["a", "b", "x", "y"],
                &[("a", "x"), ("b", "y")],
            ),
            invoke("ia", "a", "ca"),
            invoke("ib", "b", "cb"),
            invoke("ix", "x", "cx"),
            invoke("iy", "y", "cy"),
        ],
        vec![
            holding_cap("ca", 10.0, &["resolve"]),
            holding_cap("cb", 10.0, &["resolve"]),
            cap("cx", 10.0),
            cap("cy", 10.0),
        ],
    );
    plan.resource = vec![resource("resolve", 1.0)];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 30.0);
    assert!(
        compiled
            .trace
            .iter()
            .any(|row| row.decision == "resource-limited"
                && row.reason.contains("`resolve`")
                && row.reason.contains("20.000 to 30.000")),
        "{:?}",
        compiled.trace
    );
    let critical: Vec<&str> = compiled
        .timings
        .iter()
        .filter(|(_, t)| t.critical)
        .map(|(task, _)| task.as_str())
        .collect();
    assert_eq!(critical, ["a", "b", "y"]);
}

#[test]
fn resource_limited_rows_name_only_resources_that_hold_back_critical_work() {
    // g1 and g2 serialize on one Resolve instance (0-30, 30-60) and also
    // hold a CI executor (capacity 3) that t1 shares 0-30; u1 and u2
    // serialize on one GPU (0-5, 5-10). Only Resolve stretches the
    // makespan: CI never saturates, so t1 finishing at 30 does not hold
    // back the second grade, and u2 keeps 50 minutes of slack. Fails when
    // every resource whose holder delayed any task gets a row, or when an
    // unsaturated co-held resource adds a blocking edge.
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("g1", TaskState::Resolved),
            task("g2", TaskState::Resolved),
            task("t1", TaskState::Resolved),
            task("u1", TaskState::Resolved),
            task("u2", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["g1", "g2", "t1", "u1", "u2"], &[]),
            invoke("ig1", "g1", "cg1"),
            invoke("ig2", "g2", "cg2"),
            invoke("it1", "t1", "ct1"),
            invoke("iu1", "u1", "cu1"),
            invoke("iu2", "u2", "cu2"),
        ],
        vec![
            holding_cap("cg1", 30.0, &["resolve", "ci"]),
            holding_cap("cg2", 30.0, &["resolve", "ci"]),
            holding_cap("ct1", 30.0, &["ci"]),
            holding_cap("cu1", 5.0, &["gpu"]),
            holding_cap("cu2", 5.0, &["gpu"]),
        ],
    );
    plan.resource = vec![
        resource("resolve", 1.0),
        resource("ci", 3.0),
        resource("gpu", 1.0),
    ];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 60.0);
    let rows: Vec<&str> = compiled
        .trace
        .iter()
        .filter(|row| row.decision == "resource-limited")
        .map(|row| row.reason.as_str())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("`resolve`"), "{rows:?}");
    let critical: Vec<&str> = compiled
        .timings
        .iter()
        .filter(|(_, t)| t.critical)
        .map(|(task, _)| task.as_str())
        .collect();
    assert_eq!(critical, ["g1", "g2"]);
    assert_eq!(compiled.timings["t1"].slack, 30.0);
    assert_eq!(compiled.timings["u2"].slack, 50.0);
}

#[test]
fn resource_holder_ahead_of_an_on_time_follower_is_critical() {
    // One Resolve instance: a (resolve, 10) runs 0-10 while p (10) runs,
    // then b (resolve, 20) follows p at 10 without waiting. Delaying a at
    // all would push b and the makespan, so a has no slack even though b
    // was never held back. Fails when only delayed followers get resource
    // edges, which leaves a 20 minutes of slack.
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("a", TaskState::Resolved),
            task("p", TaskState::Resolved),
            task("b", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["a", "p", "b"], &[("p", "b")]),
            invoke("ia", "a", "ca"),
            invoke("ip", "p", "cp"),
            invoke("ib", "b", "cb"),
        ],
        vec![
            holding_cap("ca", 10.0, &["resolve"]),
            cap("cp", 10.0),
            holding_cap("cb", 20.0, &["resolve"]),
        ],
    );
    plan.resource = vec![resource("resolve", 1.0)];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 30.0);
    assert_eq!(compiled.timings["a"].slack, 0.0);
    let critical: Vec<&str> = compiled
        .timings
        .iter()
        .filter(|(_, t)| t.critical)
        .map(|(task, _)| task.as_str())
        .collect();
    assert_eq!(critical, ["a", "b", "p"]);
    assert!(
        compiled
            .trace
            .iter()
            .all(|row| row.decision != "resource-limited"),
        "{:?}",
        compiled.trace
    );
}

#[test]
fn holder_ahead_of_a_follower_with_a_free_slot_keeps_its_slack() {
    // The contention example with two Resolve instances plus p (30) ->
    // c (resolve, 30): grade-a and grade-b run 0-30 side by side and c
    // runs 30-60. Either grade can slip 30 minutes because c only needs
    // one of the two slots. Fails when every holder points to the next
    // holder on its slot regardless of free capacity, which marks one
    // grade critical by tie-break.
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/resource-contention/plan.toml"
    ))
    .unwrap();
    let mut plan = pc_ir::from_toml(&text).unwrap();
    plan.resource
        .iter_mut()
        .find(|r| r.name == "resolve")
        .unwrap()
        .capacity = 2.0;
    plan.task.push(task("p", TaskState::Resolved));
    plan.task.push(task("c", TaskState::Resolved));
    let pipeline = plan
        .method
        .iter_mut()
        .find(|m| m.id == "reels-pipeline")
        .unwrap();
    pipeline.subtasks.extend(["p".into(), "c".into()]);
    pipeline.order_edges.push(OrderEdge {
        from: "p".into(),
        to: "c".into(),
    });
    plan.method.push(invoke("ip", "p", "cp"));
    plan.method.push(invoke("ic", "c", "cc"));
    plan.capability.push(cap("cp", 30.0));
    plan.capability.push(holding_cap("cc", 30.0, &["resolve"]));
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 60.0);
    assert_eq!(compiled.timings["grade-a"].slack, 30.0);
    assert_eq!(compiled.timings["grade-b"].slack, 30.0);
    assert!(compiled.timings["p"].critical && compiled.timings["c"].critical);
}

#[test]
fn resource_limited_row_names_a_saturated_multi_slot_resource() {
    // Two GPU workers, three ten-minute GPU tasks, no edges: g1 and g2 run
    // 0-10 and g3 waits for a slot, 10-20, against a 10-minute precedence
    // span. Either early holder could slip, so neither is critical, but
    // the saturated GPU still held back critical g3. Fails when the row
    // requires a critical holder, which no multi-slot resource provides.
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("g1", TaskState::Resolved),
            task("g2", TaskState::Resolved),
            task("g3", TaskState::Resolved),
        ],
        vec![
            decompose("r", "root", &["g1", "g2", "g3"], &[]),
            invoke("ig1", "g1", "cg1"),
            invoke("ig2", "g2", "cg2"),
            invoke("ig3", "g3", "cg3"),
        ],
        vec![
            holding_cap("cg1", 10.0, &["gpu"]),
            holding_cap("cg2", 10.0, &["gpu"]),
            holding_cap("cg3", 10.0, &["gpu"]),
        ],
    );
    plan.resource = vec![resource("gpu", 2.0)];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 20.0);
    let rows: Vec<&str> = compiled
        .trace
        .iter()
        .filter(|row| row.decision == "resource-limited")
        .map(|row| row.reason.as_str())
        .collect();
    assert_eq!(rows.len(), 1, "{:?}", compiled.trace);
    assert!(
        rows[0].contains("`gpu`") && rows[0].contains("10.000 to 20.000"),
        "{rows:?}"
    );
}

#[test]
fn resource_saturated_only_as_a_delayed_task_starts_gets_no_row() {
    // One Resolve instance: g1 (resolve, 30) runs 0-30 and holds back j
    // (resolve + ci, 30) to 30-60. Two CI executors: t1 and t1b (ci, 10)
    // follow p (20) and run 20-30, so CI is full only for the last ten
    // minutes of j's 30-minute wait and frees exactly as j starts. Raising
    // CI capacity changes nothing, so only Resolve gets a row. Fails when
    // saturation is checked only at the instant j starts.
    let mut plan = doc(
        vec![
            task("root", TaskState::Decomposed),
            task("g1", TaskState::Resolved),
            task("j", TaskState::Resolved),
            task("p", TaskState::Resolved),
            task("t1", TaskState::Resolved),
            task("t1b", TaskState::Resolved),
        ],
        vec![
            decompose(
                "r",
                "root",
                &["g1", "j", "p", "t1", "t1b"],
                &[("p", "t1"), ("p", "t1b")],
            ),
            invoke("ig1", "g1", "cg1"),
            invoke("ij", "j", "cj"),
            invoke("ip", "p", "cp"),
            invoke("it1", "t1", "ct1"),
            invoke("it1b", "t1b", "ct1b"),
        ],
        vec![
            holding_cap("cg1", 30.0, &["resolve"]),
            holding_cap("cj", 30.0, &["resolve", "ci"]),
            cap("cp", 20.0),
            holding_cap("ct1", 10.0, &["ci"]),
            holding_cap("ct1b", 10.0, &["ci"]),
        ],
    );
    plan.resource = vec![resource("resolve", 1.0), resource("ci", 2.0)];
    let compiled = compile(&plan).unwrap();
    assert_eq!(compiled.makespan_minutes, 60.0);
    let rows: Vec<&str> = compiled
        .trace
        .iter()
        .filter(|row| row.decision == "resource-limited")
        .map(|row| row.reason.as_str())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("`resolve`"), "{rows:?}");
    assert_eq!(compiled.timings["t1"].slack, 30.0);
    assert_eq!(compiled.timings["t1b"].slack, 30.0);
}
