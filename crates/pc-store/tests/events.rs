//! Persistence tests for first-class decision and observed-outcome events.

use pc_ir::{
    CompilerRunEvent, DecisionAlternative, DecisionEvent, EvaluationStatus, EventMetadata,
    OutcomeEvent, OutcomeSummary, Usefulness,
};
use std::collections::BTreeMap;

fn metadata(id: &str) -> EventMetadata {
    EventMetadata {
        id: id.into(),
        timestamp: "2026-10-13T12:00:00Z".into(),
        plan_revision: "plan-r7".into(),
        source: "pc-cli".into(),
        schema_version: 1,
        tasks: vec!["render".into()],
        capabilities: vec![],
    }
}

fn summary(description: &str, value: f64) -> OutcomeSummary {
    OutcomeSummary {
        description: description.into(),
        metrics: BTreeMap::from([("duration_minutes".into(), value)]),
    }
}

fn directory() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "pc-store-events-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join("plan.toml"), "").unwrap();
    path
}

#[test]
fn prediction_and_observed_outcome_are_separate_append_only_logs() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    let decision = DecisionEvent {
        event: metadata("decision-1"),
        objective: "Improve throughput".into(),
        project_revision: "abc123".into(),
        alternatives: vec![
            DecisionAlternative {
                id: "batch".into(),
                description: "Batch rendering".into(),
            },
            DecisionAlternative {
                id: "current".into(),
                description: "Current implementation".into(),
            },
        ],
        selected_alternative: "batch".into(),
        initial_decision: Some("current".into()),
        compiler_recommendation: Some("batch".into()),
        decision_provenance: "planner accepted recommendation".into(),
        supporting_evidence: vec!["ev-1".into()],
        compiler_runs: vec!["run-1".into()],
        constraints: vec!["preserve fidelity".into()],
        predicted_outcome: summary("predicted", 20.0),
        baseline_alternative: Some("current".into()),
    };
    pc_store::append_decision(&plan, &decision).unwrap();
    assert!(
        pc_store::append_decision(&plan, &decision).is_err(),
        "duplicate IDs would make event references ambiguous"
    );
    assert_eq!(pc_store::read_decisions(&plan).unwrap(), vec![decision]);
    assert!(
        pc_store::read_outcomes(&plan).unwrap().is_empty(),
        "a prediction must not become an observed result"
    );

    let outcome = OutcomeEvent {
        event: metadata("outcome-1"),
        decision_id: "decision-1".into(),
        actual_outcome: summary("measured", 24.0),
        confidence: Some(0.9),
        evaluation_status: EvaluationStatus::Changed,
        usefulness: Some(Usefulness::Useful),
    };
    pc_store::append_outcome(&plan, &outcome).unwrap();
    assert_eq!(pc_store::read_outcomes(&plan).unwrap(), vec![outcome]);
    assert_eq!(
        std::fs::read_to_string(dir.join("decisions.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("outcomes.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn decisions_with_unresolvable_alternative_references_are_refused() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    let alternative = |id: &str| DecisionAlternative {
        id: id.into(),
        description: id.into(),
    };
    let valid = DecisionEvent {
        event: metadata("decision-1"),
        objective: "Improve throughput".into(),
        project_revision: "abc123".into(),
        alternatives: vec![alternative("batch"), alternative("current")],
        selected_alternative: "batch".into(),
        initial_decision: Some("current".into()),
        compiler_recommendation: Some("batch".into()),
        decision_provenance: "planner".into(),
        supporting_evidence: vec![],
        compiler_runs: vec![],
        constraints: vec![],
        predicted_outcome: summary("predicted", 20.0),
        baseline_alternative: Some("current".into()),
    };
    let invalid = [
        DecisionEvent {
            alternatives: vec![alternative("batch"), alternative("batch")],
            baseline_alternative: None,
            ..valid.clone()
        },
        DecisionEvent {
            compiler_recommendation: Some("typo".into()),
            ..valid.clone()
        },
        DecisionEvent {
            initial_decision: Some("typo".into()),
            ..valid.clone()
        },
        DecisionEvent {
            baseline_alternative: Some("typo".into()),
            ..valid.clone()
        },
        DecisionEvent {
            selected_alternative: "typo".into(),
            ..valid.clone()
        },
    ];
    for decision in invalid {
        assert!(pc_store::append_decision(&plan, &decision).is_err());
    }
    assert!(pc_store::read_decisions(&plan).unwrap().is_empty());
    pc_store::append_decision(&plan, &valid).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

fn compiler_run(id: &str, operation: &str) -> CompilerRunEvent {
    CompilerRunEvent {
        event: metadata(id),
        operation: operation.into(),
        plan_file: "plan.toml".into(),
        selection_objective: Some("effort".into()),
        selection_time_budget_ms: Some(2000),
        duration_ms: 12,
        validation_duration_ms: 3,
        memory_bytes: Some(1024),
        task_count: 2,
        method_count: 2,
        constraint_count: 1,
        alternatives_considered: 2,
        contract_rejections: 0,
        evidence_coverage: Some(0.5),
        estimate_uncertainty_minutes: Some(4.0),
        decisions_affected_by_evidence: Some(0),
        critical_path_tasks: Some(vec!["render".into()]),
        critical_path_changes: None,
        context_size_chars: None,
        selection_duration_ms: Some(5),
        optimality_status: Some("optimal".into()),
        optimality_gap_percent: None,
        frontier_duration_ms: None,
    }
}

/// A dropped metrics write would silently lose the compiler's own
/// performance record, so append/read must round-trip beside the
/// decision/outcome logs, and corrupt operations must be refused.
#[test]
fn compiler_runs_append_beside_decision_logs_and_refuse_unknown_operations() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    let run = compiler_run("run-1", "compile");
    pc_store::append_compiler_run(&plan, &run).unwrap();
    let mut unknown = compiler_run("run-2", "hallucinate");
    assert!(
        pc_store::append_compiler_run(&plan, &unknown).is_err(),
        "an unknown operation would corrupt the metrics log"
    );
    unknown.operation = "frontier".into();
    unknown.frontier_duration_ms = Some(2);
    unknown.event.id = "run-2".into();
    pc_store::append_compiler_run(&plan, &unknown).unwrap();
    let runs = pc_store::read_compiler_runs(&plan).unwrap();
    assert_eq!(runs, vec![run, unknown]);
    assert!(
        pc_store::read_decisions(&plan).unwrap().is_empty(),
        "metrics must not leak into the decision log"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// The metrics log grows on every read command, so one unparsable row (a
/// hand edit or a crash mid-write) must not stop every later recording.
#[test]
fn compiler_runs_keep_appending_after_an_unparsable_row() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    std::fs::write(dir.join("metrics.jsonl"), "{\"id\":\"run-0\", trunc\n").unwrap();
    let run = compiler_run("run-1", "check");
    let log = pc_store::append_compiler_run(&plan, &run).unwrap();
    let text = std::fs::read_to_string(log).unwrap();
    let last: CompilerRunEvent = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last, run);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A write cut short by a crash leaves a partial line with no newline; the
/// next row must start on its own line instead of being glued onto it.
#[test]
fn compiler_runs_start_a_new_line_after_a_partial_trailing_row() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    std::fs::write(dir.join("metrics.jsonl"), "{\"id\":\"partial").unwrap();
    let run = compiler_run("run-1", "compile");
    let log = pc_store::append_compiler_run(&plan, &run).unwrap();
    let text = std::fs::read_to_string(log).unwrap();
    let last: CompilerRunEvent = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last, run);
    assert_eq!(
        pc_store::previous_critical_path(&plan, "effort", 2000),
        Some(vec!["render".to_string()])
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// The previous critical path must come from the last row of the same plan
/// file and selection configuration that has one, even when rows span many
/// read chunks and later rows have none, are unparsable, or were selected
/// differently, so critical-path changes compare against the right run.
#[test]
fn previous_critical_path_reads_the_latest_row_with_one() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    assert_eq!(
        pc_store::previous_critical_path(&plan, "effort", 2000),
        None
    );
    let mut old = compiler_run("run-0", "compile");
    old.critical_path_tasks = Some(vec!["old".into()]);
    pc_store::append_compiler_run(&plan, &old).unwrap();
    let mut latest = compiler_run("run-1", "compile");
    latest.event.tasks = (0..5_000).map(|i| format!("task-{i}")).collect();
    latest.critical_path_tasks = Some(vec!["latest".into()]);
    pc_store::append_compiler_run(&plan, &latest).unwrap();
    for i in 2..2_000 {
        let mut check = compiler_run(&format!("run-{i}"), "check");
        check.critical_path_tasks = None;
        pc_store::append_compiler_run(&plan, &check).unwrap();
    }
    let mut makespan = compiler_run("run-makespan", "compile");
    makespan.selection_objective = Some("makespan".into());
    makespan.critical_path_tasks = Some(vec!["makespan".into()]);
    pc_store::append_compiler_run(&plan, &makespan).unwrap();
    let mut other_budget = compiler_run("run-budget", "frontier");
    other_budget.selection_time_budget_ms = Some(500);
    other_budget.critical_path_tasks = Some(vec!["budget".into()]);
    pc_store::append_compiler_run(&plan, &other_budget).unwrap();
    let mut other_plan = compiler_run("run-other-plan", "compile");
    other_plan.plan_file = "other.toml".into();
    other_plan.critical_path_tasks = Some(vec!["other".into()]);
    pc_store::append_compiler_run(&plan, &other_plan).unwrap();
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("metrics.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut log, b"{\"id\":\"run-x\", trunc\n").unwrap();
    assert_eq!(
        pc_store::previous_critical_path(&plan, "effort", 2000),
        Some(vec!["latest".to_string()])
    );
    assert_eq!(
        pc_store::previous_critical_path(&plan, "makespan", 2000),
        Some(vec!["makespan".to_string()])
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// A crash mid-write leaves a partial metrics row behind; evaluation and
/// history read the log lossily so one unparsable row never hides the valid
/// runs beside it.
#[test]
fn compiler_runs_skipping_invalid_returns_valid_rows_and_skip_count() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    assert_eq!(
        pc_store::read_compiler_runs_skipping_invalid(&plan).unwrap(),
        (Vec::new(), 0),
        "a missing metrics log reads as empty, like the strict reader"
    );
    pc_store::append_compiler_run(&plan, &compiler_run("run-1", "compile")).unwrap();
    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(dir.join("metrics.jsonl"))
        .unwrap();
    std::io::Write::write_all(&mut log, b"{\"id\":\"run-x\", trunc\n").unwrap();
    pc_store::append_compiler_run(&plan, &compiler_run("run-2", "check")).unwrap();
    let (runs, skipped) = pc_store::read_compiler_runs_skipping_invalid(&plan).unwrap();
    assert_eq!(
        runs.iter()
            .map(|run| run.event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["run-1", "run-2"]
    );
    assert_eq!(skipped, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Decisions recorded before `compiler_runs` existed carry no such key;
/// they must still parse as unattributed rather than failing the log.
#[test]
fn decisions_without_compiler_runs_parse_as_unattributed() {
    let dir = directory();
    let plan = dir.join("plan.toml");
    let legacy = serde_json::json!({
        "id":"decision-legacy", "timestamp":"2026-10-13T12:00:00Z", "plan_revision":"r1", "source":"test", "schema_version":1,
        "tasks":["render"], "capabilities":[], "objective":"Improve throughput", "project_revision":"abc",
        "alternatives":[{"id":"batch","description":"Batch"}], "selected_alternative":"batch",
        "decision_provenance":"planner", "supporting_evidence":[], "constraints":[],
        "predicted_outcome":{"description":"p","metrics":{}}, "baseline_alternative":null,
        "compiler_recommendation":null, "initial_decision":null
    });
    std::fs::write(
        dir.join("decisions.jsonl"),
        serde_json::to_string(&legacy).unwrap() + "\n",
    )
    .unwrap();
    let decisions = pc_store::read_decisions(&plan).unwrap();
    assert_eq!(decisions.len(), 1);
    assert!(decisions[0].compiler_runs.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
