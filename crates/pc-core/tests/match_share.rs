//! Acceptance tests for P3 (match) and P4 (share).
//!
//! P3 proposes derived invoke methods for open tasks whose contract is
//! covered by a capability, skipping tasks that promise nothing (vacuous
//! matches) and tasks already covered. P4 merges structurally identical
//! tasks (same title, state, contract and committed subtree) by content
//! hash. False-merge guards: the same title with different inputs must not
//! merge, different titles with the same contract must not merge (the 2.7.1
//! union trap), states must not mix, different decompositions must not
//! merge, and a merge that would create an order cycle is refused.

use pc_core::passes::check;
use pc_ir::{
    ArtifactDecl, Capability, CapabilityKind, ContractItem, Evidence, Method, MethodKind,
    MethodStatus, Objective, OrderEdge, PlanDoc, PlanMeta, Task, TaskState,
};

fn contract(artifact: &str, fields: &[&str]) -> ContractItem {
    ContractItem {
        artifact: artifact.to_string(),
        fields: fields.iter().map(ToString::to_string).collect(),
    }
}

fn base_doc() -> PlanDoc {
    PlanDoc {
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
            weights: Default::default(),
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
                produces: vec![contract("transcript", &["words"])],
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
            provenance: "test".into(),
            assumptions: vec![],
        }],
        capability: vec![],
        evidence: vec![],
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

fn cap(id: &str, produces: Vec<ContractItem>, consumes: Vec<ContractItem>) -> Capability {
    Capability {
        id: id.into(),
        kind: CapabilityKind::Tool,
        version: "1".into(),
        consumes,
        produces,
        estimates: vec![],
        resources: vec![],
    }
}

// ── P3: match ───────────────────────────────────────────────────────────

#[test]
fn p3_proposes_derived_method_for_matching_capability() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let derived: Vec<_> = doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Derived)
        .collect();
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].task, "leaf");
    assert_eq!(derived[0].capability.as_deref(), Some("whisperx@3"));
    assert_eq!(derived[0].provenance, "pass:match");
    assert_eq!(derived[0].kind, MethodKind::Invoke);
    assert!(
        report
            .trace
            .iter()
            .any(|e| e.pass == "P3-match" && e.subject == "task:leaf")
    );
}

#[test]
fn p3_skips_capability_that_does_not_cover_contract() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability
        .push(cap("partial@1", vec![contract("transcript", &[])], vec![]));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.method.iter().any(|m| m.status == MethodStatus::Derived));
}

#[test]
fn p3_skips_task_with_committed_method() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    doc.method.push(invoke("existing", "leaf", "whisperx@3"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.method.iter().any(|m| m.status == MethodStatus::Derived));
}

#[test]
fn p3_uses_external_inputs_to_satisfy_consumes() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task[1].consumes = vec![contract("transcript", &["words"])];
    doc.capability.push(cap(
        "voz@1",
        vec![contract("transcript", &["words"])],
        vec![contract("transcript", &["words"])],
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(!report.ok(), "should fail E009 without external inputs");
    let mut doc2 = base_doc();
    doc2.task[1].consumes = vec![contract("transcript", &["words"])];
    doc2.plan.external_inputs = vec![contract("transcript", &["words"])];
    doc2.capability.push(cap(
        "voz@1",
        vec![contract("transcript", &["words"])],
        vec![contract("transcript", &["words"])],
    ));
    let report2 = check("plan.toml", &text, &mut doc2, None);
    assert!(report2.ok(), "got {:?}", report2.diagnostics);
    assert!(
        doc2.method
            .iter()
            .any(|m| m.status == MethodStatus::Derived && m.task == "leaf")
    );
}

#[test]
fn p3_skips_task_that_promises_nothing() {
    // A task with no output fields is covered by every capability; proposing
    // for it would spam vacuous substitutions (e.g. proposing a transcriber
    // for a task that only consumes a transcript). Such tasks need
    // decomposition, not invocation.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task[1].consumes = vec![contract("transcript", &["words"])];
    doc.task[1].produces = vec![];
    doc.plan.external_inputs = vec![contract("transcript", &["words"])];
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.method.iter().any(|m| m.status == MethodStatus::Derived));
}

#[test]
fn p3_skips_task_with_live_invoke_for_same_capability() {
    // A proposed hypothesis for the same task and capability already covers
    // the substitution; P3 must not stack a derived duplicate on it.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    doc.method.push(Method {
        id: "hypothesis".to_string(),
        task: "leaf".to_string(),
        kind: MethodKind::Invoke,
        subtasks: vec![],
        order_edges: vec![],
        capability: Some("whisperx@3".to_string()),
        status: MethodStatus::Proposed,
        provenance: "llm:scout".into(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.method.iter().any(|m| m.status == MethodStatus::Derived));
}

#[test]
fn p3_skips_proposal_when_method_id_is_taken() {
    // Derived ids are deterministic (`match-<task>-<cap>`); on the rare
    // collision with an authored id the proposal is dropped, never duplicated.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    doc.method.push(Method {
        id: "match-leaf-whisperx@3".to_string(),
        task: "root".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["leaf".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(
        doc.method
            .iter()
            .filter(|m| m.id == "match-leaf-whisperx@3")
            .count(),
        1
    );
}

#[test]
fn p3_skips_non_open_tasks() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task[1].state = TaskState::Resolved;
    doc.capability.push(cap(
        "whisperx@3",
        vec![contract("transcript", &["words", "timings"])],
        vec![],
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.method.iter().any(|m| m.status == MethodStatus::Derived));
}

// ── P4: share ───────────────────────────────────────────────────────────

#[test]
fn p4_does_not_merge_different_titles_with_same_contract() {
    // The spike's naive-union trap (report 2.7.1): two clips' transcriptions
    // share a contract but are different work. Titles tell them apart.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "clip-1".to_string(),
        title: "Transcribe clip 1".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.task.push(Task {
        id: "clip-2".to_string(),
        title: "Transcribe clip 2".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method[0].subtasks = vec!["clip-1".to_string(), "clip-2".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 4, "distinct titles must not merge");
    assert!(doc.task.iter().any(|t| t.id == "clip-1"));
    assert!(doc.task.iter().any(|t| t.id == "clip-2"));
    assert!(
        !report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.decision == "merged")
    );
}

#[test]
fn p4_does_not_merge_across_states() {
    // Finished work and live work are never the same node, even when the
    // title and contract match: merging them would rewrite live references
    // onto a done task or vice versa.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-done".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Done,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 3);
    assert!(doc.task.iter().any(|t| t.id == "leaf-done"));
}

#[test]
fn p4_preserves_subtask_order_of_untouched_methods() {
    // Merging one pair must not reshuffle subtask order elsewhere: only
    // methods that reference a duplicate are rewritten.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.task.push(Task {
        id: "zeta".to_string(),
        title: "zeta".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["timings"])],
    });
    doc.method[0].subtasks = vec![
        "zeta".to_string(),
        "leaf".to_string(),
        "leaf-dup".to_string(),
    ];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(
        doc.method[0].subtasks,
        vec!["zeta".to_string(), "leaf".to_string()]
    );
}

#[test]
fn p4_merges_identical_tasks() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "leaf-dup".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 2, "duplicate should be merged");
    assert!(!doc.task.iter().any(|t| t.id == "leaf-dup"));
    assert_eq!(doc.method[0].subtasks, vec!["leaf".to_string()]);
    assert!(
        report.trace.iter().any(|e| e.pass == "P4-share"
            && e.subject == "task:leaf-dup"
            && e.decision == "merged")
    );
}

#[test]
fn p4_does_not_merge_same_title_different_contract() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words", "timings"])],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "leaf-dup".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 3, "different contracts must not merge");
    assert!(doc.task.iter().any(|t| t.id == "leaf-dup"));
}

#[test]
fn p4_does_not_merge_different_consumes() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.plan.external_inputs = vec![contract("transcript", &["timings"])];
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![contract("transcript", &["timings"])],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "leaf-dup".to_string()];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 3, "different consumes must not merge");
}

#[test]
fn p4_rewrites_objective_root() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "root-dup".to_string(),
        title: "root".to_string(),
        state: TaskState::Decomposed,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![],
    });
    doc.method.push(Method {
        id: "m2".to_string(),
        task: "root-dup".to_string(),
        kind: MethodKind::Decompose,
        subtasks: vec!["leaf".to_string()],
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    });
    doc.objective.root_task = "root-dup".to_string();
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 2);
    assert_eq!(doc.objective.root_task, "root");
}

#[test]
fn p4_rewrites_method_references() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method.push(invoke("extra", "leaf-dup", "other@1"));
    doc.capability.push(cap(
        "other@1",
        vec![contract("transcript", &["words"])],
        vec![],
    ));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 2);
    assert!(!doc.method.is_empty());
    for m in &doc.method {
        assert_ne!(m.task, "leaf-dup");
        if m.kind == MethodKind::Decompose {
            assert!(!m.subtasks.iter().any(|s| s == "leaf-dup"));
        }
    }
}

#[test]
fn p4_deduplicates_invoke_methods_after_merge() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.capability.push(cap(
        "shared@1",
        vec![contract("transcript", &["words"])],
        vec![],
    ));
    doc.method.push(invoke("m1", "leaf", "shared@1"));
    doc.method.push(invoke("m2", "leaf-dup", "shared@1"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 2);
    let shared_invocations: Vec<_> = doc
        .method
        .iter()
        .filter(|m| m.kind == MethodKind::Invoke && m.capability.as_deref() == Some("shared@1"))
        .collect();
    assert_eq!(
        shared_invocations.len(),
        1,
        "duplicate invoke methods should be deduplicated"
    );
}

#[test]
fn p4_merge_is_deterministic_regardless_of_input_order() {
    let text = "test".to_string();
    let mut doc_a = base_doc();
    doc_a.task.push(Task {
        id: "a".to_string(),
        title: "a".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc_a.task.push(Task {
        id: "b".to_string(),
        title: "b".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc_a.method[0].subtasks = vec!["a".to_string(), "b".to_string()];
    let mut doc_b = doc_a.clone();
    doc_b.task.reverse();
    doc_b.method.reverse();
    let report_a = check("plan.toml", &text, &mut doc_a, None);
    let report_b = check("plan.toml", &text, &mut doc_b, None);
    assert!(report_a.ok(), "got {:?}", report_a.diagnostics);
    assert!(report_b.ok(), "got {:?}", report_b.diagnostics);
    assert_eq!(doc_a.task.len(), doc_b.task.len());
    let json_a = serde_json::to_string(&doc_a).unwrap();
    let json_b = serde_json::to_string(&doc_b).unwrap();
    assert_eq!(json_a, json_b);
}

fn task(id: &str, title: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        title: title.into(),
        state,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![],
    }
}

fn decompose(id: &str, task: &str, subtasks: &[&str]) -> Method {
    Method {
        id: id.into(),
        task: task.into(),
        kind: MethodKind::Decompose,
        subtasks: subtasks.iter().map(ToString::to_string).collect(),
        order_edges: vec![],
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "test".into(),
        assumptions: vec![],
    }
}

fn edge(from: &str, to: &str) -> OrderEdge {
    OrderEdge {
        from: from.into(),
        to: to.into(),
    }
}

#[test]
fn p4_does_not_merge_same_title_with_different_subtrees() {
    // Two "QA" tasks decompose into different work: merging them would make
    // the decompositions alternatives and silently drop one subtree.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("qa-a", "QA", TaskState::Decomposed),
        task("qa-b", "QA", TaskState::Decomposed),
        task("x1", "Test login", TaskState::Open),
        task("x2", "Test billing", TaskState::Open),
    ];
    doc.method = vec![
        decompose("m", "root", &["qa-a", "qa-b"]),
        decompose("qa-a-m", "qa-a", &["x1"]),
        decompose("qa-b-m", "qa-b", &["x2"]),
    ];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 5, "different subtrees must not merge");
    assert_eq!(doc.method.len(), 3);
    assert!(
        !report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.decision == "merged")
    );
}

#[test]
fn p4_refuses_merge_that_would_create_order_cycle() {
    // a -> c -> b with a and b identical: merging gives a -> c -> a.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("a", "Review", TaskState::Open),
        task("b", "Review", TaskState::Open),
        task("c", "Fix", TaskState::Open),
    ];
    let mut m = decompose("m", "root", &["a", "b", "c"]);
    m.order_edges = vec![edge("a", "c"), edge("c", "b")];
    doc.method = vec![m];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 4, "cycle-creating merge must be refused");
    assert_eq!(
        doc.method[0].order_edges,
        vec![edge("a", "c"), edge("c", "b")]
    );
    assert!(
        report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.subject == "task:b" && e.decision == "kept")
    );
}

#[test]
fn p4_keeps_accepted_invoke_listed_after_rejected_duplicate() {
    // Without any merge P4 must not touch methods: an accepted invoke that
    // follows a rejected invoke of the same capability survives.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.capability.push(cap(
        "ing@1",
        vec![contract("transcript", &["words"])],
        vec![],
    ));
    let mut old = invoke("old", "leaf", "ing@1");
    old.status = MethodStatus::Rejected;
    doc.method.push(old);
    doc.method.push(invoke("new", "leaf", "ing@1"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let ids: Vec<&str> = doc.method.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["m", "old", "new"]);
}

#[test]
fn p4_dedup_after_merge_keeps_accepted_over_derived() {
    // leaf gets a derived proposal from P3; leaf-dup carries an accepted
    // invoke of the same capability. After the merge the accepted one wins.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "leaf-dup".to_string()];
    doc.capability.push(cap(
        "ing@1",
        vec![contract("transcript", &["words"])],
        vec![],
    ));
    doc.method.push(invoke("authored", "leaf-dup", "ing@1"));
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let invokes: Vec<&Method> = doc
        .method
        .iter()
        .filter(|m| m.kind == MethodKind::Invoke)
        .collect();
    assert_eq!(invokes.len(), 1);
    assert_eq!(invokes[0].id, "authored");
    assert_eq!(invokes[0].task, "leaf");
    assert_eq!(invokes[0].status, MethodStatus::Accepted);
    assert!(report.trace.iter().any(|e| e.pass == "P4-share"
        && e.subject == "method:match-leaf-ing@1"
        && e.decision == "deduplicated"));
}

#[test]
fn p4_rewrites_evidence_subjects_of_merged_tasks() {
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task.push(Task {
        id: "leaf-dup".to_string(),
        title: "leaf".to_string(),
        state: TaskState::Open,
        tasks_axi: None,
        beads_jsonl: None,
        consumes: vec![],
        produces: vec![contract("transcript", &["words"])],
    });
    doc.method[0].subtasks = vec!["leaf".to_string(), "leaf-dup".to_string()];
    doc.evidence.push(Evidence {
        subject: "task:leaf-dup".into(),
        metric: "duration_minutes".into(),
        value: 3.0,
        unit: "minutes".into(),
        time: "2026-10-01T00:00:00Z".into(),
        source: "test".into(),
        basis: None,
        artifact_hash: None,
    });
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert_eq!(doc.task.len(), 2);
    assert_eq!(doc.evidence[0].subject, "task:leaf");
}

#[test]
fn p4_refuses_merges_that_would_create_a_cycle_across_methods() {
    // p orders a -> c, q orders c2 -> a2. Merging both pairs keeps each
    // method acyclic but makes compile's global leaf graph cyclic, so only
    // the first group (a) merges and c2 is kept.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("p", "Ship web", TaskState::Decomposed),
        task("q", "Ship mobile", TaskState::Decomposed),
        task("a", "Build", TaskState::Open),
        task("a2", "Build", TaskState::Open),
        task("c", "Test", TaskState::Open),
        task("c2", "Test", TaskState::Open),
    ];
    let mut p = decompose("p-m", "p", &["a", "c"]);
    p.order_edges = vec![edge("a", "c")];
    let mut q = decompose("q-m", "q", &["c2", "a2"]);
    q.order_edges = vec![edge("c2", "a2")];
    doc.method = vec![
        decompose("m", "root", &["p", "q"]),
        p,
        q,
        invoke("a-run", "a", "build@1"),
        invoke("a2-run", "a2", "build@1"),
        invoke("c-run", "c", "test@1"),
        invoke("c2-run", "c2", "test@1"),
    ];
    doc.capability = vec![
        cap("build@1", vec![], vec![]),
        cap("test@1", vec![], vec![]),
    ];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    let has = |subject: &str, decision: &str| {
        report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.subject == subject && e.decision == decision)
    };
    assert!(has("task:a2", "merged"));
    assert!(has("task:c2", "kept"));
    assert_eq!(doc.task.len(), 6);
    pc_core::compile(&doc).expect("plan must still compile after P4");
}

#[test]
fn p4_merges_shared_task_under_ordered_parents() {
    // p -> q, p decomposes to [a], q to [a2]: one shared Build leaf is not a
    // cycle, so the merge happens and the work counts once.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("p", "Ship web", TaskState::Decomposed),
        task("q", "Ship mobile", TaskState::Decomposed),
        task("a", "Build", TaskState::Open),
        task("a2", "Build", TaskState::Open),
    ];
    let mut root = decompose("m", "root", &["p", "q"]);
    root.order_edges = vec![edge("p", "q")];
    doc.method = vec![
        root,
        decompose("p-m", "p", &["a"]),
        decompose("q-m", "q", &["a2"]),
        invoke("a-run", "a", "build@1"),
        invoke("a2-run", "a2", "build@1"),
    ];
    doc.capability = vec![cap("build@1", vec![], vec![])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(
        report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.subject == "task:a2" && e.decision == "merged")
    );
    assert!(!doc.task.iter().any(|t| t.id == "a2"));
    pc_core::compile(&doc).expect("shared plan must compile");
}

#[test]
fn p4_merges_task_ordered_before_parent_of_its_duplicate() {
    // [a, p] with a -> p and p decomposing to [a2]: compile drops the a -> a
    // leaf edge, so merging a2 into a is not a cycle.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("p", "Ship", TaskState::Decomposed),
        task("a", "Build", TaskState::Open),
        task("a2", "Build", TaskState::Open),
    ];
    let mut root = decompose("m", "root", &["a", "p"]);
    root.order_edges = vec![edge("a", "p")];
    doc.method = vec![
        root,
        decompose("p-m", "p", &["a2"]),
        invoke("a-run", "a", "build@1"),
        invoke("a2-run", "a2", "build@1"),
    ];
    doc.capability = vec![cap("build@1", vec![], vec![])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(!doc.task.iter().any(|t| t.id == "a2"));
    pc_core::compile(&doc).expect("shared plan must compile");
}

#[test]
fn p4_merges_shared_task_across_done_milestone() {
    // p -> x -> q with x a done milestone: compile relays order through the
    // leafless x, so the shared Build leaf is no cycle and must merge.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("p", "Ship web", TaskState::Decomposed),
        task("q", "Ship mobile", TaskState::Decomposed),
        task("x", "Milestone", TaskState::Done),
        task("a", "Build", TaskState::Open),
        task("a2", "Build", TaskState::Open),
    ];
    let mut root = decompose("m", "root", &["p", "x", "q"]);
    root.order_edges = vec![edge("p", "x"), edge("x", "q")];
    doc.method = vec![
        root,
        decompose("p-m", "p", &["a"]),
        decompose("q-m", "q", &["a2"]),
        invoke("a-run", "a", "build@1"),
        invoke("a2-run", "a2", "build@1"),
    ];
    doc.capability = vec![cap("build@1", vec![], vec![])];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    assert!(
        report
            .trace
            .iter()
            .any(|e| e.pass == "P4-share" && e.subject == "task:a2" && e.decision == "merged")
    );
    pc_core::compile(&doc).expect("shared plan must compile");
}

#[test]
fn p4_refuses_merge_into_cycle_that_alternatives_already_reach() {
    // t orders a -> c in m1 and c -> a in m2, so the all-selection leaf graph
    // already cycles a and c without a compile cycle. u orders c2 -> a2:
    // merging both pairs would make selecting m1 fail, so both are kept.
    let text = "test".to_string();
    let mut doc = base_doc();
    doc.task = vec![
        task("root", "root", TaskState::Decomposed),
        task("t", "Ship web", TaskState::Decomposed),
        task("u", "Ship mobile", TaskState::Decomposed),
        task("a", "Build", TaskState::Open),
        task("a2", "Build", TaskState::Open),
        task("c", "Test", TaskState::Open),
        task("c2", "Test", TaskState::Open),
    ];
    let mut m1 = decompose("m1", "t", &["a", "c"]);
    m1.order_edges = vec![edge("a", "c")];
    let mut m2 = decompose("m2", "t", &["a", "c"]);
    m2.order_edges = vec![edge("c", "a")];
    let mut u = decompose("u-m", "u", &["c2", "a2"]);
    u.order_edges = vec![edge("c2", "a2")];
    doc.method = vec![
        decompose("m", "root", &["t", "u"]),
        m1,
        m2,
        u,
        invoke("a-run", "a", "build@1"),
        invoke("a2-run", "a2", "build@1"),
        invoke("c-run", "c", "test@1"),
        invoke("c2-run", "c2", "test@1"),
    ];
    doc.capability = vec![
        cap("build@1", vec![], vec![]),
        cap("test@1", vec![], vec![]),
    ];
    let report = check("plan.toml", &text, &mut doc, None);
    assert!(report.ok(), "got {:?}", report.diagnostics);
    for dup in ["task:a2", "task:c2"] {
        assert!(
            report
                .trace
                .iter()
                .any(|e| e.pass == "P4-share" && e.subject == dup && e.decision == "kept"),
            "{dup} should be kept"
        );
    }
    assert_eq!(doc.task.len(), 7);
}
