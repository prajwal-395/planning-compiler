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
        artifact: vec![],
        resource: vec![],
        task: vec![Task {
            id: "root".to_string(),
            title: "root".to_string(),
            state: TaskState::Resolved,
            tasks_axi: None,
            beads_jsonl: None,
            consumes: vec![],
            produces: vec![],
        }],
        method: vec![],
        capability: vec![],
        evidence: vec![],
        investigation: vec![],
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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

fn proposed_invoke(id: &str, task: &str, capability: &str) -> Method {
    Method {
        id: id.into(),
        task: task.into(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some(capability.into()),
        status: MethodStatus::Proposed,
        provenance: "agent:test".into(),
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
        basis: None,
        artifact_hash: None,
    }
}

fn ev_basis(subject: &str, metric: &str, value: f64, basis: Option<&str>) -> Evidence {
    Evidence {
        subject: subject.into(),
        metric: metric.into(),
        value,
        unit: if metric == "duration_minutes" {
            "minutes"
        } else {
            "ratio"
        }
        .into(),
        time: "2026-10-01T00:00:00Z".into(),
        source: "test".into(),
        basis: basis.map(str::to_string),
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
        tasks_axi: None,
        beads_jsonl: None,
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
        tasks_axi: None,
        beads_jsonl: None,
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
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
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
        provider: None,
        source: None,
        platform_requirements: vec![],
        licensing_constraints: vec![],
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
    // nothing; an unknown subject stages pending evidence instead.
    doc.evidence
        .push(ev("method:r", 1.0, "2026-10-01T00:00:00Z"));
    doc.evidence
        .push(ev("capability:ghost@9", 1.0, "2026-10-01T00:00:00Z"));
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.unattributed, 1);
    assert!(estimated.profiles.is_empty());
    assert_eq!(estimated.pending.len(), 1);
    assert_eq!(estimated.pending[0].subject, "capability:ghost@9");
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

fn hypothesis_doc() -> (String, PlanDoc) {
    // root is served by the accepted incumbent; the challenger arrives as
    // a proposal with evidence already recorded against it (the G1 flow:
    // evidence staged together with the hypothesis). The challenger's
    // declared guesses are worse than the incumbent's, so only live
    // evidence can move the decision to it.
    let (text, mut doc) = doc();
    doc.method.push(invoke("via-old", "root", "old@1"));
    doc.method.push(proposed_invoke("via-new", "root", "new@1"));
    doc.capability.push(cap("old@1", 4.0, 5.0, 6.0));
    doc.capability.push(cap("new@1", 8.0, 10.0, 12.0));
    doc.evidence
        .push(ev("capability:new@1", 2.0, "2026-10-01T00:00:00Z"));
    (text, doc)
}

#[test]
fn g1_evidence_staged_with_a_proposal_never_moves_accepted_numbers() {
    let (text, mut doc) = hypothesis_doc();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    // Staged, attributed to the proposal: not an error, not effective.
    assert_eq!(estimated.staged.len(), 1);
    let staged = &estimated.staged[0];
    assert_eq!(staged.capability, "new@1");
    assert_eq!(staged.metric, "duration_minutes");
    assert_eq!(staged.basis, None);
    assert_eq!(staged.observations, 1);
    assert_eq!(staged.methods, vec!["via-new".to_string()]);
    assert!(estimated.pending.is_empty());
    assert!(
        estimated
            .profiles
            .iter()
            .filter(|p| p.capability == "new@1")
            .all(|p| p.measured.is_none()),
        "staged records fold into no profile: {:?}",
        estimated.profiles
    );
    assert!(
        report.trace.iter().any(|e| e.pass == "P5-estimate"
            && e.subject == "capability:new@1"
            && e.decision == "staged"),
        "staging is traced: {:?}",
        report.trace
    );
    // The accepted plan still compiles on guesses alone: the incumbent
    // holds at its declared 5 minutes.
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.totals["duration_minutes"].expected, 5.0);
    assert_eq!(compiled.staged_evidence.len(), 1);
    assert!(compiled.pending_evidence.is_empty());
}

#[test]
fn g1_accept_makes_staged_evidence_live() {
    let (_, mut doc) = hypothesis_doc();
    doc.method
        .iter_mut()
        .find(|m| m.id == "via-new")
        .unwrap()
        .status = MethodStatus::Accepted;
    let text = pc_ir::to_toml(&doc).unwrap();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert!(estimated.staged.is_empty());
    assert!(estimated.pending.is_empty());
    // Declared alone would pick the challenger at 10 minutes; the staged
    // record going live pulls it to the measured 2 minutes.
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.totals["duration_minutes"].expected, 2.0);
    assert_eq!(compiled.selection["root"], "via-new");
}

#[test]
fn g1_reject_keeps_evidence_staged() {
    let (_, mut doc) = hypothesis_doc();
    doc.method
        .iter_mut()
        .find(|m| m.id == "via-new")
        .unwrap()
        .status = MethodStatus::Rejected;
    let text = pc_ir::to_toml(&doc).unwrap();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    // The hypothesis is dead but its records stay staged: no accepted
    // method invokes the capability, so nothing moves, and there is no
    // live proposal left to attribute them to.
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.staged.len(), 1);
    assert!(estimated.staged[0].methods.is_empty());
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.totals["duration_minutes"].expected, 5.0);
}

#[test]
fn g1_method_subject_evidence_follows_method_status() {
    let (text, mut doc) = doc();
    doc.method.push(proposed_invoke("via-new", "root", "new@1"));
    doc.capability.push(cap("new@1", 8.0, 10.0, 12.0));
    doc.evidence
        .push(ev("method:via-new", 2.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(pc_core::estimate(&doc).staged.len(), 1);
    doc.method
        .iter_mut()
        .find(|m| m.id == "via-new")
        .unwrap()
        .status = MethodStatus::Accepted;
    let text = pc_ir::to_toml(&doc).unwrap();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert!(estimated.staged.is_empty());
    assert!(estimated.profiles.iter().any(|p| p.measured.is_some()));
}

#[test]
fn g1_method_evidence_goes_live_with_any_accepted_method_for_its_capability() {
    // Two proposals invoke new@1; evidence names via-p1. Rejecting via-p1
    // keeps it staged with the remaining proposal, and accepting via-p2
    // makes it live: it measures the capability, not the proposal.
    let (text, mut doc) = doc();
    doc.method.push(invoke("via-old", "root", "old@1"));
    doc.method.push(proposed_invoke("via-p1", "root", "new@1"));
    doc.method.push(proposed_invoke("via-p2", "root", "new@1"));
    doc.capability.push(cap("old@1", 4.0, 5.0, 6.0));
    doc.capability.push(cap("new@1", 8.0, 10.0, 12.0));
    doc.evidence
        .push(ev("method:via-p1", 2.0, "2026-10-01T00:00:00Z"));
    let decide = |doc: &mut PlanDoc, id: &str, status: MethodStatus| {
        doc.method.iter_mut().find(|m| m.id == id).unwrap().status = status;
        let text = pc_ir::to_toml(doc).unwrap();
        let report = check("plan.toml", &text, doc, None);
        assert!(report.ok(), "got {:?}", report.diagnostics);
    };
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    decide(&mut doc, "via-p1", MethodStatus::Rejected);
    let staged = pc_core::estimate(&doc).staged;
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].methods, ["via-p2"]);
    decide(&mut doc, "via-p2", MethodStatus::Accepted);
    let estimated = pc_core::estimate(&doc);
    assert!(estimated.staged.is_empty(), "{:?}", estimated.staged);
    assert!(
        estimated
            .profiles
            .iter()
            .any(|p| p.capability == "new@1" && p.measured.is_some())
    );
}

#[test]
fn g1_pending_evidence_activates_on_propose_then_accept() {
    // The retrospective flow: the observation predates the capability.
    let (text, mut doc) = doc();
    doc.method.push(invoke("via-old", "root", "old@1"));
    doc.capability.push(cap("old@1", 4.0, 5.0, 6.0));
    doc.evidence
        .push(ev("capability:new@1", 2.0, "2026-10-01T00:00:00Z"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(pc_core::estimate(&doc).pending.len(), 1);
    // The hypothesis arrives: capability plus a proposed method.
    doc.capability.push(cap("new@1", 8.0, 10.0, 12.0));
    doc.method.push(proposed_invoke("via-new", "root", "new@1"));
    let text = pc_ir::to_toml(&doc).unwrap();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert!(estimated.pending.is_empty());
    assert_eq!(estimated.staged.len(), 1);
    // The decision lands: the record goes live with no record movement.
    doc.method
        .iter_mut()
        .find(|m| m.id == "via-new")
        .unwrap()
        .status = MethodStatus::Accepted;
    let text = pc_ir::to_toml(&doc).unwrap();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.totals["duration_minutes"].expected, 2.0);
}

#[test]
fn g4_one_subject_and_metric_carries_one_basis() {
    // Same subject and metric under two bases is refused, never resolved:
    // a basis-less legacy record and a typed one conflict too.
    for (first, second) in [
        (None, Some("distance-from:old@1")),
        (Some("absolute"), Some("distance-from:old@1")),
        (Some("distance-from:old@1"), None),
    ] {
        let (text, mut doc) = doc();
        doc.method.push(invoke("m", "root", "c@1"));
        doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
        for basis in [first, first, second] {
            doc.evidence
                .push(ev_basis("capability:c@1", "duration_minutes", 10.0, basis));
        }
        let report = check("plan.toml", &text, &mut doc, None);
        let messages: Vec<&str> = report
            .diagnostics
            .iter()
            .filter(|d| d.code == "E014")
            .map(|d| d.message.as_str())
            .collect();
        let name = |b: Option<&str>| b.unwrap_or("none").to_string();
        assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
        assert!(
            messages[0].contains(&format!("`{}`", name(first)))
                && messages[0].contains(&format!("`{}`", name(second)))
                && messages[0].contains("distinct metric"),
            "{}",
            messages[0]
        );
    }
    // Different metrics on one subject may use different bases.
    let (text, mut doc) = doc();
    doc.method.push(invoke("m", "root", "c@1"));
    doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    doc.evidence
        .push(ev_basis("capability:c@1", "duration_minutes", 10.0, None));
    doc.evidence.push(ev_basis(
        "capability:c@1",
        "accuracy_distance_from_incumbent",
        0.1,
        Some("distance-from:old@1"),
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
}

/// E014 messages from a check report.
fn e014(report: &pc_core::passes::Report) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code == "E014")
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn g4_attributed_family_carries_one_basis_across_subjects() {
    // R2-1: b@1 declares no duration. Every record says about 30 minutes,
    // but `capability:b@1` is basis-less and `method:via-b` is absolute.
    // The family is refused, never dropped from selection or costed at
    // zero (which would silently select via-b over the 10-minute via-a).
    let (text, mut doc) = doc();
    doc.method.push(invoke("via-a", "root", "a@1"));
    doc.method.push(invoke("via-b", "root", "b@1"));
    doc.capability.push(cap("a@1", 10.0, 10.0, 10.0));
    let mut b = cap("b@1", 0.0, 0.0, 0.0);
    b.estimates.clear();
    doc.capability.push(b);
    for v in [30.0, 31.0, 32.0] {
        doc.evidence
            .push(ev_basis("capability:b@1", "duration_minutes", v, None));
    }
    let mut mixed = doc.clone();
    mixed.evidence.push(ev_basis(
        "method:via-b",
        "duration_minutes",
        30.0,
        Some("absolute"),
    ));
    let report = check("plan.toml", &text, &mut mixed, None);
    let messages = e014(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].contains("`capability:b@1`")
            && messages[0].contains("`absolute` (method:via-b)")
            && messages[0].contains("`none` (capability:b@1)")
            && messages[0].contains("distinct metric"),
        "{}",
        messages[0]
    );
    // One basis: the family is measured and via-a stays selected.
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let compiled = pc_core::compile(&doc).unwrap();
    assert_eq!(compiled.selection["root"], "via-a");
    assert_eq!(compiled.totals["duration_minutes"].expected, 10.0);
}

#[test]
fn g4_staged_family_carries_one_basis_across_subjects() {
    // Staged records join the same family: accepting the proposal could
    // otherwise make a mix live.
    let (text, mut doc) = doc();
    doc.method.push(invoke("via-old", "root", "old@1"));
    doc.method.push(proposed_invoke("via-new", "root", "new@1"));
    doc.capability.push(cap("old@1", 4.0, 5.0, 6.0));
    doc.capability.push(cap("new@1", 8.0, 10.0, 12.0));
    doc.evidence
        .push(ev_basis("capability:new@1", "duration_minutes", 2.0, None));
    doc.evidence.push(ev_basis(
        "method:via-new",
        "duration_minutes",
        3.0,
        Some("distance-from:old@1"),
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    let messages = e014(&report);
    assert_eq!(messages.len(), 1, "{:?}", report.diagnostics);
    assert!(
        messages[0].contains("`distance-from:old@1` (method:via-new)")
            && messages[0].contains("`none` (capability:new@1)"),
        "{}",
        messages[0]
    );
}

#[test]
fn g4_evidence_without_basis_behaves_as_before() {
    let (text, mut doc) = doc();
    doc.method.push(invoke("m", "root", "c@1"));
    doc.capability.push(cap("c@1", 4.0, 5.0, 6.0));
    for v in [10.0, 12.0] {
        doc.evidence
            .push(ev_basis("capability:c@1", "duration_minutes", v, None));
    }
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let estimated = pc_core::estimate(&doc);
    assert_eq!(estimated.profiles.len(), 1);
    let profile = &estimated.profiles[0];
    assert_eq!(profile.basis, None);
    assert!(estimated.basis_conflicts.is_empty());
}
