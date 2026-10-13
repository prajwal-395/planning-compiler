//! Deterministic reconciliation of externally managed task state.
//!
//! Explicit identity mappings and observed revisions are held outside the
//! planning IR so source-owned metadata remains authoritative.
use pc_ir::{DecisionEvent, Evidence, MethodKind, MethodStatus, PlanDoc, TaskState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Supported explicitly polled external sources.
pub enum Source {
    /// GitHub issues and pull requests fetched through `gh`; a closure alone
    /// never completes a task, but a closure with observed CI verification
    /// for the same mapped task does.
    Github,
    /// Canonical tasks-axi backlog snapshots.
    TasksAxi,
    /// Beads JSONL issue snapshots.
    Beads,
    /// CI check results (`gh run` data or a recorded snapshot). Successful
    /// checks record verification evidence; they never edit task state
    /// themselves, but they let a mapped GitHub closure complete the task.
    Ci,
    /// The plan's own objectives, alternatives, decisions and evidence, read
    /// live and re-checked for drift. Read-only: drift invalidates the mapped
    /// task's analyses, and missing entities are conflicts, never edits.
    PlanningCompiler,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A stable external identifier to Planning IR task mapping.
pub struct Mapping {
    /// System that owns the external identifier.
    pub source: Source,
    /// Stable source-specific identifier.
    pub external_id: String,
    /// Existing task ID in the plan.
    pub task_id: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Durable synchronization state stored beside the plan.
pub struct State {
    /// Last successfully observed revision and lifecycle state for each mapped entity.
    #[serde(default)]
    pub observed: BTreeMap<String, Observation>,
    /// Source revision behind every task state change sync has applied.
    #[serde(default)]
    pub provenance: Vec<Provenance>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// State observation persisted for one mapped external entity.
pub struct Observation {
    /// Source revision token.
    pub revision: String,
    /// Lifecycle state represented by that revision: `open` or `done`.
    pub state: TaskState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// The external revision that caused one applied task state change.
pub struct Provenance {
    /// Source of the change.
    pub source: Source,
    /// External entity identifier.
    pub external_id: String,
    /// Source revision that was applied.
    pub revision: String,
    /// Changed Planning IR task.
    pub task_id: String,
    /// Local task state before the change.
    pub from: TaskState,
    /// Local task state after the change.
    pub to: TaskState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Normalized external task state.
pub struct Record {
    /// Stable external identifier.
    pub id: String,
    /// Stable revision token or content digest.
    pub revision: String,
    /// External lifecycle state mapped to the IR state vocabulary.
    pub state: TaskState,
    /// Whether independent verification evidence accompanies completion.
    #[serde(default)]
    pub verified: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// A stale or conflicting local/external state pair.
pub struct Conflict {
    /// Source of the external state.
    pub source: Source,
    /// External entity identifier.
    pub external_id: String,
    /// Mapped Planning IR task.
    pub task_id: String,
    /// Human-readable conflict explanation.
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Preview or result of one source reconciliation.
pub struct Preview {
    /// Task IDs changed by the source.
    pub changed: Vec<String>,
    /// Task IDs already matching the source.
    pub unchanged: Vec<String>,
    /// Conflicts left untouched.
    pub conflicts: Vec<Conflict>,
    /// Source record IDs without an explicit identity mapping.
    pub unmapped: Vec<String>,
}
#[derive(Debug, Error)]
/// Failures that prevent a deterministic reconciliation.
pub enum SyncError {
    /// Mapping points to a task absent from the plan.
    #[error("mapping references missing task `{0}`")]
    MissingTask(String),
    /// Two mappings claim one external ID.
    #[error("duplicate mapping for {external_source:?} id `{id}`")]
    DuplicateMapping {
        /// Source containing the duplicate.
        external_source: Source,
        /// Duplicated external ID.
        id: String,
    },
    /// A planning-compiler snapshot row names no known entity kind.
    #[error(
        "unknown planning-compiler record `{0}`: use objective:<plan>, method:<id>, decision:<id>, evidence:<subject> or task:<id>"
    )]
    UnknownPlannerEntity(String),
}

/// Terminal meaning of one CI check result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiConclusion {
    /// The check passed: verification evidence for the mapped task.
    Success,
    /// The check failed or was cancelled: negative verification evidence.
    Failure,
    /// The check has not finished: no verification either way.
    Pending,
    /// The check was skipped or ended neutral: never verification, never
    /// observed, so it neither verifies nor blocks the mapped task.
    Neutral,
}

#[derive(Debug, Clone, PartialEq)]
/// One normalized CI check result.
pub struct CiRecord {
    /// Stable check identity (run database ID or check-run ID).
    pub id: String,
    /// Revision token; doubles as the verification evidence timestamp, so
    /// snapshots must carry RFC 3339 here.
    pub revision: String,
    /// Terminal meaning of the check result.
    pub conclusion: CiConclusion,
    /// Raw conclusion string from the snapshot, for conflict messages.
    pub conclusion_raw: String,
    /// Reported runtime in seconds, when reported.
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One expected planning entity for drift checking.
pub struct PlannerRecord {
    /// `objective:<plan>`, `method:<id>`, `decision:<id>`,
    /// `evidence:<subject>` or `task:<id>`.
    pub id: String,
    /// Expected [`planner_revision`] of the entity.
    pub revision: String,
}

/// Stable content hash of one planning entity, serialized as canonical JSON.
///
/// [`planner_revision`] hashes entities with this helper, so snapshot
/// overrides compare identical bytes.
pub fn content_revision<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
    blake3::hash(json.to_string().as_bytes())
        .to_hex()
        .to_string()
}

/// Parse source snapshots with the existing tasks-axi and Beads adapters.
/// CI snapshots use [`parse_ci_snapshot`] and planning-compiler snapshots
/// use [`parse_planner_snapshot`]: they carry verification and drift data,
/// not task lifecycles.
pub fn parse_snapshot(source: Source, input: &str) -> Result<Vec<Record>, String> {
    match source {
        Source::Ci => Err("CI snapshots must be parsed with parse_ci_snapshot".into()),
        Source::PlanningCompiler => {
            Err("planning-compiler snapshots must be parsed with parse_planner_snapshot".into())
        }
        Source::TasksAxi => {
            let backlog = pc_adapter_tasks_axi::parse_backlog(input).map_err(|e| e.to_string())?;
            Ok(backlog
                .items
                .into_iter()
                .map(|i| {
                    let revision = blake3::hash(format!("{i:?}").as_bytes())
                        .to_hex()
                        .to_string();
                    Record {
                        id: i.id,
                        revision,
                        state: if i.done {
                            TaskState::Done
                        } else {
                            TaskState::Open
                        },
                        verified: false,
                    }
                })
                .collect())
        }
        Source::Beads => {
            let imported = pc_adapter_beads::import_jsonl(input).map_err(|e| e.to_string())?;
            Ok(imported
                .doc
                .task
                .into_iter()
                .filter_map(|t| {
                    t.beads_jsonl.map(|raw| Record {
                        id: t.id,
                        revision: blake3::hash(raw.as_bytes()).to_hex().to_string(),
                        state: t.state,
                        verified: false,
                    })
                })
                .collect())
        }
        Source::Github => {
            let value: serde_json::Value =
                serde_json::from_str(input).map_err(|e| e.to_string())?;
            let rows = value
                .as_array()
                .ok_or_else(|| "GitHub snapshot must be a JSON array".to_owned())?
                .iter()
                .flat_map(|row| match row {
                    serde_json::Value::Array(rows) => rows.clone(),
                    _ => vec![row.clone()],
                })
                .collect::<Vec<_>>();
            Ok(rows
                .into_iter()
                .filter_map(|v| {
                    let id = v.get("node_id")?.as_str()?.to_owned();
                    let revision = v.get("updated_at")?.as_str()?.to_owned();
                    let state = if v.get("state")?.as_str()? == "closed" {
                        TaskState::Done
                    } else {
                        TaskState::Open
                    };
                    Some(Record {
                        id,
                        revision,
                        state,
                        verified: false,
                    })
                })
                .collect())
        }
    }
}

/// Parse a CI snapshot: a JSON array of check results (a nested array per
/// page is flattened, as with GitHub). Each row accepts canonical keys plus
/// the `gh run list --json databaseId,conclusion,updatedAt` keys:
///
/// ```json
/// [{"id": "123", "revision": "2026-10-13T12:00:00Z", "conclusion": "success",
///   "duration_seconds": 120}]
/// ```
///
/// `databaseId` reads as `id` and `updatedAt` as `revision`. Conclusions are
/// GitHub's; a missing or empty conclusion is pending, never a failure.
pub fn parse_ci_snapshot(input: &str) -> Result<Vec<CiRecord>, String> {
    let value: serde_json::Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let rows = value
        .as_array()
        .ok_or_else(|| "CI snapshot must be a JSON array".to_owned())?
        .iter()
        .flat_map(|row| match row {
            serde_json::Value::Array(rows) => rows.clone(),
            _ => vec![row.clone()],
        })
        .collect::<Vec<_>>();
    rows.into_iter()
        .map(|v| {
            let id = v
                .get("id")
                .or_else(|| v.get("databaseId"))
                .map(json_scalar)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "CI row needs a stable `id` (or `databaseId`)".to_owned())?;
            let revision = v
                .get("revision")
                .or_else(|| v.get("updatedAt"))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("CI check `{id}` needs a revision timestamp"))?
                .to_owned();
            let raw = v
                .get("conclusion")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            let conclusion = match raw.as_str() {
                "success" => CiConclusion::Success,
                "failure" | "cancelled" | "timed_out" | "action_required" | "stale"
                | "startup_failure" => CiConclusion::Failure,
                "skipped" | "neutral" => CiConclusion::Neutral,
                "" => CiConclusion::Pending,
                other => {
                    return Err(format!("CI check `{id}` has unknown conclusion `{other}`"));
                }
            };
            let duration_seconds = v
                .get("duration_seconds")
                .map(|n| {
                    n.as_f64()
                        .ok_or_else(|| format!("CI check `{id}` has a non-numeric duration"))
                })
                .transpose()?;
            if duration_seconds.is_some_and(|d| !d.is_finite() || d < 0.0) {
                return Err(format!("CI check `{id}` has a negative duration"));
            }
            Ok(CiRecord {
                id,
                revision,
                conclusion,
                conclusion_raw: raw,
                duration_seconds,
            })
        })
        .collect()
}

/// Read one scalar (string or number) as text for stable external IDs.
fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse a planning-compiler snapshot: a JSON array of expected entity
/// revisions exported from objectives, alternatives, decisions and evidence:
///
/// ```json
/// [{"id": "task:pc-05-sync", "revision": "<blake3 hex>"},
///  {"id": "method:implement", "revision": "<blake3 hex>"},
///  {"id": "decision:2026-10-13-ren-engine", "revision": "<blake3 hex>"},
///  {"id": "evidence:task:pc-05-sync", "revision": "<blake3 hex>"},
///  {"id": "objective:self-plan", "revision": "<blake3 hex>"}]
/// ```
///
/// This is an optional override of the live read in [`reconcile_planner`].
/// Revisions must come from [`planner_revision`], so a re-export after an
/// intentional plan edit moves the expectation instead of conflicting forever.
pub fn parse_planner_snapshot(input: &str) -> Result<Vec<PlannerRecord>, String> {
    let value: serde_json::Value = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let rows = value
        .as_array()
        .ok_or_else(|| "planning-compiler snapshot must be a JSON array".to_owned())?;
    rows.iter()
        .map(|v| {
            let id = v
                .get("id")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "planning-compiler row needs an `id`".to_owned())?
                .to_owned();
            if !id.starts_with("objective:")
                && !id.starts_with("method:")
                && !id.starts_with("decision:")
                && !id.starts_with("evidence:")
                && !id.starts_with("task:")
            {
                return Err(
                    "unknown planning-compiler record `{id}`: use objective:<plan>, method:<id>, decision:<id>, evidence:<subject> or task:<id>"
                        .replace("{id}", &id),
                );
            }
            let revision = v
                .get("revision")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("planning-compiler record `{id}` needs a revision"))?
                .to_owned();
            Ok(PlannerRecord { id, revision })
        })
        .collect()
}
/// Collapse IR states to the lifecycle external sources report: work that is
/// not finished (open, decomposed, resolved) compares as `open`.
fn lifecycle(state: TaskState) -> TaskState {
    match state {
        TaskState::Open | TaskState::Decomposed | TaskState::Resolved => TaskState::Open,
        other => other,
    }
}

/// Reconcile by explicit IDs. A changed local value is preserved and reported
/// as a conflict unless it still equals the previous observed external value.
/// Only lifecycle transitions are written; every other task field is left as is.
/// `doc` and `sync` are always updated, so later records see earlier changes;
/// preview by reconciling copies and discarding them.
pub fn reconcile(
    doc: &mut PlanDoc,
    mappings: &[Mapping],
    sync: &mut State,
    source: Source,
    records: &[Record],
) -> Result<Preview, SyncError> {
    let mut ids = BTreeSet::new();
    for m in mappings.iter().filter(|m| m.source == source) {
        if !ids.insert(m.external_id.as_str()) {
            return Err(SyncError::DuplicateMapping {
                external_source: source,
                id: m.external_id.clone(),
            });
        }
    }
    let mut out = Preview {
        changed: vec![],
        unchanged: vec![],
        conflicts: vec![],
        unmapped: vec![],
    };
    for r in records {
        let Some(m) = mappings
            .iter()
            .find(|m| m.source == source && m.external_id == r.id)
        else {
            out.unmapped.push(r.id.clone());
            continue;
        };
        let decomposed = doc.method.iter().any(|x| {
            x.task == m.task_id
                && x.kind == MethodKind::Decompose
                && x.status == MethodStatus::Accepted
        });
        let task = doc
            .task
            .iter_mut()
            .find(|t| t.id == m.task_id)
            .ok_or_else(|| SyncError::MissingTask(m.task_id.clone()))?;
        let key = format!("{:?}:{}", source, r.id);
        let local = lifecycle(task.state);
        let external = lifecycle(r.state);
        let prev = sync.observed.get(&key);
        let reason = if local == external {
            out.unchanged.push(m.task_id.clone());
            None
        } else if prev.is_some_and(|p| p.revision == r.revision) {
            Some(format!(
                "local state {:?} no longer matches external state {:?} observed at this revision",
                task.state, r.state
            ))
        } else if source == Source::Github
            && external == TaskState::Done
            && !r.verified
            && !ci_verified(&m.task_id, mappings, sync)
        {
            Some("GitHub closure or merged PR is not acceptance-criteria verification".into())
        } else if prev.is_some_and(|p| p.state == local) {
            let to = match external {
                TaskState::Open if decomposed => TaskState::Decomposed,
                other => other,
            };
            out.changed.push(m.task_id.clone());
            sync.provenance.push(Provenance {
                source,
                external_id: r.id.clone(),
                revision: r.revision.clone(),
                task_id: m.task_id.clone(),
                from: task.state,
                to,
            });
            task.state = to;
            None
        } else {
            Some(format!(
                "local state {:?} differs from external state {:?}",
                task.state, r.state
            ))
        };
        match reason {
            Some(reason) => out.conflicts.push(Conflict {
                source,
                external_id: r.id.clone(),
                task_id: m.task_id.clone(),
                reason,
            }),
            None => {
                sync.observed.insert(
                    key,
                    Observation {
                        revision: r.revision.clone(),
                        state: external,
                    },
                );
            }
        }
    }
    Ok(out)
}

/// CI verification for one mapped task: at least one mapped check has been
/// observed and every observed mapped check passed. Checks absent from the
/// snapshot do not block: explicit sync only judges the snapshots it was
/// given. A pending check observes as `open`, so running CI still blocks a
/// GitHub completion until it passes.
fn ci_verified(task_id: &str, mappings: &[Mapping], sync: &State) -> bool {
    let mut any = false;
    for m in mappings
        .iter()
        .filter(|m| m.source == Source::Ci && m.task_id == task_id)
    {
        match sync.observed.get(&format!("Ci:{}", m.external_id)) {
            Some(observed) if observed.state == TaskState::Done => any = true,
            Some(_) => return false,
            None => {}
        }
    }
    any
}

/// One append-only verification observation for a mapped task.
///
/// CI evidence uses only metrics the compiler already folds: runtimes as
/// `duration_minutes` and pass/fail results as `failure_risk` 0/1. Subjects
/// are `task:` records, which P5 holds unattributed, so verification never
/// moves estimate profiles; it only lets an explicit GitHub closure derive
/// completion, with the evidence log as the audit trail.
fn verification_evidence(task_id: &str, record: &CiRecord, metric: &str) -> Evidence {
    let (value, unit) = match metric {
        "duration_minutes" => (record.duration_seconds.unwrap_or(0.0) / 60.0, "minutes"),
        _ => (
            if record.conclusion == CiConclusion::Success {
                0.0
            } else {
                1.0
            },
            "ratio",
        ),
    };
    Evidence {
        subject: format!("task:{task_id}"),
        metric: metric.into(),
        value,
        unit: unit.into(),
        time: record.revision.clone(),
        source: format!("ci:{}", record.id),
        basis: None,
        artifact_hash: None,
    }
}

/// Reconcile CI check results by explicit check ID.
///
/// Successful checks record verification evidence (runtime plus pass) and
/// observe `done`; failed checks record failure evidence and observe `open`.
/// A failed check for a locally completed task is a conflict on every run:
/// the local completion is preserved, while the check still observes `open`
/// so it never verifies the task. Skipped and neutral checks are ignored. CI never
/// writes task state, so `changed` names tasks with newly recorded
/// verification, and the returned evidence must be appended to the sibling
/// `evidence.jsonl` (skipped under `--dry-run`). Evidence already in `doc`
/// is never staged again, so replaying the same revision records nothing twice.
///
/// Like [`reconcile`], mappings are explicit: records without a mapping are
/// `unmapped`, duplicate mappings and mappings to missing tasks fail.
pub fn reconcile_ci(
    doc: &PlanDoc,
    mappings: &[Mapping],
    sync: &mut State,
    records: &[CiRecord],
) -> Result<(Preview, Vec<Evidence>), SyncError> {
    let mut ids = BTreeSet::new();
    for m in mappings.iter().filter(|m| m.source == Source::Ci) {
        if !ids.insert(m.external_id.as_str()) {
            return Err(SyncError::DuplicateMapping {
                external_source: Source::Ci,
                id: m.external_id.clone(),
            });
        }
    }
    let mut out = Preview {
        changed: vec![],
        unchanged: vec![],
        conflicts: vec![],
        unmapped: vec![],
    };
    let mut evidence = Vec::new();
    for r in records {
        let Some(m) = mappings
            .iter()
            .find(|m| m.source == Source::Ci && m.external_id == r.id)
        else {
            out.unmapped.push(r.id.clone());
            continue;
        };
        let done = doc
            .task
            .iter()
            .find(|t| t.id == m.task_id)
            .map(|t| lifecycle(t.state) == TaskState::Done)
            .ok_or_else(|| SyncError::MissingTask(m.task_id.clone()))?;
        let key = format!("Ci:{}", r.id);
        let replayed = sync
            .observed
            .get(&key)
            .is_some_and(|p| p.revision == r.revision);
        let conflict = r.conclusion == CiConclusion::Failure && done;
        if replayed && !conflict {
            out.unchanged.push(m.task_id.clone());
            continue;
        }
        let state = match r.conclusion {
            CiConclusion::Pending => {
                out.unchanged.push(m.task_id.clone());
                Some(TaskState::Open)
            }
            CiConclusion::Neutral => {
                out.unchanged.push(m.task_id.clone());
                None
            }
            CiConclusion::Success | CiConclusion::Failure => {
                let metrics = r
                    .duration_seconds
                    .map(|_| "duration_minutes")
                    .into_iter()
                    .chain(["failure_risk"]);
                for metric in metrics {
                    let record = verification_evidence(&m.task_id, r, metric);
                    let same = |e: &Evidence| {
                        e.source == record.source
                            && e.time == record.time
                            && e.metric == record.metric
                            && e.subject == record.subject
                    };
                    if !doc.evidence.iter().any(same) && !evidence.iter().any(same) {
                        evidence.push(record);
                    }
                }
                if conflict {
                    out.conflicts.push(Conflict {
                        source: Source::Ci,
                        external_id: r.id.clone(),
                        task_id: m.task_id.clone(),
                        reason: format!(
                            "CI check `{}` concluded `{}` while the task is marked done; local completion preserved",
                            r.id, r.conclusion_raw
                        ),
                    });
                } else {
                    out.changed.push(m.task_id.clone());
                }
                Some(if r.conclusion == CiConclusion::Success {
                    TaskState::Done
                } else {
                    TaskState::Open
                })
            }
        };
        if let Some(state) = state {
            sync.observed.insert(
                key,
                Observation {
                    revision: r.revision.clone(),
                    state,
                },
            );
        }
    }
    Ok((out, evidence))
}

/// Current [`content_revision`] of one planning entity, `None` when it no
/// longer exists. Evidence revisions cover every record for the subject, so
/// any appended or edited record moves them.
pub fn planner_revision(
    doc: &PlanDoc,
    decisions: &[DecisionEvent],
    id: &str,
) -> Result<Option<String>, SyncError> {
    if let Some(task) = id.strip_prefix("task:") {
        return Ok(doc.task.iter().find(|t| t.id == task).map(content_revision));
    }
    if let Some(method) = id.strip_prefix("method:") {
        return Ok(doc
            .method
            .iter()
            .find(|x| x.id == method)
            .map(content_revision));
    }
    if let Some(event) = id.strip_prefix("decision:") {
        return Ok(decisions
            .iter()
            .find(|d| d.event.id == event)
            .map(content_revision));
    }
    if let Some(subject) = id.strip_prefix("evidence:") {
        let records = doc
            .evidence
            .iter()
            .filter(|e| e.subject == subject)
            .collect::<Vec<_>>();
        return Ok((!records.is_empty()).then(|| content_revision(&records)));
    }
    if let Some(plan) = id.strip_prefix("objective:") {
        return Ok((doc.plan.id == plan).then(|| content_revision(&doc.objective)));
    }
    Err(SyncError::UnknownPlannerEntity(id.into()))
}

/// Reconcile the plan's own objectives, alternatives, decisions and evidence.
///
/// Every mapped entity is read live and hashed with [`planner_revision`]. An
/// entity whose revision moved since its last observation is `changed`: the
/// mapped task's planning analyses are invalidated, and the new revision is
/// observed. A first observation is `unchanged`. A mapped entity that no
/// longer exists is a conflict. With `records` (an optional snapshot
/// override), only those rows are checked and each must still match its
/// expected revision; a mismatch is a conflict naming the stale record. The
/// plan is never written and no provenance is recorded.
///
/// Mappings stay explicit: snapshot rows without a mapping are `unmapped`,
/// duplicate mappings and mappings to missing tasks fail like every other
/// source.
pub fn reconcile_planner(
    doc: &PlanDoc,
    decisions: &[DecisionEvent],
    mappings: &[Mapping],
    sync: &mut State,
    records: Option<&[PlannerRecord]>,
) -> Result<Preview, SyncError> {
    let mut ids = BTreeSet::new();
    for m in mappings
        .iter()
        .filter(|m| m.source == Source::PlanningCompiler)
    {
        if !ids.insert(m.external_id.as_str()) {
            return Err(SyncError::DuplicateMapping {
                external_source: Source::PlanningCompiler,
                id: m.external_id.clone(),
            });
        }
    }
    let mut out = Preview {
        changed: vec![],
        unchanged: vec![],
        conflicts: vec![],
        unmapped: vec![],
    };
    let rows = match records {
        Some(records) => records
            .iter()
            .map(|r| (r.id.clone(), Some(r.revision.clone())))
            .collect::<Vec<_>>(),
        None => mappings
            .iter()
            .filter(|m| m.source == Source::PlanningCompiler)
            .map(|m| (m.external_id.clone(), None))
            .collect(),
    };
    for (id, expected) in rows {
        let Some(m) = mappings
            .iter()
            .find(|m| m.source == Source::PlanningCompiler && m.external_id == id)
        else {
            out.unmapped.push(id);
            continue;
        };
        if !doc.task.iter().any(|t| t.id == m.task_id) {
            return Err(SyncError::MissingTask(m.task_id.clone()));
        }
        let key = format!("PlanningCompiler:{id}");
        let reason = match (planner_revision(doc, decisions, &id)?, expected) {
            (None, _) => Some(format!("planning record `{id}` does not exist")),
            (Some(live), Some(expected)) if live != expected => Some(format!(
                "planning record `{id}` no longer matches revision `{expected}`"
            )),
            (Some(live), _) => {
                if sync.observed.get(&key).is_some_and(|p| p.revision != live) {
                    out.changed.push(m.task_id.clone());
                } else {
                    out.unchanged.push(m.task_id.clone());
                }
                sync.observed.insert(
                    key,
                    Observation {
                        revision: live,
                        state: TaskState::Open,
                    },
                );
                None
            }
        };
        if let Some(reason) = reason {
            out.conflicts.push(Conflict {
                source: Source::PlanningCompiler,
                external_id: id,
                task_id: m.task_id.clone(),
                reason,
            });
        }
    }
    Ok(out)
}
