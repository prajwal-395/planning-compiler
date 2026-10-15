//! One test per P1/P2 diagnostic (E000-E017) plus determinism.
//!
//! Each test starts from [`valid_doc`] and applies the smallest mutation
//! that triggers exactly one code, so a regression in any single check
//! fails exactly one test.

use pc_core::passes::{EvidenceLog, check};
use pc_ir::{
    AcceptanceKind, ArtifactDecl, Capability, CapabilityKind, ContractItem, Estimate,
    EstimateProvenance, Evidence, HypothesisAlternativeValue, Investigation,
    InvestigationHypothesis, Method, MethodKind, MethodStatus, Objective, OrderEdge, PlanDoc,
    PlanMeta, ProbabilityEstimate, ResourceDecl, Task, TaskState,
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
            tasks_axi_sections: Vec::new(),
        },
        objective: Objective {
            root_task: "root".to_string(),
            weights: [("duration_minutes".to_string(), 1.0)]
                .into_iter()
                .collect(),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            platforms: Vec::new(),
            licenses: Vec::new(),
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
                tasks_axi: None,
                beads_jsonl: None,
                consumes: vec![],
                produces: vec![],
            },
            Task {
                id: "leaf".to_string(),
                title: "leaf".to_string(),
                state: TaskState::Open,
                tasks_axi: None,
                beads_jsonl: None,
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
        investigation: vec![],
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
        tasks_axi: None,
        beads_jsonl: None,
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
        tasks_axi: None,
        beads_jsonl: None,
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
fn e012_reports_invalid_resource_capacity() {
    // P6/P7 reject a negative or non-finite capacity, so check must too.
    for capacity in [-1.0, f64::NAN] {
        let (text, mut doc) = valid_doc();
        doc.resource.push(ResourceDecl {
            name: "resolve".to_string(),
            capacity,
            unit: "instances".to_string(),
        });
        let report = check("plan.toml", &text, &mut doc, None);
        assert!(codes(&report).contains(&"E012".to_string()), "{capacity}");
    }
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
fn e014_holds_unknown_evidence_subjects_as_pending() {
    // G1: evidence may arrive before the hypothesis it supports. The record
    // is accepted and held as pending — never an error, and never folded
    // into the effective numbers — until the subject is proposed.
    let (text, mut doc) = valid_doc();
    doc.evidence.push(Evidence {
        subject: "capability:ghost@1".to_string(),
        metric: "duration_minutes".to_string(),
        value: 4.0,
        unit: "minutes".to_string(),
        time: "2026-10-01T00:00:00Z".to_string(),
        source: "manual".to_string(),
        basis: None,
        artifact_hash: None,
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.unattributed, 0);
    assert_eq!(
        estimated.pending,
        [pc_core::PendingEvidence {
            subject: "capability:ghost@1".into(),
            metric: "duration_minutes".into(),
            basis: None,
            observations: 1,
        }]
    );
    assert!(
        report.trace.iter().any(|e| e.pass == "P5-estimate"
            && e.subject == "capability:ghost@1"
            && e.decision == "pending"),
        "pending is traced: {:?}",
        report.trace
    );
}

#[test]
fn e014_reports_malformed_evidence_subject() {
    // Only well-formed subjects may be pending: a subject outside the
    // capability/method/task kinds, or with an empty id, can never be
    // proposed, so it is refused instead of pending forever.
    for subject in [
        "capabilty:ghost@1",
        "cap:x",
        "ghost@1",
        "task:",
        "capability:",
    ] {
        let (text, mut doc) = valid_doc();
        doc.evidence.push(Evidence {
            subject: subject.to_string(),
            metric: "duration_minutes".to_string(),
            value: 4.0,
            unit: "minutes".to_string(),
            time: "2026-10-01T00:00:00Z".to_string(),
            source: "manual".to_string(),
            basis: None,
            artifact_hash: None,
        });
        let report = check("plan.toml", &text, &mut doc, None);
        assert_eq!(codes(&report), vec!["E014"], "subject {subject}");
    }
}

#[test]
fn e014_reports_malformed_evidence_basis() {
    // G4: the basis is typed. Anything but `absolute` or
    // `distance-from:<capability>` is refused, so an absolute-accuracy
    // number cannot slip in where a distance belongs.
    for basis in [
        Some(""),
        Some("whisperx@3"),
        Some("distance-from:"),
        Some("distance-from:whisper x@3"),
        Some("distance-from:a:b"),
        Some("ground-truth"),
    ] {
        let (text, mut doc) = valid_doc();
        doc.evidence.push(Evidence {
            subject: "task:root".to_string(),
            metric: "duration_minutes".to_string(),
            value: 4.0,
            unit: "minutes".to_string(),
            time: "2026-10-01T00:00:00Z".to_string(),
            source: "manual".to_string(),
            basis: basis.map(str::to_string),
            artifact_hash: None,
        });
        let report = check("plan.toml", &text, &mut doc, None);
        assert_eq!(
            report
                .diagnostics
                .iter()
                .map(|d| d.code.clone())
                .collect::<Vec<_>>(),
            vec!["E014"],
            "basis {basis:?}: {:?}",
            report.diagnostics
        );
    }
}

#[test]
fn typed_evidence_bases_check_clean() {
    for basis in [None, Some("absolute"), Some("distance-from:whisperx@3")] {
        let (text, mut doc) = valid_doc();
        doc.evidence.push(Evidence {
            subject: "task:root".to_string(),
            metric: "duration_minutes".to_string(),
            value: 4.0,
            unit: "minutes".to_string(),
            time: "2026-10-01T00:00:00Z".to_string(),
            source: "manual".to_string(),
            basis: basis.map(str::to_string),
            artifact_hash: None,
        });
        let report = check("plan.toml", &text, &mut doc, None);
        assert!(report.ok(), "basis {basis:?}: {:?}", report.diagnostics);
    }
}

fn root_evidence(metric: &str, unit: &str) -> Evidence {
    Evidence {
        subject: "task:root".to_string(),
        metric: metric.to_string(),
        value: 0.5,
        unit: unit.to_string(),
        time: "2026-10-01T00:00:00Z".to_string(),
        source: "manual".to_string(),
        basis: None,
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
    let mut pending = root_evidence("duration_minutes", "minutes");
    pending.subject = "task:ghost".to_string();
    doc.evidence.push(pending);
    let mut odd = root_evidence("bogus_metric", "x");
    odd.subject = "task:leaf".to_string();
    doc.evidence.push(odd);
    let log = EvidenceLog {
        file: "evidence.jsonl",
        lines: &[2, 4, 7, 9],
    };
    let report = check("plan.toml", &text, &mut doc, Some(log));
    let anchors: Vec<(&str, &str, Option<u32>)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.as_str(), d.file.as_str(), d.line))
        .collect();
    // The unknown subject stages pending evidence, not a diagnostic: only
    // the bad unit and the unknown metric anchor to the log.
    assert_eq!(
        anchors,
        vec![
            ("E010", "evidence.jsonl", Some(9)),
            ("E014", "evidence.jsonl", Some(4)),
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

fn constrained_doc(status: MethodStatus) -> (String, PlanDoc) {
    let (text, mut doc) = valid_doc();
    doc.objective.platforms = vec!["linux-x86_64".to_string()];
    doc.objective.licenses = vec!["Apache-2.0".to_string()];
    doc.capability.push(Capability {
        id: "c@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        provider: None,
        source: None,
        platform_requirements: vec!["macos-arm64".to_string()],
        licensing_constraints: vec!["GPL-3.0".to_string()],
        consumes: vec![],
        produces: vec![],
        estimates: vec![],
        resources: vec![],
    });
    doc.method.push(Method {
        id: "inv".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some("c@1".to_string()),
        status,
        provenance: "agent:claude".to_string(),
        assumptions: vec![],
    });
    (text, doc)
}

#[test]
fn e016_reports_undeclared_platform_and_licence() {
    for status in [MethodStatus::Accepted, MethodStatus::Proposed] {
        let (text, mut doc) = constrained_doc(status);
        let report = check("plan.toml", &text, &mut doc, None);
        let messages: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(codes(&report), vec!["E016", "E016"], "{status:?}");
        assert!(
            messages
                .iter()
                .any(|m| m.contains("platforms `macos-arm64`")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("licences `GPL-3.0`")),
            "{messages:?}"
        );
    }
}

#[test]
fn e016_allows_declared_or_unconstrained_requirements() {
    let (text, mut doc) = constrained_doc(MethodStatus::Accepted);
    doc.objective.platforms.push("macos-arm64".to_string());
    doc.objective.licenses.push("GPL-3.0".to_string());
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);

    let (text, mut doc) = constrained_doc(MethodStatus::Accepted);
    doc.objective.platforms.clear();
    doc.objective.licenses.clear();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);

    let (text, mut doc) = constrained_doc(MethodStatus::Rejected);
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);
}

#[test]
fn e016_treats_listed_platforms_and_licences_as_alternatives() {
    let (text, mut doc) = constrained_doc(MethodStatus::Accepted);
    doc.capability[0].platform_requirements =
        vec!["linux-x86_64".to_string(), "macos-arm64".to_string()];
    doc.capability[0].licensing_constraints = vec!["MIT".to_string(), "Apache-2.0".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);

    let (text, mut doc) = constrained_doc(MethodStatus::Accepted);
    doc.capability[0].platform_requirements =
        vec!["macos-arm64".to_string(), "windows-x86_64".to_string()];
    doc.capability[0].licensing_constraints = vec!["MIT".to_string(), "Apache-2.0".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E016"]);
    assert!(
        report.diagnostics[0]
            .message
            .contains("platforms `macos-arm64`, `windows-x86_64`"),
        "{:?}",
        report.diagnostics
    );
}

/// Smallest plan carrying a valid VOI model: accepted `m-a` and proposed
/// `m-b` on `leaf` plus an investigation valuing both under two hypotheses
/// whose bounds [0.4, 0.6] admit a distribution.
fn investigation_doc() -> (String, PlanDoc) {
    let (text, mut doc) = valid_doc();
    for cap in ["a@1", "b@1"] {
        doc.capability.push(Capability {
            id: cap.to_string(),
            kind: CapabilityKind::Tool,
            version: "1".to_string(),
            provider: None,
            source: None,
            platform_requirements: vec![],
            licensing_constraints: vec![],
            consumes: vec![],
            produces: vec![],
            estimates: vec![],
            resources: vec![],
        });
    }
    for (id, cap, status) in [
        ("m-a", "a@1", MethodStatus::Accepted),
        ("m-b", "b@1", MethodStatus::Proposed),
    ] {
        doc.method.push(Method {
            id: id.to_string(),
            task: "leaf".to_string(),
            kind: MethodKind::Invoke,
            subtasks: vec![],
            order_edges: vec![],
            capability: Some(cap.to_string()),
            status,
            provenance: "agent:test".to_string(),
            assumptions: vec![],
        });
    }
    let hypothesis = |id: &str, probability: Option<ProbabilityEstimate>| InvestigationHypothesis {
        id: id.to_string(),
        probability,
        source: "test:review".to_string(),
        alternatives: vec![
            HypothesisAlternativeValue {
                alternative: "m-a".to_string(),
                value: 10.0,
            },
            HypothesisAlternativeValue {
                alternative: "m-b".to_string(),
                value: 20.0,
            },
        ],
    };
    let bounded = || {
        Some(ProbabilityEstimate {
            low: 0.4,
            high: 0.6,
            provenance: EstimateProvenance::Declared,
            source: "test:calibration".to_string(),
            observations: None,
        })
    };
    doc.investigation.push(Investigation {
        task: "leaf".to_string(),
        cost: 1.0,
        cost_source: "test:bench".to_string(),
        hypotheses: vec![hypothesis("h1", bounded()), hypothesis("h2", bounded())],
    });
    (text, doc)
}

#[test]
fn e017_accepts_shared_alternative_sets_across_hypotheses() {
    // Catches the validator treating the same alternative in two hypotheses
    // as a duplicate: every hypothesis must value the same set.
    let (text, mut doc) = investigation_doc();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);
}

#[test]
fn e017_accepts_unknown_probability() {
    // Unknown probabilities are represented as unknown, never fabricated.
    let (text, mut doc) = investigation_doc();
    doc.investigation[0].hypotheses[1].probability = None;
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "expected clean, got {:?}", report.diagnostics);
}

#[test]
fn e017_rejects_divergent_alternative_sets() {
    // EVPI compares the same implementations under every hypothesis, so a
    // hypothesis valuing a different set must fail validation.
    let (text, mut doc) = investigation_doc();
    doc.investigation[0].hypotheses[1].alternatives.pop();
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E017"]);
}

#[test]
fn e017_rejects_unknown_method_alternative() {
    let (text, mut doc) = investigation_doc();
    doc.investigation[0].hypotheses[0].alternatives[0].alternative = "ghost".to_string();
    let report = check("plan.toml", &text, &mut doc, None);
    // One E017 per offending hypothesis; no other code may fire.
    assert!(!report.diagnostics.is_empty());
    assert!(codes(&report).iter().all(|code| code == "E017"));
}

#[test]
fn e017_rejects_incoherent_probability_bounds() {
    // Bounds whose lows already sum past 1 admit no distribution.
    let (text, mut doc) = investigation_doc();
    for hypothesis in &mut doc.investigation[0].hypotheses {
        let probability = hypothesis.probability.as_mut().unwrap();
        probability.low = 0.7;
        probability.high = 0.9;
    }
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E017"]);
}

#[test]
fn e017_rejects_known_lows_past_one_beside_an_unknown_probability() {
    // Known lows already past 1 admit no distribution whatever the unknown
    // probability turns out to be.
    let (text, mut doc) = investigation_doc();
    for hypothesis in &mut doc.investigation[0].hypotheses {
        let probability = hypothesis.probability.as_mut().unwrap();
        probability.low = 0.7;
        probability.high = 0.9;
    }
    let mut unknown = doc.investigation[0].hypotheses[0].clone();
    unknown.id = "h3".to_string();
    unknown.probability = None;
    doc.investigation[0].hypotheses.push(unknown);
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E017"]);
}

#[test]
fn e017_rejects_hypotheses_missing_an_accepted_method() {
    // P6 may select any accepted method, so EVPI needs its value under every
    // hypothesis; otherwise the model silently yields no VOI.
    let (text, mut doc) = investigation_doc();
    for hypothesis in &mut doc.investigation[0].hypotheses {
        hypothesis
            .alternatives
            .retain(|value| value.alternative != "m-a");
    }
    let report = check("plan.toml", &text, &mut doc, None);
    // One E017 per offending hypothesis; no other code may fire.
    assert!(!report.diagnostics.is_empty());
    assert!(codes(&report).iter().all(|code| code == "E017"));
}

#[test]
fn e017_rejects_model_without_an_accepted_method() {
    // P6 selects only accepted methods, so a task with none has no selection
    // and the model would silently yield no VOI.
    let (text, mut doc) = investigation_doc();
    doc.method
        .iter_mut()
        .find(|method| method.id == "m-a")
        .unwrap()
        .status = MethodStatus::Proposed;
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(codes(&report), vec!["E017"]);
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

#[test]
fn check_notes_flag_proposed_contract_violation_without_error() {
    // G2: `pc check` is informational on proposed contract violations. A
    // proposed invoke that drops a consumed field stays `ok` (exit 0) but
    // carries a contract note with the same dropped field and consumer the
    // frontier blocked-alternative entry names.
    let (text, mut doc) = contracted_doc();
    let proposed = doc.method.iter_mut().find(|m| m.id == "inv").unwrap();
    proposed.status = MethodStatus::Proposed;
    proposed.provenance = "agent:test".to_string();
    // A live consumer of the dropped field, so the note names it.
    doc.task.push(Task {
        id: "consumer".to_string(),
        title: "consumer".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![contract("transcript", &["timings"])],
        produces: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(
        report.ok(),
        "notes never fail check: {:?}",
        report.diagnostics
    );
    assert_eq!(report.contract_notes.len(), 1);
    let note = &report.contract_notes[0];
    assert_eq!(note.method, "inv");
    assert_eq!(note.missing_outputs, vec!["transcript.timings".to_string()]);
    assert_eq!(note.consumers, vec!["consumer".to_string()]);
    assert!(
        report
            .trace
            .iter()
            .any(|e| e.pass == "check-notes" && e.subject == "method:inv" && e.decision == "note"),
        "notes are traced: {:?}",
        report.trace
    );
}

#[test]
fn check_notes_ignore_rejected_and_match_frontier_blocked() {
    // Rejected methods are ignored by every pass, including G2 notes. Live
    // proposed notes otherwise match the frontier blocked-alternative set.
    let (text, mut doc) = contracted_doc();
    let proposed = doc.method.iter_mut().find(|m| m.id == "inv").unwrap();
    proposed.status = MethodStatus::Proposed;
    proposed.provenance = "agent:test".to_string();
    doc.task.push(Task {
        id: "consumer".to_string(),
        title: "consumer".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![contract("transcript", &["timings"])],
        produces: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert_eq!(report.contract_notes.len(), 1);
    let compiled = pc_core::compile(&doc).unwrap();
    let frontier = pc_core::frontier(&doc, &compiled);
    let blocked: Vec<&str> = frontier
        .blocked
        .iter()
        .filter(|item| item.kind == "blocked-alternative")
        .map(|item| item.subject.as_str())
        .collect();
    assert_eq!(blocked, vec!["method:inv"]);
    assert!(
        frontier
            .blocked
            .iter()
            .any(|item| item.subject == "method:inv" && item.reason.contains("transcript.timings")),
        "frontier names the same field: {:?}",
        frontier.blocked
    );

    // Rejecting the proposal clears both the note and the frontier entry.
    let rejected = doc.method.iter_mut().find(|m| m.id == "inv").unwrap();
    rejected.status = MethodStatus::Rejected;
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(report.contract_notes.is_empty());
}
