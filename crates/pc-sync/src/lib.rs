//! Deterministic reconciliation of externally managed task state.
//!
//! Explicit identity mappings and observed revisions are held outside the
//! planning IR so source-owned metadata remains authoritative.
use pc_ir::{MethodKind, MethodStatus, PlanDoc, TaskState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Supported explicitly polled external sources.
pub enum Source {
    /// GitHub issues and pull requests fetched through `gh`; a closure alone never completes a task.
    Github,
    /// Canonical tasks-axi backlog snapshots.
    TasksAxi,
    /// Beads JSONL issue snapshots.
    Beads,
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
}

/// Parse source snapshots with the existing tasks-axi and Beads adapters.
pub fn parse_snapshot(source: Source, input: &str) -> Result<Vec<Record>, String> {
    match source {
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
        } else if source == Source::Github && external == TaskState::Done && !r.verified {
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
