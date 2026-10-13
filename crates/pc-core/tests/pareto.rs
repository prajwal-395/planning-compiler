//! PC-09 acceptance: deterministic Pareto evaluation over alternatives.
//!
//! Each test names the defect it would catch: a dominated alternative
//! reported without dominators, or an unvalued objective invented instead
//! of left unknown.

use pc_core::{Dominance, pareto_frontier};
use pc_ir::{
    Capability, CapabilityKind, Estimate, EstimateProvenance, Method, MethodKind, MethodStatus,
    Objective, PlanDoc, PlanMeta, Task, TaskState,
};
use std::time::Duration;

fn task(id: &str) -> Task {
    Task {
        id: id.into(),
        title: id.into(),
        state: TaskState::Resolved,
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

fn priced(id: &str, minutes: f64, usd: f64) -> Capability {
    Capability {
        id: id.into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![
            Estimate {
                metric: "duration_minutes".into(),
                low: minutes,
                likely: minutes,
                high: minutes,
                provenance: EstimateProvenance::Declared,
                observations: None,
                window: None,
            },
            Estimate {
                metric: "money_usd".into(),
                low: usd,
                likely: usd,
                high: usd,
                provenance: EstimateProvenance::Declared,
                observations: None,
                window: None,
            },
        ],
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
        resources: vec![],
    }
}

fn doc(methods: Vec<Method>, caps: Vec<Capability>) -> PlanDoc {
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
            weights: [
                ("duration_minutes".to_owned(), 1.0),
                ("money_usd".to_owned(), 1.0),
            ]
            .into_iter()
            .collect(),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            platforms: Vec::new(),
            licenses: Vec::new(),
            acceptance: vec![],
        },
        artifact: vec![],
        resource: vec![],
        task: vec![task("root")],
        method: methods,
        capability: caps,
        evidence: vec![],
        investigation: vec![],
    }
}

#[test]
fn pareto_reports_frontier_and_names_dominators() {
    // Captain's PC-09 example: A/B/C each optimal under different
    // tradeoffs, D dominated. Fails when D lands on the frontier or names
    // no frontier dominator.
    let plan = doc(
        vec![
            invoke("via-a", "root", "cap-a"),
            invoke("via-b", "root", "cap-b"),
            invoke("via-c", "root", "cap-c"),
            invoke("via-d", "root", "cap-d"),
        ],
        vec![
            priced("cap-a", 10.0, 12.0),
            priced("cap-b", 14.0, 4.0),
            priced("cap-c", 12.0, 7.0),
            priced("cap-d", 20.0, 15.0),
        ],
    );
    let report = pareto_frontier(&plan, Duration::from_secs(2)).unwrap();
    assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
    assert!(report.not_compared.is_empty());
    assert!(report.incomplete.is_empty());
    assert_eq!(report.status, "optimal");
    assert_eq!(report.evaluated, 4);
    let frontier: Vec<&str> = report
        .frontier
        .iter()
        .map(|alt| alt.methods["root"].as_str())
        .collect();
    assert_eq!(frontier, vec!["via-a", "via-c", "via-b"]);
    assert_eq!(report.dominated.len(), 1);
    assert_eq!(report.dominated[0].methods["root"], "via-d");
    assert!(
        !report.dominated[0].dominated_by.is_empty(),
        "dominated alternatives must name what dominates them"
    );
    let frontier_ids: Vec<&str> = report.frontier.iter().map(|alt| alt.id.as_str()).collect();
    assert!(
        report.dominated[0]
            .dominated_by
            .iter()
            .all(|d| frontier_ids.contains(&d.by.as_str())),
        "dominators must be frontier ids"
    );
    // Weighted objectives survive beside the frontier for clear preferences.
    assert_eq!(report.frontier[0].objectives["makespan_minutes"], 10.0);
    assert_eq!(report.frontier[0].objective, 22.0);
}

#[test]
fn pareto_reports_sparse_reliability_as_not_compared() {
    // The reviewer's case: reliability (`failure_risk`) priced for cap-a
    // only. Time and money are still compared, so D is dominated by A on
    // those two with failure_risk listed as not compared. Fails when one
    // sparse metric stops every other comparison.
    let mut risky = priced("cap-a", 10.0, 12.0);
    risky.estimates.push(Estimate {
        metric: "failure_risk".into(),
        low: 0.05,
        likely: 0.05,
        high: 0.05,
        provenance: EstimateProvenance::Declared,
        observations: None,
        window: None,
    });
    let plan = doc(
        vec![
            invoke("via-a", "root", "cap-a"),
            invoke("via-b", "root", "cap-b"),
            invoke("via-c", "root", "cap-c"),
            invoke("via-d", "root", "cap-d"),
        ],
        vec![
            risky,
            priced("cap-b", 14.0, 4.0),
            priced("cap-c", 12.0, 7.0),
            priced("cap-d", 20.0, 15.0),
        ],
    );
    let report = pareto_frontier(&plan, Duration::from_secs(2)).unwrap();
    assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
    assert_eq!(report.not_compared, vec!["failure_risk"]);
    assert!(report.incomplete.is_empty());
    let frontier: Vec<&str> = report
        .frontier
        .iter()
        .map(|alt| alt.methods["root"].as_str())
        .collect();
    assert_eq!(frontier, vec!["via-a", "via-c", "via-b"]);
    assert_eq!(report.dominated.len(), 1);
    let d = &report.dominated[0];
    assert_eq!(d.methods["root"], "via-d");
    assert_eq!(d.unknown, vec!["failure_risk"]);
    let by_a = Dominance {
        by: report.frontier[0].id.clone(),
        on: vec!["makespan_minutes".into(), "money_usd".into()],
        unknown: vec!["failure_risk".into()],
    };
    assert!(d.dominated_by.contains(&by_a), "{:?}", d.dominated_by);
}

#[test]
fn pareto_shows_open_work_separately_without_ranking_it() {
    // `split` needs `b`, which has no accepted method, so its time and cost
    // are unknown: `split` is incomplete and shown separately, never
    // ranked, while complete `whole` still dominates `whole2`. Once `b` is
    // done its work is genuinely zero and `split` dominates. Fails when
    // `split` is ranked, `whole` is labeled dominated while `b` is open,
    // or `whole2` is not.
    let mut plan = doc(
        vec![
            Method {
                id: "split".into(),
                task: "root".into(),
                kind: MethodKind::Decompose,
                subtasks: vec!["a".into(), "b".into()],
                order_edges: vec![],
                capability: None,
                status: MethodStatus::Accepted,
                provenance: "test".into(),
                assumptions: vec![],
            },
            invoke("whole", "root", "cap-whole"),
            invoke("whole2", "root", "cap-whole2"),
            invoke("via-a", "a", "cap-a"),
        ],
        vec![
            priced("cap-whole", 8.0, 8.0),
            priced("cap-whole2", 10.0, 10.0),
            priced("cap-a", 5.0, 5.0),
        ],
    );
    plan.task.push(task("a"));
    plan.task.push(task("b"));
    let report = pareto_frontier(&plan, Duration::from_secs(2)).unwrap();
    assert_eq!(report.evaluated, 3);
    assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
    assert!(report.not_compared.is_empty());
    assert_eq!(report.frontier.len(), 1);
    assert_eq!(report.frontier[0].methods["root"], "whole");
    assert_eq!(report.dominated.len(), 1);
    assert_eq!(report.dominated[0].methods["root"], "whole2");
    assert_eq!(report.dominated[0].dominated_by[0].by, "alt-0");
    assert_eq!(report.incomplete.len(), 1);
    assert_eq!(report.incomplete[0].methods["root"], "split");

    plan.task.last_mut().unwrap().state = TaskState::Done;
    let report = pareto_frontier(&plan, Duration::from_secs(2)).unwrap();
    assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
    assert!(report.incomplete.is_empty());
    assert_eq!(report.frontier.len(), 1);
    assert_eq!(report.frontier[0].methods["root"], "split");
    assert_eq!(report.dominated.len(), 2);
}

#[test]
fn pareto_leaves_unvalued_objectives_unknown() {
    // No capability prices money: the time dimension alone decides and
    // money never appears as an invented zero column. Fails on an extra
    // dimension.
    let mut plan = doc(vec![invoke("via-a", "root", "cap-a")], vec![]);
    plan.method.push(invoke("via-b", "root", "cap-b"));
    plan.capability.push(Capability {
        id: "cap-a".into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "duration_minutes".into(),
            low: 10.0,
            likely: 10.0,
            high: 10.0,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
        resources: vec![],
    });
    plan.capability.push(Capability {
        id: "cap-b".into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "duration_minutes".into(),
            low: 5.0,
            likely: 5.0,
            high: 5.0,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
        resources: vec![],
    });
    let report = pareto_frontier(&plan, Duration::from_secs(2)).unwrap();
    assert_eq!(report.compared, vec!["makespan_minutes"]);
    assert!(report.not_compared.is_empty());
    assert!(report.incomplete.is_empty());
    assert_eq!(report.frontier.len(), 1);
    assert_eq!(report.frontier[0].methods["root"], "via-b");
    assert_eq!(report.dominated.len(), 1);
    assert_eq!(report.dominated[0].methods["root"], "via-a");
}
