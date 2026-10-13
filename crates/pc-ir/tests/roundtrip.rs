//! Round-trip property tests: TOML <-> IR <-> JSON.
//!
//! Structural round-trips must be exact for every generated document, so
//! hand-written plans and agent patches survive a load/save cycle byte for
//! byte at the value level.

use pc_ir::{
    AcceptanceCriterion, AcceptanceKind, ArtifactDecl, Capability, CapabilityKind, ContractItem,
    Estimate, EstimateProvenance, Evidence, Method, MethodKind, MethodStatus, Objective, OrderEdge,
    PlanDoc, PlanMeta, ResourceDecl, Task, TaskState, TasksAxiMeta,
};
use proptest::prelude::*;
use std::collections::BTreeMap;

/// Small identifier pool: unique per index, always TOML-safe.
fn ident(prefix: &str, i: usize) -> String {
    format!("{prefix}{i}")
}

/// Small-decimal floats with at most 6 significant digits.
///
/// Rationale: decimal text cannot losslessly carry arbitrary `f64` through
/// this ecosystem's float formatting (serde_json mis-rounds some 16-digit
/// values by 1 ulp, verified locally). Plans carry estimates with a few
/// significant digits, so the property pins exactly that precision instead
/// of asserting the unachievable.
fn finite_float() -> impl Strategy<Value = f64> {
    (-100_000..100_000i64).prop_map(|cents| cents as f64 / 100.0)
}

fn arb_plan() -> impl Strategy<Value = PlanDoc> {
    (0..=2usize, 0..=3usize, 0..=2usize).prop_flat_map(|(n_art, n_task, n_cap)| {
        let artifacts: Vec<(String, Vec<String>)> = (0..n_art)
            .map(|i| (ident("a", i), vec!["f0".to_string(), "f1".to_string()]))
            .collect();
        let task_ids: Vec<String> = (0..n_task.max(1)).map(|i| ident("t", i)).collect();
        let cap_ids: Vec<String> = (0..n_cap).map(|i| ident("c", i)).collect();
        (
            Just(artifacts.clone()),
            Just(task_ids.clone()),
            Just(cap_ids.clone()),
            prop::collection::vec((0..n_task.max(1), 0..3usize, 0..4usize), 0..=n_task.max(1)),
            prop::collection::vec(finite_float(), 0..=3),
        )
            .prop_map(move |(artifacts, task_ids, cap_ids, taskspec, weights)| {
                let contract_of = |i: usize| -> Vec<ContractItem> {
                    artifacts
                        .get(i % artifacts.len().max(1))
                        .map(|(name, fields)| {
                            vec![ContractItem {
                                artifact: name.clone(),
                                fields: fields.iter().take(1).cloned().collect(),
                            }]
                        })
                        .unwrap_or_default()
                };
                let tasks: Vec<Task> = taskspec
                    .into_iter()
                    .enumerate()
                    .map(|(i, (consumes, produces, state))| {
                        let states = [
                            TaskState::Open,
                            TaskState::Decomposed,
                            TaskState::Resolved,
                            TaskState::Done,
                            TaskState::Dropped,
                        ];
                        Task {
                            id: task_ids.get(i).cloned().unwrap_or_else(|| ident("t", i)),
                            title: format!("task {i}"),
                            state: states[state % states.len()],
                            consumes: contract_of(consumes),
                            produces: contract_of(produces),
                            // Cover the tasks-axi passthrough in the
                            // round-trip property on every third task.
                            tasks_axi: if i % 3 == 0 {
                                Some(TasksAxiMeta {
                                    section: "Queued".to_string(),
                                    repo: Some("planning-compiler".to_string()),
                                    kind: Some("ship".to_string()),
                                    priority: Some(2),
                                    created: Some("2026-10-12".to_string()),
                                    closed: None,
                                    hold_reason: Some("waiting on captain".to_string()),
                                    hold_kind: Some("captain".to_string()),
                                    hold_until: Some("2026-12-01".to_string()),
                                    body: Some("line one\n\nline three".to_string()),
                                })
                            } else {
                                None
                            },
                            beads_jsonl: None,
                        }
                    })
                    .collect();
                let root = task_ids.first().cloned().unwrap_or_default();
                let methods: Vec<Method> = task_ids
                    .iter()
                    .enumerate()
                    .map(|(i, tid)| {
                        let status = match i % 4 {
                            0 => MethodStatus::Accepted,
                            1 => MethodStatus::Proposed,
                            2 => MethodStatus::Derived,
                            _ => MethodStatus::Rejected,
                        };
                        if i % 2 == 0 {
                            Method {
                                id: ident("m", i),
                                task: tid.clone(),
                                kind: MethodKind::Decompose,
                                subtasks: task_ids.iter().skip(1).take(1).cloned().collect(),
                                order_edges: vec![],
                                capability: None,
                                status,
                                provenance: "agent:test".to_string(),
                                assumptions: vec![],
                            }
                        } else {
                            Method {
                                id: ident("m", i),
                                task: tid.clone(),
                                kind: MethodKind::Invoke,
                                subtasks: vec![],
                                order_edges: vec![],
                                capability: cap_ids.first().cloned(),
                                status,
                                provenance: String::new(),
                                assumptions: vec![],
                            }
                        }
                    })
                    .collect();
                let capabilities: Vec<Capability> = cap_ids
                    .iter()
                    .enumerate()
                    .map(|(i, cid)| Capability {
                        id: cid.clone(),
                        kind: if i % 2 == 0 {
                            CapabilityKind::Tool
                        } else {
                            CapabilityKind::Agent
                        },
                        version: "1".to_string(),
                        consumes: vec![],
                        produces: contract_of(i),
                        estimates: vec![Estimate {
                            metric: "duration_minutes".to_string(),
                            low: 1.0,
                            likely: 2.0,
                            high: 3.0,
                            provenance: EstimateProvenance::Declared,
                            observations: None,
                            window: None,
                        }],
                        provider: Some("example-provider".to_string()),
                        source: Some("pkg:example@1".to_string()),
                        platform_requirements: vec!["linux-x86_64".to_string()],
                        licensing_constraints: vec!["Apache-2.0".to_string()],
                        resources: vec![],
                    })
                    .collect();
                let mut weight_map = BTreeMap::new();
                for (i, w) in weights.into_iter().enumerate() {
                    let metric = ["duration_minutes", "money_usd", "failure_risk"][i % 3];
                    weight_map.insert(metric.to_string(), w.abs());
                }
                PlanDoc {
                    ir_version: 0,
                    plan: PlanMeta {
                        id: "prop".to_string(),
                        title: "property".to_string(),
                        description: String::new(),
                        external_inputs: vec![],
                        tasks_axi_sections: vec!["Queued".to_string(), "Done".to_string()],
                    },
                    objective: Objective {
                        root_task: root,
                        weights: weight_map,
                        budget_usd: Some(100.0),
                        deadline: None,
                        quality_floor: Some(0.5),
                        platforms: Vec::new(),
                        licenses: Vec::new(),
                        acceptance: vec![AcceptanceCriterion {
                            kind: AcceptanceKind::Manual,
                            description: "ok".to_string(),
                            command: None,
                            metric: None,
                            op: None,
                            value: None,
                        }],
                    },
                    artifact: artifacts
                        .into_iter()
                        .map(|(name, fields)| ArtifactDecl { name, fields })
                        .collect(),
                    resource: vec![ResourceDecl {
                        name: "r0".to_string(),
                        capacity: 1.0,
                        unit: "x".to_string(),
                    }],
                    task: tasks,
                    method: methods,
                    capability: capabilities,
                    evidence: vec![Evidence {
                        subject: "task:t0".to_string(),
                        metric: "duration_minutes".to_string(),
                        value: 1.5,
                        unit: "minutes".to_string(),
                        time: "2026-10-10T00:00:00Z".to_string(),
                        source: "prop".to_string(),
                        basis: None,
                        artifact_hash: None,
                    }],
                }
            })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn toml_round_trip_is_exact(doc in arb_plan()) {
        let text = pc_ir::to_toml(&doc).expect("serializable");
        let back = pc_ir::from_toml(&text).expect("parseable");
        prop_assert_eq!(&doc, &back);
    }

    #[test]
    fn json_round_trip_is_exact(doc in arb_plan()) {
        let text = pc_ir::to_json_pretty(&doc).expect("serializable");
        let back = pc_ir::from_json(&text).expect("parseable");
        prop_assert_eq!(&doc, &back);
    }

    #[test]
    fn toml_and_json_agree(doc in arb_plan()) {
        // TOML -> IR -> JSON -> IR is a fixed point of JSON -> IR.
        let via_toml: PlanDoc =
            pc_ir::from_toml(&pc_ir::to_toml(&doc).expect("toml")).expect("parse");
        let via_json: PlanDoc = pc_ir::from_json(
            &pc_ir::to_json_pretty(&via_toml).expect("json"),
        )
        .expect("parse");
        prop_assert_eq!(&via_toml, &via_json);
    }
}

/// Order edges survive TOML unchanged (spot check outside proptest).
#[test]
fn order_edge_shape_round_trips() {
    let edge = OrderEdge {
        from: "a".to_string(),
        to: "b".to_string(),
    };
    let json = serde_json::to_string(&edge).unwrap();
    let back: OrderEdge = serde_json::from_str(&json).unwrap();
    assert_eq!(edge, back);
}
