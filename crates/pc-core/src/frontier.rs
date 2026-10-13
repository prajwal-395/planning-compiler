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

/// Slack for comparing value-of-information LP results against zero, so
/// floating-point solver noise never decides a conclusion.
const SOLVER_TOLERANCE: f64 = 1e-9;

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
    #[serde(serialize_with = "crate::json::ser_opt_f64_3")]
    pub score: Option<f64>,
    /// Share of the objective: the weighted expected cost over the total of
    /// open acceptable work when `estimated`, otherwise the structural
    /// dependent share. `None` for blocked alternatives.
    #[serde(serialize_with = "crate::json::ser_opt_f64_3")]
    pub share: Option<f64>,
    /// Uncertainty (`1.0`, or `0.5` when measured or staged evidence on a
    /// weighted metric backs the capability).
    #[serde(serialize_with = "crate::json::ser_f64_3")]
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

/// A plan question worth investigating, separate from work ready to execute.
/// `investigation_cost` carries the explicit `investigation` cost when the
/// task has a validated model, and `value_of_information` carries the EVSI
/// interval when every required probability is explicit and
/// provenance-backed. Unknown probabilities stay unknown (no midpoint is
/// fabricated), so such models carry no VOI. Change likelihood stays unknown:
/// the IR carries no likelihood model for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscoveryItem {
    /// Task whose selected approach or alternatives are being investigated.
    pub subject: String,
    /// Whether the decision affects the selected critical path.
    pub critical: bool,
    /// Decision uncertainty on the execution frontier's scale: the measured
    /// factor when the selected capability is backed by evidence on an
    /// objective-weighted metric, the uncertain factor otherwise (including
    /// when no capability is selected).
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub uncertainty: f64,
    /// Proposed invocation alternatives currently available to investigate.
    pub alternatives: usize,
    /// Number of downstream decisions depending on this task.
    pub downstream_decisions: usize,
    /// Investigation effort in utility points, from the explicit
    /// `investigation` model when the task has one.
    pub investigation_cost: Option<f64>,
    /// Likelihood that investigation materially changes the selected plan.
    pub material_change_likelihood: Option<f64>,
    /// EVSI interval in utility points, when every required probability is
    /// explicit and provenance-backed.
    pub value_of_information: Option<ValueOfInformation>,
    /// Why the task's investigation model yields no `value_of_information`
    /// (for example an unknown probability); `None` when there is no model
    /// or the value is known.
    pub value_of_information_unknown: Option<String>,
    /// Human-readable investigation question.
    pub question: String,
}

/// Result of evaluating an explicit investigation model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueOfInformation {
    /// Lower bound on expected value with information, minus the value of
    /// the prior-optimal choice and the investigation cost.
    pub evsi_low: f64,
    /// Upper bound on expected value with information, minus the value of
    /// the prior-optimal choice and the investigation cost.
    pub evsi_high: f64,
    /// Lower bound on the gain from switching away from the selected method
    /// on current beliefs alone; never counted as information value.
    pub switch_gain_low: f64,
    /// Upper bound on the gain from switching away from the selected method
    /// on current beliefs alone.
    pub switch_gain_high: f64,
    /// Method to implement without investigating: one that beats the
    /// selected method throughout the bounds, else the selected method.
    pub preferred_method: String,
    /// When switching away from the selected method gains throughout the
    /// bounds but no single method beats it throughout, every method that
    /// is prior-optimal for some allowed distribution; empty otherwise.
    pub switch_candidates: Vec<String>,
    /// Whether investigation has positive value throughout the interval,
    /// could have positive value, or is dominated by its cost, in which case
    /// the preferred method is implemented directly (switching to it when it
    /// differs from the selected method), or, when only `switch_candidates`
    /// beat the selected method, the selected method is switched away from
    /// with the replacement depending on the probabilities.
    pub conclusion: String,
    /// Human-readable explanation including the decision basis.
    pub explanation: String,
}

impl FrontierItem {
    /// One text line naming the entry, its kind and its flags, shared by
    /// `pc context` and `pc explain task:` so both read identically.
    #[must_use]
    pub fn line(&self) -> String {
        let mut flags = String::new();
        if self.critical {
            flags.push_str(" (critical)");
        }
        if !self.ready {
            flags.push_str(" (blocked)");
        }
        if self.tied {
            flags.push_str(" (tied)");
        }
        if self.score.is_some() && !self.estimated {
            flags.push_str(" (unestimated)");
        }
        format!(
            "frontier {} [{}]{}: {}",
            self.subject, self.kind, flags, self.reason
        )
    }
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
    /// Ranked information-acquisition work. Entries with an explicit
    /// `investigation` model carry its cost and, when every required
    /// probability is explicit and provenance-backed, its EVSI interval;
    /// unknown probabilities stay unknown and carry no VOI.
    pub discovery: Vec<DiscoveryItem>,
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

    // Discovery candidates come only from structured uncertainty already in
    // the plan: proposed invocation alternatives or selected capabilities
    // without evidence on an objective-weighted metric.
    let mut proposed_by_task: BTreeMap<&str, Vec<&pc_ir::Method>> = BTreeMap::new();
    for method in &doc.method {
        if method.kind == pc_ir::MethodKind::Invoke
            && method.status == MethodStatus::Proposed
            && live.contains(&method.task)
        {
            proposed_by_task
                .entry(&method.task)
                .or_default()
                .push(method);
        }
    }
    let selected_by_task: BTreeMap<&str, &pc_ir::Method> = compiled
        .selection
        .iter()
        .filter_map(|(task, id)| {
            methods
                .get(id.as_str())
                .map(|method| (task.as_str(), *method))
        })
        .collect();
    let rejected: BTreeSet<&str> = doc
        .method
        .iter()
        .filter(|method| method.status == MethodStatus::Rejected)
        .map(|method| method.id.as_str())
        .collect();
    let mut discovery = Vec::new();
    for task in &live {
        let alternatives = proposed_by_task.get(task.as_str()).map_or(0, Vec::len);
        let selected_cap = selected_by_task
            .get(task.as_str())
            .and_then(|method| method.capability.as_deref());
        let uncertain = selected_cap.is_some() && !backed(selected_cap);
        let model = doc.investigation.iter().find(|model| model.task == *task);
        // An explicit investigation model is itself structured uncertainty:
        // a task carrying one is a discovery candidate even when the plan
        // otherwise looks settled.
        if alternatives == 0 && !uncertain && model.is_none() {
            continue;
        }
        let uncertainty = if backed(selected_cap) {
            MEASURED_FACTOR
        } else {
            UNCERTAIN_FACTOR
        };
        let question = match (selected_cap, alternatives) {
            (Some(cap), n) if n > 0 => format!(
                "Investigate whether an alternative to capability:{cap} improves task:{task} ({n} proposed alternative(s))"
            ),
            (Some(cap), _) => format!(
                "Investigate whether evidence or another approach changes task:{task} via capability:{cap}"
            ),
            (_, n) if n > 0 => {
                format!("Investigate the {n} proposed alternative(s) for task:{task}")
            }
            _ => format!("Investigate the explicit hypotheses for task:{task}"),
        };
        let investigation_cost = model
            .filter(|model| model.cost.is_finite() && model.cost >= 0.0)
            .map(|model| model.cost);
        let selected = selected_by_task.get(task.as_str()).map(|m| m.id.as_str());
        let accepted: Vec<&str> = doc
            .method
            .iter()
            .filter(|m| m.task == *task && m.status == MethodStatus::Accepted)
            .map(|m| m.id.as_str())
            .collect();
        let (value_of_information, value_of_information_unknown) = match model {
            None => (None, None),
            Some(model) => match evaluate_voi(model, task, selected, &accepted, &rejected) {
                Ok(voi) => (Some(voi), None),
                Err(reason) => (None, Some(reason)),
            },
        };
        discovery.push(DiscoveryItem {
            subject: format!("task:{task}"),
            critical: on_path.contains(task),
            uncertainty,
            alternatives,
            downstream_decisions: dependents.get(task).copied().unwrap_or(0),
            investigation_cost,
            material_change_likelihood: None,
            value_of_information,
            value_of_information_unknown,
            question,
        });
    }
    discovery.sort_by(|a, b| {
        b.critical
            .cmp(&a.critical)
            .then_with(|| b.uncertainty.total_cmp(&a.uncertainty))
            .then_with(|| b.alternatives.cmp(&a.alternatives))
            .then_with(|| b.downstream_decisions.cmp(&a.downstream_decisions))
            .then_with(|| a.subject.cmp(&b.subject))
    });

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

    // Critical-path cost resting on declared guesses, or with no duration
    // estimate at all: critical leaf tasks whose selected duration
    // capability has no measured evidence. Leaves without a duration
    // estimate stay in the report so the gap is visible, but their reason
    // must say unknown instead of fabricating 0.000 expected minutes.
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
        // A critical leaf whose capability has no effective duration
        // estimate has no expected minutes to report: label the duration
        // unknown instead of fabricating 0.000 from the zero-length
        // timing. The weighted cost only ranks the item.
        let has_duration =
            effective.contains_key(&(capability.to_string(), "duration_minutes".to_string()));
        let reason = if has_duration {
            format!(
                "critical path; {duration:.3} expected minutes via \
                 capability:{capability} rest on declared guesses"
            )
        } else {
            format!(
                "critical path; duration unknown via \
                 capability:{capability} (no duration estimate declared)"
            )
        };
        push(
            format!("task:{task_id}"),
            KIND_UNMEASURED_CRITICAL,
            task_id,
            capability_cost(capability, &effective, &doc.objective.weights),
            UNCERTAIN_FACTOR,
            true,
            reason,
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
        discovery,
    }
}

/// EVSI for `task`'s investigation model against a baseline: the `selected`
/// method, else (a task outside the selected plan) the `accepted` method
/// with the highest worst-case expected value over the bounds, ties broken
/// by id; E017 guarantees every hypothesis values each accepted method.
/// Every way the value can stay unknown returns its reason.
fn evaluate_voi(
    model: &pc_ir::Investigation,
    task: &str,
    selected: Option<&str>,
    accepted: &[&str],
    rejected: &BTreeSet<&str>,
) -> Result<ValueOfInformation, String> {
    if !model.cost.is_finite() || model.cost < 0.0 || model.cost_source.trim().is_empty() {
        return Err("investigation cost is not finite, non-negative and sourced".to_string());
    }
    if model.hypotheses.is_empty() {
        return Err("the model has no explicit hypotheses".to_string());
    }
    let unsolved = || "the probability bounds could not be solved".to_string();
    let mut bounds = Vec::new();
    let mut values: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut sources = Vec::new();
    for hypothesis in &model.hypotheses {
        let id = &hypothesis.id;
        if id.trim().is_empty() || hypothesis.source.trim().is_empty() {
            return Err(format!("hypothesis `{id}` lacks an id or provenance"));
        }
        let Some(probability) = hypothesis.probability.as_ref() else {
            return Err(format!(
                "hypothesis `{id}` has an unknown probability, so EVSI stays unknown"
            ));
        };
        if !probability.low.is_finite()
            || !probability.high.is_finite()
            || probability.low < 0.0
            || probability.high > 1.0
            || probability.low > probability.high
            || probability.source.trim().is_empty()
        {
            return Err(format!(
                "hypothesis `{id}` has invalid or unsourced probability bounds"
            ));
        }
        for alternative in &hypothesis.alternatives {
            if !alternative.value.is_finite() {
                return Err(format!("hypothesis `{id}` has a non-finite utility"));
            }
            if !rejected.contains(alternative.alternative.as_str()) {
                values
                    .entry(alternative.alternative.as_str())
                    .or_default()
                    .push(alternative.value);
            }
        }
        bounds.push((probability.low, probability.high));
        sources.push(format!("{} ({})", hypothesis.id, probability.source));
    }
    if values.values().any(|row| row.len() != bounds.len()) {
        return Err("the hypotheses do not value the same alternatives".to_string());
    }
    let low_sum: f64 = bounds.iter().map(|bound| bound.0).sum();
    let high_sum: f64 = bounds.iter().map(|bound| bound.1).sum();
    if low_sum > 1.0 + f64::EPSILON || high_sum < 1.0 - f64::EPSILON {
        return Err("the probability bounds admit no distribution summing to 1".to_string());
    }
    let (role, selected, scope) = if let Some(selected) = selected {
        ("selected method", selected, String::new())
    } else {
        let mut ranked = Vec::new();
        for id in accepted {
            let Some(row) = values.get(id) else {
                return Err(format!("the hypotheses do not value accepted method {id}"));
            };
            ranked.push((*id, expected_bound(&bounds, row, false)));
        }
        let Some(&(best, _)) = ranked
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        else {
            return Err(
                "the task has no selected or accepted method to compare against".to_string(),
            );
        };
        let mut names = accepted.to_vec();
        names.sort_unstable();
        (
            "accepted method",
            best,
            format!(
                "; task:{task} is not in the currently selected plan, so the comparison is against accepted method {best}, the highest worst-case expected value among accepted methods {} (ties broken by id)",
                names.join(", ")
            ),
        )
    };
    let Some(current) = values.get(selected) else {
        return Err(format!("the hypotheses do not value {role} {selected}"));
    };
    let best: Vec<f64> = (0..bounds.len())
        .map(|i| {
            values
                .values()
                .map(|row| row[i])
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .collect();
    let difference =
        |a: &[f64], b: &[f64]| -> Vec<f64> { a.iter().zip(b).map(|(x, y)| x - y).collect() };
    // EVSI(p) = E_p[max_a V] - max_a E_p[V(a)] = min_a E_p[best - V(a)].
    let regret: Vec<Vec<f64>> = values.values().map(|row| difference(&best, row)).collect();
    let evsi_low = regret
        .iter()
        .map(|row| expected_bound(&bounds, row, false))
        .fold(f64::INFINITY, f64::min);
    let negated: Vec<Vec<f64>> = regret
        .iter()
        .map(|row| row.iter().map(|value| -value).collect())
        .collect();
    let evsi_high = -minimax_expectation(&bounds, &negated).ok_or_else(unsolved)?;
    // Switch gain(p) = max_a E_p[V(a) - V(selected)]: the gain from acting on
    // current beliefs alone, never information value.
    let advantage: Vec<(&str, Vec<f64>)> = values
        .iter()
        .map(|(id, row)| (*id, difference(row, current)))
        .collect();
    let switch_low = minimax_expectation(
        &bounds,
        &advantage
            .iter()
            .map(|(_, row)| row.clone())
            .collect::<Vec<_>>(),
    )
    .ok_or_else(unsolved)?;
    let switch_high = advantage
        .iter()
        .map(|(_, row)| expected_bound(&bounds, row, true))
        .fold(0.0, f64::max);
    let preferred = advantage
        .iter()
        .map(|(id, row)| (*id, expected_bound(&bounds, row, false)))
        .filter(|(_, gain)| *gain > SOLVER_TOLERANCE)
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or(selected, |(id, _)| id);
    // A method is prior-optimal somewhere in the bounds when
    // min_p max_b E_p[V(b) - V(a)] <= 0.
    let switch_candidates: Vec<&str> = if preferred == selected && switch_low > SOLVER_TOLERANCE {
        values
            .iter()
            .filter(|(id, _)| **id != selected)
            .filter(|(_, row)| {
                let losses: Vec<Vec<f64>> = values
                    .values()
                    .map(|other| difference(other, row))
                    .collect();
                minimax_expectation(&bounds, &losses).is_some_and(|loss| loss <= SOLVER_TOLERANCE)
            })
            .map(|(id, _)| *id)
            .collect()
    } else {
        Vec::new()
    };
    let low = evsi_low - model.cost;
    let high = evsi_high - model.cost;
    let conclusion = if low > 0.0 {
        "investigation-outweighs-implementation"
    } else if high > 0.0 {
        "investigation-may-outweigh-implementation"
    } else if preferred != selected {
        "switch-to-preferred-approach"
    } else if !switch_candidates.is_empty() {
        "switch-from-selected-approach"
    } else {
        "implement-preferred-approach"
    };
    let switch = if preferred != selected {
        format!(
            "without investigating, implement {preferred} instead of {role} {selected} (gain [{switch_low:.3}, {switch_high:.3}] on current beliefs)"
        )
    } else if !switch_candidates.is_empty() {
        format!(
            "without investigating, switch away from {role} {selected} (gain [{switch_low:.3}, {switch_high:.3}] on current beliefs); the best replacement depends on the probabilities among {}",
            switch_candidates.join(", ")
        )
    } else {
        format!(
            "switching from {role} {selected} gains [{switch_low:.3}, {switch_high:.3}] on current beliefs"
        )
    };
    Ok(ValueOfInformation {
        evsi_low: low,
        evsi_high: high,
        switch_gain_low: switch_low,
        switch_gain_high: switch_high,
        preferred_method: preferred.to_string(),
        switch_candidates: switch_candidates.iter().map(ToString::to_string).collect(),
        conclusion: conclusion.to_string(),
        explanation: format!(
            "EVSI [{low:.3}, {high:.3}] utility points over the prior-optimal choice after cost {:.3} from {}; {switch}{scope}; hypothesis probability sources: {}",
            model.cost,
            model.cost_source,
            sources.join(", ")
        ),
    })
}

/// Extreme of `sum_i p_i * values_i` over every distribution inside the
/// probability `bounds` that sums to 1: start each hypothesis at its low
/// bound and hand the remaining mass to the highest values first when
/// maximizing, the lowest values first when minimizing.
fn expected_bound(bounds: &[(f64, f64)], values: &[f64], maximize: bool) -> f64 {
    let mut order: Vec<usize> = (0..bounds.len()).collect();
    order.sort_by(|&a, &b| {
        if maximize {
            values[b].total_cmp(&values[a])
        } else {
            values[a].total_cmp(&values[b])
        }
    });
    let mut slack = 1.0 - bounds.iter().map(|bound| bound.0).sum::<f64>();
    let mut total = 0.0;
    for i in order {
        let (low, high) = bounds[i];
        let extra = slack.min(high - low).max(0.0);
        slack -= extra;
        total += (low + extra) * values[i];
    }
    total
}

/// `min_p max_r sum_i p_i * r_i` over distributions inside `bounds` summing
/// to 1, solved exactly as a linear program.
fn minimax_expectation(bounds: &[(f64, f64)], rows: &[Vec<f64>]) -> Option<f64> {
    use good_lp::{Expression, ProblemVariables, Solution, SolverModel, default_solver, variable};
    let mut vars = ProblemVariables::new();
    let probabilities: Vec<_> = bounds
        .iter()
        .map(|&(low, high)| vars.add(variable().min(low).max(high)))
        .collect();
    let ceiling = vars.add(variable());
    let mut problem = vars
        .minimise(ceiling)
        .using(default_solver)
        .with(probabilities.iter().copied().sum::<Expression>().eq(1.0));
    for row in rows {
        let expectation: Expression = probabilities
            .iter()
            .zip(row)
            .map(|(probability, value)| *value * *probability)
            .sum();
        problem = problem.with(expectation.leq(ceiling));
    }
    problem.solve().ok().map(|solution| solution.value(ceiling))
}

/// Estimated remaining open work for `pc compile`'s open-work line, using
/// the frontier's own numbers: the summed per-task weighted expected cost
/// over the unresolved live tasks that carry a contract-clean pending
/// estimate, beside the unresolved live tasks with no such estimate.
/// Blocked alternatives never feed the total, matching the ranking, and
/// this never re-ranks anything.
#[must_use]
pub fn open_work(
    doc: &PlanDoc,
    root_task: &str,
    unresolved_tasks: &[String],
) -> crate::compile::OpenWorkSummary {
    let live = live_tasks(doc, root_task);
    let estimated = crate::estimate(doc);
    let effective = estimated.effective_map();
    let pending = pending_invokes(doc, &live);
    // Per-task largest cost among contract-clean pending invokes with a
    // weighted estimate, mirroring the ranked group's open total but
    // restricted to open tasks below.
    let mut task_cost: BTreeMap<String, f64> = BTreeMap::new();
    for method in &pending {
        if crate::impact::contract_note_for(doc, method).is_some() {
            continue;
        }
        let Some(cost) = method
            .capability
            .as_deref()
            .and_then(|cap| capability_cost(cap, &effective, &doc.objective.weights))
        else {
            continue;
        };
        let entry = task_cost.entry(method.task.clone()).or_insert(cost);
        *entry = entry.max(cost);
    }
    let mut estimated_tasks = Vec::new();
    let mut unestimated_tasks = Vec::new();
    let mut total = 0.0;
    let mut unresolved: Vec<&String> = unresolved_tasks.iter().collect();
    unresolved.sort();
    for id in unresolved {
        if !live.contains(id) {
            continue;
        }
        match task_cost.get(id) {
            Some(cost) => {
                total += cost;
                estimated_tasks.push(id.clone());
            }
            None => unestimated_tasks.push(id.clone()),
        }
    }
    crate::compile::OpenWorkSummary {
        estimated_weighted_cost: total,
        estimated_tasks,
        unestimated_tasks,
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

    #[test]
    fn discovery_without_selected_capability_is_uncertain() {
        let mut plan = doc();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let open = report
            .discovery
            .iter()
            .find(|item| item.subject == "task:open-work")
            .unwrap();
        assert_eq!(open.alternatives, 1);
        assert_eq!(open.uncertainty, UNCERTAIN_FACTOR);
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
            tasks_axi: None,
            beads_jsonl: None,
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
            tasks_axi: None,
            beads_jsonl: None,
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
            tasks_axi: None,
            beads_jsonl: None,
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
            tasks_axi: None,
            beads_jsonl: None,
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

    /// Base plan for VOI tests: `work` selects accepted `work-keep`, with a
    /// proposed `work-try` alternative. Hypothesis `h-slow` (keep 10, try 20)
    /// and `h-fast` (keep 18, try 12) each carry probability [0.4, 0.6]. With
    /// p = P(h-slow), pre-cost EVSI is min(10p, 6(1 - p)) in [2.4, 3.6] and
    /// work-try beats work-keep on current beliefs by 16p - 6 in [0.4, 3.6].
    fn voi_plan(cost: f64) -> PlanDoc {
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
id = "work"
title = "Work"
state = "resolved"

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["work"]
status = "accepted"
provenance = "human:test"

[[method]]
id = "work-keep"
task = "work"
kind = "invoke"
capability = "keep@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "work-try"
task = "work"
kind = "invoke"
capability = "try@1"
status = "proposed"
provenance = "agent:test"

[[capability]]
id = "keep@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 2.0, likely = 4.0, high = 6.0, provenance = "declared" }]

[[capability]]
id = "try@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 1.0, likely = 2.0, high = 3.0, provenance = "declared" }]
"#;
        let mut plan: PlanDoc = pc_ir::from_toml(text).unwrap();
        let probability = |low: f64, high: f64| pc_ir::ProbabilityEstimate {
            low,
            high,
            provenance: pc_ir::EstimateProvenance::Declared,
            source: "test:calibration".to_string(),
            observations: None,
        };
        let hypothesis =
            |id: &str, keep: f64, attempt: f64, probability: Option<pc_ir::ProbabilityEstimate>| {
                pc_ir::InvestigationHypothesis {
                    id: id.to_string(),
                    probability,
                    source: "test:review".to_string(),
                    alternatives: vec![
                        pc_ir::HypothesisAlternativeValue {
                            alternative: "work-keep".to_string(),
                            value: keep,
                        },
                        pc_ir::HypothesisAlternativeValue {
                            alternative: "work-try".to_string(),
                            value: attempt,
                        },
                    ],
                }
            };
        plan.investigation.push(pc_ir::Investigation {
            task: "work".to_string(),
            cost,
            cost_source: "test:bench".to_string(),
            hypotheses: vec![
                hypothesis("h-slow", 10.0, 20.0, Some(probability(0.4, 0.6))),
                hypothesis("h-fast", 18.0, 12.0, Some(probability(0.4, 0.6))),
            ],
        });
        plan
    }

    fn discovery_entry<'a>(report: &'a FrontierReport, subject: &str) -> &'a DiscoveryItem {
        report
            .discovery
            .iter()
            .find(|item| item.subject == subject)
            .unwrap()
    }

    fn voi_of(plan: &mut PlanDoc) -> ValueOfInformation {
        assert!(checked(plan).ok());
        let compiled = crate::compile(plan).unwrap();
        let report = frontier(plan, &compiled);
        discovery_entry(&report, "task:work")
            .value_of_information
            .clone()
            .unwrap()
    }

    fn assert_interval(actual: (f64, f64), expected: (f64, f64)) {
        assert!(
            (actual.0 - expected.0).abs() < 1e-9 && (actual.1 - expected.1).abs() < 1e-9,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn voi_outweighs_implementation_when_gain_exceeds_cost() {
        // Catches a missing or sign-flipped cost subtraction: EVSI [2.4, 3.6]
        // minus cost 1 must stay positive throughout.
        let mut plan = voi_plan(1.0);
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (1.4, 2.6));
        assert_eq!(voi.conclusion, "investigation-outweighs-implementation");
        assert!(voi.explanation.contains("work-keep"), "{}", voi.explanation);
    }

    #[test]
    fn voi_measures_information_against_the_prior_optimal_choice() {
        // Catches measuring EVSI against the selected method: at p = 0.5,
        // E[keep] = 14 and E[try] = 16, so information is worth 19 - 16 = 3,
        // not 19 - 14 = 5; the 2 from switching is reported separately.
        let mut plan = voi_plan(0.0);
        for hypothesis in &mut plan.investigation[0].hypotheses {
            let probability = hypothesis.probability.as_mut().unwrap();
            probability.low = 0.5;
            probability.high = 0.5;
        }
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (3.0, 3.0));
        assert_interval((voi.switch_gain_low, voi.switch_gain_high), (2.0, 2.0));
        assert_eq!(voi.preferred_method, "work-try");
    }

    #[test]
    fn voi_recommends_switching_when_an_alternative_dominates() {
        // Catches calling a switch gain information value: work-try beats
        // work-keep by 10 under every hypothesis, so information is worth
        // nothing and the advice is to implement work-try directly.
        let mut plan = voi_plan(1.0);
        for hypothesis in &mut plan.investigation[0].hypotheses {
            let keep = hypothesis.alternatives[0].value;
            hypothesis.alternatives[1].value = keep + 10.0;
        }
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (-1.0, -1.0));
        assert_interval((voi.switch_gain_low, voi.switch_gain_high), (10.0, 10.0));
        assert_eq!(voi.preferred_method, "work-try");
        assert_eq!(voi.conclusion, "switch-to-preferred-approach");
    }

    #[test]
    fn voi_may_outweigh_when_interval_straddles_cost() {
        // Catches a collapsed point estimate: EVSI [2.4, 3.6] minus cost 3
        // straddles zero, so the verdict must stay qualified.
        let mut plan = voi_plan(3.0);
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (-0.6, 0.6));
        assert_eq!(voi.conclusion, "investigation-may-outweigh-implementation");
    }

    #[test]
    fn voi_prefers_implementation_when_cost_dominates() {
        // Catches recommending a probe whose cost exceeds any possible gain;
        // work-try still beats work-keep throughout, so switch to it.
        let mut plan = voi_plan(100.0);
        let voi = voi_of(&mut plan);
        assert!(voi.evsi_high < 0.0);
        assert_interval((voi.switch_gain_low, voi.switch_gain_high), (0.4, 3.6));
        assert_eq!(voi.conclusion, "switch-to-preferred-approach");
    }

    #[test]
    fn voi_switches_away_when_no_single_alternative_dominates() {
        // Catches keeping the selected method when it is prior-optimal under
        // no allowed belief: relative to work-keep, work-try is (3, -1) and
        // work-alt is (-1, 3) with p in [0, 1], so some alternative gains at
        // least 1 everywhere, yet neither wins throughout and EVSI <= 2 < 5.
        let mut plan = voi_plan(5.0);
        let mut alt = plan
            .method
            .iter()
            .find(|m| m.id == "work-try")
            .unwrap()
            .clone();
        alt.id = "work-alt".into();
        plan.method.push(alt);
        for (hypothesis, (attempt, other)) in plan.investigation[0]
            .hypotheses
            .iter_mut()
            .zip([(3.0, -1.0), (-1.0, 3.0)])
        {
            let probability = hypothesis.probability.as_mut().unwrap();
            probability.low = 0.0;
            probability.high = 1.0;
            hypothesis.alternatives[0].value = 0.0;
            hypothesis.alternatives[1].value = attempt;
            hypothesis
                .alternatives
                .push(pc_ir::HypothesisAlternativeValue {
                    alternative: "work-alt".to_string(),
                    value: other,
                });
        }
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (-5.0, -3.0));
        assert_interval((voi.switch_gain_low, voi.switch_gain_high), (1.0, 3.0));
        assert_eq!(voi.preferred_method, "work-keep");
        assert_eq!(voi.switch_candidates, ["work-alt", "work-try"]);
        assert_eq!(voi.conclusion, "switch-from-selected-approach");
    }

    #[test]
    fn voi_bounds_respect_probabilities_summing_to_one() {
        // Catches bounding each hypothesis independently: with [0.3, 0.9]
        // bounds, h-fast >= 0.3 caps h-slow at 0.7, so pre-cost EVSI is
        // min(10p, 6(1 - p)) over p in [0.3, 0.7]: [1.8, 3.75].
        let mut plan = voi_plan(3.0);
        for hypothesis in &mut plan.investigation[0].hypotheses {
            let probability = hypothesis.probability.as_mut().unwrap();
            probability.low = 0.3;
            probability.high = 0.9;
        }
        let voi = voi_of(&mut plan);
        assert_interval((voi.evsi_low, voi.evsi_high), (-1.2, 0.75));
        assert_eq!(voi.conclusion, "investigation-may-outweigh-implementation");
    }

    #[test]
    fn voi_stays_unknown_when_a_probability_is_unknown() {
        // Catches fabricating a midpoint for a missing probability: the
        // cost is still reported, but no EVSI interval may appear.
        let mut plan = voi_plan(1.0);
        plan.investigation[0].hypotheses[1].probability = None;
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let entry = discovery_entry(&report, "task:work");
        assert_eq!(entry.investigation_cost, Some(1.0));
        assert_eq!(entry.value_of_information, None);
        assert_eq!(
            entry.value_of_information_unknown.as_deref(),
            Some("hypothesis `h-fast` has an unknown probability, so EVSI stays unknown")
        );
    }

    #[test]
    fn voi_stays_unknown_when_bounds_admit_no_distribution() {
        // Catches emitting EVSI from incoherent bounds (lows sum past 1).
        let mut plan = voi_plan(1.0);
        for hypothesis in &mut plan.investigation[0].hypotheses {
            hypothesis.probability.as_mut().unwrap().low = 0.55;
        }
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let entry = discovery_entry(&report, "task:work");
        assert_eq!(entry.value_of_information, None);
        assert_eq!(
            entry.value_of_information_unknown.as_deref(),
            Some("the probability bounds admit no distribution summing to 1")
        );
    }

    /// `voi_plan(1.0)` with `work` reachable only through a proposed
    /// decomposition, so P6 selects accepted `[other]` and never `work`.
    fn unselected_voi_plan() -> PlanDoc {
        let mut plan = voi_plan(1.0);
        plan.task.push(pc_ir::Task {
            id: "other".into(),
            title: "Other".into(),
            state: pc_ir::TaskState::Resolved,
            tasks_axi: None,
            beads_jsonl: None,
            consumes: Vec::new(),
            produces: Vec::new(),
        });
        let root = plan.method.iter_mut().find(|m| m.id == "root-all").unwrap();
        root.subtasks = vec!["other".into()];
        let mut alternative = root.clone();
        alternative.id = "root-alt".into();
        alternative.subtasks = vec!["work".into()];
        alternative.status = MethodStatus::Proposed;
        plan.method.push(alternative);
        let mut other = plan
            .method
            .iter()
            .find(|m| m.id == "work-keep")
            .unwrap()
            .clone();
        other.id = "other-run".into();
        other.task = "other".into();
        plan.method.push(other);
        plan
    }

    #[test]
    fn voi_compares_unselected_task_against_its_accepted_method() {
        // Catches a silent empty VOI for a modelled task outside the
        // selection: root selects accepted [other], and work is reachable
        // only through proposed decomposition [work], so P6 never selects
        // work-keep, yet the model still values it.
        let mut plan = unselected_voi_plan();
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        assert!(!compiled.selection.contains_key("work"));
        let report = frontier(&plan, &compiled);
        let entry = discovery_entry(&report, "task:work");
        assert_eq!(entry.value_of_information_unknown, None);
        let voi = entry.value_of_information.as_ref().unwrap();
        assert_interval((voi.evsi_low, voi.evsi_high), (1.4, 2.6));
        assert_eq!(voi.preferred_method, "work-try");
        assert!(
            voi.explanation
                .contains("task:work is not in the currently selected plan"),
            "{}",
            voi.explanation
        );
    }

    #[test]
    fn voi_baseline_for_unselected_task_is_best_accepted_method() {
        // Catches an id-order baseline: with work-try also accepted, its
        // worst-case expected value 15.2 beats work-keep's 13.2, so the
        // comparison is against work-try and both accepted methods are named.
        let mut plan = unselected_voi_plan();
        plan.method
            .iter_mut()
            .find(|m| m.id == "work-try")
            .unwrap()
            .status = MethodStatus::Accepted;
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let voi = discovery_entry(&report, "task:work")
            .value_of_information
            .clone()
            .unwrap();
        assert_interval((voi.evsi_low, voi.evsi_high), (1.4, 2.6));
        assert_interval((voi.switch_gain_low, voi.switch_gain_high), (0.0, 0.0));
        assert_eq!(voi.preferred_method, "work-try");
        assert!(
            voi.explanation.contains(
                "against accepted method work-try, the highest worst-case expected value among accepted methods work-keep, work-try (ties broken by id)"
            ),
            "{}",
            voi.explanation
        );
    }

    #[test]
    fn explicit_model_keeps_settled_task_on_discovery_frontier() {
        // Catches silently dropping an explicit model on a task the plan
        // otherwise looks settled on (measured selection, no proposals), and
        // a rejected alternative feeding the gain: only work-keep remains,
        // so information is worth nothing and the cost decides.
        let mut plan = voi_plan(1.0);
        plan.method
            .iter_mut()
            .find(|method| method.id == "work-try")
            .unwrap()
            .status = MethodStatus::Rejected;
        plan.evidence.push(pc_ir::Evidence {
            subject: "capability:keep@1".into(),
            metric: "duration_minutes".into(),
            value: 4.0,
            unit: "minutes".into(),
            time: "2026-10-01T00:00:00Z".into(),
            source: "test".into(),
            basis: None,
            artifact_hash: None,
        });
        assert!(checked(&mut plan).ok());
        let compiled = crate::compile(&plan).unwrap();
        let report = frontier(&plan, &compiled);
        let entry = discovery_entry(&report, "task:work");
        let voi = entry.value_of_information.as_ref().unwrap();
        assert_interval((voi.evsi_low, voi.evsi_high), (-1.0, -1.0));
        assert_eq!(voi.preferred_method, "work-keep");
        assert_eq!(voi.conclusion, "implement-preferred-approach");
    }

    #[test]
    fn unmeasured_critical_without_estimate_reports_unknown_not_zero_minutes() {
        // Regression for the Ren native cycle (examples/ren-native):
        // `measure-native-baseline` was described as "0.000 expected
        // minutes ... rest on declared guesses" although its capability
        // declares no estimate at all. Unknown must stay unknown.
        // A zero-duration leaf is off the critical path beside estimated
        // work, so each side gets its own single-leaf plan where it is
        // the critical leaf.
        let bare_text = r#"
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
id = "leaf"
title = "Leaf"
state = "open"

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["leaf"]
status = "accepted"
provenance = "human:test"

[[method]]
id = "do-leaf"
task = "leaf"
kind = "invoke"
capability = "bare@1"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "bare@1"
kind = "tool"
version = "1"
"#;
        let mut bare = pc_ir::from_toml(bare_text).unwrap();
        assert!(checked(&mut bare).ok());
        let compiled = crate::compile(&bare).unwrap();
        let report = frontier(&bare, &compiled);
        let missing = report
            .unestimated
            .iter()
            .find(|i| i.subject == "task:leaf" && i.kind == KIND_UNMEASURED_CRITICAL)
            .unwrap();
        assert!(!missing.estimated, "{missing:?}");
        assert!(
            missing.reason.contains("unknown") || missing.reason.contains("unestimated"),
            "{}",
            missing.reason
        );
        assert!(
            !missing.reason.contains("expected minutes"),
            "{}",
            missing.reason
        );
        assert!(!missing.reason.contains("0.000"), "{}", missing.reason);
        assert!(
            !missing.reason.contains("declared guesses"),
            "{}",
            missing.reason
        );
        // Estimated work keeps its existing wording and ranking.
        let estimated_text = bare_text.replace("bare@1", "tool@1").replace(
            "[[capability]]\nid = \"tool@1\"\nkind = \"tool\"\nversion = \"1\"",
            "[[capability]]\nid = \"tool@1\"\nkind = \"tool\"\nversion = \"1\"\nestimates = [{ metric = \"duration_minutes\", low = 2.0, likely = 4.0, high = 6.0, provenance = \"declared\" }]",
        );
        let mut estimated = pc_ir::from_toml(&estimated_text).unwrap();
        assert!(checked(&mut estimated).ok());
        let compiled = crate::compile(&estimated).unwrap();
        let report = frontier(&estimated, &compiled);
        let kept = report
            .items
            .iter()
            .find(|i| i.subject == "task:leaf" && i.kind == KIND_UNMEASURED_CRITICAL)
            .unwrap();
        assert!(kept.estimated, "{kept:?}");
        assert!(
            kept.reason.contains("expected minutes") && kept.reason.contains("declared guesses"),
            "{}",
            kept.reason
        );
        // A multi-metric objective where the capability estimates only a
        // non-duration metric still has no expected minutes to report.
        let money_only_text = bare_text
            .replace(
                "weights = { duration_minutes = 1.0 }",
                "weights = { duration_minutes = 1.0, money_usd = 0.5 }",
            )
            .replace(
                "[[capability]]\nid = \"bare@1\"\nkind = \"tool\"\nversion = \"1\"",
                "[[capability]]\nid = \"bare@1\"\nkind = \"tool\"\nversion = \"1\"\nestimates = [{ metric = \"money_usd\", low = 1.0, likely = 2.0, high = 3.0, provenance = \"declared\" }]",
            );
        let mut money_only = pc_ir::from_toml(&money_only_text).unwrap();
        assert!(checked(&mut money_only).ok());
        let compiled = crate::compile(&money_only).unwrap();
        let report = frontier(&money_only, &compiled);
        let costed = report
            .items
            .iter()
            .find(|i| i.subject == "task:leaf" && i.kind == KIND_UNMEASURED_CRITICAL)
            .unwrap();
        assert!(costed.estimated, "{costed:?}");
        assert!(
            costed.reason.contains("duration unknown"),
            "{}",
            costed.reason
        );
        assert!(!costed.reason.contains("0.000"), "{}", costed.reason);
        assert!(
            !costed.reason.contains("expected minutes"),
            "{}",
            costed.reason
        );
        // An objective that does not weigh duration still reports the
        // declared duration estimate in minutes.
        let unweighted_text = estimated_text.replace(
            "weights = { duration_minutes = 1.0 }",
            "weights = { money_usd = 1.0 }",
        );
        let mut unweighted = pc_ir::from_toml(&unweighted_text).unwrap();
        assert!(checked(&mut unweighted).ok());
        let compiled = crate::compile(&unweighted).unwrap();
        let report = frontier(&unweighted, &compiled);
        let timed = report
            .unestimated
            .iter()
            .find(|i| i.subject == "task:leaf" && i.kind == KIND_UNMEASURED_CRITICAL)
            .unwrap();
        assert!(
            timed.reason.contains("expected minutes") && !timed.reason.contains("unknown"),
            "{}",
            timed.reason
        );
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
