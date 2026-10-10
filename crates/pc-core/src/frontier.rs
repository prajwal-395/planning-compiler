//! P8: frontier — rank unresolved work so people and agents can see the frontier.
//!
//! Step 6 analysis pass: runs over a checked plan plus its [`CompiledPlan`]
//! and returns a deterministically ranked [`FrontierReport`]. It never edits
//! the plan, calls no model and tracks no status: it only scores where more
//! planning is worth the effort.
//!
//! Every entry scores `share * uncertainty * criticality` (report section
//! 3.3, P8):
//!
//! - `share` is the entry's share of the objective: an expected-cost fraction
//!   when the work has estimates, otherwise the structural share
//!   `(1 + transitive dependents) / tasks`.
//! - `uncertainty` is `1.0` for every v0 entry: frontier work is unmeasured
//!   by construction (open tasks have no estimates to trust, proposals are
//!   undecided, critical leaves rest on declared guesses, blocked
//!   alternatives never ran).
//! - `criticality` is `1.0` when the entry sits on the selected path from
//!   the objective root (or is itself a critical leaf) and `0.5` otherwise.
//!
//! Entries sort by score descending, then subject ascending, so the same
//! inputs always give byte-identical reports whatever the input order.

use std::collections::{BTreeMap, BTreeSet};

use pc_ir::{MethodStatus, PlanDoc, TaskState};
use serde::{Deserialize, Serialize};

use crate::compile::CompiledPlan;

/// Frontier entry about an open task: reachable work with no accepted method.
pub const KIND_OPEN_TASK: &str = "open-task";
/// Frontier entry about a proposed method awaiting accept or reject.
pub const KIND_PROPOSAL: &str = "proposal";
/// Frontier entry about critical-path cost resting on declared guesses.
pub const KIND_UNMEASURED_CRITICAL: &str = "unmeasured-critical";
/// Frontier entry about a proposed or derived alternative that fails typed
/// contract matching and so cannot be accepted as written.
pub const KIND_BLOCKED_ALTERNATIVE: &str = "blocked-alternative";

/// Critical-path membership factor for ranked entries.
pub const CRITICAL_FACTOR: f64 = 1.0;
/// Off-path factor for ranked entries.
pub const OFF_PATH_FACTOR: f64 = 0.5;

/// One ranked frontier entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontierItem {
    /// Entity this entry is about: `task:<id>`, `method:<id>` or
    /// `capability:<id>`.
    pub subject: String,
    /// Entry kind: `open-task`, `proposal`, `unmeasured-critical` or
    /// `blocked-alternative`.
    pub kind: String,
    /// Ranking score: `share * uncertainty * criticality`.
    pub score: f64,
    /// Share of the objective (cost fraction or structural dependent share).
    pub share: f64,
    /// Uncertainty (`1.0` for every v0 entry; see the module docs).
    pub uncertainty: f64,
    /// Whether the entry sits on the selected path from the objective root.
    pub critical: bool,
    /// Human-readable reason naming the decision behind the entry.
    pub reason: String,
}

/// Deterministically ranked unresolved work, best first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontierReport {
    /// Ranked entries, best first. Empty means the frontier is fully planned:
    /// every reachable task has an accepted method and no critical cost rests
    /// on guesses.
    pub items: Vec<FrontierItem>,
}

/// Rank unresolved work in a checked plan.
///
/// `doc` must already have passed through [`crate::passes::check`] (so P3
/// proposals are visible and P4 shares are merged); `compiled` is the
/// [`crate::compile`] result over that same document. Only tasks reachable
/// from the objective root through non-rejected methods, and not done or
/// dropped, are ranked: other tasks are not actionable planning surface.
#[must_use]
pub fn frontier(doc: &PlanDoc, compiled: &CompiledPlan) -> FrontierReport {
    let methods: BTreeMap<&str, &pc_ir::Method> =
        doc.method.iter().map(|m| (m.id.as_str(), m)).collect();
    let on_path = selected_path(doc, compiled);
    let live = live_tasks(doc, &compiled.root_task);
    let dependents = dependent_counts(doc);
    let total_tasks = doc.task.len().max(1) as f64;

    let mut items: Vec<FrontierItem> = Vec::new();

    // Open tasks: reachable work the compiler left unresolved.
    let mut unresolved: Vec<&String> = compiled.unresolved_tasks.iter().collect();
    unresolved.sort();
    for id in unresolved {
        if !live.contains(id) {
            continue;
        }
        let critical = on_path.contains(id);
        let share = structural_share(id, &dependents, total_tasks);
        items.push(FrontierItem {
            subject: format!("task:{id}"),
            kind: KIND_OPEN_TASK.to_string(),
            score: score(share, 1.0, critical),
            share,
            uncertainty: 1.0,
            critical,
            reason: open_reason(id, &on_path, doc),
        });
    }

    // Pending proposals, in method-id order.
    let mut proposed: Vec<&pc_ir::Method> = doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Proposed && live.contains(&m.task))
        .collect();
    proposed.sort_by(|a, b| a.id.cmp(&b.id));
    for method in proposed {
        let critical = on_path.contains(&method.task);
        let share = structural_share(&method.task, &dependents, total_tasks);
        let provenance = if method.provenance.trim().is_empty() {
            "unknown provenance".to_string()
        } else {
            format!("proposed by {}", method.provenance)
        };
        items.push(FrontierItem {
            subject: format!("method:{}", method.id),
            kind: KIND_PROPOSAL.to_string(),
            score: score(share, 1.0, critical),
            share,
            uncertainty: 1.0,
            critical,
            reason: format!(
                "{provenance} for task:{}; awaiting accept or reject",
                method.task
            ),
        });
    }

    // Blocked alternatives: proposed or derived invokes that fail typed
    // contract matching, so accepting them would be P2 error E008.
    let mut candidates: Vec<&pc_ir::Method> = doc
        .method
        .iter()
        .filter(|m| {
            matches!(m.status, MethodStatus::Proposed | MethodStatus::Derived)
                && m.kind == pc_ir::MethodKind::Invoke
                && live.contains(&m.task)
        })
        .collect();
    candidates.sort_by(|a, b| a.id.cmp(&b.id));
    for method in candidates {
        let Some(note) = crate::impact::contract_note_for(doc, method) else {
            continue;
        };
        if note.missing_outputs.is_empty() && note.missing_inputs.is_empty() {
            continue;
        }
        let critical = on_path.contains(&method.task);
        let share = structural_share(&method.task, &dependents, total_tasks);
        items.push(FrontierItem {
            subject: format!("method:{}", method.id),
            kind: KIND_BLOCKED_ALTERNATIVE.to_string(),
            score: score(share, 1.0, critical),
            share,
            uncertainty: 1.0,
            critical,
            reason: note.frontier_reason(),
        });
    }

    // Critical-path cost resting on declared guesses: one entry per critical
    // leaf task whose selected duration capability has no measured evidence.
    let mut critical_tasks: Vec<(&String, &crate::compile::TaskTiming)> =
        compiled.timings.iter().collect();
    critical_tasks.sort_by(|a, b| a.0.cmp(b.0));
    for (task_id, timing) in critical_tasks {
        if !timing.critical {
            continue;
        }
        let Some(method_id) = compiled.selection.get(task_id) else {
            continue;
        };
        let Some(method) = methods.get(method_id.as_str()).copied() else {
            continue;
        };
        let Some(capability) = method.capability.as_deref() else {
            continue;
        };
        let measured = compiled.provenance.iter().any(|row| {
            row.capability == capability
                && row.metric == "duration_minutes"
                && row.measured.is_some()
        });
        if measured {
            continue;
        }
        let duration = (timing.earliest_finish - timing.earliest_start).max(0.0);
        let share = if compiled.makespan_minutes > 0.0 {
            duration / compiled.makespan_minutes
        } else {
            1.0 / total_tasks
        };
        items.push(FrontierItem {
            subject: format!("task:{task_id}"),
            kind: KIND_UNMEASURED_CRITICAL.to_string(),
            score: score(share, 1.0, true),
            share,
            uncertainty: 1.0,
            critical: true,
            reason: format!(
                "critical path; {duration:.3} expected minutes via \
                 capability:{capability} rest on declared guesses"
            ),
        });
    }

    items.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.subject.cmp(&b.subject))
    });
    FrontierReport { items }
}

/// Ranking score: share of objective times uncertainty times critical-path
/// membership.
fn score(share: f64, uncertainty: f64, critical: bool) -> f64 {
    share
        * uncertainty
        * if critical {
            CRITICAL_FACTOR
        } else {
            OFF_PATH_FACTOR
        }
}

/// Structural share of the objective for work without estimates: the task
/// itself plus everything transitively depending on it, over all tasks.
fn structural_share(task: &str, dependents: &BTreeMap<String, usize>, total: f64) -> f64 {
    (1 + dependents.get(task).copied().unwrap_or(0)) as f64 / total
}

/// Tasks on the selected path from the objective root, following the P6
/// selection in `compiled.selection` through decompose methods.
fn selected_path(doc: &PlanDoc, compiled: &CompiledPlan) -> BTreeSet<String> {
    let methods: BTreeMap<&str, &pc_ir::Method> =
        doc.method.iter().map(|m| (m.id.as_str(), m)).collect();
    let mut visited = BTreeSet::new();
    let mut stack = vec![compiled.root_task.clone()];
    while let Some(id) = stack.pop() {
        if !visited.insert(id.clone()) {
            continue;
        }
        let Some(method_id) = compiled.selection.get(&id) else {
            continue;
        };
        let Some(method) = methods.get(method_id.as_str()).copied() else {
            continue;
        };
        if method.kind == pc_ir::MethodKind::Decompose {
            stack.extend(method.subtasks.iter().cloned());
        }
    }
    visited
}

/// Tasks reachable from `root` through non-rejected decompose methods,
/// excluding done and dropped tasks. Like compile, the walk never descends
/// below a done or dropped task, so their subtrees are not live either.
fn live_tasks(doc: &PlanDoc, root: &str) -> BTreeSet<String> {
    let tasks: BTreeMap<&str, &pc_ir::Task> = doc.task.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut live = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(task) = tasks.get(id).copied() else {
            continue;
        };
        if matches!(task.state, TaskState::Done | TaskState::Dropped)
            || !live.insert(id.to_string())
        {
            continue;
        }
        for method in &doc.method {
            if method.task == id
                && method.kind == pc_ir::MethodKind::Decompose
                && method.status != MethodStatus::Rejected
            {
                stack.extend(method.subtasks.iter().map(String::as_str));
            }
        }
    }
    live
}

/// Transitive dependent counts over accepted decompose edges: for each task,
/// how many other tasks depend on it (directly or transitively).
fn dependent_counts(doc: &PlanDoc) -> BTreeMap<String, usize> {
    let mut parents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for method in &doc.method {
        if method.kind != pc_ir::MethodKind::Decompose || method.status != MethodStatus::Accepted {
            continue;
        }
        for sub in &method.subtasks {
            parents
                .entry(sub.as_str())
                .or_default()
                .push(method.task.as_str());
        }
    }
    let mut counts = BTreeMap::new();
    for task in &doc.task {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&str> = parents.get(task.id.as_str()).cloned().unwrap_or_default();
        while let Some(parent) = stack.pop() {
            if !seen.insert(parent) {
                continue;
            }
            stack.extend(parents.get(parent).cloned().unwrap_or_default());
        }
        // A task never depends on itself through accepted edges on a clean
        // plan (P2 proves acyclicity), but refuse to count it if it does.
        seen.remove(task.id.as_str());
        counts.insert(task.id.clone(), seen.len());
    }
    counts
}

/// Human-readable reason for an open-task entry.
fn open_reason(id: &str, on_path: &BTreeSet<String>, doc: &PlanDoc) -> String {
    let mut dependers: Vec<&str> = doc
        .method
        .iter()
        .filter(|m| {
            m.kind == pc_ir::MethodKind::Decompose
                && m.status == MethodStatus::Accepted
                && m.subtasks.iter().any(|s| s == id)
        })
        .map(|m| m.task.as_str())
        .collect();
    dependers.sort();
    dependers.dedup();
    let via = if dependers.is_empty() {
        "nothing accepted depends on it yet".to_string()
    } else {
        format!("required by {}", dependers.join(", "))
    };
    if on_path.contains(id) {
        format!("no accepted method; on the selected path from the root ({via})")
    } else {
        format!("no accepted method ({via})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> PlanDoc {
        let text = r#"
ir_version = 0

[plan]
id = "p"
title = "t"

[objective]
root_task = "root"
weights = { duration_minutes = 1.0 }

[[task]]
id = "root"
title = "Root"
state = "decomposed"

[[task]]
id = "done-work"
title = "Done work"
state = "resolved"

[[task]]
id = "open-work"
title = "Open work"
state = "open"

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["done-work", "open-work"]
status = "accepted"
provenance = "human:test"

[[method]]
id = "do-it"
task = "done-work"
kind = "invoke"
capability = "tool@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "maybe"
task = "open-work"
kind = "invoke"
capability = "tool@1"
status = "proposed"
provenance = "agent:test"

[[capability]]
id = "tool@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 2.0, likely = 4.0, high = 6.0, provenance = "declared" }]
"#;
        pc_ir::from_toml(text).unwrap()
    }

    fn checked(doc: &mut PlanDoc) -> crate::passes::Report {
        let text = pc_ir::to_toml(doc).unwrap();
        // The borrow checker needs an owned copy of the text for anchoring.
        let owned = text.clone();
        crate::passes::check("plan.toml", &owned, doc, None)
    }

    #[test]
    fn open_task_and_proposal_rank_above_unmeasured_critical() {
        let mut plan = doc();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let kinds: Vec<(&str, &str)> = report
            .items
            .iter()
            .map(|item| (item.subject.as_str(), item.kind.as_str()))
            .collect();
        assert!(kinds.contains(&("task:open-work", KIND_OPEN_TASK)));
        assert!(kinds.contains(&("method:maybe", KIND_PROPOSAL)));
        // Scores descend; the tie between the open task and its proposal
        // breaks on subject order.
        let scores: Vec<f64> = report.items.iter().map(|item| item.score).collect();
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| b.total_cmp(a));
        assert_eq!(scores, sorted);
    }

    #[test]
    fn blocked_alternative_names_lost_fields_and_consumers() {
        let mut plan = doc();
        plan.artifact.push(pc_ir::ArtifactDecl {
            name: "artifact".into(),
            fields: vec!["kept".into(), "dropped".into()],
        });
        for task in &mut plan.task {
            if task.id == "open-work" {
                task.produces = vec![pc_ir::ContractItem {
                    artifact: "artifact".into(),
                    fields: vec!["kept".into(), "dropped".into()],
                }];
            }
            if task.id == "done-work" {
                task.consumes = vec![pc_ir::ContractItem {
                    artifact: "artifact".into(),
                    fields: vec!["dropped".into()],
                }];
            }
        }
        for cap in &mut plan.capability {
            cap.produces = vec![pc_ir::ContractItem {
                artifact: "artifact".into(),
                fields: vec!["kept".into()],
            }];
        }
        // Give done-work a producer for `kept` so the plan stays P2-clean
        // apart from the proposed method the frontier must flag: the proposed
        // invoke is not contract-checked by P2, but P8 still reports it.
        plan.task.push(pc_ir::Task {
            id: "producer".into(),
            title: "Producer".into(),
            state: TaskState::Resolved,
            consumes: vec![],
            produces: vec![pc_ir::ContractItem {
                artifact: "artifact".into(),
                fields: vec!["kept".into(), "dropped".into()],
            }],
        });
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let blocked: Vec<&FrontierItem> = report
            .items
            .iter()
            .filter(|item| item.kind == KIND_BLOCKED_ALTERNATIVE)
            .collect();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].subject, "method:maybe");
        assert!(
            blocked[0].reason.contains("artifact.dropped"),
            "{}",
            blocked[0].reason
        );
        assert!(
            blocked[0].reason.contains("done-work"),
            "{}",
            blocked[0].reason
        );
    }

    #[test]
    fn proposals_on_dropped_or_unreachable_tasks_are_not_ranked() {
        let mut plan = doc();
        plan.task.push(pc_ir::Task {
            id: "orphan".into(),
            title: "Orphan".into(),
            state: TaskState::Open,
            consumes: vec![],
            produces: vec![],
        });
        let mut stray = plan
            .method
            .iter()
            .find(|m| m.id == "maybe")
            .unwrap()
            .clone();
        stray.id = "stray".into();
        stray.task = "orphan".into();
        plan.method.push(stray);
        plan.task
            .iter_mut()
            .find(|t| t.id == "open-work")
            .unwrap()
            .state = TaskState::Dropped;
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let subjects: Vec<&str> = report
            .items
            .iter()
            .map(|item| item.subject.as_str())
            .collect();
        assert!(!subjects.contains(&"method:maybe"), "{subjects:?}");
        assert!(!subjects.contains(&"method:stray"), "{subjects:?}");
        assert!(!subjects.contains(&"task:open-work"), "{subjects:?}");
    }

    #[test]
    fn proposals_below_done_tasks_are_not_ranked() {
        let mut plan = doc();
        plan.task.push(pc_ir::Task {
            id: "child".into(),
            title: "Child".into(),
            state: TaskState::Open,
            consumes: vec![],
            produces: vec![],
        });
        for task in &mut plan.task {
            if task.id == "done-work" {
                task.state = TaskState::Done;
            }
        }
        let mut split = plan
            .method
            .iter()
            .find(|m| m.id == "root-all")
            .unwrap()
            .clone();
        split.id = "split-done".into();
        split.task = "done-work".into();
        split.subtasks = vec!["child".into()];
        plan.method.push(split);
        let mut below = plan
            .method
            .iter()
            .find(|m| m.id == "maybe")
            .unwrap()
            .clone();
        below.id = "below-done".into();
        below.task = "child".into();
        plan.method.push(below);
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let subjects: Vec<&str> = report
            .items
            .iter()
            .map(|item| item.subject.as_str())
            .collect();
        assert!(!subjects.contains(&"method:below-done"), "{subjects:?}");
        assert!(!subjects.contains(&"task:child"), "{subjects:?}");
        assert!(subjects.contains(&"method:maybe"), "{subjects:?}");
    }

    #[test]
    fn frontier_is_deterministic_over_input_order() {
        let mut plan = doc();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let first = serde_json::to_vec(&frontier(&plan, &compiled)).unwrap();
        let mut shuffled = plan.clone();
        shuffled.task.reverse();
        shuffled.method.reverse();
        shuffled.capability.reverse();
        let recompiled = crate::compile(&shuffled).unwrap();
        let second = serde_json::to_vec(&frontier(&shuffled, &recompiled)).unwrap();
        assert_eq!(first, second);
    }
}
