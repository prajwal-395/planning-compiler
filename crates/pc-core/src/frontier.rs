//! P8: frontier — rank unresolved work so people and agents can see the frontier.
//!
//! Step 6 analysis pass: runs over a checked plan plus its [`CompiledPlan`]
//! and returns a deterministically ranked [`FrontierReport`]. It never edits
//! the plan, calls no model and tracks no status: it only scores where more
//! planning is worth the effort.
//!
//! Every entry scores `share * uncertainty * criticality * readiness`
//! (report section 3.3, P8):
//!
//! - `share` is the entry's share of the objective. Estimated entries carry
//!   a weighted expected cost (`objective.weights` times the P5 effective
//!   expected numbers, so measured evidence moves it when live) and their
//!   share is that cost over the total weighted cost of open acceptable
//!   work: per task, the largest cost among its pending invokes that pass
//!   typed contract matching (alternatives, never summed) or its guessed
//!   critical-path capability. Entries without such a share go to the
//!   separate `unestimated` group, ordered by the structural share
//!   `(1 + transitive dependents) / tasks` and never compared with the
//!   estimated ranking.
//! - Pending invokes that fail typed contract matching cannot be accepted
//!   (E008): they never feed the open total, carry no share or score, and
//!   are listed once each in the separate `blocked` group, by subject, with
//!   a reason naming the dropped fields and their consumers.
//! - `uncertainty` is `1.0`, lowered to `0.5` when the entry's capability
//!   already has live measured or staged evidence on a weighted metric
//!   (`duration_minutes` when weights are empty): the numbers behind its
//!   cost are less of a guess.
//! - `criticality` is `1.0` when the entry sits on the selected path from
//!   the objective root (or is itself a critical leaf) and `0.5` otherwise.
//!   When nothing is executable yet (makespan zero) every reachable task
//!   sits on that path, so criticality ties by construction and the other
//!   factors do the separating.
//! - `readiness` is `1.0` when nothing live blocks the entry, and `0.5`
//!   when it is blocked: an accepted decompose orders a live predecessor
//!   before it, or a parent through an accepted decompose is itself
//!   blocked. Blocked planning cannot run in parallel yet.
//!
//! The ranked and unestimated groups sort by score descending, then kind
//! (proposal, open-task, unmeasured-critical), then subject ascending, so the
//! same inputs always give byte-identical reports whatever the input order.
//! Items in one group that share both score and kind are genuinely tied:
//! they carry `tied: true` and their reasons say the order is subject order,
//! not priority, instead of presenting the sort artefact as a ranking.

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
/// Ready factor: nothing live blocks this entry.
pub const READY_FACTOR: f64 = 1.0;
/// Blocked factor: a live order predecessor or a blocked parent holds it.
pub const BLOCKED_FACTOR: f64 = 0.5;
/// Uncertainty for entries with no measured backing.
pub const UNCERTAIN_FACTOR: f64 = 1.0;
/// Uncertainty when live measured or staged evidence on a weighted metric
/// backs the capability.
pub const MEASURED_FACTOR: f64 = 0.5;

/// One ranked frontier entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontierItem {
    /// Entity this entry is about: `task:<id>`, `method:<id>` or
    /// `capability:<id>`.
    pub subject: String,
    /// Entry kind: `open-task`, `proposal`, `unmeasured-critical` or
    /// `blocked-alternative`.
    pub kind: String,
    /// Ranking score within the entry's group:
    /// `share * uncertainty * criticality * readiness`. `None` for blocked
    /// alternatives, which are not ranked.
    pub score: Option<f64>,
    /// Share of the objective: the weighted expected cost over the total of
    /// open acceptable work when `estimated`, otherwise the structural
    /// dependent share. `None` for blocked alternatives.
    pub share: Option<f64>,
    /// Uncertainty (`1.0`, or `0.5` when measured or staged evidence on a
    /// weighted metric backs the capability).
    pub uncertainty: f64,
    /// Whether the share is a fraction of the open acceptable weighted cost
    /// (ranked in `items`) rather than structural (listed in `unestimated`)
    /// or absent (listed in `blocked`).
    pub estimated: bool,
    /// Whether the entry sits on the selected path from the objective root.
    pub critical: bool,
    /// Whether nothing live blocks this entry.
    pub ready: bool,
    /// True when another entry in the same group shares this score and
    /// kind: the order between them is subject order, not priority.
    pub tied: bool,
    /// Human-readable reason naming the decision behind the entry.
    pub reason: String,
}

/// Deterministically ranked unresolved work, best first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrontierReport {
    /// Estimated entries ranked by their share of the open acceptable
    /// weighted cost, best first.
    pub items: Vec<FrontierItem>,
    /// Entries with no weighted estimate, ordered structurally and
    /// never ranked against `items`.
    pub unestimated: Vec<FrontierItem>,
    /// Pending invokes that fail typed contract matching, so accepting them
    /// would be E008: one `blocked-alternative` entry each, by subject, with
    /// no share or score. All three lists empty means the frontier is fully
    /// planned: every reachable task has an accepted method and no critical
    /// cost rests on guesses.
    pub blocked: Vec<FrontierItem>,
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
    let blockers = blockers(doc, &live);
    let estimated = crate::estimate(doc);
    let effective = estimated.effective_map();
    let weighted: Vec<String> = if doc.objective.weights.is_empty() {
        vec!["duration_minutes".to_string()]
    } else {
        doc.objective.weights.keys().cloned().collect()
    };
    let backed_caps: BTreeSet<String> = estimated
        .measured_set()
        .into_iter()
        .chain(
            estimated
                .staged
                .iter()
                .map(|s| (s.capability.clone(), s.metric.clone())),
        )
        .filter(|(_, metric)| weighted.contains(metric))
        .map(|(cap, _)| cap)
        .collect();
    let backed = |cap: Option<&str>| cap.is_some_and(|c| backed_caps.contains(c));

    // Pending invokes on live tasks, their weighted costs, and the ones
    // that fail typed contract matching (accepting them would be E008).
    let pending = pending_invokes(doc, &live);
    let mut proposal_cost: BTreeMap<&str, f64> = BTreeMap::new();
    let mut contract_notes: BTreeMap<&str, crate::impact::ContractNote> = BTreeMap::new();
    for method in &pending {
        if let Some(cost) = method
            .capability
            .as_deref()
            .and_then(|cap| capability_cost(cap, &effective, &doc.objective.weights))
        {
            proposal_cost.insert(method.id.as_str(), cost);
        }
        if let Some(note) = crate::impact::contract_note_for(doc, method) {
            contract_notes.insert(method.id.as_str(), note);
        }
    }

    // Critical-path cost resting on declared guesses: critical leaf tasks
    // whose selected duration capability has no measured evidence.
    let mut guessed: Vec<(&str, &str, f64)> = Vec::new();
    for (task_id, timing) in &compiled.timings {
        if !timing.critical {
            continue;
        }
        let Some(capability) = compiled
            .selection
            .get(task_id)
            .and_then(|method_id| methods.get(method_id.as_str()))
            .and_then(|method| method.capability.as_deref())
        else {
            continue;
        };
        let measured = compiled.provenance.iter().any(|row| {
            row.capability == capability
                && row.metric == "duration_minutes"
                && row.measured.is_some()
        });
        if !measured {
            let duration = (timing.earliest_finish - timing.earliest_start).max(0.0);
            guessed.push((task_id.as_str(), capability, duration));
        }
    }
    guessed.sort_by(|a, b| a.0.cmp(b.0));

    // Open acceptable work, one cost per task: the largest of its
    // contract-clean pending invokes and its guessed critical capability.
    let acceptable = pending
        .iter()
        .filter(|m| !contract_notes.contains_key(m.id.as_str()))
        .filter_map(|m| Some((m.task.as_str(), *proposal_cost.get(m.id.as_str())?)));
    let critical_guesses = guessed.iter().filter_map(|&(task_id, capability, _)| {
        Some((
            task_id,
            capability_cost(capability, &effective, &doc.objective.weights)?,
        ))
    });
    let mut task_cost: BTreeMap<&str, f64> = BTreeMap::new();
    for (task, cost) in acceptable.chain(critical_guesses) {
        let entry = task_cost.entry(task).or_insert(cost);
        *entry = entry.max(cost);
    }
    let open_total: f64 = task_cost.values().sum();

    let mut items: Vec<FrontierItem> = Vec::new();
    let blocked_suffix = |task: &str| {
        let blocked_by = blockers.get(task).cloned().unwrap_or_default();
        let suffix = if blocked_by.is_empty() {
            String::new()
        } else {
            format!(
                "; blocked by {}",
                blocked_by.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        (suffix.is_empty(), suffix)
    };
    let mut push = |subject: String,
                    kind: &str,
                    task: &str,
                    cost: Option<f64>,
                    uncertainty: f64,
                    critical: bool,
                    mut reason: String| {
        let (ready, suffix) = blocked_suffix(task);
        if let Some(c) = cost {
            reason.push_str(&format!("; weighted expected cost {c:.3}"));
        }
        let share = match cost {
            Some(c) if open_total > 0.0 => {
                reason.push_str(&format!(" of {open_total:.3} open"));
                Some(c / open_total)
            }
            _ => None,
        };
        if uncertainty < UNCERTAIN_FACTOR {
            reason.push_str("; measured or staged evidence lowers uncertainty");
        }
        reason.push_str(&suffix);
        let estimated = share.is_some();
        let share = share.unwrap_or_else(|| structural_share(task, &dependents, total_tasks));
        items.push(FrontierItem {
            subject,
            kind: kind.to_string(),
            score: Some(score4(share, uncertainty, critical, ready)),
            share: Some(share),
            uncertainty,
            estimated,
            critical,
            ready,
            tied: false,
            reason,
        });
    };

    // Open tasks: reachable work the compiler left unresolved.
    let mut unresolved: Vec<&String> = compiled.unresolved_tasks.iter().collect();
    unresolved.sort();
    for id in unresolved {
        if !live.contains(id) {
            continue;
        }
        // Open tasks have no single capability; their uncertainty stays at
        // the guess level unless every proposal behind them is measured.
        push(
            format!("task:{id}"),
            KIND_OPEN_TASK,
            id,
            task_cost.get(id.as_str()).copied(),
            UNCERTAIN_FACTOR,
            on_path.contains(id),
            open_reason(id, &on_path, doc),
        );
    }

    // Pending proposals that could be accepted, in method-id order.
    let mut proposed: Vec<&pc_ir::Method> = doc
        .method
        .iter()
        .filter(|m| {
            m.status == MethodStatus::Proposed
                && live.contains(&m.task)
                && !contract_notes.contains_key(m.id.as_str())
        })
        .collect();
    proposed.sort_by(|a, b| a.id.cmp(&b.id));
    for method in proposed {
        let provenance = if method.provenance.trim().is_empty() {
            "unknown provenance".to_string()
        } else {
            format!("proposed by {}", method.provenance)
        };
        push(
            format!("method:{}", method.id),
            KIND_PROPOSAL,
            &method.task,
            proposal_cost.get(method.id.as_str()).copied(),
            if backed(method.capability.as_deref()) {
                MEASURED_FACTOR
            } else {
                UNCERTAIN_FACTOR
            },
            on_path.contains(&method.task),
            format!(
                "{provenance} for task:{}; awaiting accept or reject",
                method.task
            ),
        );
    }

    for &(task_id, capability, duration) in &guessed {
        push(
            format!("task:{task_id}"),
            KIND_UNMEASURED_CRITICAL,
            task_id,
            capability_cost(capability, &effective, &doc.objective.weights),
            UNCERTAIN_FACTOR,
            true,
            format!(
                "critical path; {duration:.3} expected minutes via \
                 capability:{capability} rest on declared guesses"
            ),
        );
    }

    // Blocked alternatives: pending invokes that fail typed contract
    // matching, so accepting them would be P2 error E008. They stay out of
    // the ranking, listed once each by subject.
    let blocked = pending
        .iter()
        .filter_map(|method| {
            let note = contract_notes.get(method.id.as_str())?;
            let (ready, suffix) = blocked_suffix(&method.task);
            Some(FrontierItem {
                subject: format!("method:{}", method.id),
                kind: KIND_BLOCKED_ALTERNATIVE.to_string(),
                score: None,
                share: None,
                uncertainty: UNCERTAIN_FACTOR,
                estimated: false,
                critical: on_path.contains(&method.task),
                ready,
                tied: false,
                reason: format!("{}{suffix}", note.frontier_reason()),
            })
        })
        .collect();

    let (items, unestimated): (Vec<FrontierItem>, Vec<FrontierItem>) =
        items.into_iter().partition(|item| item.estimated);
    FrontierReport {
        items: ranked(items),
        unestimated: ranked(unestimated),
        blocked,
    }
}

/// Sort one group and flag its genuine ties: entries sharing score and kind
/// keep subject order, not priority, and the reason says so instead of
/// hiding behind the sort.
fn ranked(mut items: Vec<FrontierItem>) -> Vec<FrontierItem> {
    let score = |item: &FrontierItem| item.score.unwrap_or(f64::NEG_INFINITY);
    items.sort_by(|a, b| {
        score(b)
            .total_cmp(&score(a))
            .then_with(|| kind_rank(&a.kind).cmp(&kind_rank(&b.kind)))
            .then_with(|| a.subject.cmp(&b.subject))
    });
    for i in 0..items.len() {
        let tied = (i > 0
            && score(&items[i - 1]).total_cmp(&score(&items[i])).is_eq()
            && items[i - 1].kind == items[i].kind)
            || (i + 1 < items.len()
                && score(&items[i + 1]).total_cmp(&score(&items[i])).is_eq()
                && items[i + 1].kind == items[i].kind);
        items[i].tied = tied;
        if tied {
            items[i].reason.push_str(
                "; tied at this score with same-kind work; order is subject order, not priority",
            );
        }
    }
    items
}

/// Ranking score: share of objective times uncertainty times critical-path
/// membership times readiness.
fn score4(share: f64, uncertainty: f64, critical: bool, ready: bool) -> f64 {
    share
        * uncertainty
        * if critical {
            CRITICAL_FACTOR
        } else {
            OFF_PATH_FACTOR
        }
        * if ready { READY_FACTOR } else { BLOCKED_FACTOR }
}

/// Sort priority between kinds when scores tie: pending decisions first,
/// then open decomposition, then measured guesses on the critical path.
fn kind_rank(kind: &str) -> u8 {
    match kind {
        KIND_PROPOSAL => 0,
        KIND_OPEN_TASK => 1,
        _ => 2,
    }
}

/// Weighted expected cost for one capability, using the P5 effective numbers
/// (measured when live, declared otherwise) times the objective weights.
/// `None` when the capability has no effective estimate for any weighted
/// metric: there is nothing to value yet.
fn capability_cost(
    capability: &str,
    effective: &BTreeMap<(String, String), crate::estimate::EffectiveEstimate>,
    weights: &BTreeMap<String, f64>,
) -> Option<f64> {
    if weights.is_empty() {
        return effective
            .get(&(capability.to_string(), "duration_minutes".to_string()))
            .map(|e| e.expected);
    }
    let mut cost = 0.0;
    let mut covered = false;
    for (metric, weight) in weights {
        if let Some(e) = effective.get(&(capability.to_string(), metric.clone())) {
            cost += weight * e.expected;
            covered = true;
        }
    }
    covered.then_some(cost)
}

/// Live tasks blocking each live task: its live order predecessors under
/// accepted decompose methods, plus everything blocking a live parent
/// through an accepted decompose. Undecided proposals never order work.
fn blockers(doc: &PlanDoc, live: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut predecessors: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut parents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for method in &doc.method {
        if method.kind != pc_ir::MethodKind::Decompose || method.status != MethodStatus::Accepted {
            continue;
        }
        for edge in &method.order_edges {
            predecessors
                .entry(edge.to.as_str())
                .or_default()
                .insert(edge.from.as_str());
        }
        for sub in &method.subtasks {
            parents
                .entry(sub.as_str())
                .or_default()
                .insert(method.task.as_str());
        }
    }
    fn resolve(
        task: &str,
        predecessors: &BTreeMap<&str, BTreeSet<&str>>,
        parents: &BTreeMap<&str, BTreeSet<&str>>,
        live: &BTreeSet<String>,
        out: &mut BTreeMap<String, BTreeSet<String>>,
        visiting: &mut BTreeSet<String>,
    ) -> BTreeSet<String> {
        if let Some(done) = out.get(task) {
            return done.clone();
        }
        // P2 proves accepted hierarchies acyclic; refuse to loop if not.
        if !visiting.insert(task.to_string()) {
            return BTreeSet::new();
        }
        let mut blocked: BTreeSet<String> = predecessors
            .get(task)
            .into_iter()
            .flatten()
            .filter(|pred| live.contains(**pred))
            .map(|pred| (*pred).to_string())
            .collect();
        for parent in parents.get(task).into_iter().flatten() {
            if live.contains(*parent) {
                blocked.extend(resolve(parent, predecessors, parents, live, out, visiting));
            }
        }
        out.insert(task.to_string(), blocked.clone());
        blocked
    }
    let mut out = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    for task in live {
        resolve(task, &predecessors, &parents, live, &mut out, &mut visiting);
    }
    out
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

/// Proposed or derived invoke methods on `live` tasks, sorted by id: the
/// pending set behind both frontier blocked alternatives and `pc check`
/// contract notes.
pub(crate) fn pending_invokes<'a>(
    doc: &'a PlanDoc,
    live: &BTreeSet<String>,
) -> Vec<&'a pc_ir::Method> {
    let mut pending: Vec<&pc_ir::Method> = doc
        .method
        .iter()
        .filter(|m| {
            matches!(m.status, MethodStatus::Proposed | MethodStatus::Derived)
                && m.kind == pc_ir::MethodKind::Invoke
                && live.contains(&m.task)
        })
        .collect();
    pending.sort_by(|a, b| a.id.cmp(&b.id));
    pending
}

/// Tasks reachable from `root` through non-rejected decompose methods,
/// excluding done and dropped tasks. Like compile, the walk never descends
/// below a done or dropped task, so their subtrees are not live either.
pub(crate) fn live_tasks(doc: &PlanDoc, root: &str) -> BTreeSet<String> {
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

    fn all(report: &FrontierReport) -> impl Iterator<Item = &FrontierItem> {
        report
            .items
            .iter()
            .chain(&report.unestimated)
            .chain(&report.blocked)
    }

    fn item<'a>(report: &'a FrontierReport, subject: &str) -> &'a FrontierItem {
        all(report).find(|i| i.subject == subject).unwrap()
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
        // Scores descend; kind breaks score ties (proposal before open),
        // then subject order.
        let scores: Vec<f64> = report
            .items
            .iter()
            .map(|item| item.score.unwrap())
            .collect();
        let mut sorted = scores.clone();
        sorted.sort_by(|a, b| b.total_cmp(a));
        assert_eq!(scores, sorted);
    }

    fn frontier_fixture() -> PlanDoc {
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
id = "first"
title = "First"
state = "open"

[[task]]
id = "second"
title = "Second"
state = "open"

[[task]]
id = "twin-a"
title = "Twin A"
state = "open"

[[task]]
id = "twin-b"
title = "Twin B"
state = "open"

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["first", "second", "twin-a", "twin-b"]
status = "accepted"
provenance = "human:test"
order_edges = [{ from = "first", to = "second" }]

[[method]]
id = "cheap-way"
task = "first"
kind = "invoke"
capability = "cheap@1"
status = "proposed"
provenance = "agent:test"

[[method]]
id = "pricey-way"
task = "first"
kind = "invoke"
capability = "pricey@1"
status = "proposed"
provenance = "agent:test"

[[capability]]
id = "cheap@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 1.0, likely = 2.0, high = 3.0, provenance = "declared" }]

[[capability]]
id = "pricey@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 10.0, likely = 20.0, high = 30.0, provenance = "declared" }]
"#;
        pc_ir::from_toml(text).unwrap()
    }

    #[test]
    fn declared_estimates_separate_proposals() {
        let mut plan = frontier_fixture();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let proposals: Vec<&FrontierItem> = report
            .items
            .iter()
            .filter(|i| i.kind == KIND_PROPOSAL)
            .collect();
        assert_eq!(proposals.len(), 2);
        // Pricey outranks cheap on declared weighted cost.
        assert_eq!(proposals[0].subject, "method:pricey-way");
        assert_eq!(proposals[1].subject, "method:cheap-way");
        assert!(proposals[0].score > proposals[1].score, "{proposals:?}");
        assert!(
            proposals[0].reason.contains("weighted expected cost"),
            "{}",
            proposals[0].reason
        );
    }

    #[test]
    fn blocked_tasks_rank_below_ready() {
        let mut plan = frontier_fixture();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let second = item(&report, "task:second");
        let twin = item(&report, "task:twin-a");
        assert!(item(&report, "task:first").ready);
        assert!(!second.ready);
        assert!(twin.ready);
        // Same structure, same group: only readiness separates them.
        assert!(!second.estimated && !twin.estimated);
        assert!(second.score < twin.score, "{second:?} vs {twin:?}");
        assert!(
            second.reason.contains("blocked by first"),
            "{}",
            second.reason
        );
    }

    #[test]
    fn staged_evidence_lowers_uncertainty() {
        let mut plan = frontier_fixture();
        plan.evidence.push(pc_ir::Evidence {
            subject: "capability:cheap@1".into(),
            metric: "duration_minutes".into(),
            value: 2.5,
            unit: "minutes".into(),
            time: "2026-10-01T00:00:00Z".into(),
            source: "test".into(),
            basis: None,
            artifact_hash: None,
        });
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let cheap = item(&report, "method:cheap-way");
        let pricey = item(&report, "method:pricey-way");
        assert_eq!(cheap.uncertainty, MEASURED_FACTOR);
        assert_eq!(pricey.uncertainty, UNCERTAIN_FACTOR);
        assert!(
            cheap.reason.contains("lowers uncertainty"),
            "{}",
            cheap.reason
        );
    }

    #[test]
    fn costed_work_shares_one_scale_and_unestimated_work_stays_apart() {
        let mut plan = frontier_fixture();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        assert!(report.items.iter().all(|i| i.estimated), "{report:?}");
        assert!(
            report.unestimated.iter().all(|i| !i.estimated),
            "{report:?}"
        );
        // The open total is first's largest alternative (20), never the sum.
        assert_eq!(item(&report, "method:pricey-way").share, Some(1.0));
        assert_eq!(item(&report, "method:cheap-way").share, Some(0.1));
        let unestimated: Vec<&str> = report
            .unestimated
            .iter()
            .map(|i| i.subject.as_str())
            .collect();
        assert_eq!(unestimated, ["task:twin-a", "task:twin-b", "task:second"]);

        // A far pricier proposal elsewhere rescales costed shares but can
        // never push costed work below unestimated work.
        plan.capability.push(pc_ir::Capability {
            id: "huge@1".into(),
            ..plan.capability[0].clone()
        });
        plan.capability.last_mut().unwrap().estimates[0].likely = 2000.0;
        plan.capability.last_mut().unwrap().estimates[0].high = 3000.0;
        let mut huge = plan
            .method
            .iter()
            .find(|m| m.id == "cheap-way")
            .unwrap()
            .clone();
        huge.id = "huge-way".into();
        huge.task = "twin-a".into();
        huge.capability = Some("huge@1".into());
        plan.method.push(huge);
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        assert!(
            report.items.iter().any(|i| i.subject == "method:cheap-way"),
            "{report:?}"
        );
        assert!(
            report
                .unestimated
                .iter()
                .all(|i| i.subject != "method:cheap-way"),
            "{report:?}"
        );
    }

    #[test]
    fn only_accepted_decompositions_order_work_and_blocking_inherits() {
        let mut plan = frontier_fixture();
        let mut alternative = plan
            .method
            .iter()
            .find(|m| m.id == "root-all")
            .unwrap()
            .clone();
        alternative.id = "root-alt".into();
        alternative.status = MethodStatus::Proposed;
        alternative.provenance = "agent:test".into();
        alternative.order_edges = vec![pc_ir::OrderEdge {
            from: "twin-a".into(),
            to: "twin-b".into(),
        }];
        plan.method.push(alternative);
        plan.task
            .iter_mut()
            .find(|t| t.id == "second")
            .unwrap()
            .state = TaskState::Decomposed;
        plan.task.push(pc_ir::Task {
            id: "leaf".into(),
            title: "Leaf".into(),
            state: TaskState::Open,
            consumes: vec![],
            produces: vec![],
        });
        let mut split = plan
            .method
            .iter()
            .find(|m| m.id == "root-all")
            .unwrap()
            .clone();
        split.id = "split-second".into();
        split.task = "second".into();
        split.subtasks = vec!["leaf".into()];
        split.order_edges = vec![];
        plan.method.push(split);
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let twin = item(&report, "task:twin-b");
        assert!(twin.ready, "{twin:?}");
        assert!(!twin.reason.contains("blocked by"), "{}", twin.reason);
        let leaf = item(&report, "task:leaf");
        assert!(!leaf.ready, "{leaf:?}");
        assert!(leaf.reason.contains("blocked by first"), "{}", leaf.reason);
    }

    #[test]
    fn evidence_on_unweighted_metrics_keeps_uncertainty() {
        let mut plan = frontier_fixture();
        plan.evidence.push(pc_ir::Evidence {
            subject: "capability:cheap@1".into(),
            metric: "money_usd".into(),
            value: 2.5,
            unit: "usd".into(),
            time: "2026-10-01T00:00:00Z".into(),
            source: "test".into(),
            basis: None,
            artifact_hash: None,
        });
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let cheap = item(&report, "method:cheap-way");
        assert_eq!(cheap.uncertainty, UNCERTAIN_FACTOR);
        assert!(
            !cheap.reason.contains("lowers uncertainty"),
            "{}",
            cheap.reason
        );
    }

    #[test]
    fn genuine_ties_are_flagged() {
        let mut plan = frontier_fixture();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let twins: Vec<&FrontierItem> = report
            .unestimated
            .iter()
            .filter(|i| i.subject == "task:twin-a" || i.subject == "task:twin-b")
            .collect();
        assert_eq!(twins.len(), 2);
        assert_eq!(
            twins[0].score.unwrap().total_cmp(&twins[1].score.unwrap()),
            std::cmp::Ordering::Equal
        );
        assert!(twins[0].tied && twins[1].tied);
        assert!(
            twins[0].reason.contains("tied at this score"),
            "{}",
            twins[0].reason
        );
    }

    #[test]
    fn kind_breaks_score_ties_before_subject() {
        let mut plan = frontier_fixture();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        // task:first shares its best proposal cost with method:pricey-way:
        // same score, but the proposal sorts first by kind, untied.
        let pricey_pos = report
            .items
            .iter()
            .position(|i| i.subject == "method:pricey-way")
            .unwrap();
        let first_pos = report
            .items
            .iter()
            .position(|i| i.subject == "task:first")
            .unwrap();
        assert!(pricey_pos < first_pos);
        assert_eq!(
            report.items[pricey_pos]
                .score
                .unwrap()
                .total_cmp(&report.items[first_pos].score.unwrap()),
            std::cmp::Ordering::Equal
        );
        assert!(!report.items[pricey_pos].tied);
        assert!(!report.items[first_pos].tied);
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
        // The method cannot be accepted: it is listed once, in the blocked
        // group, with no share or score, and never as a ranked proposal.
        let blocked = &report.blocked;
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].subject, "method:maybe");
        assert_eq!(blocked[0].kind, KIND_BLOCKED_ALTERNATIVE);
        assert_eq!((blocked[0].share, blocked[0].score), (None, None));
        assert_eq!(
            all(&report).filter(|i| i.subject == "method:maybe").count(),
            1
        );
        // Its cost stays out of the open total, so open-work has nothing to
        // value.
        assert!(!item(&report, "task:open-work").estimated);
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
        let subjects: Vec<&str> = all(&report).map(|item| item.subject.as_str()).collect();
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
        let subjects: Vec<&str> = all(&report).map(|item| item.subject.as_str()).collect();
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
