//! Deterministic Pareto evaluation over complete method selections.
//!
//! Weighted P6 selection answers "what is best under these preferences".
//! Pareto evaluation answers "what are the tradeoffs when preferences are
//! not clear": every feasible assignment compares only on the measures
//! known for every one of them, and the report splits the set into the
//! non-dominated frontier, the dominated remainder naming its dominators,
//! and the incomplete alternatives (no value on a compared measure) shown
//! separately, never ranked. A measure only some alternatives price is
//! listed as not compared. No model, no database, no new IR fields.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use pc_ir::PlanDoc;
use serde::{Deserialize, Serialize};

use crate::compile::{CompileError, selection_inputs};

/// One feasible assignment no other complete alternative is better than
/// on the compared measures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParetoAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub makespan: f64,
    /// Weighted total-effort objective, preserved for clear preferences.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub objective: f64,
    /// Values over the measures this alternative prices.
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub objectives: BTreeMap<String, f64>,
    /// Valued measures this alternative does not price, sorted.
    pub unknown: Vec<String>,
}

/// One dominance on the compared measures: `by` is no worse on every one
/// of them (`on`) and strictly better on at least one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dominance {
    /// Id of the dominating alternative.
    pub by: String,
    /// Compared measures the dominance was decided on, sorted.
    pub on: Vec<String>,
    /// Valued measures unknown for either side, so not compared, sorted.
    pub unknown: Vec<String>,
}

/// One feasible assignment another complete alternative is worse than on
/// the compared measures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DominatedAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub makespan: f64,
    /// Weighted total-effort objective, preserved for clear preferences.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub objective: f64,
    /// Values over the measures this alternative prices.
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub objectives: BTreeMap<String, f64>,
    /// Valued measures this alternative does not price, sorted.
    pub unknown: Vec<String>,
    /// Frontier alternatives dominating this one, in id order.
    pub dominated_by: Vec<Dominance>,
}

/// One feasible assignment with no value on a compared measure: shown
/// separately, never ranked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncompleteAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub makespan: f64,
    /// Weighted total-effort objective, preserved for clear preferences.
    #[serde(serialize_with = "crate::json::ser_f64_3")]
    pub objective: f64,
    /// Values over the measures this alternative prices.
    #[serde(serialize_with = "crate::json::ser_map_f64_3")]
    pub objectives: BTreeMap<String, f64>,
    /// Valued measures this alternative does not price, sorted.
    pub unknown: Vec<String>,
}

/// Non-dominated alternatives, the dominated remainder naming what
/// dominates it, and the incomplete alternatives shown separately. Lower
/// is better on every compared measure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParetoReport {
    /// Measures known for every feasible assignment, in lexical order.
    pub compared: Vec<String>,
    /// Valued measures known for only some assignments, in lexical order:
    /// listed, never compared.
    pub not_compared: Vec<String>,
    /// Non-dominated complete alternatives in deterministic order.
    pub frontier: Vec<ParetoAlternative>,
    /// Dominated complete alternatives in deterministic order.
    pub dominated: Vec<DominatedAlternative>,
    /// Alternatives with no value on a compared measure, in deterministic
    /// order: shown separately, never ranked.
    pub incomplete: Vec<IncompleteAlternative>,
    /// `optimal` when the assignment search ran to completion,
    /// `best-found (gap unavailable)` when the time budget or the
    /// assignment cap cut it short.
    pub status: String,
    /// Feasible assignments evaluated (deduplicated).
    pub evaluated: usize,
    /// P6 time budget in milliseconds.
    pub time_budget_ms: u64,
}

/// Evaluate the Pareto frontier for the accepted plan.
///
/// Builds the same P6 selection inputs `compile` uses (accepted methods,
/// P5 effective expected costs, hard limits), then splits every feasible
/// assignment into frontier and dominated. Deterministic: same inputs,
/// byte-identical report.
pub fn pareto_frontier(
    doc: &PlanDoc,
    selection_budget: Duration,
) -> Result<ParetoReport, CompileError> {
    let estimated = crate::estimate::estimate(doc);
    let (selection_tasks, constraints, _, _) = selection_inputs(doc, &estimated);
    let unresolved: BTreeSet<String> = doc
        .task
        .iter()
        .filter(|task| {
            !matches!(
                task.state,
                pc_ir::TaskState::Done | pc_ir::TaskState::Dropped
            )
        })
        .filter(|task| {
            selection_tasks
                .iter()
                .any(|t| t.id == task.id && t.methods.is_empty())
        })
        .map(|task| task.id.clone())
        .collect();
    let report = pc_select::pareto(
        &selection_tasks,
        &doc.objective.root_task,
        &constraints,
        &["quality_loss"],
        &unresolved,
        selection_budget,
    )
    .map_err(|error| match error {
        pc_select::SelectError::Infeasible => CompileError::SelectionInfeasible,
        other => CompileError::SelectionFailed {
            message: other.to_string(),
        },
    })?;
    Ok(ParetoReport {
        compared: report.compared,
        not_compared: report.not_compared,
        frontier: report
            .frontier
            .into_iter()
            .map(|alt| ParetoAlternative {
                id: alt.id,
                methods: alt.methods,
                totals: alt.totals,
                makespan: alt.makespan,
                objective: alt.objective,
                objectives: alt.objectives,
                unknown: alt.unknown,
            })
            .collect(),
        dominated: report
            .dominated
            .into_iter()
            .map(|alt| DominatedAlternative {
                id: alt.id,
                methods: alt.methods,
                totals: alt.totals,
                makespan: alt.makespan,
                objective: alt.objective,
                objectives: alt.objectives,
                unknown: alt.unknown,
                dominated_by: alt
                    .dominated_by
                    .into_iter()
                    .map(|d| Dominance {
                        by: d.by,
                        on: d.on,
                        unknown: d.unknown,
                    })
                    .collect(),
            })
            .collect(),
        incomplete: report
            .incomplete
            .into_iter()
            .map(|alt| IncompleteAlternative {
                id: alt.id,
                methods: alt.methods,
                totals: alt.totals,
                makespan: alt.makespan,
                objective: alt.objective,
                objectives: alt.objectives,
                unknown: alt.unknown,
            })
            .collect(),
        status: match report.optimality {
            pc_select::Optimality::Optimal => "optimal".into(),
            pc_select::Optimality::BestFound { .. } => "best-found (gap unavailable)".into(),
            pc_select::Optimality::GreedyOnly => "greedy-only".into(),
        },
        evaluated: report.evaluated,
        time_budget_ms: selection_budget.as_millis() as u64,
    })
}
