//! Step 7: the proposal loop — `pc propose`, `pc accept`/`pc reject` and
//! `pc context`.
//!
//! An external agent drives recursive decomposition through these commands
//! while the core stays model-free: `propose` stages new tasks, methods and
//! capabilities as `proposed`, `accept`/`reject` decide pending methods, and
//! `context` renders the compact slice an LLM prompt needs. Nothing here
//! calls a model, executes work or tracks status.
//!
//! File-edit rule: these functions operate on the pristine plan as loaded
//! from disk. Pass mutations (P3 derived proposals, P4 share merges, P1 unit
//! canonicalization) live only in the checked copy the CLI probes for
//! cleanliness — they are never written back. Every mutating command refuses
//! to write when the merged plan fails `check`, so files stay compilable and
//! `pc impact` remains the preview path for decisions that would break the
//! plan (for example accepting a substitution that drops a contracted
//! output).

use std::collections::{BTreeMap, BTreeSet};

use pc_ir::{
    Capability, ContractItem, Estimate, KNOWN_METRICS, Method, MethodKind, MethodStatus, PlanDoc,
    Task, TaskState, canonical_unit,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compile::CompiledPlan;
use crate::frontier::{FrontierItem, FrontierReport};
use crate::impact::{ImpactError, status_str};

/// Default token budget for `pc context`: a compact slice for an LLM prompt.
pub const DEFAULT_CONTEXT_BUDGET_TOKENS: u64 = 2000;

/// Method-id prefix owned by the P3 matcher. Agent patches must not claim
/// it: a proposed `match-*` id would shadow (or collide with) a derived
/// proposal for the same task and capability.
pub const MATCH_ID_PREFIX: &str = "match-";

/// Why one agent patch was refused by [`validate_patch`].
///
/// Messages mirror the P1/P2 codes `pc check` would report for the same
/// mistake, so `pc propose` and `pc check` never disagree about validity.
/// Contract violations (E008) are deliberately absent: a proposed invoke
/// whose capability drops a field is a hypothesis the frontier ranks as a
/// blocked alternative, not a patch error.
#[derive(Debug, Error)]
pub enum PatchError {
    /// The patch adds nothing.
    #[error(
        "patch adds nothing: include at least one entry in add_task, add_method or add_capability"
    )]
    EmptyPatch,
    /// An id appears twice inside the patch.
    #[error("patch adds {kind} `{id}` twice")]
    DuplicateInPatch {
        /// Entity kind: `task`, `method` or `capability`.
        kind: String,
        /// Duplicated id.
        id: String,
    },
    /// An id already exists in the plan.
    #[error("{kind} `{id}` already exists in the plan")]
    AlreadyExists {
        /// Entity kind: `task`, `method` or `capability`.
        kind: String,
        /// Colliding id.
        id: String,
    },
    /// A method id claims the P3 matcher's namespace.
    #[error(
        "patch uses reserved method id `{id}` (names starting with `match-` belong to the P3 matcher)"
    )]
    ReservedMethodId {
        /// Reserved id.
        id: String,
    },
    /// A new task claims the tracking state the compiler only reads.
    #[error(
        "proposed task `{id}` uses state `done`: the compiler reads done from imports and never sets it"
    )]
    TaskStateDone {
        /// Task id.
        id: String,
    },
    /// A patched method is not proposed.
    #[error(
        "method `{id}` must be proposed in a patch (decide it later with `pc accept` or `pc reject`)"
    )]
    MethodNotProposed {
        /// Method id.
        id: String,
    },
    /// A patched method names no provenance.
    #[error(
        "method `{id}` names no provenance: set agent:<name>, llm:<model> or human:<name> so the proposal stays traceable"
    )]
    MethodMissingProvenance {
        /// Method id.
        id: String,
    },
    /// A patched method names a task neither the plan nor the patch declares.
    #[error("method `{id}` names unknown task `{task}`")]
    UnknownTask {
        /// Method id.
        id: String,
        /// Missing task id.
        task: String,
    },
    /// A patched invoke names a capability neither the plan nor the patch declares.
    #[error("method `{id}` names unknown capability `{capability}`")]
    UnknownCapability {
        /// Method id.
        id: String,
        /// Missing capability id.
        capability: String,
    },
    /// A patched method mixes the decompose and invoke shapes.
    #[error("method `{id}` {reason}")]
    BadMethodShape {
        /// Method id.
        id: String,
        /// What is wrong with the shape.
        reason: String,
    },
    /// A patched contract names an undeclared artifact.
    #[error(
        "{subject} uses unknown artifact `{artifact}` (artifact declarations stay human-maintained; use an existing type)"
    )]
    UnknownArtifact {
        /// Entity the contract belongs to, e.g. `task:draft`.
        subject: String,
        /// Missing artifact name.
        artifact: String,
    },
    /// A patched contract names a field the artifact does not declare.
    #[error("{subject} uses unknown field `{artifact}.{field}`")]
    UnknownField {
        /// Entity the contract belongs to, e.g. `capability:writer@1`.
        subject: String,
        /// Artifact name.
        artifact: String,
        /// Missing field name.
        field: String,
    },
    /// A patched estimate names a metric v0 does not know (P2 reports E010).
    #[error("{subject} uses unknown metric `{metric}`")]
    UnknownMetric {
        /// Entity the estimate belongs to.
        subject: String,
        /// Missing metric name.
        metric: String,
    },
    /// A patched estimate breaks the three-point rules (P2 reports E011).
    #[error("{subject}: {reason}")]
    BadEstimate {
        /// Entity the estimate belongs to.
        subject: String,
        /// What is wrong with the triple.
        reason: String,
    },
    /// A patched capability draws on an undeclared resource (P2 reports E012).
    #[error("{subject} uses undeclared resource `{resource}`")]
    UnknownResource {
        /// Entity the resource belongs to.
        subject: String,
        /// Missing resource name.
        resource: String,
    },
}

/// Validate an agent patch against the pristine plan, without applying it.
///
/// Checks, in patch order: the patch is non-empty; no id repeats inside the
/// patch or collides with the plan; method ids avoid the P3 namespace; new
/// tasks are not `done` (the task-tracking boundary: the compiler reads
/// `done`, never sets it); new methods are `proposed` with provenance; every
/// task, capability, artifact, field, metric and resource reference resolves
/// against the plan plus the patch; method shapes match their kind (E013);
/// estimates follow the three-point rules (E011). The merged plan must still
/// pass `check`: the CLI probes that on a copy after applying, so coherence
/// problems the patch cannot see alone (dataflow E009, hierarchy cycles
/// E006) still refuse the write.
pub fn validate_patch(doc: &PlanDoc, patch: &pc_ir::Patch) -> Result<(), PatchError> {
    if patch.add_task.is_empty() && patch.add_method.is_empty() && patch.add_capability.is_empty() {
        return Err(PatchError::EmptyPatch);
    }
    let mut seen_tasks = BTreeSet::new();
    for task in &patch.add_task {
        if !seen_tasks.insert(task.id.as_str()) {
            return Err(PatchError::DuplicateInPatch {
                kind: "task".to_string(),
                id: task.id.clone(),
            });
        }
        if doc.task.iter().any(|t| t.id == task.id) {
            return Err(PatchError::AlreadyExists {
                kind: "task".to_string(),
                id: task.id.clone(),
            });
        }
    }
    let mut seen_methods = BTreeSet::new();
    for method in &patch.add_method {
        if !seen_methods.insert(method.id.as_str()) {
            return Err(PatchError::DuplicateInPatch {
                kind: "method".to_string(),
                id: method.id.clone(),
            });
        }
        if doc.method.iter().any(|m| m.id == method.id) {
            return Err(PatchError::AlreadyExists {
                kind: "method".to_string(),
                id: method.id.clone(),
            });
        }
    }
    let mut seen_caps = BTreeSet::new();
    for cap in &patch.add_capability {
        if !seen_caps.insert(cap.id.as_str()) {
            return Err(PatchError::DuplicateInPatch {
                kind: "capability".to_string(),
                id: cap.id.clone(),
            });
        }
        if doc.capability.iter().any(|c| c.id == cap.id) {
            return Err(PatchError::AlreadyExists {
                kind: "capability".to_string(),
                id: cap.id.clone(),
            });
        }
    }

    let tasks: BTreeSet<&str> = doc
        .task
        .iter()
        .map(|t| t.id.as_str())
        .chain(patch.add_task.iter().map(|t| t.id.as_str()))
        .collect();
    let capabilities: BTreeSet<&str> = doc
        .capability
        .iter()
        .map(|c| c.id.as_str())
        .chain(patch.add_capability.iter().map(|c| c.id.as_str()))
        .collect();
    let artifacts: BTreeMap<&str, &Vec<String>> = doc
        .artifact
        .iter()
        .map(|a| (a.name.as_str(), &a.fields))
        .collect();
    let resources: BTreeSet<&str> = doc.resource.iter().map(|r| r.name.as_str()).collect();

    for task in &patch.add_task {
        check_new_task(task, &artifacts)?;
    }
    for cap in &patch.add_capability {
        check_new_capability(cap, &artifacts, &resources)?;
    }
    for method in &patch.add_method {
        check_new_method(method, &tasks, &capabilities)?;
    }
    Ok(())
}

/// Shape and reference checks for one patched task.
fn check_new_task(task: &Task, artifacts: &BTreeMap<&str, &Vec<String>>) -> Result<(), PatchError> {
    if task.state == TaskState::Done {
        return Err(PatchError::TaskStateDone {
            id: task.id.clone(),
        });
    }
    let subject = format!("task:{}", task.id);
    for item in task.consumes.iter().chain(&task.produces) {
        check_contract_item(&subject, item, artifacts)?;
    }
    Ok(())
}

/// Shape and reference checks for one patched capability.
fn check_new_capability(
    cap: &Capability,
    artifacts: &BTreeMap<&str, &Vec<String>>,
    resources: &BTreeSet<&str>,
) -> Result<(), PatchError> {
    let subject = format!("capability:{}", cap.id);
    for item in cap.consumes.iter().chain(&cap.produces) {
        check_contract_item(&subject, item, artifacts)?;
    }
    for estimate in &cap.estimates {
        check_estimate(&subject, estimate)?;
    }
    for resource in &cap.resources {
        if !resources.contains(resource.as_str()) {
            return Err(PatchError::UnknownResource {
                subject: subject.clone(),
                resource: resource.clone(),
            });
        }
    }
    Ok(())
}

/// One contract entry names a declared artifact and declared fields (E004/E005).
fn check_contract_item(
    subject: &str,
    item: &ContractItem,
    artifacts: &BTreeMap<&str, &Vec<String>>,
) -> Result<(), PatchError> {
    let Some(declared) = artifacts.get(item.artifact.as_str()) else {
        return Err(PatchError::UnknownArtifact {
            subject: subject.to_string(),
            artifact: item.artifact.clone(),
        });
    };
    for field in &item.fields {
        if !declared.iter().any(|f| f == field) {
            return Err(PatchError::UnknownField {
                subject: subject.to_string(),
                artifact: item.artifact.clone(),
                field: field.clone(),
            });
        }
    }
    Ok(())
}

/// One estimate names a known metric and follows the three-point rules (E011).
fn check_estimate(subject: &str, estimate: &Estimate) -> Result<(), PatchError> {
    if !KNOWN_METRICS.contains(&estimate.metric.as_str()) {
        return Err(PatchError::UnknownMetric {
            subject: subject.to_string(),
            metric: estimate.metric.clone(),
        });
    }
    let mut bad: Option<String> = None;
    if ![estimate.low, estimate.likely, estimate.high]
        .iter()
        .all(|v| v.is_finite())
    {
        bad = Some("estimates must be finite numbers".to_string());
    } else if estimate.low > estimate.likely || estimate.likely > estimate.high {
        bad = Some(format!(
            "three-point estimate violates low <= likely <= high ({} <= {} <= {})",
            estimate.low, estimate.likely, estimate.high
        ));
    } else if estimate.low < 0.0 {
        bad = Some("estimates must be non-negative".to_string());
    } else if canonical_unit(&estimate.metric) == Some("ratio") && estimate.high > 1.0 {
        bad = Some(format!("{} estimates must lie in [0, 1]", estimate.metric));
    } else if estimate.provenance == pc_ir::EstimateProvenance::Measured
        && estimate.observations.unwrap_or(0) == 0
    {
        bad = Some("measured estimates must cite observations".to_string());
    }
    if let Some(reason) = bad {
        return Err(PatchError::BadEstimate {
            subject: format!("{subject} for `{}`", estimate.metric),
            reason,
        });
    }
    Ok(())
}

/// Status, provenance, shape and reference checks for one patched method.
fn check_new_method(
    method: &Method,
    tasks: &BTreeSet<&str>,
    capabilities: &BTreeSet<&str>,
) -> Result<(), PatchError> {
    if method.id.starts_with(MATCH_ID_PREFIX) {
        return Err(PatchError::ReservedMethodId {
            id: method.id.clone(),
        });
    }
    if method.status != MethodStatus::Proposed {
        return Err(PatchError::MethodNotProposed {
            id: method.id.clone(),
        });
    }
    if method.provenance.trim().is_empty() {
        return Err(PatchError::MethodMissingProvenance {
            id: method.id.clone(),
        });
    }
    if !tasks.contains(method.task.as_str()) {
        return Err(PatchError::UnknownTask {
            id: method.id.clone(),
            task: method.task.clone(),
        });
    }
    let shape = |reason: String| PatchError::BadMethodShape {
        id: method.id.clone(),
        reason,
    };
    match method.kind {
        MethodKind::Invoke => {
            let Some(cap) = method.capability.as_deref() else {
                return Err(shape(format!(
                    "invoke method `{}` names no capability",
                    method.id
                )));
            };
            if !capabilities.contains(cap) {
                return Err(PatchError::UnknownCapability {
                    id: method.id.clone(),
                    capability: cap.to_string(),
                });
            }
            if !method.subtasks.is_empty() || !method.order_edges.is_empty() {
                return Err(shape(format!(
                    "invoke method `{}` must not list subtasks or order edges",
                    method.id
                )));
            }
        }
        MethodKind::Decompose => {
            if method.capability.is_some() {
                return Err(shape(format!(
                    "decompose method `{}` must not name a capability",
                    method.id
                )));
            }
            if method.subtasks.is_empty() {
                return Err(shape(format!(
                    "decompose method `{}` lists no subtasks",
                    method.id
                )));
            }
            for sub in &method.subtasks {
                if !tasks.contains(sub.as_str()) {
                    return Err(PatchError::UnknownTask {
                        id: method.id.clone(),
                        task: sub.clone(),
                    });
                }
            }
            let members: BTreeSet<&str> = method.subtasks.iter().map(String::as_str).collect();
            let mut ends: Vec<(&str, &str)> = method
                .order_edges
                .iter()
                .map(|e| (e.from.as_str(), e.to.as_str()))
                .collect();
            ends.sort();
            for (from, to) in ends {
                for end in [from, to] {
                    if !members.contains(end) {
                        return Err(shape(format!(
                            "method `{}` orders `{end}`, which is not one of its subtasks",
                            method.id
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Apply a validated patch to the pristine plan.
///
/// Pushes tasks, then capabilities, then methods, so the file order stays
/// deterministic whatever order the patch listed them in. Returns one note
/// per added entity, sorted by id within each kind. Callers run `check` on a
/// copy afterwards and only write when it is clean.
pub fn apply_patch(doc: &mut PlanDoc, patch: pc_ir::Patch) -> Vec<String> {
    let mut applied = Vec::new();
    let mut tasks: Vec<Task> = patch.add_task;
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    for task in tasks {
        applied.push(format!("proposed task:{}", task.id));
        doc.task.push(task);
    }
    let mut capabilities: Vec<Capability> = patch.add_capability;
    capabilities.sort_by(|a, b| a.id.cmp(&b.id));
    for cap in capabilities {
        applied.push(format!("proposed capability:{}", cap.id));
        doc.capability.push(cap);
    }
    let mut methods: Vec<Method> = patch.add_method;
    methods.sort_by(|a, b| a.id.cmp(&b.id));
    for method in methods {
        applied.push(format!("proposed method:{}", method.id));
        doc.method.push(method);
    }
    applied.sort();
    applied
}

/// Decide one pending method: shared core of accept and reject.
///
/// `file_doc` is the pristine plan to edit; `checked_doc` is the same plan
/// after `check`, which carries the P3 derived proposals that exist only in
/// memory. A proposed (or file-persisted derived) method flips in place; a
/// derived method missing from the file materializes with the decided status
/// (so P3's id-collision guard keeps it from being proposed again).
/// Anything else is [`ImpactError`], the same refusal `pc impact` previews.
fn decide(
    file_doc: &mut PlanDoc,
    checked_doc: &PlanDoc,
    id: &str,
    to: MethodStatus,
    verb: &str,
) -> Result<String, ImpactError> {
    if let Some(method) = file_doc.method.iter_mut().find(|m| m.id == id) {
        if !matches!(
            method.status,
            MethodStatus::Proposed | MethodStatus::Derived
        ) {
            return Err(ImpactError::NotAcceptable {
                id: id.to_string(),
                status: status_str(method.status).to_string(),
            });
        }
        method.status = to;
        return Ok(format!("{verb} method:{id}"));
    }
    if let Some(derived) = checked_doc.method.iter().find(|m| m.id == id) {
        if derived.status != MethodStatus::Derived {
            return Err(ImpactError::NotAcceptable {
                id: id.to_string(),
                status: status_str(derived.status).to_string(),
            });
        }
        let mut materialized = derived.clone();
        materialized.status = to;
        file_doc.method.push(materialized);
        return Ok(format!("{verb} method:{id}"));
    }
    Err(ImpactError::UnknownMethod { id: id.to_string() })
}

/// Accept one pending method: flip a proposed or derived method to accepted.
///
/// Mirrors the `accept_method` preview in [`crate::impact`], but edits the
/// pristine plan instead of a copy. Callers probe `check` on the result and
/// only write when it is clean: accepting a substitution that drops a
/// contracted output is refused with the fresh E008 diagnostics.
pub fn accept_method(
    file_doc: &mut PlanDoc,
    checked_doc: &PlanDoc,
    id: &str,
) -> Result<String, ImpactError> {
    decide(
        file_doc,
        checked_doc,
        id,
        MethodStatus::Accepted,
        "accepted",
    )
}

/// Reject one pending method: flip a proposed or derived method to rejected.
///
/// Materializes in-memory derived proposals as rejected so the P3 matcher
/// does not propose them again on the next run.
pub fn reject_method(
    file_doc: &mut PlanDoc,
    checked_doc: &PlanDoc,
    id: &str,
) -> Result<String, ImpactError> {
    decide(
        file_doc,
        checked_doc,
        id,
        MethodStatus::Rejected,
        "rejected",
    )
}

/// Failures building a [`ContextReport`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContextError {
    /// The plan declares no such task.
    #[error("plan declares no task `{id}`")]
    UnknownTask {
        /// Task id the context named.
        id: String,
    },
}

/// Estimate the tokens a text costs inside an LLM prompt.
///
/// A fixed `chars / 4` approximation, documented as such: real model
/// tokenizers differ, but the estimate is deterministic, so the same plan
/// always yields the same slice under the same budget.
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.chars().count().div_ceil(4)).unwrap_or(u64::MAX)
}

/// Lowercase state name matching the IR serialization.
#[must_use]
pub fn task_state_str(state: TaskState) -> &'static str {
    match state {
        TaskState::Open => "open",
        TaskState::Decomposed => "decomposed",
        TaskState::Resolved => "resolved",
        TaskState::Done => "done",
        TaskState::Dropped => "dropped",
    }
}

/// Lowercase kind name matching the IR serialization.
#[must_use]
pub fn method_kind_str(kind: MethodKind) -> &'static str {
    match kind {
        MethodKind::Decompose => "decompose",
        MethodKind::Invoke => "invoke",
    }
}

/// One task in a context slice, with its contract and plan position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    /// Task id.
    pub id: String,
    /// Human title.
    pub title: String,
    /// Lifecycle state.
    pub state: String,
    /// Typed inputs.
    pub consumes: Vec<ContractItem>,
    /// Typed outputs.
    pub produces: Vec<ContractItem>,
    /// Chosen method, when the task is needed in the selected plan.
    pub selected_method: Option<String>,
    /// Whether the compiler left the task without an accepted method.
    pub unresolved: bool,
}

/// One method in a context slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodView {
    /// Method id.
    pub id: String,
    /// Task it achieves.
    pub task: String,
    /// `decompose` or `invoke`.
    pub kind: String,
    /// Lifecycle status.
    pub status: String,
    /// Who proposed it.
    pub provenance: String,
    /// Capability id, for invoke methods.
    pub capability: Option<String>,
    /// Child task ids, for decompose methods.
    pub subtasks: Vec<String>,
    /// Assumptions `explain` surfaces.
    pub assumptions: Vec<String>,
}

/// One capability in a context slice: contract plus declared estimates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityView {
    /// Capability id.
    pub id: String,
    /// Executor flavour.
    pub kind: String,
    /// Capability version.
    pub version: String,
    /// Typed inputs.
    pub consumes: Vec<ContractItem>,
    /// Typed outputs.
    pub produces: Vec<ContractItem>,
    /// Three-point estimates as written.
    pub estimates: Vec<Estimate>,
}

/// One parent of the focal task: the parent task plus the decompose method
/// linking them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParentView {
    /// Parent task.
    pub task: TaskView,
    /// Decompose method listing the focal task as a subtask.
    pub via_method: MethodView,
}

/// The compact slice an agent needs to propose the next decomposition of one
/// task: the task itself, its methods, the capabilities those methods invoke,
/// parent and child neighbours, the measured-evidence summary and the
/// proposal-staged evidence for the in-scope capabilities, and the frontier
/// entries touching the task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextReport {
    /// `task:<id>` of the focal task.
    pub subject: String,
    /// Token budget the slice was fitted to.
    pub budget_tokens: u64,
    /// Estimated tokens of the rendered slice.
    pub tokens: u64,
    /// True when sections were dropped to fit the budget.
    pub truncated: bool,
    /// The focal task. Always present, even over budget.
    pub task: TaskView,
    /// Non-rejected methods achieving the focal task, by id.
    pub methods: Vec<MethodView>,
    /// Capabilities those invoke methods call, by id.
    pub capabilities: Vec<CapabilityView>,
    /// Non-rejected decompose methods listing the focal task, with parents.
    pub parents: Vec<ParentView>,
    /// Subtasks of the focal task's decompose methods, by id.
    pub children: Vec<TaskView>,
    /// Measured-evidence summary for the in-scope capabilities.
    pub provenance: Vec<crate::compile::ProvenanceRow>,
    /// Ignored `task:<id>` evidence for the focal task, if any.
    pub unattributed: Vec<crate::estimate::UnattributedTaskEvidence>,
    /// Evidence staged with proposals for the in-scope capabilities: not
    /// counted until the proposal is accepted.
    pub staged: Vec<crate::estimate::StagedEvidence>,
    /// Frontier entries about the focal task or its methods, ranked order.
    pub frontier: Vec<FrontierItem>,
}

/// Build the full context slice for one task.
///
/// `doc` must already have passed through [`crate::passes::check`] (so P3
/// proposals are visible and P4 shares are merged); `compiled` is the
/// [`crate::compile`] result and `frontier` the [`crate::frontier`] report
/// over that same document. Only non-rejected methods are shown: rejected
/// alternatives are decided work, not planning surface. Every list is sorted
/// by id (frontier keeps its ranked order), so the same inputs always give a
/// byte-identical report.
pub fn context(
    doc: &PlanDoc,
    compiled: &CompiledPlan,
    frontier: &FrontierReport,
    task_id: &str,
) -> Result<ContextReport, ContextError> {
    let tasks: BTreeMap<&str, &pc_ir::Task> = doc.task.iter().map(|t| (t.id.as_str(), t)).collect();
    let focal = tasks
        .get(task_id)
        .copied()
        .ok_or(ContextError::UnknownTask {
            id: task_id.to_string(),
        })?;

    let mut methods: Vec<MethodView> = doc
        .method
        .iter()
        .filter(|m| m.task == task_id && m.status != MethodStatus::Rejected)
        .map(method_view)
        .collect();
    methods.sort_by(|a, b| a.id.cmp(&b.id));

    let invoked: BTreeSet<&str> = methods
        .iter()
        .filter_map(|m| m.capability.as_deref())
        .collect();
    let mut capabilities: Vec<CapabilityView> = doc
        .capability
        .iter()
        .filter(|c| invoked.contains(c.id.as_str()))
        .map(|c| CapabilityView {
            id: c.id.clone(),
            kind: format!("{:?}", c.kind).to_lowercase(),
            version: c.version.clone(),
            consumes: c.consumes.clone(),
            produces: c.produces.clone(),
            estimates: c.estimates.clone(),
        })
        .collect();
    capabilities.sort_by(|a, b| a.id.cmp(&b.id));

    let mut parents: Vec<ParentView> = doc
        .method
        .iter()
        .filter(|m| {
            m.kind == MethodKind::Decompose
                && m.status != MethodStatus::Rejected
                && m.subtasks.iter().any(|s| s == task_id)
        })
        .filter_map(|m| {
            tasks
                .get(m.task.as_str())
                .copied()
                .map(|parent| ParentView {
                    task: task_view(parent, compiled),
                    via_method: method_view(m),
                })
        })
        .collect();
    parents.sort_by(|a, b| {
        a.task
            .id
            .cmp(&b.task.id)
            .then_with(|| a.via_method.id.cmp(&b.via_method.id))
    });

    let mut child_ids = BTreeSet::new();
    for method in &doc.method {
        if method.task == task_id
            && method.kind == MethodKind::Decompose
            && method.status != MethodStatus::Rejected
        {
            child_ids.extend(method.subtasks.iter().cloned());
        }
    }
    let children: Vec<TaskView> = child_ids
        .into_iter()
        .filter_map(|id| tasks.get(id.as_str()).copied())
        .map(|t| task_view(t, compiled))
        .collect();

    let cap_ids: BTreeSet<&str> = capabilities.iter().map(|c| c.id.as_str()).collect();
    // The evidence summary names measured numbers only: declared triples are
    // already on the capability views, and the frontier flags unmeasured
    // critical-path cost separately.
    let mut provenance: Vec<crate::compile::ProvenanceRow> = compiled
        .provenance
        .iter()
        .filter(|row| cap_ids.contains(row.capability.as_str()) && row.measured.is_some())
        .cloned()
        .collect();
    provenance.sort_by(|a, b| {
        a.capability
            .cmp(&b.capability)
            .then_with(|| a.metric.cmp(&b.metric))
            .then_with(|| a.measurement_basis.cmp(&b.measurement_basis))
    });

    let unattributed: Vec<crate::estimate::UnattributedTaskEvidence> = compiled
        .unattributed_tasks
        .iter()
        .filter(|entry| entry.task == task_id)
        .cloned()
        .collect();

    let staged: Vec<crate::estimate::StagedEvidence> = compiled
        .staged_evidence
        .iter()
        .filter(|entry| cap_ids.contains(entry.capability.as_str()))
        .cloned()
        .collect();

    let method_subjects: BTreeSet<String> =
        methods.iter().map(|m| format!("method:{}", m.id)).collect();
    let focal_subject = format!("task:{task_id}");
    let frontier_items: Vec<FrontierItem> = frontier
        .items
        .iter()
        .filter(|item| item.subject == focal_subject || method_subjects.contains(&item.subject))
        .cloned()
        .collect();

    let mut report = ContextReport {
        subject: focal_subject,
        budget_tokens: u64::MAX,
        tokens: 0,
        truncated: false,
        task: task_view(focal, compiled),
        methods,
        capabilities,
        parents,
        children,
        provenance,
        unattributed,
        staged,
        frontier: frontier_items,
    };
    report.tokens = estimate_tokens(&render_context_text(&report));
    Ok(report)
}

/// Focal-task view with its position in the selected plan.
fn task_view(task: &pc_ir::Task, compiled: &CompiledPlan) -> TaskView {
    TaskView {
        id: task.id.clone(),
        title: task.title.clone(),
        state: task_state_str(task.state).to_string(),
        consumes: task.consumes.clone(),
        produces: task.produces.clone(),
        selected_method: compiled.selection.get(&task.id).cloned(),
        unresolved: compiled.unresolved_tasks.iter().any(|t| t == &task.id),
    }
}

/// Method view as stored, with enum names in their serialized lowercase form.
fn method_view(method: &pc_ir::Method) -> MethodView {
    MethodView {
        id: method.id.clone(),
        task: method.task.clone(),
        kind: method_kind_str(method.kind).to_string(),
        status: status_str(method.status).to_string(),
        provenance: method.provenance.clone(),
        capability: method.capability.clone(),
        subtasks: method.subtasks.clone(),
        assumptions: method.assumptions.clone(),
    }
}

/// Fit a full slice to a token budget by prefix: keep the focal task, then
/// add methods, capabilities, parents, children, evidence and frontier while
/// the rendered text fits. The first section that overflows stops the fill;
/// `truncated` records the drop. Deterministic: same report plus same budget
/// always yields the same slice.
pub fn fit_context(mut full: ContextReport, budget_tokens: u64) -> ContextReport {
    full.budget_tokens = budget_tokens;
    let empty: ContextReport = ContextReport {
        budget_tokens,
        tokens: 0,
        truncated: false,
        subject: full.subject.clone(),
        task: full.task.clone(),
        methods: Vec::new(),
        capabilities: Vec::new(),
        parents: Vec::new(),
        children: Vec::new(),
        provenance: Vec::new(),
        unattributed: Vec::new(),
        staged: Vec::new(),
        frontier: Vec::new(),
    };
    // The focal task always survives, even over budget: an empty slice helps
    // no agent.
    let mut fitted = empty.clone();
    fitted.tokens = estimate_tokens(&render_context_text(&fitted));
    // Sections in fill order. Evidence travels as one section: the measured
    // summary, the unattributed notes and the staged holdings explain each
    // other.
    let sections: [fn(&mut ContextReport, &ContextReport); 6] = [
        |f, r| f.methods = r.methods.clone(),
        |f, r| f.capabilities = r.capabilities.clone(),
        |f, r| f.parents = r.parents.clone(),
        |f, r| f.children = r.children.clone(),
        |f, r| {
            f.provenance = r.provenance.clone();
            f.unattributed = r.unattributed.clone();
            f.staged = r.staged.clone();
        },
        |f, r| f.frontier = r.frontier.clone(),
    ];
    let section_present: [fn(&ContextReport) -> bool; 6] = [
        |r| !r.methods.is_empty(),
        |r| !r.capabilities.is_empty(),
        |r| !r.parents.is_empty(),
        |r| !r.children.is_empty(),
        |r| !r.provenance.is_empty() || !r.unattributed.is_empty() || !r.staged.is_empty(),
        |r| !r.frontier.is_empty(),
    ];
    let mut kept: Vec<usize> = Vec::new();
    let mut dropped = false;
    for (index, (add, present)) in sections.iter().zip(section_present.iter()).enumerate() {
        if !present(&full) {
            continue;
        }
        let mut candidate = fitted.clone();
        add(&mut candidate, &full);
        if estimate_tokens(&render_context_text(&candidate)) <= budget_tokens {
            fitted = candidate;
            kept.push(index);
        } else {
            dropped = true;
            break;
        }
    }
    // The fill measured candidates without the final header (truncation flag
    // and token count), so re-render with it and shed the last-kept section
    // while the rendering still overflows. A later non-empty section also
    // counts as dropped when an earlier one overflowed: the fill stopped, so
    // everything after it is missing too.
    loop {
        let mut fitted = empty.clone();
        for &index in &kept {
            sections[index](&mut fitted, &full);
        }
        fitted.truncated = dropped;
        settle_tokens(&mut fitted);
        if !fitted.truncated && fitted.tokens > budget_tokens {
            fitted.truncated = true;
            settle_tokens(&mut fitted);
        }
        if fitted.tokens <= budget_tokens || kept.is_empty() {
            return fitted;
        }
        kept.pop();
        dropped = true;
    }
}

/// Re-estimate the header's own token count to a fixed point: digit-length
/// wobble settles within a couple of rounds.
fn settle_tokens(report: &mut ContextReport) {
    for _ in 0..3 {
        let restimated = estimate_tokens(&render_context_text(report));
        if restimated == report.tokens {
            break;
        }
        report.tokens = restimated;
    }
}

/// One contract list as `artifact[field, field]`, items comma-separated.
fn render_contract(items: &[ContractItem]) -> String {
    let mut rendered: Vec<String> = items
        .iter()
        .map(|item| format!("{}[{}]", item.artifact, item.fields.join(", ")))
        .collect();
    rendered.sort();
    rendered.join(", ")
}

/// Render a slice as compact agent-ergonomic text.
///
/// [`fit_context`] measures this rendering, so the text and the `--json`
/// struct always agree about what the budget holds.
#[must_use]
pub fn render_context_text(report: &ContextReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "context {} (budget {} tokens, ~{} used{})\n",
        report.subject,
        report.budget_tokens,
        report.tokens,
        if report.truncated { ", truncated" } else { "" }
    ));
    let task = &report.task;
    out.push_str(&format!(
        "task {}: {} [{}]\n",
        task.id, task.title, task.state
    ));
    if !task.consumes.is_empty() {
        out.push_str(&format!(
            "  consumes: {}\n",
            render_contract(&task.consumes)
        ));
    }
    if !task.produces.is_empty() {
        out.push_str(&format!(
            "  produces: {}\n",
            render_contract(&task.produces)
        ));
    }
    match &task.selected_method {
        Some(method) => out.push_str(&format!("  selected: {method}\n")),
        None if task.unresolved => out.push_str("  unresolved: no accepted method\n"),
        None => {}
    }
    for method in &report.methods {
        let by = if method.provenance.trim().is_empty() {
            "unknown provenance".to_string()
        } else {
            format!("by {}", method.provenance)
        };
        let shape = match method.capability.as_deref() {
            Some(cap) => format!("invoke capability:{cap}"),
            None => format!("decompose into {}", method.subtasks.join(", ")),
        };
        out.push_str(&format!(
            "method {} [{} {shape} {by}]\n",
            method.id, method.status
        ));
        for assumption in &method.assumptions {
            out.push_str(&format!("  assumes: {assumption}\n"));
        }
    }
    for cap in &report.capabilities {
        out.push_str(&format!(
            "capability {} [{} v{}]\n",
            cap.id, cap.kind, cap.version
        ));
        if !cap.consumes.is_empty() {
            out.push_str(&format!("  consumes: {}\n", render_contract(&cap.consumes)));
        }
        if !cap.produces.is_empty() {
            out.push_str(&format!("  produces: {}\n", render_contract(&cap.produces)));
        }
        for estimate in &cap.estimates {
            out.push_str(&format!(
                "  {}: {:.3}/{:.3}/{:.3} {}\n",
                estimate.metric,
                estimate.low,
                estimate.likely,
                estimate.high,
                match estimate.provenance {
                    pc_ir::EstimateProvenance::Declared => "declared".to_string(),
                    pc_ir::EstimateProvenance::Measured =>
                        format!("measured({})", estimate.observations.unwrap_or(0)),
                }
            ));
        }
    }
    for parent in &report.parents {
        out.push_str(&format!(
            "parent {} ({}) via {} [{}]\n",
            parent.task.id, parent.task.title, parent.via_method.id, parent.via_method.status
        ));
    }
    for child in &report.children {
        out.push_str(&format!(
            "child {}: {} [{}]\n",
            child.id, child.title, child.state
        ));
    }
    for row in &report.provenance {
        let measured = row
            .measured
            .map_or("no measured evidence".to_string(), |m| {
                format!(
                    "{:.3}/{:.3}/{:.3} (n={}{})",
                    m.low,
                    m.likely,
                    m.high,
                    row.observations,
                    if row.contradicts_declared {
                        ", contradicts declared"
                    } else {
                        ""
                    }
                )
            });
        out.push_str(&format!(
            "evidence {} {}: {measured} [basis {}]\n",
            row.capability,
            row.metric,
            row.measurement_basis.as_deref().unwrap_or("none"),
        ));
    }
    for entry in &report.unattributed {
        out.push_str(&format!(
            "evidence task:{} {}: {} observation{} ignored\n",
            entry.task,
            entry.metric,
            entry.observations,
            if entry.observations == 1 { "" } else { "s" }
        ));
    }
    for entry in &report.staged {
        out.push_str(&entry.line());
        out.push('\n');
    }
    for item in &report.frontier {
        out.push_str(&format!(
            "frontier {} [{}]{}: {}\n",
            item.subject,
            item.kind,
            if item.critical { " (critical)" } else { "" },
            item.reason
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE: &str = r#"
ir_version = 0

[plan]
id = "p"
title = "t"

[objective]
root_task = "job"
weights = { duration_minutes = 1.0 }

[[artifact]]
name = "doc"
fields = ["text"]

[[task]]
id = "job"
title = "Job"
state = "decomposed"

[[task]]
id = "draft"
title = "Draft"
state = "open"
produces = [{ artifact = "doc", fields = ["text"] }]

[[method]]
id = "job-all"
task = "job"
kind = "decompose"
subtasks = ["draft"]
status = "accepted"
provenance = "human:test"

[[capability]]
id = "writer@1"
kind = "tool"
version = "1"
produces = [{ artifact = "doc", fields = ["text"] }]
estimates = [{ metric = "duration_minutes", low = 2.0, likely = 4.0, high = 6.0, provenance = "declared" }]
"#;

    fn baseline() -> PlanDoc {
        pc_ir::from_toml(BASELINE).unwrap()
    }

    fn checked(doc: &mut PlanDoc) -> crate::passes::Report {
        let text = pc_ir::to_toml(doc).unwrap();
        let owned = text.clone();
        crate::passes::check("plan.toml", &owned, doc, None)
    }

    fn patch(text: &str) -> pc_ir::Patch {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn empty_patch_is_refused() {
        let doc = baseline();
        let err = validate_patch(&doc, &patch("{}")).unwrap_err();
        assert!(matches!(err, PatchError::EmptyPatch), "{err}");
    }

    #[test]
    fn duplicate_and_colliding_ids_are_refused() {
        let doc = baseline();
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_task": [
                    {"id": "a", "title": "A", "state": "open"},
                    {"id": "a", "title": "A2", "state": "open"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::DuplicateInPatch { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(r#"{"add_task": [{"id": "draft", "title": "D", "state": "open"}]}"#),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::AlreadyExists { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(r#"{"add_capability": [{"id": "writer@1", "kind": "tool", "version": "2"}]}"#),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::AlreadyExists { .. }), "{err}");
    }

    #[test]
    fn methods_must_be_proposed_with_provenance() {
        let doc = baseline();
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "m", "task": "draft", "kind": "invoke", "capability": "writer@1",
                     "status": "accepted", "provenance": "agent:t"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::MethodNotProposed { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "m", "task": "draft", "kind": "invoke", "capability": "writer@1",
                     "status": "proposed", "provenance": ""}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(
            matches!(err, PatchError::MethodMissingProvenance { .. }),
            "{err}"
        );
    }

    #[test]
    fn dangling_references_are_refused() {
        let doc = baseline();
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "m", "task": "ghost", "kind": "invoke", "capability": "writer@1",
                     "status": "proposed", "provenance": "agent:t"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownTask { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "m", "task": "draft", "kind": "invoke", "capability": "ghost@9",
                     "status": "proposed", "provenance": "agent:t"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownCapability { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "m", "task": "draft", "kind": "decompose", "subtasks": ["ghost"],
                     "status": "proposed", "provenance": "agent:t"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownTask { .. }), "{err}");
    }

    #[test]
    fn patch_may_reference_its_own_new_entities() {
        let doc = baseline();
        let result = validate_patch(
            &doc,
            &patch(
                r#"{"add_task": [{"id": "outline", "title": "Outline", "state": "open"}],
                    "add_method": [
                    {"id": "draft-plan", "task": "draft", "kind": "decompose",
                     "subtasks": ["outline"], "status": "proposed", "provenance": "agent:t"}
                ]}"#,
            ),
        );
        assert!(result.is_ok(), "{:?}", result.unwrap_err());
    }

    #[test]
    fn bad_shapes_and_reserved_ids_are_refused() {
        let doc = baseline();
        for (body, _) in [
            (
                r#"{"id": "m", "task": "draft", "kind": "invoke", "status": "proposed", "provenance": "agent:t"}"#,
                "invoke without capability",
            ),
            (
                r#"{"id": "m", "task": "draft", "kind": "decompose", "capability": "writer@1", "subtasks": ["draft"], "status": "proposed", "provenance": "agent:t"}"#,
                "decompose with capability",
            ),
            (
                r#"{"id": "m", "task": "draft", "kind": "decompose", "status": "proposed", "provenance": "agent:t"}"#,
                "decompose without subtasks",
            ),
        ] {
            let err = validate_patch(&doc, &patch(&format!(r#"{{"add_method": [{body}]}}"#)))
                .unwrap_err();
            assert!(matches!(err, PatchError::BadMethodShape { .. }), "{err}");
        }
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_method": [
                    {"id": "match-draft-writer@1", "task": "draft", "kind": "invoke",
                     "capability": "writer@1", "status": "proposed", "provenance": "agent:t"}
                ]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::ReservedMethodId { .. }), "{err}");
    }

    #[test]
    fn contracts_metrics_estimates_and_states_are_checked() {
        let doc = baseline();
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_task": [{"id": "a", "title": "A", "state": "open",
                    "consumes": [{"artifact": "ghost", "fields": ["text"]}]}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownArtifact { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_task": [{"id": "a", "title": "A", "state": "open",
                    "produces": [{"artifact": "doc", "fields": ["ghost"]}]}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownField { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_capability": [{"id": "c@1", "kind": "tool", "version": "1",
                    "estimates": [{"metric": "vibes", "low": 1.0, "likely": 1.0, "high": 1.0, "provenance": "declared"}]}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::UnknownMetric { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(
                r#"{"add_capability": [{"id": "c@1", "kind": "tool", "version": "1",
                    "estimates": [{"metric": "duration_minutes", "low": 9.0, "likely": 1.0, "high": 2.0, "provenance": "declared"}]}]}"#,
            ),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::BadEstimate { .. }), "{err}");
        let err = validate_patch(
            &doc,
            &patch(r#"{"add_task": [{"id": "a", "title": "A", "state": "done"}]}"#),
        )
        .unwrap_err();
        assert!(matches!(err, PatchError::TaskStateDone { .. }), "{err}");
    }

    #[test]
    fn apply_is_deterministic_over_patch_order() {
        let doc = baseline();
        let first = patch(
            r#"{"add_task": [
                {"id": "b-task", "title": "B", "state": "open"},
                {"id": "a-task", "title": "A", "state": "open"}
            ]}"#,
        );
        let second = patch(
            r#"{"add_task": [
                {"id": "a-task", "title": "A", "state": "open"},
                {"id": "b-task", "title": "B", "state": "open"}
            ]}"#,
        );
        let mut a = doc.clone();
        let mut b = doc.clone();
        let notes_a = apply_patch(&mut a, first);
        let notes_b = apply_patch(&mut b, second);
        assert_eq!(notes_a, notes_b);
        assert_eq!(
            notes_a,
            vec![
                "proposed task:a-task".to_string(),
                "proposed task:b-task".to_string(),
            ]
        );
    }

    #[test]
    fn accept_flips_proposed_and_materializes_derived() {
        let mut file = baseline();
        assert!(checked(&mut file.clone()).ok());
        let mut probed = file.clone();
        assert!(checked(&mut probed).ok());
        // Baseline has no proposals: accepting nothing is unknown.
        assert_eq!(
            accept_method(&mut file, &probed, "ghost"),
            Err(ImpactError::UnknownMethod { id: "ghost".into() })
        );

        // Stage a proposal the file way, then accept it.
        file.method.push(Method {
            id: "via-writer".into(),
            task: "draft".into(),
            kind: MethodKind::Invoke,
            subtasks: Vec::new(),
            order_edges: Vec::new(),
            capability: Some("writer@1".into()),
            status: MethodStatus::Proposed,
            provenance: "agent:test".into(),
            assumptions: Vec::new(),
        });
        let mut probed = file.clone();
        assert!(checked(&mut probed).ok());
        assert_eq!(
            accept_method(&mut file, &probed, "via-writer"),
            Ok("accepted method:via-writer".to_string())
        );
        assert_eq!(
            accept_method(&mut file, &probed, "via-writer"),
            Err(ImpactError::NotAcceptable {
                id: "via-writer".into(),
                status: "accepted".into(),
            })
        );

        // A P3 derived proposal materializes into the file as accepted.
        let mut file = baseline();
        let mut probed = file.clone();
        assert!(checked(&mut probed).ok());
        let derived: Vec<String> = probed
            .method
            .iter()
            .filter(|m| m.status == MethodStatus::Derived)
            .map(|m| m.id.clone())
            .collect();
        assert_eq!(derived, vec!["match-draft-writer@1".to_string()]);
        assert_eq!(
            accept_method(&mut file, &probed, "match-draft-writer@1"),
            Ok("accepted method:match-draft-writer@1".to_string())
        );
        let stored = file
            .method
            .iter()
            .find(|m| m.id == "match-draft-writer@1")
            .unwrap();
        assert_eq!(stored.status, MethodStatus::Accepted);
    }

    #[test]
    fn reject_flips_and_suppresses_rederivation() {
        let mut file = baseline();
        let mut probed = file.clone();
        assert!(checked(&mut probed).ok());
        assert_eq!(
            reject_method(&mut file, &probed, "match-draft-writer@1"),
            Ok("rejected method:match-draft-writer@1".to_string())
        );
        // The rejected materialization stays in the file, so P3's
        // id-collision guard keeps it from proposing the same id again.
        let mut reprobed = file.clone();
        let report = checked(&mut reprobed);
        assert!(report.ok(), "{:?}", report.diagnostics);
        let derived: Vec<&Method> = reprobed
            .method
            .iter()
            .filter(|m| m.status == MethodStatus::Derived)
            .collect();
        assert!(derived.is_empty(), "{derived:?}");
    }

    fn compiled(doc: &mut PlanDoc) -> CompiledPlan {
        assert!(checked(doc).ok());
        crate::compile(doc).unwrap()
    }

    fn full_context() -> (PlanDoc, CompiledPlan, FrontierReport) {
        let mut doc = baseline();
        let plan = compiled(&mut doc);
        let frontier = crate::frontier(&doc, &plan);
        (doc, plan, frontier)
    }

    #[test]
    fn context_names_task_neighbours_and_frontier() {
        let (doc, plan, frontier) = full_context();
        let report = context(&doc, &plan, &frontier, "draft").unwrap();
        assert_eq!(report.subject, "task:draft");
        assert_eq!(report.task.id, "draft");
        assert_eq!(report.task.selected_method, None);
        assert!(report.task.unresolved);
        assert_eq!(report.parents.len(), 1);
        assert_eq!(report.parents[0].task.id, "job");
        // The P3 derived proposal is planning surface too: it names the
        // covering capability, so the agent sees the Substitute option.
        assert_eq!(report.methods.len(), 1);
        assert_eq!(report.methods[0].id, "match-draft-writer@1");
        assert_eq!(report.capabilities.len(), 1);
        assert_eq!(report.capabilities[0].id, "writer@1");
        assert!(!report.frontier.is_empty());
        let kinds: Vec<&str> = report.frontier.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"open-task"));
        let err = context(&doc, &plan, &frontier, "ghost").unwrap_err();
        assert_eq!(err, ContextError::UnknownTask { id: "ghost".into() });
    }

    #[test]
    fn context_budget_fits_by_prefix_and_stays_deterministic() {
        let (doc, plan, frontier) = full_context();
        let full = context(&doc, &plan, &frontier, "job").unwrap();
        assert!(!full.truncated);
        let fitted = fit_context(full.clone(), u64::MAX);
        assert!(!fitted.truncated);
        assert_eq!(
            serde_json::to_vec(&fitted).unwrap(),
            serde_json::to_vec(&full).unwrap()
        );

        let tiny = fit_context(full.clone(), 10);
        assert!(tiny.truncated);
        assert_eq!(tiny.task.id, "job");
        assert!(tiny.methods.is_empty());
        assert_eq!(tiny.tokens, estimate_tokens(&render_context_text(&tiny)));

        // Same budget, same bytes, whatever the input order.
        let mut shuffled = doc.clone();
        shuffled.task.reverse();
        shuffled.method.reverse();
        let replan = compiled(&mut shuffled);
        let refrontier = crate::frontier(&shuffled, &replan);
        let other = fit_context(
            context(&shuffled, &replan, &refrontier, "job").unwrap(),
            2000,
        );
        let base = fit_context(full, 2000);
        assert_eq!(
            serde_json::to_vec(&other).unwrap(),
            serde_json::to_vec(&base).unwrap()
        );
    }

    #[test]
    fn context_fits_every_budget_including_its_final_header() {
        let (doc, plan, frontier) = full_context();
        let full = context(&doc, &plan, &frontier, "job").unwrap();
        let focal_only = fit_context(full.clone(), 0);
        for budget in 0..=full.tokens + 8 {
            let fitted = fit_context(full.clone(), budget);
            let rendered = estimate_tokens(&render_context_text(&fitted));
            assert_eq!(fitted.tokens, rendered, "budget {budget}");
            assert!(
                fitted.tokens <= budget || fitted.methods.is_empty() && budget < focal_only.tokens,
                "budget {budget} rendered {rendered} tokens"
            );
        }
    }

    #[test]
    fn token_estimate_is_four_chars_per_token() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
