//! Persistence tests for first-class decision and observed-outcome events.

use pc_ir::{
    DecisionAlternative, DecisionEvent, EvaluationStatus, EventMetadata, OutcomeEvent,
    OutcomeSummary, Usefulness,
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
    let path = std::env::temp_dir().join(format!(
        "pc-store-events-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
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
