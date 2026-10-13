//! Defect-focused regression tests for source reconciliation.
use pc_ir::{Task, TaskState};
use pc_sync::{Mapping, Provenance, Record, Source, State, parse_snapshot, reconcile};

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
