//! Defect-focused regression tests for source reconciliation.
use pc_ir::{Task, TaskState};
use pc_sync::{
    CiConclusion, Mapping, PlannerRecord, Provenance, Record, Source, State, content_revision,
    parse_ci_snapshot, parse_planner_snapshot, parse_snapshot, planner_revision, reconcile,
    reconcile_ci, reconcile_planner,
};

const TASK: &str = "post-mvp-01-tasks-axi";

fn plan(state: TaskState) -> pc_ir::PlanDoc {
    plan_task("post-mvp-01-tasks-axi", state)
}

fn plan_task(id: &str, state: TaskState) -> pc_ir::PlanDoc {
    let mut doc: pc_ir::PlanDoc =
        pc_ir::from_toml(include_str!("../../../examples/self-plan/plan.toml")).unwrap();
    doc.task.iter_mut().find(|t| t.id == id).unwrap().state = state;
    doc
}

fn task<'a>(doc: &'a pc_ir::PlanDoc, id: &str) -> &'a Task {
    doc.task.iter().find(|t| t.id == id).unwrap()
}

fn mapping(source: Source, task_id: &str) -> Vec<Mapping> {
    vec![Mapping {
        source,
        external_id: "external-1".into(),
        task_id: task_id.into(),
    }]
}

fn record(revision: &str, state: TaskState) -> Record {
    Record {
        id: "external-1".into(),
        revision: revision.into(),
        state,
        verified: false,
    }
}

#[test]
fn github_pull_request_state_never_completes_a_mapped_task() {
    let rows = parse_snapshot(
        Source::Github,
        include_str!("../../../tests/fixtures/github/issues.json"),
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "I_kwDOExample1");
    assert_eq!(rows[0].state, TaskState::Done);
    assert_eq!(rows[1].id, "PR_kwDOExample2");
    let mut doc = plan(TaskState::Open);
    let mappings = vec![Mapping {
        source: Source::Github,
        external_id: rows[1].id.clone(),
        task_id: TASK.into(),
    }];
    let mut state = State::default();
    let preview = reconcile(&mut doc, &mappings, &mut state, Source::Github, &rows[1..]).unwrap();
    assert_eq!(preview.conflicts.len(), 1);
    assert!(
        preview.conflicts[0]
            .reason
            .contains("not acceptance-criteria verification")
    );
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
}

#[test]
fn github_closure_agreeing_with_local_done_is_unchanged_and_idempotent() {
    let mut doc = plan(TaskState::Done);
    let mappings = mapping(Source::Github, TASK);
    let mut state = State::default();
    let closed = [record("2026-10-12T12:00:00Z", TaskState::Done)];
    for _ in 0..2 {
        let preview = reconcile(&mut doc, &mappings, &mut state, Source::Github, &closed).unwrap();
        assert!(preview.conflicts.is_empty(), "{:?}", preview.conflicts);
        assert_eq!(preview.unchanged, vec![TASK]);
    }
    assert_eq!(
        state.observed["Github:external-1"].revision,
        "2026-10-12T12:00:00Z"
    );
}

#[test]
fn unmapped_records_are_counted_separately_from_conflicts() {
    let mut doc = plan(TaskState::Open);
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    let mut other = record("r1", TaskState::Done);
    other.id = "other".into();
    let preview = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::TasksAxi,
        &[record("r1", TaskState::Open), other],
    )
    .unwrap();
    assert!(preview.conflicts.is_empty());
    assert_eq!(preview.unmapped, vec!["other"]);
    assert_eq!(preview.unchanged, vec![TASK]);
}

#[test]
fn first_observation_conflicts_and_never_overwrites_local_state() {
    let mut doc = plan(TaskState::Open);
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    let result = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::TasksAxi,
        &[record("r1", TaskState::Done)],
    )
    .unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert!(
        state.observed.is_empty(),
        "a conflict must remain retryable after local resolution"
    );
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
}

#[test]
fn later_record_for_the_same_task_sees_an_earlier_applied_change() {
    let mut doc = plan(TaskState::Open);
    let mut mappings = mapping(Source::TasksAxi, TASK);
    mappings.push(Mapping {
        source: Source::TasksAxi,
        external_id: "external-2".into(),
        task_id: TASK.into(),
    });
    let mut state = State::default();
    let second = |revision: &str, state: TaskState| Record {
        id: "external-2".into(),
        ..record(revision, state)
    };
    reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::TasksAxi,
        &[record("r1", TaskState::Open), second("s1", TaskState::Open)],
    )
    .unwrap();
    let result = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::TasksAxi,
        &[record("r2", TaskState::Done), second("s1", TaskState::Open)],
    )
    .unwrap();
    assert_eq!(result.changed, vec![TASK]);
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].external_id, "external-2");
}

#[test]
fn later_revision_applies_with_provenance_and_preserves_external_fields() {
    let mut doc = plan(TaskState::Open);
    let before = task(&doc, TASK).clone();
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::TasksAxi,
        &[record("r1", TaskState::Open)],
    )
    .unwrap();
    let next = [record("r2", TaskState::Done)];
    let result = reconcile(&mut doc, &mappings, &mut state, Source::TasksAxi, &next).unwrap();
    assert_eq!(result.changed, vec![TASK]);
    assert_eq!(
        task(&doc, TASK),
        &Task {
            state: TaskState::Done,
            ..before
        }
    );
    assert_eq!(
        state.provenance,
        vec![Provenance {
            source: Source::TasksAxi,
            external_id: "external-1".into(),
            revision: "r2".into(),
            task_id: TASK.into(),
            from: TaskState::Open,
            to: TaskState::Done,
        }]
    );
    let replay = reconcile(&mut doc, &mappings, &mut state, Source::TasksAxi, &next).unwrap();
    assert_eq!(replay.unchanged, vec![TASK]);
    assert_eq!(state.provenance.len(), 1);
}

#[test]
fn reverted_content_hash_revision_reopens_the_task() {
    let mut doc = plan(TaskState::Open);
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    for (revision, lifecycle) in [
        ("h-open", TaskState::Open),
        ("h-done", TaskState::Done),
        ("h-open", TaskState::Open),
    ] {
        let result = reconcile(
            &mut doc,
            &mappings,
            &mut state,
            Source::TasksAxi,
            &[record(revision, lifecycle)],
        )
        .unwrap();
        assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
        assert_eq!(task(&doc, TASK).state, lifecycle);
    }
}

#[test]
fn local_change_after_observed_revision_is_a_conflict() {
    let mut doc = plan(TaskState::Open);
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    let same = [record("r1", TaskState::Open)];
    reconcile(&mut doc, &mappings, &mut state, Source::TasksAxi, &same).unwrap();
    doc.task.iter_mut().find(|t| t.id == TASK).unwrap().state = TaskState::Done;
    let result = reconcile(&mut doc, &mappings, &mut state, Source::TasksAxi, &same).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(task(&doc, TASK).state, TaskState::Done);
}

#[test]
fn same_state_revision_is_recorded_so_reruns_are_idempotent() {
    let mut doc = plan(TaskState::Open);
    let mappings = mapping(Source::TasksAxi, TASK);
    let mut state = State::default();
    for revision in ["r1", "r2", "r2"] {
        let result = reconcile(
            &mut doc,
            &mappings,
            &mut state,
            Source::TasksAxi,
            &[record(revision, TaskState::Open)],
        )
        .unwrap();
        assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
        assert_eq!(result.unchanged, vec![TASK]);
    }
    assert_eq!(state.observed["TasksAxi:external-1"].revision, "r2");
}

#[test]
fn decomposed_task_matches_external_open_and_reopens_as_decomposed() {
    let mut doc = plan_task("post-mvp", TaskState::Decomposed);
    let mappings = mapping(Source::TasksAxi, "post-mvp");
    let mut state = State::default();
    for (revision, external, local) in [
        ("r1", TaskState::Open, TaskState::Decomposed),
        ("r2", TaskState::Done, TaskState::Done),
        ("r3", TaskState::Open, TaskState::Decomposed),
    ] {
        let result = reconcile(
            &mut doc,
            &mappings,
            &mut state,
            Source::TasksAxi,
            &[record(revision, external)],
        )
        .unwrap();
        assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
        assert_eq!(task(&doc, "post-mvp").state, local);
    }
}

// Defect: live `gh run list` output rejected, forcing hand-transcribed
// snapshots that drift from what CI actually reported.
#[test]
fn ci_gh_alias_keys_parse_to_the_same_record() {
    let canonical = parse_ci_snapshot(
        r#"[{"id": "7", "revision": "2026-10-13T12:00:00Z", "conclusion": "success",
              "duration_seconds": 60}]"#,
    )
    .unwrap();
    let gh = parse_ci_snapshot(
        r#"[{"databaseId": 7, "updatedAt": "2026-10-13T12:00:00Z", "conclusion": "success",
              "duration_seconds": 60}]"#,
    )
    .unwrap();
    assert_eq!(canonical, gh);
    assert_eq!(gh[0].conclusion, CiConclusion::Success);
}

// Defect: an unknown conclusion silently treated as pass or fail, inventing
// verification that never happened.
#[test]
fn ci_unknown_conclusion_fails_instead_of_silently_verifying() {
    let err = parse_ci_snapshot(
        r#"[{"id": "7", "revision": "2026-10-13T12:00:00Z", "conclusion": "mystery"}]"#,
    )
    .unwrap_err();
    assert!(err.contains("unknown conclusion"), "{err}");
}

// Defect: a negative runtime becoming a negative evidence value, which
// `pc check` would refuse with E014 after sync already appended it.
#[test]
fn ci_negative_duration_fails_before_it_can_poison_evidence() {
    let err = parse_ci_snapshot(
        r#"[{"id": "7", "revision": "2026-10-13T12:00:00Z", "conclusion": "success",
              "duration_seconds": -5}]"#,
    )
    .unwrap_err();
    assert!(err.contains("negative duration"), "{err}");
}

fn ci_mapping() -> Vec<Mapping> {
    vec![Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: TASK.into(),
    }]
}

fn ci_record(revision: &str, conclusion: &str) -> pc_sync::CiRecord {
    parse_ci_snapshot(&format!(
        r#"[{{"id": "ci-1", "revision": "{revision}", "conclusion": "{conclusion}",
              "duration_seconds": 120}}]"#
    ))
    .unwrap()
    .pop()
    .unwrap()
}

// Defect: CI editing task state directly, or passing without leaving the
// verification evidence a later GitHub closure must derive completion from.
#[test]
fn ci_success_records_verification_without_editing_task_state() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let (preview, evidence) = reconcile_ci(
        &doc,
        &ci_mapping(),
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap();
    assert!(preview.conflicts.is_empty());
    assert_eq!(preview.changed, vec![TASK]);
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
    assert_eq!(evidence.len(), 2);
    let runtime = &evidence[0];
    assert_eq!(runtime.subject, format!("task:{TASK}"));
    assert_eq!(runtime.metric, "duration_minutes");
    assert_eq!(runtime.value, 2.0);
    assert_eq!(runtime.unit, "minutes");
    assert_eq!(runtime.time, "2026-10-13T12:00:00Z");
    assert_eq!(runtime.source, "ci:ci-1");
    let pass = &evidence[1];
    assert_eq!(pass.metric, "failure_risk");
    assert_eq!(pass.value, 0.0);
    assert_eq!(pass.unit, "ratio");
    assert_eq!(state.observed["Ci:ci-1"].state, TaskState::Done);
}

// Defect: reruns duplicating verification evidence for the same check
// revision, double-counting one CI run.
#[test]
fn ci_success_replay_is_idempotent_and_records_no_duplicate_evidence() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let rows = [ci_record("2026-10-13T12:00:00Z", "success")];
    let (first, first_evidence) = reconcile_ci(&doc, &ci_mapping(), &mut state, &rows).unwrap();
    assert_eq!(first.changed, vec![TASK]);
    assert_eq!(first_evidence.len(), 2);
    let (second, second_evidence) = reconcile_ci(&doc, &ci_mapping(), &mut state, &rows).unwrap();
    assert_eq!(second.unchanged, vec![TASK]);
    assert!(second_evidence.is_empty());
    assert_eq!(state.observed["Ci:ci-1"].revision, "2026-10-13T12:00:00Z");
}

// Defect: a still-running check either recording empty verification or never
// being observed, so its later success cannot apply.
#[test]
fn ci_pending_observes_without_recording_evidence() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let (preview, evidence) = reconcile_ci(
        &doc,
        &ci_mapping(),
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "")],
    )
    .unwrap();
    assert!(preview.conflicts.is_empty());
    assert_eq!(preview.unchanged, vec![TASK]);
    assert!(evidence.is_empty());
    assert_eq!(state.observed["Ci:ci-1"].state, TaskState::Open);
    let (next, next_evidence) = reconcile_ci(
        &doc,
        &ci_mapping(),
        &mut state,
        &[ci_record("2026-10-13T12:01:00Z", "success")],
    )
    .unwrap();
    assert_eq!(next.changed, vec![TASK]);
    assert_eq!(next_evidence.len(), 2);
}

// Defect: a failed check either conflicting noisily on an unfinished task or
// vanishing without leaving failure evidence.
#[test]
fn ci_failure_on_open_task_records_failure_evidence_without_conflict() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let (preview, evidence) = reconcile_ci(
        &doc,
        &ci_mapping(),
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "failure")],
    )
    .unwrap();
    assert!(preview.conflicts.is_empty());
    assert_eq!(preview.changed, vec![TASK]);
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
    assert!(
        evidence
            .iter()
            .any(|e| e.metric == "failure_risk" && e.value == 1.0)
    );
    assert_eq!(state.observed["Ci:ci-1"].state, TaskState::Open);
}

// Defect: a failing check silently reopening a completed task, re-appending
// its failure evidence on every rerun, or leaving an earlier pass of the same
// check counted as verification.
#[test]
fn ci_failure_after_local_done_is_a_conflict_preserving_completion() {
    let mut doc = plan(TaskState::Done);
    let mut state = State::default();
    let (passed, pass_evidence) = reconcile_ci(
        &doc,
        &ci_mapping(),
        &mut state,
        &[ci_record("2026-10-13T11:00:00Z", "success")],
    )
    .unwrap();
    assert_eq!(passed.changed, vec![TASK]);
    doc.evidence.extend(pass_evidence);
    let rows = [ci_record("2026-10-13T12:00:00Z", "failure")];
    let (preview, evidence) = reconcile_ci(&doc, &ci_mapping(), &mut state, &rows).unwrap();
    assert_eq!(preview.conflicts.len(), 1);
    assert!(preview.conflicts[0].reason.contains("concluded `failure`"));
    assert_eq!(task(&doc, TASK).state, TaskState::Done);
    assert!(
        evidence
            .iter()
            .any(|e| e.metric == "failure_risk" && e.value == 1.0),
        "failure evidence still stages the signal"
    );
    assert_eq!(
        state.observed["Ci:ci-1"].state,
        TaskState::Open,
        "a failed rerun must replace the earlier pass"
    );
    doc.evidence.extend(evidence.into_iter().map(|mut e| {
        e.value = f64::from_bits(e.value.to_bits() + 1);
        e
    }));
    let (replay, replay_evidence) = reconcile_ci(&doc, &ci_mapping(), &mut state, &rows).unwrap();
    assert_eq!(replay.conflicts.len(), 1, "the conflict is reported again");
    assert!(replay_evidence.is_empty(), "{replay_evidence:?}");
}

// Defect: GitHub conclusions such as `skipped` failing the whole snapshot,
// or a skipped check observing as pending and blocking its task forever.
#[test]
fn ci_skipped_and_neutral_neither_verify_nor_block() {
    let rows = parse_ci_snapshot(
        r#"[{"id": "ci-1", "revision": "2026-10-13T12:00:00Z", "conclusion": "skipped"},
            {"id": "ci-2", "revision": "2026-10-13T12:00:00Z", "conclusion": "neutral"},
            {"id": "ci-3", "revision": "2026-10-13T12:00:00Z", "conclusion": "startup_failure"}]"#,
    )
    .unwrap();
    assert_eq!(rows[0].conclusion, CiConclusion::Neutral);
    assert_eq!(rows[1].conclusion, CiConclusion::Neutral);
    assert_eq!(rows[2].conclusion, CiConclusion::Failure);
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let (preview, evidence) = reconcile_ci(&doc, &ci_mapping(), &mut state, &rows[..1]).unwrap();
    assert_eq!(preview.unchanged, vec![TASK]);
    assert!(preview.conflicts.is_empty());
    assert!(evidence.is_empty());
    assert!(state.observed.is_empty());
}

// Defect: CI mappings held to looser hygiene than every other source,
// letting two checks claim one ID or a check point at a missing task.
#[test]
fn ci_duplicate_mapping_and_missing_task_fail_like_other_sources() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let mut mappings = ci_mapping();
    mappings.push(Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: "other".into(),
    });
    let err = reconcile_ci(
        &doc,
        &mappings,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap_err();
    assert!(err.to_string().contains("duplicate mapping"), "{err}");
    let missing = vec![Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: "absent".into(),
    }];
    let err = reconcile_ci(
        &doc,
        &missing,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap_err();
    assert!(err.to_string().contains("absent"), "{err}");
}

fn github_mapping() -> Vec<Mapping> {
    vec![Mapping {
        source: Source::Github,
        external_id: "PR-1".into(),
        task_id: TASK.into(),
    }]
}

fn github_record(revision: &str, done: bool) -> Record {
    Record {
        id: "PR-1".into(),
        revision: revision.into(),
        state: if done {
            TaskState::Done
        } else {
            TaskState::Open
        },
        verified: false,
    }
}

fn mappings_with(task_extra: Mapping) -> Vec<Mapping> {
    let mut mappings = github_mapping();
    mappings.push(task_extra);
    mappings
}

// Defect: the headline restriction bypassed either way - a merged PR alone
// completing a task, or a merged PR with passing CI never completing it.
#[test]
fn github_closure_completes_only_with_observed_ci_verification() {
    let mut doc = plan(TaskState::Open);
    let mappings = mappings_with(Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: TASK.into(),
    });
    let mut state = State::default();
    reconcile_ci(
        &doc,
        &mappings,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap();
    let closed = [github_record("2026-10-13T12:01:00Z", true)];
    let first = reconcile(&mut doc, &mappings, &mut state, Source::Github, &closed).unwrap();
    assert_eq!(
        first.conflicts.len(),
        1,
        "first sight of the closure conflicts"
    );
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
    let mut doc = plan(TaskState::Open);
    let mut state = State::default();
    let open = [github_record("2026-10-13T11:00:00Z", false)];
    let seen = reconcile(&mut doc, &mappings, &mut state, Source::Github, &open).unwrap();
    assert!(seen.conflicts.is_empty());
    reconcile_ci(
        &doc,
        &mappings,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap();
    let done = reconcile(&mut doc, &mappings, &mut state, Source::Github, &closed).unwrap();
    assert!(done.conflicts.is_empty(), "{:?}", done.conflicts);
    assert_eq!(done.changed, vec![TASK]);
    assert_eq!(task(&doc, TASK).state, TaskState::Done);
    assert_eq!(state.provenance.last().unwrap().source, Source::Github);
}

// Defect: failed verification still unlocking a GitHub completion.
#[test]
fn github_closure_stays_conflicted_when_ci_failed() {
    let mut doc = plan(TaskState::Open);
    let mappings = mappings_with(Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: TASK.into(),
    });
    let mut state = State::default();
    reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::Github,
        &[github_record("2026-10-13T11:00:00Z", false)],
    )
    .unwrap();
    reconcile_ci(
        &doc,
        &mappings,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "failure")],
    )
    .unwrap();
    let done = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::Github,
        &[github_record("2026-10-13T12:01:00Z", true)],
    )
    .unwrap();
    assert_eq!(done.conflicts.len(), 1);
    assert!(
        done.conflicts[0]
            .reason
            .contains("not acceptance-criteria verification")
    );
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
}

// Defect: a first-seen closure applying without a prior open observation,
// bypassing the first-observation rule even when CI passed.
#[test]
fn github_first_closure_conflicts_even_with_ci_pass() {
    let mut doc = plan(TaskState::Open);
    let mappings = mappings_with(Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: TASK.into(),
    });
    let mut state = State::default();
    reconcile_ci(
        &doc,
        &mappings,
        &mut state,
        &[ci_record("2026-10-13T12:00:00Z", "success")],
    )
    .unwrap();
    let done = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::Github,
        &[github_record("2026-10-13T12:01:00Z", true)],
    )
    .unwrap();
    assert_eq!(done.conflicts.len(), 1);
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
}

// Defect: one passing check outvoting a failing check mapped to the same
// task, completing on partial verification.
#[test]
fn ci_partially_passing_checks_do_not_verify() {
    let mut doc = plan(TaskState::Open);
    let mut mappings = mappings_with(Mapping {
        source: Source::Ci,
        external_id: "ci-1".into(),
        task_id: TASK.into(),
    });
    mappings.push(Mapping {
        source: Source::Ci,
        external_id: "ci-2".into(),
        task_id: TASK.into(),
    });
    let mut state = State::default();
    reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::Github,
        &[github_record("2026-10-13T11:00:00Z", false)],
    )
    .unwrap();
    let mut pass = ci_record("2026-10-13T12:00:00Z", "success");
    reconcile_ci(&doc, &mappings, &mut state, std::slice::from_ref(&pass)).unwrap();
    let mut fail = ci_record("2026-10-13T12:00:00Z", "failure");
    fail.id = "ci-2".into();
    pass.id = "ci-2".into();
    reconcile_ci(&doc, &mappings, &mut state, std::slice::from_ref(&fail)).unwrap();
    let done = reconcile(
        &mut doc,
        &mappings,
        &mut state,
        Source::Github,
        &[github_record("2026-10-13T12:01:00Z", true)],
    )
    .unwrap();
    assert_eq!(done.conflicts.len(), 1);
    assert_eq!(task(&doc, TASK).state, TaskState::Open);
}

fn planner_mapping(id: &str) -> Vec<Mapping> {
    vec![Mapping {
        source: Source::PlanningCompiler,
        external_id: id.into(),
        task_id: TASK.into(),
    }]
}

fn planner_snapshot(rows: &[(String, String)]) -> Vec<PlannerRecord> {
    parse_planner_snapshot(
        &serde_json::to_string(
            &rows
                .iter()
                .map(|(id, revision)| serde_json::json!({"id": id, "revision": revision}))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
    )
    .unwrap()
}

// Defect: the drift check rewriting the plan it only inspects, or reporting
// differently on identical reruns.
#[test]
fn planner_matching_snapshot_is_unchanged_and_idempotent() {
    let doc = plan(TaskState::Open);
    let before = doc.clone();
    let entity = doc.task.iter().find(|t| t.id == TASK).unwrap().clone();
    let rows = planner_snapshot(&[(format!("task:{TASK}"), content_revision(&entity))]);
    let mut state = State::default();
    for _ in 0..2 {
        let preview = reconcile_planner(
            &doc,
            &[],
            &planner_mapping(&format!("task:{TASK}")),
            &mut state,
            Some(&rows),
        )
        .unwrap();
        assert!(preview.conflicts.is_empty());
        assert!(preview.changed.is_empty());
        assert_eq!(preview.unchanged, vec![TASK]);
    }
    assert_eq!(doc, before);
    assert!(state.provenance.is_empty());
}

// Defect: the planning source needing a hand-built snapshot, or plan edits
// to a mapped entity never invalidating the analyses that depend on it.
#[test]
fn planner_live_read_reports_drift_as_changed_and_missing_as_conflict() {
    let mut doc = plan(TaskState::Open);
    let mappings = planner_mapping("method:plan-steps");
    let mut state = State::default();
    let first = reconcile_planner(&doc, &[], &mappings, &mut state, None).unwrap();
    assert_eq!(first.unchanged, vec![TASK]);
    assert!(first.changed.is_empty() && first.conflicts.is_empty());
    doc.method
        .iter_mut()
        .find(|x| x.id == "plan-steps")
        .unwrap()
        .provenance = "agent:drift".into();
    let drift = reconcile_planner(&doc, &[], &mappings, &mut state, None).unwrap();
    assert_eq!(drift.changed, vec![TASK]);
    assert!(drift.conflicts.is_empty());
    assert_eq!(
        state.observed["PlanningCompiler:method:plan-steps"].revision,
        planner_revision(&doc, &[], "method:plan-steps")
            .unwrap()
            .unwrap()
    );
    let rerun = reconcile_planner(&doc, &[], &mappings, &mut state, None).unwrap();
    assert_eq!(rerun.unchanged, vec![TASK]);
    assert!(rerun.changed.is_empty());
    doc.method.retain(|x| x.id != "plan-steps");
    let missing = reconcile_planner(&doc, &[], &mappings, &mut state, None).unwrap();
    assert_eq!(missing.conflicts.len(), 1);
    assert!(missing.conflicts[0].reason.contains("does not exist"));
    assert!(state.provenance.is_empty());
}

// Defect: a re-hashed method silently applied, or missed so dependent
// planning analyses keep reasoning over a stale alternative.
#[test]
fn planner_stale_method_is_a_conflict_without_touching_the_plan() {
    let mut doc = plan(TaskState::Open);
    let before = doc.clone();
    let entity = doc
        .method
        .iter()
        .find(|x| x.id == "plan-steps")
        .unwrap()
        .clone();
    let rows = planner_snapshot(&[("method:plan-steps".into(), content_revision(&entity))]);
    let mappings = planner_mapping("method:plan-steps");
    let mut state = State::default();
    let clean = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert!(clean.conflicts.is_empty());
    doc.method
        .iter_mut()
        .find(|x| x.id == "plan-steps")
        .unwrap()
        .provenance = "agent:drift".into();
    let stale = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert_eq!(stale.conflicts.len(), 1);
    assert!(stale.conflicts[0].reason.contains("no longer matches"));
    assert_eq!(
        doc.method
            .iter()
            .find(|x| x.id == "plan-steps")
            .unwrap()
            .provenance,
        "agent:drift"
    );
    assert_eq!(doc.task, before.task);
}

// Defect: a deleted decision going unnoticed while the frontier still cites
// it as supporting evidence.
#[test]
fn planner_missing_decision_is_a_conflict() {
    let doc = plan(TaskState::Open);
    let rows = planner_snapshot(&[("decision:d1".into(), "deadbeef".into())]);
    let mut state = State::default();
    let preview = reconcile_planner(
        &doc,
        &[],
        &planner_mapping("decision:d1"),
        &mut state,
        Some(&rows),
    )
    .unwrap();
    assert_eq!(preview.conflicts.len(), 1);
    assert!(preview.conflicts[0].reason.contains("does not exist"));
    assert!(state.observed.is_empty());
}

// Defect: verification evidence edited under a stayed snapshot either
// matching the wrong record or escaping the drift check entirely.
#[test]
fn planner_evidence_drift_is_a_conflict() {
    let mut doc = plan(TaskState::Open);
    doc.evidence.push(pc_ir::Evidence {
        subject: format!("task:{TASK}"),
        metric: "duration_minutes".into(),
        value: 2.0,
        unit: "minutes".into(),
        time: "2026-10-13T12:00:00Z".into(),
        source: "ci:ci-1".into(),
        basis: None,
        artifact_hash: None,
    });
    let rows = planner_snapshot(&[(
        format!("evidence:task:{TASK}"),
        planner_revision(&doc, &[], &format!("evidence:task:{TASK}"))
            .unwrap()
            .unwrap(),
    )]);
    let mappings = planner_mapping(&format!("evidence:task:{TASK}"));
    let mut state = State::default();
    let clean = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert!(clean.conflicts.is_empty());
    doc.evidence.last_mut().unwrap().value = 99.0;
    let stale = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert_eq!(stale.conflicts.len(), 1);
    doc.evidence.clear();
    let missing = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert_eq!(missing.conflicts.len(), 1);
    assert!(missing.conflicts[0].reason.contains("does not exist"));
}

// Defect: the objective edited (new weights, new root) while the snapshot
// still claims the old acceptance contract.
#[test]
fn planner_objective_mismatch_is_a_conflict() {
    let mut doc = plan(TaskState::Open);
    let rows = planner_snapshot(&[(
        "objective:self-plan".into(),
        content_revision(&doc.objective),
    )]);
    let mappings = planner_mapping("objective:self-plan");
    let mut state = State::default();
    let clean = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert!(clean.conflicts.is_empty());
    doc.objective.budget_usd = Some(1.0);
    let stale = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap();
    assert_eq!(stale.conflicts.len(), 1);
}

// Defect: snapshot rows naming entity kinds the checker cannot resolve,
// either sneaking past the parser or panicking the reconciler.
#[test]
fn planner_unknown_rows_fail_at_parse_and_reconcile() {
    let err = parse_planner_snapshot(r#"[{"id": "bogus:x", "revision": "r"}]"#).unwrap_err();
    assert!(err.contains("unknown planning-compiler record"), "{err}");
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let err = reconcile_planner(
        &doc,
        &[],
        &planner_mapping("bogus:x"),
        &mut state,
        Some(&[PlannerRecord {
            id: "bogus:x".into(),
            revision: "r".into(),
        }]),
    )
    .unwrap_err();
    assert!(err.to_string().contains("unknown planning-compiler record"));
}

// Defect: planner mappings held to looser hygiene than other sources, or
// unmapped planning rows reported as conflicts instead of ignored.
#[test]
fn planner_mapping_hygiene_matches_other_sources() {
    let doc = plan(TaskState::Open);
    let mut state = State::default();
    let mut mappings = planner_mapping("task:x");
    mappings.push(Mapping {
        source: Source::PlanningCompiler,
        external_id: "task:x".into(),
        task_id: TASK.into(),
    });
    let rows = planner_snapshot(&[("task:x".into(), "r".into())]);
    let err = reconcile_planner(&doc, &[], &mappings, &mut state, Some(&rows)).unwrap_err();
    assert!(err.to_string().contains("duplicate mapping"));
    let missing = vec![Mapping {
        source: Source::PlanningCompiler,
        external_id: "task:x".into(),
        task_id: "absent".into(),
    }];
    let err = reconcile_planner(&doc, &[], &missing, &mut state, Some(&rows)).unwrap_err();
    assert!(err.to_string().contains("absent"));
    let preview = reconcile_planner(
        &doc,
        &[],
        &planner_mapping("task:other"),
        &mut state,
        Some(&rows),
    )
    .unwrap();
    assert!(preview.conflicts.is_empty());
    assert_eq!(preview.unmapped, vec!["task:x"]);
}

// Defect: re-exports churning revisions for untouched entities, so every
// export looks stale against the last snapshot.
#[test]
fn content_revision_is_stable_for_identical_entities() {
    let doc = plan(TaskState::Open);
    let entity = doc.task.iter().find(|t| t.id == TASK).unwrap();
    assert_eq!(content_revision(entity), content_revision(entity));
    let mut edited = entity.clone();
    edited.title.push('!');
    assert_ne!(content_revision(entity), content_revision(&edited));
}
