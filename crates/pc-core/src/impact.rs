//! P9: impact and diff — evaluate a hypothetical change and compare two plans.
//!
//! Step 6 analysis passes: both run over checked plans and recompile with
//! P5-P7, so they stay deterministic and model-free like every other pass.
//! Neither writes to disk: `impact` applies a [`Change`] to an in-memory
//! copy, and `diff` only reads the two plans it is given.
//!
//! Impact lists affected tasks by reverse reachability (decompose ancestors
//! and descendants, plus one dataflow hop to the producers and consumers of
//! the touched tasks' fields) plus recompile, so a substitution that drops
//! an output names the consumers that would break.
//!
//! A [`Change`] is the JSON document `pc impact` reads:
//!
//! ```json
//! {
//!   "accept_method": ["via-hybrid"],
//!   "add_evidence": []
//! }
//! ```
//!
//! Both keys are optional and default to empty. `accept_method` flips
//! proposed or derived methods to accepted (the step-7 decision, previewed
//! here); `add_evidence` stages observations without touching the files.
//! Applying a change then re-running check surfaces exactly what accepting
//! it would do: new diagnostics (for example the E008 dropped-output error
//! when a substitution loses a field its consumers need), the affected
//! tasks by reverse reachability, and the recompiled selection and totals.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use pc_ir::{Evidence, Method, MethodKind, MethodStatus, PlanDoc, TaskState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compile::{CompiledPlan, EstimateRange, compile_with_budget};
use crate::diagnostic::Diagnostic;

/// A hypothetical plan change, read by `pc impact` from a JSON file.
///
/// Unknown fields are refused so a typo'd change never silently applies.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Change {
    /// Proposed or derived method ids to flip to accepted.
    pub accept_method: Vec<String>,
    /// Evidence observations to stage in memory.
    pub add_evidence: Vec<Evidence>,
}

/// Failures applying a [`Change`] to a checked plan.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImpactError {
    /// A named method does not exist in the checked plan.
    #[error("change names unknown method `{id}`")]
    UnknownMethod {
        /// Method id the change named.
        id: String,
    },
    /// A named method is not proposed or derived and so cannot be accepted.
    #[error(
        "change cannot accept method `{id}` with status `{status}` (only proposed or derived methods can be accepted)"
    )]
    NotAcceptable {
        /// Method id the change named.
        id: String,
        /// Its current status.
        status: String,
    },
}

/// Apply a [`Change`] to a checked plan in memory.
///
/// Returns one note per applied operation, in deterministic order
/// (accepts sorted by id, then evidence sorted by subject). Diagnostics are
/// left for the check that follows: this function only refuses operations
/// that name nothing.
pub fn apply_change(doc: &mut PlanDoc, change: &Change) -> Result<Vec<String>, ImpactError> {
    let mut applied = Vec::new();
    let mut accepts: Vec<&String> = change.accept_method.iter().collect();
    accepts.sort();
    for id in accepts {
        let Some(method) = doc.method.iter_mut().find(|m| &m.id == id) else {
            return Err(ImpactError::UnknownMethod { id: id.clone() });
        };
        if !matches!(
            method.status,
            MethodStatus::Proposed | MethodStatus::Derived
        ) {
            return Err(ImpactError::NotAcceptable {
                id: id.clone(),
                status: status_str(method.status).to_string(),
            });
        }
        method.status = MethodStatus::Accepted;
        applied.push(format!("accepted method:{id}"));
    }
    let mut evidence: Vec<&Evidence> = change.add_evidence.iter().collect();
    evidence.sort_by(|a, b| {
        a.subject
            .cmp(&b.subject)
            .then(a.metric.cmp(&b.metric))
            .then(a.value.total_cmp(&b.value))
    });
    for record in evidence {
        applied.push(format!(
            "added evidence:{}:{}",
            record.subject, record.metric
        ));
        doc.evidence.push(record.clone());
    }
    Ok(applied)
}

/// Lowercase status name matching the IR serialization.
#[must_use]
pub fn status_str(status: MethodStatus) -> &'static str {
    match status {
        MethodStatus::Accepted => "accepted",
        MethodStatus::Proposed => "proposed",
        MethodStatus::Derived => "derived",
        MethodStatus::Rejected => "rejected",
    }
}

/// One contract violation a change would introduce: an accepted
/// invoke method whose capability does not cover its task's contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractNote {
    /// Method that cannot be satisfied as written.
    pub method: String,
    /// Task the method achieves.
    pub task: String,
    /// Capability the method invokes.
    pub capability: String,
    /// `artifact.field` entries the task promises but the capability lacks.
    pub missing_outputs: Vec<String>,
    /// `artifact.field` entries the capability needs but the task and plan
    /// external inputs do not provide.
    pub missing_inputs: Vec<String>,
    /// Live tasks consuming a missing output field: thenamed consumers of
    /// the lost output.
    pub consumers: Vec<String>,
}

impl ContractNote {
    /// One-line reason for frontier entries naming the lost fields and their
    /// consumers.
    #[must_use]
    pub fn frontier_reason(&self) -> String {
        let mut missing = self.missing_outputs.clone();
        missing.extend(self.missing_inputs.iter().cloned());
        missing.sort();
        missing.dedup();
        let consumers = if self.consumers.is_empty() {
            "no live task consumes them".to_string()
        } else {
            format!("consumed by {}", self.consumers.join(", "))
        };
        format!(
            "accepting it would drop {} ({consumers})",
            missing.join(", ")
        )
    }
}

/// Contract analysis for one invoke method: `None` when the capability or
/// task is missing (P1/P2 report those) or the contract holds.
#[must_use]
pub fn contract_note_for(doc: &PlanDoc, method: &Method) -> Option<ContractNote> {
    if method.kind != MethodKind::Invoke {
        return None;
    }
    let capability_id = method.capability.as_deref()?;
    let capability = doc.capability.iter().find(|c| c.id == capability_id)?;
    let task = doc.task.iter().find(|t| t.id == method.task)?;
    let missing_outputs = crate::passes::missing_fields(&task.produces, &capability.produces);
    let mut available = task.consumes.clone();
    available.extend(doc.plan.external_inputs.clone());
    let missing_inputs = crate::passes::missing_fields(&capability.consumes, &available);
    if missing_outputs.is_empty() && missing_inputs.is_empty() {
        return None;
    }
    let mut consumers = BTreeSet::new();
    for missing in &missing_outputs {
        let (artifact, field) = missing.split_once('.').unwrap_or((missing.as_str(), ""));
        for other in &doc.task {
            if other.id == task.id || matches!(other.state, TaskState::Dropped) {
                continue;
            }
            let consumes = other
                .consumes
                .iter()
                .any(|item| item.artifact == artifact && item.fields.iter().any(|f| f == field));
            if consumes {
                consumers.insert(other.id.clone());
            }
        }
    }
    Some(ContractNote {
        method: method.id.clone(),
        task: task.id.clone(),
        capability: capability.id.clone(),
        missing_outputs,
        missing_inputs,
        consumers: consumers.into_iter().collect(),
    })
}

/// One method-choice difference between two compiled plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionChange {
    /// Task whose chosen method differs.
    pub task: String,
    /// Chosen method before, if the task was needed then.
    pub before: Option<String>,
    /// Chosen method after, if the task is needed now.
    pub after: Option<String>,
}

/// Expected-value movement for one metric between two compiled plans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    /// Metric name.
    pub metric: String,
    /// Expected value before.
    pub before: f64,
    /// Expected value after.
    pub after: f64,
    /// After minus before.
    pub delta: f64,
}

/// The compiled shape of one plan side: its selection, totals and makespan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanSummary {
    /// Chosen method per needed task.
    pub selection: BTreeMap<String, String>,
    /// Effective roll-up ranges by metric.
    pub totals: BTreeMap<String, EstimateRange>,
    /// Expected makespan in minutes.
    pub makespan_minutes: f64,
}

/// Summarize one compiled plan for impact and diff reports.
#[must_use]
pub fn summarize(compiled: &CompiledPlan) -> PlanSummary {
    PlanSummary {
        selection: compiled.selection.clone(),
        totals: compiled.totals.clone(),
        makespan_minutes: compiled.makespan_minutes,
    }
}

/// Selection differences between two summaries, by task id.
#[must_use]
pub fn selection_changes(before: &PlanSummary, after: &PlanSummary) -> Vec<SelectionChange> {
    let mut tasks: BTreeSet<&String> = before
        .selection
        .keys()
        .chain(after.selection.keys())
        .collect();
    let mut out = Vec::new();
    for task in std::mem::take(&mut tasks) {
        let (a, b) = (
            before.selection.get(task).cloned(),
            after.selection.get(task).cloned(),
        );
        if a != b {
            out.push(SelectionChange {
                task: task.clone(),
                before: a,
                after: b,
            });
        }
    }
    out.sort_by(|a, b| a.task.cmp(&b.task));
    out
}

/// Expected-value movement per metric between two summaries.
#[must_use]
pub fn totals_delta(before: &PlanSummary, after: &PlanSummary) -> Vec<MetricDelta> {
    let mut metrics: BTreeSet<&String> = before.totals.keys().chain(after.totals.keys()).collect();
    let mut out = Vec::new();
    for metric in std::mem::take(&mut metrics) {
        let (a, b) = (
            before.totals.get(metric).map_or(0.0, |r| r.expected),
            after.totals.get(metric).map_or(0.0, |r| r.expected),
        );
        out.push(MetricDelta {
            metric: metric.clone(),
            before: a,
            after: b,
            delta: b - a,
        });
    }
    out.sort_by(|a, b| a.metric.cmp(&b.metric));
    out
}

/// The result of previewing a [`Change`]: affected tasks by reverse
/// reachability, fresh diagnostics, and the recompiled plan when clean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactReport {
    /// Notes for each applied operation, in deterministic order.
    pub applied: Vec<String>,
    /// Reachable tasks depending on (or depended on by) the change, sorted.
    pub affected_tasks: Vec<String>,
    /// Check diagnostics over the modified plan. Empty means the change is
    /// contract-clean.
    pub diagnostics: Vec<Diagnostic>,
    /// True when the modified plan checks clean.
    pub clean: bool,
    /// Baseline summary, when the unmodified plan compiles.
    pub baseline: Option<PlanSummary>,
    /// Modified-plan summary, when it checks clean and compiles.
    pub revised: Option<PlanSummary>,
    /// Compile failure for the modified plan, if check passed but P6/P7 failed.
    pub revised_error: Option<String>,
    /// Method-choice differences, when both sides compile.
    pub selection_changes: Vec<SelectionChange>,
    /// Expected-value movement per metric, when both sides compile.
    pub totals_delta: Vec<MetricDelta>,
    /// Makespan movement in minutes, when both sides compile.
    pub makespan_delta: Option<f64>,
    /// Contract violations the change would introduce, with consumers.
    pub contract_notes: Vec<ContractNote>,
}

/// Preview a [`Change`] against a checked baseline plan.
///
/// The baseline is re-checked and recompiled in memory (a checked plan
/// re-checks identically), then the change is applied to a copy and the
/// copy is checked and, when clean, recompiled with `budget`. `file` and
/// `text` anchor diagnostics on both sides: a change flips statuses and adds
/// evidence without moving any ids, so lines in the original file stay
/// valid, and staged evidence absent from it gets no line.
pub fn impact(
    baseline: &PlanDoc,
    change: &Change,
    file: &str,
    text: &str,
    budget: Duration,
) -> Result<ImpactReport, ImpactError> {
    let mut base_copy = baseline.clone();
    crate::passes::check(file, text, &mut base_copy, None);
    let baseline_summary = compile_with_budget(&base_copy, budget)
        .ok()
        .map(|compiled| summarize(&compiled));

    let mut modified = baseline.clone();
    let applied = apply_change(&mut modified, change)?;
    let modified_report = crate::passes::check(file, text, &mut modified, None);
    let clean = modified_report.diagnostics.is_empty();
    let affected_tasks = affected(&modified, change);
    let mut contract_notes = Vec::new();
    if !clean {
        let mut methods: Vec<&Method> = modified
            .method
            .iter()
            .filter(|m| matches!(m.status, MethodStatus::Accepted | MethodStatus::Derived))
            .collect();
        methods.sort_by(|a, b| a.id.cmp(&b.id));
        for method in methods {
            if let Some(note) = contract_note_for(&modified, method) {
                contract_notes.push(note);
            }
        }
        contract_notes.sort_by(|a, b| a.method.cmp(&b.method));
    }
    let (revised, revised_error) = if clean {
        match compile_with_budget(&modified, budget) {
            Ok(compiled) => (Some(summarize(&compiled)), None),
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    let (selection_changes, totals_delta, makespan_delta) = match (&baseline_summary, &revised) {
        (Some(before), Some(after)) => (
            selection_changes(before, after),
            totals_delta(before, after),
            Some(after.makespan_minutes - before.makespan_minutes),
        ),
        _ => (Vec::new(), Vec::new(), None),
    };
    Ok(ImpactReport {
        applied,
        affected_tasks,
        diagnostics: modified_report.diagnostics,
        clean,
        baseline: baseline_summary,
        revised,
        revised_error,
        selection_changes,
        totals_delta,
        makespan_delta,
        contract_notes,
    })
}

/// Tasks affected by a change: the seed tasks it touches directly, their
/// transitive ancestors and descendants through non-rejected decompose
/// edges, their dataflow neighbours (tasks producing a field a seed
/// consumes, or consuming a field a seed produces). Sorted.
fn affected(modified: &PlanDoc, change: &Change) -> Vec<String> {
    let mut seeds = BTreeSet::new();
    let tasks_with_invoke: BTreeMap<&str, Vec<&str>> = {
        let mut map: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for method in &modified.method {
            if method.kind == MethodKind::Invoke
                && method.status != MethodStatus::Rejected
                && let Some(cap) = method.capability.as_deref()
            {
                map.entry(cap).or_default().push(method.task.as_str());
            }
        }
        map
    };
    let method_task: BTreeMap<&str, &str> = modified
        .method
        .iter()
        .map(|m| (m.id.as_str(), m.task.as_str()))
        .collect();
    for id in &change.accept_method {
        if let Some(task) = method_task.get(id.as_str()) {
            seeds.insert((*task).to_string());
        }
    }
    for record in &change.add_evidence {
        match record.subject.split_once(':') {
            Some(("capability", id)) => {
                if let Some(tasks) = tasks_with_invoke.get(id) {
                    seeds.extend(tasks.iter().map(|s| (*s).to_string()));
                }
            }
            Some(("method", id)) => {
                if let Some(task) = method_task.get(id) {
                    seeds.insert((*task).to_string());
                }
            }
            Some(("task", id)) => {
                seeds.insert(id.to_string());
            }
            _ => {}
        }
    }
    // One dataflow hop: producers of what the seeds consume, and consumers
    // of what the seeds produce. A dropped-output substitution affects its
    // consumers even when no decomposition edge links them.
    seeds = with_dataflow_neighbours(modified, &seeds);
    closure(modified, &seeds)
}

/// Expand `seeds` by one dataflow hop over live tasks: for every artifact
/// field a seed produces, the tasks consuming it; for every field it
/// consumes, the tasks producing it. Dropped tasks are abandoned work and
/// are never neighbours.
fn with_dataflow_neighbours(doc: &PlanDoc, seeds: &BTreeSet<String>) -> BTreeSet<String> {
    let mut produced: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut consumed: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for task in &doc.task {
        if matches!(task.state, TaskState::Dropped) {
            continue;
        }
        for item in &task.produces {
            for field in &item.fields {
                produced
                    .entry((item.artifact.clone(), field.clone()))
                    .or_default()
                    .push(task.id.clone());
            }
        }
        for item in &task.consumes {
            for field in &item.fields {
                consumed
                    .entry((item.artifact.clone(), field.clone()))
                    .or_default()
                    .push(task.id.clone());
            }
        }
    }
    let by_id: BTreeMap<&str, &pc_ir::Task> = doc.task.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut expanded = seeds.clone();
    for seed in seeds {
        let Some(task) = by_id.get(seed.as_str()).copied() else {
            continue;
        };
        for item in task.produces.iter().chain(&task.consumes) {
            for field in &item.fields {
                let key = (item.artifact.clone(), field.clone());
                expanded.extend(produced.get(&key).iter().flat_map(|v| v.iter()).cloned());
                expanded.extend(consumed.get(&key).iter().flat_map(|v| v.iter()).cloned());
            }
        }
    }
    expanded
}

/// Transitive closure of `seeds` through non-rejected decompose edges, both
/// directions, restricted to tasks in the plan. Sorted.
fn closure(doc: &PlanDoc, seeds: &BTreeSet<String>) -> Vec<String> {
    let present: BTreeSet<&str> = doc.task.iter().map(|t| t.id.as_str()).collect();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for method in &doc.method {
        if method.kind != MethodKind::Decompose || method.status == MethodStatus::Rejected {
            continue;
        }
        for sub in &method.subtasks {
            children
                .entry(method.task.as_str())
                .or_default()
                .push(sub.as_str());
            parents
                .entry(sub.as_str())
                .or_default()
                .push(method.task.as_str());
        }
    }
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&str> = seeds.iter().map(String::as_str).collect();
    while let Some(id) = stack.pop() {
        if !present.contains(id) || !seen.insert(id) {
            continue;
        }
        stack.extend(children.get(id).cloned().unwrap_or_default());
        stack.extend(parents.get(id).cloned().unwrap_or_default());
    }
    seen.into_iter().map(str::to_string).collect()
}

/// One method status movement between two plans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusChange {
    /// Method id.
    pub method: String,
    /// Status before.
    pub before: String,
    /// Status after.
    pub after: String,
}

/// One method whose content (beyond a pure status flip) differs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodContentChange {
    /// Method id.
    pub method: String,
    /// Method before.
    pub before: Method,
    /// Method after.
    pub after: Method,
}

/// Diagnostic identity ignoring file paths and line numbers: the same
/// mistake in two plan files is the same diagnostic for diff purposes.
fn diagnostic_key(diag: &Diagnostic) -> (&str, &str, &str) {
    (
        diag.code.as_str(),
        diag.subject.as_str(),
        diag.message.as_str(),
    )
}

/// The structural and compiled differences between two checked plans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffReport {
    /// Task ids only in the second plan.
    pub tasks_added: Vec<String>,
    /// Task ids only in the first plan.
    pub tasks_removed: Vec<String>,
    /// Task ids in both with differing content (contracts, state, titles).
    pub tasks_changed: Vec<String>,
    /// Capability ids only in the second plan.
    pub capabilities_added: Vec<String>,
    /// Capability ids only in the first plan.
    pub capabilities_removed: Vec<String>,
    /// Capability ids in both with differing content.
    pub capabilities_changed: Vec<String>,
    /// Method ids only in the second plan.
    pub methods_added: Vec<String>,
    /// Method ids only in the first plan.
    pub methods_removed: Vec<String>,
    /// Methods whose status alone flipped.
    pub method_status_changes: Vec<StatusChange>,
    /// Methods whose content beyond status differs.
    pub method_content_changes: Vec<MethodContentChange>,
    /// Method-choice differences, when both sides compile.
    pub selection_changes: Vec<SelectionChange>,
    /// Makespan before, when the first plan compiles.
    pub makespan_before: Option<f64>,
    /// Makespan after, when the second plan compiles.
    pub makespan_after: Option<f64>,
    /// After minus before, when both plans compile.
    pub makespan_delta: Option<f64>,
    /// Expected-value movement per metric, when both compile.
    pub totals_delta: Vec<MetricDelta>,
    /// Diagnostics (by code, subject and message) only in the second plan.
    pub diagnostics_added: Vec<Diagnostic>,
    /// Diagnostics (by code, subject and message) only in the first plan.
    pub diagnostics_removed: Vec<Diagnostic>,
}

/// Compare two checked plans: structure first, then compiled selection and
/// totals when the respective side compiles.
///
/// `a_comp`/`b_comp` are `None` for sides that failed check or selection;
/// structural and diagnostic diffs are still reported for those sides.
#[must_use]
pub fn diff(
    a_doc: &PlanDoc,
    a_comp: Option<&CompiledPlan>,
    a_diags: &[Diagnostic],
    b_doc: &PlanDoc,
    b_comp: Option<&CompiledPlan>,
    b_diags: &[Diagnostic],
) -> DiffReport {
    let task_diff = entity_diff(
        &a_doc.task.iter().map(|t| (t.id.clone(), t)).collect(),
        &b_doc.task.iter().map(|t| (t.id.clone(), t)).collect(),
    );
    let cap_diff = entity_diff(
        &a_doc.capability.iter().map(|c| (c.id.clone(), c)).collect(),
        &b_doc.capability.iter().map(|c| (c.id.clone(), c)).collect(),
    );
    let a_methods: BTreeMap<String, &Method> =
        a_doc.method.iter().map(|m| (m.id.clone(), m)).collect();
    let b_methods: BTreeMap<String, &Method> =
        b_doc.method.iter().map(|m| (m.id.clone(), m)).collect();
    let mut ids: BTreeSet<&String> = a_methods.keys().chain(b_methods.keys()).collect();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut status_changes = Vec::new();
    let mut content_changes = Vec::new();
    for id in std::mem::take(&mut ids) {
        match (a_methods.get(id), b_methods.get(id)) {
            (None, Some(_)) => added.push((*id).clone()),
            (Some(_), None) => removed.push((*id).clone()),
            (Some(a), Some(b)) => {
                if a == b {
                    continue;
                }
                let mut rest_a = (*a).clone();
                let mut rest_b = (*b).clone();
                rest_a.status = MethodStatus::Accepted;
                rest_b.status = MethodStatus::Accepted;
                if rest_a == rest_b {
                    status_changes.push(StatusChange {
                        method: (*id).clone(),
                        before: status_str(a.status).to_string(),
                        after: status_str(b.status).to_string(),
                    });
                } else {
                    content_changes.push(MethodContentChange {
                        method: (*id).clone(),
                        before: (*a).clone(),
                        after: (*b).clone(),
                    });
                }
            }
            (None, None) => {}
        }
    }
    let makespan_before = a_comp.map(|a| a.makespan_minutes);
    let makespan_after = b_comp.map(|b| b.makespan_minutes);
    let (selection_changes, makespan_delta, totals_delta) = match (a_comp, b_comp) {
        (Some(a), Some(b)) => {
            let (before, after) = (summarize(a), summarize(b));
            (
                selection_changes(&before, &after),
                Some(after.makespan_minutes - before.makespan_minutes),
                totals_delta(&before, &after),
            )
        }
        _ => (Vec::new(), None, Vec::new()),
    };
    let a_keys: BTreeSet<_> = a_diags.iter().map(diagnostic_key).collect();
    let b_keys: BTreeSet<_> = b_diags.iter().map(diagnostic_key).collect();
    DiffReport {
        tasks_added: task_diff.0,
        tasks_removed: task_diff.1,
        tasks_changed: task_diff.2,
        capabilities_added: cap_diff.0,
        capabilities_removed: cap_diff.1,
        capabilities_changed: cap_diff.2,
        methods_added: added,
        methods_removed: removed,
        method_status_changes: status_changes,
        method_content_changes: content_changes,
        selection_changes,
        makespan_before,
        makespan_after,
        makespan_delta,
        totals_delta,
        diagnostics_added: b_diags
            .iter()
            .filter(|d| !a_keys.contains(&diagnostic_key(d)))
            .cloned()
            .collect(),
        diagnostics_removed: a_diags
            .iter()
            .filter(|d| !b_keys.contains(&diagnostic_key(d)))
            .cloned()
            .collect(),
    }
}

/// Added, removed and changed ids between two entity maps.
fn entity_diff<T: PartialEq>(
    a: &BTreeMap<String, &T>,
    b: &BTreeMap<String, &T>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut ids: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    for id in std::mem::take(&mut ids) {
        match (a.get(id), b.get(id)) {
            (None, Some(_)) => added.push((*id).clone()),
            (Some(_), None) => removed.push((*id).clone()),
            (Some(x), Some(y)) => {
                if x != y {
                    changed.push((*id).clone());
                }
            }
            (None, None) => {}
        }
    }
    (added, removed, changed)
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
name = "artifact"
fields = ["kept", "dropped"]

[[task]]
id = "job"
title = "Job"
state = "resolved"
produces = [{ artifact = "artifact", fields = ["kept", "dropped"] }]

[[task]]
id = "downstream"
title = "Downstream"
state = "open"
consumes = [{ artifact = "artifact", fields = ["dropped"] }]

[[method]]
id = "via-full"
task = "job"
kind = "invoke"
capability = "full@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "via-partial"
task = "job"
kind = "invoke"
capability = "partial@1"
status = "proposed"
provenance = "agent:test"

[[capability]]
id = "full@1"
kind = "tool"
version = "1"
produces = [{ artifact = "artifact", fields = ["kept", "dropped"] }]
estimates = [{ metric = "duration_minutes", low = 8.0, likely = 10.0, high = 12.0, provenance = "declared" }]

[[capability]]
id = "partial@1"
kind = "tool"
version = "1"
produces = [{ artifact = "artifact", fields = ["kept"] }]
estimates = [{ metric = "duration_minutes", low = 2.0, likely = 3.0, high = 4.0, provenance = "declared" }]
"#;

    fn baseline() -> PlanDoc {
        pc_ir::from_toml(BASELINE).unwrap()
    }

    fn checked(doc: &mut PlanDoc) -> crate::passes::Report {
        let text = pc_ir::to_toml(doc).unwrap();
        let owned = text.clone();
        crate::passes::check("plan.toml", &owned, doc, None)
    }

    #[test]
    fn accepting_a_lossy_method_reports_e008_with_consumers() {
        let mut plan = baseline();
        assert!(checked(&mut plan).ok());
        let change = Change {
            accept_method: vec!["via-partial".into()],
            ..Change::default()
        };
        let report = impact(
            &plan,
            &change,
            "plan.toml",
            &pc_ir::to_toml(&plan).unwrap(),
            Duration::from_millis(2000),
        )
        .unwrap();
        assert!(!report.clean);
        assert_eq!(
            report.applied,
            vec!["accepted method:via-partial".to_string()]
        );
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "E008" && d.message.contains("artifact.dropped")),
            "{:?}",
            report.diagnostics
        );
        assert_eq!(
            report.affected_tasks,
            vec!["downstream".to_string(), "job".to_string()]
        );
        assert_eq!(report.contract_notes.len(), 1);
        let note = &report.contract_notes[0];
        assert_eq!(note.method, "via-partial");
        assert_eq!(note.missing_outputs, vec!["artifact.dropped".to_string()]);
        assert_eq!(note.consumers, vec!["downstream".to_string()]);
        assert!(report.revised.is_none());
    }

    #[test]
    fn modified_plan_diagnostics_anchor_to_the_original_file() {
        let mut plan = baseline();
        assert!(checked(&mut plan).ok());
        let change = Change {
            accept_method: vec!["via-partial".into()],
            ..Change::default()
        };
        let report = impact(
            &plan,
            &change,
            "plan.toml",
            BASELINE,
            Duration::from_millis(2000),
        )
        .unwrap();
        let e008 = report
            .diagnostics
            .iter()
            .find(|d| d.code == "E008")
            .expect("E008");
        let expected = BASELINE
            .lines()
            .position(|line| line.contains("\"via-partial\""))
            .map(|i| u32::try_from(i + 1).unwrap());
        assert_eq!(e008.line, expected);
    }

    #[test]
    fn staged_evidence_flips_selection_and_reports_deltas() {
        let mut plan = baseline();
        // Drop the consumer so the lossy method can be accepted cleanly and
        // P6 can compare the two invokes on duration evidence alone.
        plan.task.retain(|t| t.id != "downstream");
        plan.task
            .iter_mut()
            .find(|t| t.id == "job")
            .unwrap()
            .produces = vec![pc_ir::ContractItem {
            artifact: "artifact".into(),
            fields: vec!["kept".into()],
        }];
        assert!(checked(&mut plan).ok());
        let change = Change {
            accept_method: vec!["via-partial".into()],
            add_evidence: vec![
                pc_ir::Evidence {
                    subject: "capability:partial@1".into(),
                    metric: "duration_minutes".into(),
                    value: 3.0,
                    unit: "minutes".into(),
                    time: "2026-10-01T00:00:00Z".into(),
                    source: "test".into(),
                    artifact_hash: None,
                },
                pc_ir::Evidence {
                    subject: "capability:full@1".into(),
                    metric: "duration_minutes".into(),
                    value: 10.0,
                    unit: "minutes".into(),
                    time: "2026-10-01T00:00:00Z".into(),
                    source: "test".into(),
                    artifact_hash: None,
                },
            ],
        };
        let report = impact(
            &plan,
            &change,
            "plan.toml",
            &pc_ir::to_toml(&plan).unwrap(),
            Duration::from_millis(2000),
        )
        .unwrap();
        assert!(report.clean, "{:?}", report.diagnostics);
        let revised = report.revised.unwrap();
        assert_eq!(revised.selection["job"], "via-partial");
        assert_eq!(
            report.selection_changes,
            vec![SelectionChange {
                task: "job".into(),
                before: Some("via-full".into()),
                after: Some("via-partial".into()),
            }]
        );
        let duration = report
            .totals_delta
            .iter()
            .find(|d| d.metric == "duration_minutes")
            .unwrap();
        assert!(duration.delta < 0.0);
        assert!(report.makespan_delta.unwrap() < 0.0);
    }

    #[test]
    fn unknown_and_committed_methods_are_refused() {
        let mut plan = baseline();
        assert!(checked(&mut plan).ok());
        let change = Change {
            accept_method: vec!["ghost".into()],
            ..Change::default()
        };
        assert_eq!(
            impact(&plan, &change, "plan.toml", "", Duration::from_millis(100)),
            Err(ImpactError::UnknownMethod { id: "ghost".into() })
        );
        let change = Change {
            accept_method: vec!["via-full".into()],
            ..Change::default()
        };
        assert_eq!(
            impact(&plan, &change, "plan.toml", "", Duration::from_millis(100)),
            Err(ImpactError::NotAcceptable {
                id: "via-full".into(),
                status: "accepted".into(),
            })
        );
    }

    #[test]
    fn diff_reports_structure_selection_and_diagnostics() {
        let mut a = baseline();
        assert!(checked(&mut a).ok());
        let a_comp = compile_with_budget(&a, Duration::from_millis(2000)).unwrap();
        let mut b = a.clone();
        b.method
            .iter_mut()
            .find(|m| m.id == "via-partial")
            .unwrap()
            .status = MethodStatus::Accepted;
        // Re-check the edited copy the way the CLI would.
        let b_text = pc_ir::to_toml(&b).unwrap();
        let b_text_owned = b_text.clone();
        let b_diags = crate::passes::check("b.toml", &b_text_owned, &mut b, None).diagnostics;
        assert!(!b_diags.is_empty());
        let report = diff(&a, Some(&a_comp), &[], &b, None, &b_diags);
        assert_eq!(report.method_status_changes.len(), 1);
        assert_eq!(report.method_status_changes[0].method, "via-partial");
        assert_eq!(report.diagnostics_added.len(), 1);
        assert_eq!(report.diagnostics_added[0].code, "E008");
        assert!(report.selection_changes.is_empty());
        assert_eq!(report.makespan_before, Some(a_comp.makespan_minutes));
        assert_eq!(report.makespan_after, None);
        assert_eq!(report.makespan_delta, None);
    }
}
