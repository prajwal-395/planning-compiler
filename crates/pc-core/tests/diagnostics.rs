//! One test per P1/P2 diagnostic (E000-E015) plus determinism.
//!
//! Each test starts from [`valid_doc`] and applies the smallest mutation
//! that triggers exactly one code, so a regression in any single check
//! fails exactly one test.

use pc_core::passes::{EvidenceLog, check};
use pc_ir::{
    AcceptanceKind, ArtifactDecl, Capability, CapabilityKind, ContractItem, Estimate,
    EstimateProvenance, Evidence, Method, MethodKind, MethodStatus, Objective, OrderEdge, PlanDoc,
    PlanMeta, ResourceDecl, Task, TaskState,
};

fn contract(artifact: &str, fields: &[&str]) -> ContractItem {
    ContractItem {
        artifact: artifact.to_string(),
        fields: fields.iter().map(ToString::to_string).collect(),
    }
}

/// Smallest plan that checks clean: a root decomposed into one leaf.
fn valid_doc() -> (String, PlanDoc) {
    let text = "test".to_string();
    let doc = PlanDoc {
        ir_version: 0,
        plan: PlanMeta {
            id: "p".to_string(),
            title: "t".to_string(),
            description: String::new(),
            external_inputs: vec![],
        },
        objective: Objective {
            root_task: "root".to_string(),
            weights: [("duration_minutes".to_string(), 1.0)]
                .into_iter()
                .collect(),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            acceptance: vec![],
        },
        artifact: vec![ArtifactDecl {
            name: "transcript".to_string(),
            fields: vec!["words".to_string(), "timings".to_string()],
        }],
        resource: vec![],
        task: vec![
            Task {
                id: "root".to_string(),
                title: "root".to_string(),
                state: TaskState::Decomposed,
                consumes: vec![],
                produces: vec![],
            },
            Task {
                id: "leaf".to_string(),
                title: "leaf".to_string(),
                state: TaskState::Open,
                consumes: vec![],
                produces: vec![],
            },
        ],
        method: vec![Method {
            id: "m".to_string(),
            task: "root".to_string(),
            kind: MethodKind::Decompose,
            subtasks: vec!["leaf".to_string()],
            order_edges: vec![],
            capability: None,
            status: MethodStatus::Accepted,
            provenance: "agent:claude".to_string(),
            assumptions: vec![],
        }],
        capability: vec![],
        evidence: vec![],
    };
    (text, doc)
}

fn codes(report: &pc_core::passes::Report) -> Vec<String> {
    report.diagnostics.iter().map(|d| d.code.clone()).collect()
}

#[test]
fn valid_plan_checks_clean() {
    let (text, mut doc) = valid_doc();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);
    assert_eq!(report.counts.tasks, 2);
}

#[test]
fn e000_rejects_future_ir_version() {
    let (text, mut doc) = valid_doc();
    doc.ir_version = 99;
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E000"]);
}

#[test]
fn e001_reports_duplicate_task_ids() {
    let (text, mut doc) = valid_doc();
    doc.task.push(Task {
        id: "leaf".to_string(),
        title: "dupe".to_string(),
        state: TaskState::Open,
        consumes: vec![],
        produces: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E001".to_string()));
}

#[test]
fn e002_reports_unknown_subtask() {
    let (text, mut doc) = valid_doc();
    doc.method[0].subtasks.push("ghost".to_string());
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E002"]);
}

#[test]
fn e002_reports_unknown_objective_root() {
    let (text, mut doc) = valid_doc();
    doc.objective.root_task = "ghost".to_string();
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E002"]);
}

#[test]
fn e003_reports_unknown_capability() {
    let (text, mut doc) = valid_doc();
    doc.method.push(Method {
        id: "inv".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some("ghost@1".to_string()),
        status: MethodStatus::Accepted,
        provenance: String::new(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E003".to_string()));
}

#[test]
fn e004_reports_undeclared_artifact() {
    let (text, mut doc) = valid_doc();
    doc.task[1].produces = vec![contract("video", &["frames"])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E004".to_string()));
}

#[test]
fn e005_reports_undeclared_field() {
    let (text, mut doc) = valid_doc();
    doc.task[1].produces = vec![contract("transcript", &["confidence"])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E005".to_string()));
}

#[test]
fn e006_reports_hierarchy_cycle() {
    let (text, mut doc) = valid_doc();
    // leaf decomposes back into root: root -> leaf -> root.
    doc.task[1].state = TaskState::Decomposed;
    doc.method.push(Method {
        id: "back".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["root".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: String::new(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E006".to_string()));
}

#[test]
fn e006_ignores_proposed_methods() {
    // A cycle through a merely proposed method is a hypothesis, not an error.
    let (text, mut doc) = valid_doc();
    doc.task[1].state = TaskState::Decomposed;
    doc.method.push(Method {
        id: "back".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["root".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Proposed,
        provenance: String::new(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
}

#[test]
fn e007_reports_order_cycle() {
    let (text, mut doc) = valid_doc();
    doc.task.push(Task {
        id: "b".to_string(),
        title: "b".to_string(),
        state: TaskState::Open,
        consumes: vec![],
        produces: vec![],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "b".to_string()];
    doc.method[0].order_edges = vec![
        OrderEdge {
            from: "leaf".to_string(),
            to: "b".to_string(),
        },
        OrderEdge {
            from: "b".to_string(),
            to: "leaf".to_string(),
        },
    ];
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E007"]);
}

fn contracted_doc() -> (String, PlanDoc) {
    // leaf must produce transcript.words+timings; cap covers only words.
    let (text, mut doc) = valid_doc();
    doc.task[1].state = TaskState::Resolved;
    doc.task[1].produces = vec![contract("transcript", &["words", "timings"])];
    doc.capability.push(Capability {
        id: "fast@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
        estimates: vec![],
        resources: vec![],
    });
    doc.method.push(Method {
        id: "inv".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some("fast@1".to_string()),
        status: MethodStatus::Accepted,
        provenance: String::new(),
        assumptions: vec![],
    });
    (text, doc)
}

#[test]
fn e008_reports_dropped_output() {
    // The Ren Voz case in miniature: the capability drops `timings`.
    let (text, mut doc) = contracted_doc();
    let report = check("plan.toml", &text, &mut doc, None);
    let all = codes(&report);
    assert!(all.contains(&"E008".to_string()), "got {all:?}");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("timings")),
        "names the dropped field: {all:?}"
    );
}

#[test]
fn e009_reports_unsatisfied_input() {
    let (text, mut doc) = valid_doc();
    doc.task[1].consumes = vec![contract("transcript", &["words"])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E009"]);
}

#[test]
fn e009_accepts_external_inputs() {
    let (text, mut doc) = valid_doc();
    doc.task[1].consumes = vec![contract("transcript", &["words"])];
    doc.plan.external_inputs = vec![contract("transcript", &["words"])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
}

#[test]
fn e010_reports_unknown_metric() {
    let (text, mut doc) = valid_doc();
    doc.capability.push(Capability {
        id: "c@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "vibes".to_string(),
            low: 1.0,
            likely: 2.0,
            high: 3.0,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        resources: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E010"]);
}

#[test]
fn e011_reports_inverted_three_point() {
    let (text, mut doc) = valid_doc();
    doc.capability.push(Capability {
        id: "c@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![Estimate {
            metric: "duration_minutes".to_string(),
            low: 9.0,
            likely: 5.0,
            high: 3.0,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        resources: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E011"]);
}

#[test]
fn e012_reports_undeclared_resource() {
    let (text, mut doc) = valid_doc();
    doc.capability.push(Capability {
        id: "c@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        consumes: vec![],
        produces: vec![],
        estimates: vec![],
        resources: vec!["gpu-9".to_string()],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    // Empty resources would also be fine; the undeclared one must fail.
    assert!(codes(&report).contains(&"E012".to_string()));
    let _ = ResourceDecl {
        name: "gpu-9".to_string(),
        capacity: 1.0,
        unit: "cards".to_string(),
    };
}

#[test]
fn e013_reports_invoke_without_capability() {
    let (text, mut doc) = valid_doc();
    doc.method.push(Method {
        id: "inv".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: String::new(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(codes(&report).contains(&"E013".to_string()));
}

#[test]
fn e014_reports_unknown_evidence_subject() {
    let (text, mut doc) = valid_doc();
    doc.evidence.push(Evidence {
        subject: "capability:ghost@1".to_string(),
        metric: "duration_minutes".to_string(),
        value: 4.0,
        unit: "minutes".to_string(),
        time: "2026-10-01T00:00:00Z".to_string(),
        source: "manual".to_string(),
        artifact_hash: None,
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E014"]);
}

fn root_evidence(metric: &str, unit: &str) -> Evidence {
    Evidence {
        subject: "task:root".to_string(),
        metric: metric.to_string(),
        value: 0.5,
        unit: unit.to_string(),
        time: "2026-10-01T00:00:00Z".to_string(),
        source: "manual".to_string(),
        artifact_hash: None,
    }
}

#[test]
fn e014_rejects_non_canonical_units() {
    for (metric, unit) in [
        ("duration_minutes", "min"),
        ("duration_minutes", "m"),
        ("money_usd", "$"),
        ("failure_risk", "probability"),
        ("failure_risk", ""),
    ] {
        let (text, mut doc) = valid_doc();
        doc.evidence.push(root_evidence(metric, unit));
        let report = check("plan.toml", &text, &mut doc, None);
        assert_eq!(
            codes(&report),
            vec!["E014"],
            "unit `{unit}` for `{metric}`: {:?}",
            report.diagnostics
        );
    }
}

#[test]
fn e014_tied_diagnostics_are_order_independent() {
    let (text, mut doc_a) = valid_doc();
    let mut bad_time = root_evidence("duration_minutes", "minutes");
    bad_time.time = "yesterday".to_string();
    let mut bad_value = root_evidence("duration_minutes", "minutes");
    bad_value.value = f64::NAN;
    doc_a.evidence = vec![bad_time, bad_value];
    let mut doc_b = doc_a.clone();
    doc_b.evidence.reverse();
    let a = check("plan.toml", &text, &mut doc_a, None);
    let b = check("plan.toml", &text, &mut doc_b, None);
    assert_eq!(codes(&a), vec!["E014", "E014"]);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn evidence_log_records_anchor_to_the_log() {
    let (text, mut doc) = valid_doc();
    doc.evidence
        .push(root_evidence("duration_minutes", "minutes"));
    doc.evidence
        .push(root_evidence("duration_minutes", "hours"));
    let mut ghost = root_evidence("duration_minutes", "minutes");
    ghost.subject = "task:ghost".to_string();
    doc.evidence.push(ghost);
    let mut odd = root_evidence("bogus_metric", "x");
    odd.subject = "task:leaf".to_string();
    doc.evidence.push(odd);
    let log = EvidenceLog {
        file: "evidence.jsonl",
        lines: &[2, 4, 7],
    };
    let report = check("plan.toml", &text, &mut doc, Some(log));
    let anchors: Vec<(&str, &str, Option<u32>)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.file.as_str(), d.line))
        .collect();
    assert_eq!(
        anchors,
        vec![
            ("E010", "evidence.jsonl", Some(7)),
            ("E014", "evidence.jsonl", Some(4)),
            ("E014", "evidence.jsonl", Some(2)),
        ]
    );
}

#[test]
fn canonical_units_match_case_insensitively() {
    let (text, mut doc) = valid_doc();
    doc.evidence
        .push(root_evidence("duration_minutes", "Minutes"));
    doc.evidence.push(root_evidence("money_usd", "USD"));
    doc.evidence.push(root_evidence("failure_risk", "ratio"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);
    let units: Vec<&str> = doc.evidence.iter().map(|e| e.unit.as_str()).collect();
    assert_eq!(units, vec!["minutes", "usd", "ratio"]);
}

#[test]
fn e015_reports_unknown_weight_metric() {
    let (text, mut doc) = valid_doc();
    doc.objective
        .weights
        .insert("human_minutes".to_string(), 1.0);
    let report = check("plan.toml", &text, &mut doc, None);
    // The removed human-time metric must fail validation, not silently pass.
    assert_eq!(codes(&report), vec!["E015"]);
}

#[test]
fn rejected_methods_are_ignored() {
    // A rejected method may dangle: no pass looks at it.
    let (text, mut doc) = valid_doc();
    doc.method.push(Method {
        id: "bad".to_string(),
        task: "ghost".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some("ghost@1".to_string()),
        status: MethodStatus::Rejected,
        provenance: String::new(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
}

#[test]
fn reports_are_byte_identical_regardless_of_input_order() {
    // Determinism: the same entities in a different order check identical.
    let (text, mut doc_a) = valid_doc();
    let mut doc_b = doc_a.clone();
    doc_b.task.reverse();
    doc_b.method.reverse();
    let a = check("plan.toml", &text, &mut doc_a, None);
    let b = check("plan.toml", &text, &mut doc_b, None);
    let ja = serde_json::to_string(&a).unwrap();
    let jb = serde_json::to_string(&b).unwrap();
    assert_eq!(ja, jb);
}

#[test]
fn acceptance_kind_serializes() {
    // Acceptance kinds exist so objectives can state criteria; keep the
    // serde shape covered even though no diagnostic targets it alone.
    let kinds = [
        AcceptanceKind::Manual,
        AcceptanceKind::Command,
        AcceptanceKind::Metric,
    ];
    for kind in kinds {
        let json = serde_json::to_string(&kind).unwrap();
        let back: AcceptanceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }
}
