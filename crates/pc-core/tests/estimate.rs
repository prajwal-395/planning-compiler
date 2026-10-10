//! Acceptance tests for P5 (estimate).
//!
//! Evidence never edits declared estimates: it derives measured triples
//! (min/median/max) per capability and metric, and selection and roll-ups
//! run on the effective numbers. Attribution is by subject:
//! `capability:<id>` measures directly, `method:<id>` measures the invoked
//! capability, and `task:<id>` measures the one capability the task's
//! accepted invoke methods call (several make it unattributed). P5 runs after P4, so evidence recorded against a
//! merged-away task counts toward the surviving task's capability.

use pc_core::passes::check;
use pc_ir::{
    Capability, CapabilityKind, ContractItem, Estimate, EstimateProvenance, Evidence, Method,
    MethodKind, MethodStatus, Objective, PlanDoc, PlanMeta, Task, TaskState,
};

fn doc() -> (String, PlanDoc) {
    let text = "test".to_string();
    let doc = PlanDoc {
        ir_version: 0,
        plan: PlanMeta {
            id: "p".to_string(),
            title: "test".to_string(),
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
        artifact: vec![],
        resource: vec![],
        task: vec![Task {
            id: "root".to_string(),
            title: "root".to_string(),
            state: TaskState::Resolved,
            consumes: vec![],
            produces: vec![],
        }],
        method: vec![],
        capability: vec![],
        evidence: vec![],
    };
    (text, doc)
}

fn cap(id: &str, low: f64, likely: f64, high: f64) -> Capability {
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

fn ev(subject: &str, value: f64, time: &str) -> Evidence {
    Evidence {
        subject: subject.into(),
        metric: "duration_minutes".into(),
        value,
        unit: "minutes".into(),
        time: time.into(),
        source: "test".into(),
        artifact_hash: None,
    }
}

fn contract(artifact: &str, fields: &[&str]) -> ContractItem {
    ContractItem {
        artifact: artifact.to_string(),
        fields: fields.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn p5_attributes_capability_method_and_task_subjects() {
    let (text, mut doc) = doc();
    doc.task.push(Task {
        id: "leaf".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Resolved,
        consumes: vec![],
        produces: vec![],
    });
    doc.method.push(Method {
        id: "r".to_string(),
        task: "root".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["leaf".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    });
    doc.method.push(invoke("m", "leaf", "c@1"));
    doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    // One observation per subject kind, all measuring the same capability.
    doc.evidence
        .push(ev("capability:c@1", 10.0, "2026-10-01T00:00:00Z"));
    doc.evidence
        .push(ev("method:m", 20.0, "2026-10-02T00:00:00Z"));
    doc.evidence
        .push(ev("task:leaf", 30.0, "2026-10-03T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.profiles.len(), 1);
    let measured = estimated.profiles[0].measured.unwrap();
    assert_eq!(measured.low, 10.0);
    assert_eq!(measured.likely, 20.0);
    assert_eq!(measured.high, 30.0);
    assert_eq!(measured.observations, 3);
    assert_eq!(estimated.unattributed, 0);
}

#[test]
fn p5_task_evidence_with_several_accepted_capabilities_is_unattributed() {
    let (text, mut doc) = doc();
    doc.task.push(Task {
        id: "leaf".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Resolved,
        consumes: vec![],
        produces: vec![],
    });
    doc.method.push(invoke("m1", "leaf", "a@1"));
    doc.method.push(invoke("m2", "leaf", "b@1"));
    doc.capability.push(cap("a@1", 1.0, 1.0, 1.0));
    doc.capability.push(cap("b@1", 2.0, 2.0, 2.0));
    doc.evidence
        .push(ev("task:leaf", 7.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    // The record cannot say whether a@1 or b@1 ran, so neither profile
    // moves and the observation counts as unattributed.
    assert_eq!(estimated.unattributed, 1);
    assert_eq!(estimated.profiles.len(), 2);
    for profile in &estimated.profiles {
        assert!(profile.measured.is_none(), "{profile:?}");
    }
    assert_eq!(
        estimated.unattributed_tasks,
        [pc_core::UnattributedTaskEvidence {
            task: "leaf".into(),
            metric: "duration_minutes".into(),
            observations: 1,
            capabilities: vec!["a@1".into(), "b@1".into()],
            methods: vec!["m1".into(), "m2".into()],
        }]
    );
}

#[test]
fn p5_evidence_for_a_merged_away_task_still_counts() {
    // The step-4 review gap: after P4 merges leaf-dup into leaf, the
    // observation recorded as task:leaf-dup must still move the surviving
    // task's capability profile, not point at a task that no longer exists.
    let (text, mut doc) = doc();
    doc.task.push(Task {
        id: "leaf".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.artifact.push(pc_ir::ArtifactDecl {
        name: "transcript".to_string(),
        fields: vec!["words".to_string()],
    });
    doc.method.push(Method {
        id: "r".to_string(),
        task: "root".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["leaf".to_string(), "leaf-dup".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    });
    doc.capability.push(Capability {
        id: "shared@1".to_string(),
        kind: CapabilityKind::Tool,
        version: "1".to_string(),
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
        estimates: vec![Estimate {
            metric: "duration_minutes".to_string(),
            low: 5.0,
            likely: 5.0,
            high: 5.0,
            provenance: EstimateProvenance::Declared,
            observations: None,
            window: None,
        }],
        resources: vec![],
    });
    doc.method.push(invoke("m1", "leaf", "shared@1"));
    doc.method.push(invoke("m2", "leaf-dup", "shared@1"));
    doc.evidence
        .push(ev("task:leaf-dup", 7.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(
        report.trace.iter().any(|e| e.pass == "P4-share"
            && e.subject == "task:leaf-dup"
            && e.decision == "merged"),
        "expected the duplicate to merge"
    );
    assert_eq!(doc.evidence[0].subject, "task:leaf");
    let estimated = pc_core::estimate(&doc);
    let profile = estimated
        .profiles
        .iter()
        .find(|p| p.capability == "shared@1")
        .expect("shared capability keeps a profile");
    assert_eq!(profile.measured.unwrap().likely, 7.0);
    assert!(
        report.trace.iter().any(|e| e.pass == "P5-estimate"
            && e.subject == "capability:shared@1"
            && e.decision == "measured"),
        "P5 reports the folded observation: {:?}",
        report.trace
    );
    // And the measured value is what compilation rolls up, not the guess.
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.totals["duration_minutes"].expected, 7.0);
    assert_eq!(compiled.basis["duration_minutes"].measured_nodes, 1);
}

#[test]
fn p5_flags_stale_and_contradicting_evidence() {
    let (text, mut doc) = doc();
    doc.method.push(invoke("m", "root", "c@1"));
    doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    doc.evidence
        .push(ev("capability:c@1", 30.0, "2026-06-01T00:00:00Z"));
    doc.evidence
        .push(ev("capability:c@1", 31.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    let profile = &estimated.profiles[0];
    assert_eq!(profile.measured.unwrap().stale, 1);
    assert!(profile.contradicts_declared);
    let has = |decision: &str| {
        report.trace.iter().any(|e| {
            e.pass == "P5-estimate" && e.subject == "capability:c@1" && e.decision == decision
        })
    };
    assert!(has("measured"));
    assert!(has("stale"));
    assert!(has("contradicts"));
}

#[test]
fn p5_leaves_fresh_agreeing_evidence_unflagged() {
    let (text, mut doc) = doc();
    doc.method.push(invoke("m", "root", "c@1"));
    doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    doc.evidence
        .push(ev("capability:c@1", 5.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    let profile = &estimated.profiles[0];
    assert_eq!(profile.measured.unwrap().stale, 0);
    assert!(!profile.contradicts_declared);
    assert!(
        !report
            .trace
            .iter()
            .any(|e| e.pass == "P5-estimate" && e.decision != "measured"),
        "only the measured event: {:?}",
        report.trace
    );
}

#[test]
fn p5_counts_unattributed_evidence() {
    let (_, mut doc) = doc();
    doc.method.push(Method {
        id: "r".to_string(),
        task: "root".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec![],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    });
    // A decompose method invokes no capability, so its observation measures
    // nothing; an unknown subject is P2's E014 territory, counted here too.
    doc.evidence
        .push(ev("method:r", 1.0, "2026-10-01T00:00:00Z"));
    doc.evidence
        .push(ev("capability:ghost@9", 1.0, "2026-10-01T00:00:00Z"));
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.unattributed, 2);
    assert!(estimated.profiles.is_empty());
}

#[test]
fn p5_is_independent_of_evidence_order() {
    let (_, mut a) = doc();
    a.method.push(invoke("m", "root", "c@1"));
    a.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    for (i, v) in [3.0, 1.0, 2.0].into_iter().enumerate() {
        a.evidence.push(ev(
            "capability:c@1",
            v,
            &format!("2026-10-0{}T00:00:00Z", i + 1),
        ));
    }
    let mut b = a.clone();
    b.evidence.reverse();
    let first = serde_json::to_vec(&pc_core::estimate(&a)).unwrap();
    let second = serde_json::to_vec(&pc_core::estimate(&b)).unwrap();
    assert_eq!(first, second);
}

#[test]
fn p5_evidence_flips_selection() {
    // Declared guesses prefer fast; measured evidence prefers slow.
    let (_, mut doc) = doc();
    doc.method.push(invoke("via-fast", "root", "fast@1"));
    doc.method.push(invoke("via-slow", "root", "slow@1"));
    doc.capability.push(cap("fast@1", 4.0, 5.0, 6.0));
    doc.capability.push(cap("slow@1", 8.0, 10.0, 12.0));
    let before = pc_core::compile(&doc).unwrap();
    assert_eq!(before.totals["duration_minutes"].expected, 5.0);
    for v in [28.0, 30.0, 32.0] {
        doc.evidence
            .push(ev("capability:fast@1", v, "2026-10-08T00:00:00Z"));
    }
    for v in [9.0, 10.0, 11.0] {
        doc.evidence
            .push(ev("capability:slow@1", v, "2026-10-08T00:00:00Z"));
    }
    let after = pc_core::compile(&doc).unwrap();
    assert_eq!(after.totals["duration_minutes"].expected, 10.0);
    assert!(
        after
            .trace
            .iter()
            .any(|row| row.subject == "task:root" && row.reason == "uses method:via-slow"),
        "selection flipped: {:?}",
        after.trace
    );
    // The predicted column covers the selected plan's guesses: slow was
    // declared at 10 minutes, and evidence agrees.
    assert_eq!(after.basis["duration_minutes"].predicted.expected, 10.0);
    assert_eq!(after.basis["duration_minutes"].measured_nodes, 1);
    assert_eq!(after.provenance.len(), 2);
    let fast = after
        .provenance
        .iter()
        .find(|row| row.capability == "fast@1")
        .unwrap();
    assert!(!fast.selected);
    assert_eq!(fast.predicted.unwrap().expected, 5.0);
    assert_eq!(fast.measured.unwrap().likely, 30.0);
    assert!(fast.contradicts_declared);
    let slow = after
        .provenance
        .iter()
        .find(|row| row.capability == "slow@1")
        .unwrap();
    assert!(slow.selected);
    assert_eq!(slow.basis, "measured(3)");
}
