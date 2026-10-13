//! Deterministic alternative-method selection (P6).
//!
//! Tasks are OR variables and methods are AND variables. A selected method
//! activates each child task; each task's cost is therefore counted once even
//! when multiple selected methods share it.
//!
//! Resource model (PC-08): a method lists the named resources it holds while
//! it runs, one unit of each, and [`Constraints::capacities`] caps how many
//! holders may overlap. Makespan selection and [`resource_constrained_schedule`]
//! schedule leaf work against those caps with a serial schedule-generation
//! scheme: exact over every precedence-feasible order for small graphs, a
//! deterministic list-schedule heuristic above that (reported as
//! `BestFound`, never `Optimal`). Total-effort selection
//! ignores resources; only time is affected, never cost.

use good_lp::{
    Expression, ProblemVariables, ResolutionError, Solution, SolutionStatus, SolverModel, Variable,
    WithTimeLimit, default_solver, variable,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use thiserror::Error;

/// A task in the selection problem.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// Stable task id.
    pub id: String,
    /// Alternative ways to achieve this task.
    pub methods: Vec<Method>,
}

/// A candidate method and the tasks it requires.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
    /// Stable method id.
    pub id: String,
    /// Expected metric costs. Values must be finite and non-negative.
    pub costs: BTreeMap<String, f64>,
    /// Required child task ids.
    pub children: Vec<String>,
    /// Sibling order edges `(from, to)` between required children: `to`
    /// starts only after `from` finishes.
    pub order_edges: Vec<(String, String)>,
    /// Named resources this method holds while it runs, one unit of each.
    /// Leaf methods inherit their capability's resources; decompose methods
    /// hold none unless set explicitly.
    pub resources: Vec<String>,
}

/// Additive objective weights and linear hard upper bounds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Constraints {
    /// Cost weight per metric. Missing weights are zero.
    pub weights: BTreeMap<String, f64>,
    /// Maximum allowed total cost per metric.
    pub limits: BTreeMap<String, f64>,
    /// Candidate `(task, method)` pairs excluded by hard constraints.
    pub forbidden_methods: BTreeSet<(String, String)>,
    /// Available capacity per named resource: how many holders may overlap.
    /// Values are instance counts (`floor`ed, minimum one slot); resources
    /// absent here are unlimited. Values must be finite and non-negative.
    pub capacities: BTreeMap<String, f64>,
}

/// Reported quality of a selected solution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Optimality {
    /// The ILP solver proved no better feasible solution exists.
    Optimal,
    /// A feasible solution was found, but the time budget ended before proof.
    BestFound {
        /// Relative gap from the best proven bound, in percent.
        gap_percent: Option<f64>,
    },
    /// The greedy solution was returned because the exact solve ended at its
    /// time limit without an incumbent or with a worse one.
    GreedyOnly,
}

/// Chosen method ids and their optimization summary.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// One chosen method for every needed task with methods.
    pub methods: BTreeMap<String, String>,
    /// Total selected costs, with shared tasks included once.
    pub totals: BTreeMap<String, f64>,
    /// Weighted objective value.
    pub objective: f64,
    /// Whether optimality was proved.
    pub optimality: Optimality,
    /// Configured exact-solve time limit.
    pub time_budget: Duration,
    /// Estimated completion time when selected with [`Objective::Makespan`].
    pub makespan: Option<f64>,
    /// Human-readable reason the objective favored this selection.
    pub explanation: String,
}

/// Metric used to compare feasible method selections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Objective {
    /// Minimize the weighted sum of selected work, counting shared tasks once.
    TotalEffort,
    /// Minimize resource-constrained completion time using
    /// `duration_minutes` and [`Constraints::capacities`].
    Makespan,
}

/// P6 failures.
#[derive(Debug, Error, PartialEq)]
pub enum SelectError {
    /// The selection graph is malformed.
    #[error("invalid selection graph: {0}")]
    Invalid(String),
    /// No feasible solution satisfies all hard limits.
    #[error("no feasible method selection satisfies the hard constraints")]
    Infeasible,
    /// The time budget ended before any feasible selection was found.
    #[error("time budget exhausted before a feasible method selection was found")]
    TimeLimit,
    /// Solver failed for a reason other than infeasibility or the time limit.
    #[error("ILP solver failed: {0}")]
    Solver(String),
}

/// Select methods using greedy tree cost and an exact pure-Rust 0/1 ILP.
///
/// Greedy is kept as a feasible fallback. The ILP uses one binary variable per
/// needed task and candidate method, so common descendants are activated and
/// charged once. `time_budget` applies to the exact solve.
pub fn select(
    tasks: &[Task],
    root: &str,
    constraints: &Constraints,
    time_budget: Duration,
) -> Result<Selection, SelectError> {
    let by_id: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    if by_id.len() != tasks.len() {
        return Err(SelectError::Invalid("duplicate task id".into()));
    }
    if !by_id.contains_key(root) {
        return Err(SelectError::Invalid(format!("missing root task `{root}`")));
    }
    validate_graph(&by_id, constraints)?;
    let reachable = reachable_tasks(&by_id, root);
    let greedy = greedy_select(&by_id, &reachable, root, constraints)?;
    let mut vars = ProblemVariables::new();
    let needed: BTreeMap<_, Variable> = reachable
        .iter()
        .map(|id| (id.as_str(), vars.add(variable().binary())))
        .collect();
    let mut chosen = BTreeMap::new();
    for id in &reachable {
        if let Some(task) = by_id.get(id.as_str()) {
            for method in &task.methods {
                chosen.insert(
                    (id.as_str(), method.id.as_str()),
                    vars.add(variable().binary()),
                );
            }
        }
    }
    let mut objective = Expression::from(0.0);
    let mut metrics = BTreeSet::new();
    for id in &reachable {
        if let Some(task) = by_id.get(id.as_str()) {
            for method in &task.methods {
                metrics.extend(method.costs.keys().cloned());
            }
        }
    }
    let mut totals: BTreeMap<String, Expression> = metrics
        .iter()
        .map(|m| (m.clone(), Expression::from(0.0)))
        .collect();
    for id in &reachable {
        let methods = by_id
            .get(id.as_str())
            .map(|t| t.methods.as_slice())
            .unwrap_or_default();
        if methods.is_empty() {
            continue;
        }
        for method in methods {
            let var = chosen[&(id.as_str(), method.id.as_str())];
            for (metric, value) in &method.costs {
                *totals
                    .entry(metric.clone())
                    .or_insert_with(|| Expression::from(0.0)) += *value * var;
                objective += constraints.weights.get(metric).copied().unwrap_or(0.0) * *value * var;
            }
        }
    }
    // Rebuild constraints while consuming the problem builder.
    let mut problem = vars
        .minimise(objective)
        .using(default_solver)
        .with_time_limit(time_budget.as_secs_f64());
    problem.add_constraint(Expression::from(needed[root]).geq(1));
    let mut parents: BTreeMap<&str, Expression> = BTreeMap::new();
    for id in &reachable {
        for method in by_id
            .get(id.as_str())
            .map(|t| t.methods.as_slice())
            .unwrap_or_default()
        {
            let var = chosen[&(id.as_str(), method.id.as_str())];
            for child in &method.children {
                *parents.entry(child.as_str()).or_default() += var;
            }
        }
    }
    for id in reachable.iter().filter(|id| id.as_str() != root) {
        problem.add_constraint(
            Expression::from(needed[id.as_str()])
                .leq(parents.remove(id.as_str()).unwrap_or_default()),
        );
    }
    for id in &reachable {
        let task_var = needed[id.as_str()];
        let methods = by_id
            .get(id.as_str())
            .map(|t| t.methods.as_slice())
            .unwrap_or_default();
        if methods.is_empty() {
            continue;
        }
        let sum = methods.iter().fold(Expression::from(0.0), |sum, m| {
            sum + chosen[&(id.as_str(), m.id.as_str())]
        });
        problem.add_constraint(sum.eq(task_var));
        for method in methods {
            let var = chosen[&(id.as_str(), method.id.as_str())];
            if constraints
                .forbidden_methods
                .contains(&(id.clone(), method.id.clone()))
            {
                problem.add_constraint(Expression::from(var).leq(0));
            }
            for child in &method.children {
                problem.add_constraint(Expression::from(needed[child.as_str()]).geq(var));
            }
        }
    }
    for (metric, limit) in &constraints.limits {
        problem.add_constraint(
            totals
                .get(metric)
                .cloned()
                .unwrap_or_else(|| Expression::from(0.0))
                .leq(*limit),
        );
    }
    match problem.solve() {
        Ok(solution) => {
            let selected = extract(&reachable, &by_id, &chosen, &solution);
            let status = solution.status();
            let gap = solution.into_inner().gap().map(|value| value * 100.0);
            let optimality = match status {
                SolutionStatus::Optimal => Optimality::Optimal,
                SolutionStatus::TimeLimit | SolutionStatus::GapLimit => {
                    Optimality::BestFound { gap_percent: gap }
                }
            };
            let result = make_selection(
                selected,
                &by_id,
                &reachable,
                constraints,
                optimality,
                time_budget,
            );
            if optimality != Optimality::Optimal
                && greedy
                    .as_ref()
                    .is_some_and(|g| result.objective > g.objective + 1e-7)
            {
                return Ok(Selection {
                    optimality: Optimality::GreedyOnly,
                    time_budget,
                    ..greedy.expect("checked above")
                });
            }
            Ok(result)
        }
        Err(ResolutionError::Other(_)) => greedy
            .map(|g| Selection {
                optimality: Optimality::GreedyOnly,
                time_budget,
                ..g
            })
            .ok_or(SelectError::TimeLimit),
        Err(ResolutionError::Infeasible) => Err(SelectError::Infeasible),
        Err(error) => Err(SelectError::Solver(error.to_string())),
    }
}

/// Select using either additive total effort or resource-constrained makespan.
/// Makespan selection searches method assignments of activated tasks exactly
/// until the time budget expires; a feasible greedy selection is retained as
/// a fallback. Each candidate is scored with the resource-constrained
/// schedule, so contention for a limited resource can flip the winner.
pub fn select_with_objective(
    tasks: &[Task],
    root: &str,
    constraints: &Constraints,
    time_budget: Duration,
    objective: Objective,
) -> Result<Selection, SelectError> {
    if objective == Objective::TotalEffort {
        return select(tasks, root, constraints, time_budget);
    }
    let by_id: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    if by_id.len() != tasks.len() {
        return Err(SelectError::Invalid("duplicate task id".into()));
    }
    if !by_id.contains_key(root) {
        return Err(SelectError::Invalid(format!("missing root task `{root}`")));
    }
    validate_graph(&by_id, constraints)?;
    let reachable = reachable_tasks(&by_id, root);
    let greedy = greedy_select(&by_id, &reachable, root, constraints)?
        .and_then(|g| candidate(&by_id, constraints, &g.methods, root));
    let mut search = MakespanSearch {
        by_id: &by_id,
        root,
        constraints,
        start: Instant::now(),
        budget: time_budget,
        selected: BTreeMap::new(),
        queue: vec![root.to_owned()],
        fastest: None,
        cheapest: None,
        stopped: false,
        heuristic: false,
    };
    search.run(0);
    let MakespanSearch {
        fastest,
        cheapest,
        stopped,
        heuristic,
        ..
    } = search;
    let heuristic = heuristic || greedy.as_ref().is_some_and(|g| !g.exact);
    let cheapest = [cheapest, greedy.clone()]
        .into_iter()
        .flatten()
        .reduce(|a, b| if b.cheaper_than(&a) { b } else { a });
    let (chosen, optimality) = settle(fastest, greedy, stopped, heuristic)?;
    let cheapest = cheapest.expect("a chosen candidate exists");
    let kind = if constraints.capacities.is_empty() {
        "dependency-aware"
    } else {
        "resource-aware"
    };
    let mut explanation = format!(
        "Minimized {kind} completion time to {:.3} minutes at weighted effort {:.3}",
        chosen.makespan, chosen.objective
    );
    let label = if stopped {
        "lowest-effort selection found"
    } else {
        "effort-optimal selection"
    };
    if cheapest.methods == chosen.methods || cheapest.objective >= chosen.objective {
        explanation.push_str(&format!("; it is also the {label}."));
    } else {
        explanation.push_str(&format!(
            "; the {label} takes {:.3} minutes at weighted effort {:.3}, so {:.3} more effort buys {:.3} fewer minutes.",
            cheapest.makespan,
            cheapest.objective,
            chosen.objective - cheapest.objective,
            cheapest.makespan - chosen.makespan
        ));
    }
    if optimality == Optimality::GreedyOnly {
        explanation.insert_str(0, "Feasible fallback after the time budget. ");
    }
    Ok(Selection {
        methods: chosen.methods,
        totals: chosen.totals,
        objective: chosen.objective,
        optimality,
        time_budget,
        makespan: Some(chosen.makespan),
        explanation,
    })
}

/// One feasible method assignment on the Pareto frontier: no other complete
/// alternative is better on the compared measures. Weighted `objective` is
/// preserved beside the multi-objective vector for when preferences are
/// clear.
#[derive(Debug, Clone, PartialEq)]
pub struct ParetoAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    pub makespan: f64,
    /// Weighted total-effort objective for this assignment.
    pub objective: f64,
    /// Values over the dimensions this alternative prices.
    pub objectives: BTreeMap<String, f64>,
    /// Valued dimensions this alternative does not price, sorted.
    pub unknown: Vec<String>,
}

/// One pairwise dominance: `by` is no worse on every compared measure
/// (`on`) and strictly better on at least one.
#[derive(Debug, Clone, PartialEq)]
pub struct Dominance {
    /// Id of the dominating alternative.
    pub by: String,
    /// Compared measures the dominance was decided on, sorted.
    pub on: Vec<String>,
    /// Valued dimensions unknown for either side, so not compared, sorted.
    pub unknown: Vec<String>,
}

/// One feasible assignment another complete alternative dominates on the
/// compared measures.
#[derive(Debug, Clone, PartialEq)]
pub struct DominatedAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    pub makespan: f64,
    /// Weighted total-effort objective for this assignment.
    pub objective: f64,
    /// Values over the dimensions this alternative prices.
    pub objectives: BTreeMap<String, f64>,
    /// Valued dimensions this alternative does not price, sorted.
    pub unknown: Vec<String>,
    /// Frontier alternatives dominating this one, in id order.
    /// Transitivity of comparison on one shared measure set guarantees a
    /// frontier dominator exists.
    pub dominated_by: Vec<Dominance>,
}

/// One feasible assignment with no value on a compared measure: shown
/// separately, never ranked.
#[derive(Debug, Clone, PartialEq)]
pub struct IncompleteAlternative {
    /// Stable id (`alt-<n>`) in deterministic sort order.
    pub id: String,
    /// Chosen method per needed task.
    pub methods: BTreeMap<String, String>,
    /// Summed selected costs per metric (shared tasks counted once).
    pub totals: BTreeMap<String, f64>,
    /// Dependency-aware completion time over `duration_minutes`.
    pub makespan: f64,
    /// Weighted total-effort objective for this assignment.
    pub objective: f64,
    /// Values over the dimensions this alternative prices.
    pub objectives: BTreeMap<String, f64>,
    /// Valued dimensions this alternative does not price, sorted.
    pub unknown: Vec<String>,
}

/// Deterministic Pareto evaluation over complete feasible assignments.
///
/// All alternatives compare only on the measures known for every one of
/// them that values anything: a metric is valued in an assignment when
/// every chosen cost-bearing method (one with costs or without children;
/// cost-free decompositions do not count) prices it, and it is compared
/// when every feasible assignment that values any metric values it.
/// Value-less assignments never void the shared set; they are incomplete
/// instead. A measure only some alternatives price is listed as not
/// compared. `duration_minutes` becomes `makespan_minutes` (CPM),
/// `failure_risk` combines as `1 - product(1 - risk)`, the rest are summed.
/// One shared measure set keeps dominance transitive, so no dominance
/// cycle can empty the frontier. Weighted objectives are preserved per
/// alternative for when preferences are clear.
#[derive(Debug, Clone, PartialEq)]
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
    /// Alternatives with no value on a compared measure: shown separately,
    /// never ranked.
    pub incomplete: Vec<IncompleteAlternative>,
    /// `Optimal` when the assignment search ran to completion, `BestFound`
    /// when the time budget or [`MAX_PARETO_ASSIGNMENTS`] cut it short.
    pub optimality: Optimality,
    /// Feasible assignments evaluated (deduplicated).
    pub evaluated: usize,
}

/// Most feasible assignments the Pareto search collects before it stops
/// and reports [`Optimality::BestFound`], bounding dominance work.
pub const MAX_PARETO_ASSIGNMENTS: usize = 10_000;

/// Evaluate the Pareto frontier over feasible method selections.
///
/// Exhaustively searches method assignments of activated tasks until the
/// time budget expires or [`MAX_PARETO_ASSIGNMENTS`] are collected, like
/// [`select_with_objective`] with [`Objective::Makespan`], then ranks the
/// complete assignments (those pricing every compared measure) into the
/// non-dominated frontier and the dominated remainder naming its
/// dominators; the rest are shown separately as incomplete, never ranked.
/// `internal_metrics` (synthetic costs a caller adds only to enforce hard
/// limits) are never valued. `unresolved` names method-less tasks whose
/// work is still unknown (unlike done or dropped work): an assignment
/// activating one values no metric.
/// Deterministic: same inputs, byte-identical report.
pub fn pareto(
    tasks: &[Task],
    root: &str,
    constraints: &Constraints,
    internal_metrics: &[&str],
    unresolved: &BTreeSet<String>,
    time_budget: Duration,
) -> Result<ParetoReport, SelectError> {
    let by_id: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    if by_id.len() != tasks.len() {
        return Err(SelectError::Invalid("duplicate task id".into()));
    }
    if !by_id.contains_key(root) {
        return Err(SelectError::Invalid(format!("missing root task `{root}`")));
    }
    validate_graph(&by_id, constraints)?;
    let greedy = greedy_select(&by_id, &reachable_tasks(&by_id, root), root, constraints)?
        .and_then(|g| candidate(&by_id, constraints, &g.methods, root).map(|c| (g.methods, c)));
    let mut search = ParetoSearch {
        by_id: &by_id,
        root,
        constraints,
        start: Instant::now(),
        budget: time_budget,
        selected: BTreeMap::new(),
        queue: vec![root.to_owned()],
        found: Vec::new(),
        stopped: false,
    };
    search.run(0);
    let mut found = search.found;
    if let Some((methods, _)) = greedy
        && !found.iter().any(|c: &Candidate| c.methods == methods)
        && let Some(scored) = candidate(&by_id, constraints, &methods, root)
    {
        found.push(scored);
    }
    // Deduplicate identical assignments; sort deterministically by the
    // selected methods so ids are stable whatever the search order.
    found.sort_by(|a, b| methods_cmp(&a.methods, &b.methods));
    found.dedup_by(|a, b| a.methods == b.methods);
    if found.is_empty() {
        if search.stopped {
            return Err(SelectError::TimeLimit);
        }
        return Err(SelectError::Infeasible);
    }
    let optimality = if search.stopped {
        Optimality::BestFound { gap_percent: None }
    } else {
        Optimality::Optimal
    };
    // Each alternative knows only the metrics it values; a measure is
    // compared only when every alternative that values anything values
    // it, so value-less alternatives never void the shared set (they are
    // incomplete instead) and one shared set decides every dominance, so
    // cycles are impossible.
    let known: Vec<BTreeSet<String>> = found
        .iter()
        .map(|c| {
            valued_metrics(&by_id, unresolved, c)
                .into_iter()
                .filter(|m| !internal_metrics.contains(&m.as_str()))
                .map(|m| {
                    if m == "duration_minutes" {
                        "makespan_minutes".to_owned()
                    } else {
                        m
                    }
                })
                .collect()
        })
        .collect();
    let union: BTreeSet<String> = known.iter().flatten().cloned().collect();
    let valued: Vec<&BTreeSet<String>> = known.iter().filter(|k| !k.is_empty()).collect();
    let compared: Vec<String> = match valued.split_first() {
        None => Vec::new(),
        Some((first, rest)) => first
            .iter()
            .filter(|m| rest.iter().all(|k| k.contains(*m)))
            .cloned()
            .collect(),
    };
    let not_compared: Vec<String> = union
        .iter()
        .filter(|m| !compared.contains(m))
        .cloned()
        .collect();
    let complete: Vec<bool> = known
        .iter()
        .map(|k| compared.iter().all(|m| k.contains(m)))
        .collect();
    let mut frontier = Vec::new();
    let mut dominated: Vec<DominatedAlternative> = Vec::new();
    let mut incomplete = Vec::new();
    // Deterministic global order: complete alternatives by compared values
    // then method map, incomplete ones by method map after them, so
    // `alt-<n>` ids are stable whatever the search order.
    let vectors: Vec<Vec<f64>> = found
        .iter()
        .map(|c| pareto_vector(&by_id, c, &compared))
        .collect();
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by(|&a, &b| {
        complete[b]
            .cmp(&complete[a])
            .then_with(|| {
                vectors[a]
                    .iter()
                    .zip(&vectors[b])
                    .map(|(x, y)| x.total_cmp(y))
                    .find(|o| o.is_ne())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| methods_cmp(&found[a].methods, &found[b].methods))
    });
    // Classic skyline on one shared measure set is transitive, so every
    // dominated alternative has a frontier dominator.
    let dominates = |a: usize, b: usize| {
        complete[a]
            && complete[b]
            && !compared.is_empty()
            && vectors[a].iter().zip(&vectors[b]).all(|(x, y)| x <= y)
            && vectors[a].iter().zip(&vectors[b]).any(|(x, y)| x < y)
    };
    let mut is_dominated = vec![false; found.len()];
    for &b in &order {
        if complete[b] {
            is_dominated[b] = order.iter().any(|&a| a != b && dominates(a, b));
        }
    }
    let ids: BTreeMap<usize, String> = order
        .iter()
        .enumerate()
        .map(|(rank, &idx)| (idx, format!("alt-{rank}")))
        .collect();
    let unknown_of = |idx: usize| -> Vec<String> {
        union
            .iter()
            .filter(|d| !known[idx].contains(*d))
            .cloned()
            .collect()
    };
    let objectives_of = |idx: usize| -> BTreeMap<String, f64> {
        known[idx]
            .iter()
            .map(|d| (d.clone(), metric_value(&by_id, &found[idx], d)))
            .collect()
    };
    let dominance_by = |a: usize, b: usize| -> Dominance {
        Dominance {
            by: ids[&a].clone(),
            on: compared.clone(),
            unknown: union
                .iter()
                .filter(|d| !known[a].contains(*d) || !known[b].contains(*d))
                .cloned()
                .collect(),
        }
    };
    for &idx in &order {
        let candidate = &found[idx];
        let alternative = |id: String| {
            (
                id,
                candidate.methods.clone(),
                candidate.totals.clone(),
                candidate.makespan,
                candidate.objective,
                objectives_of(idx),
                unknown_of(idx),
            )
        };
        if !complete[idx] {
            let (id, methods, totals, makespan, objective, objectives, unknown) =
                alternative(ids[&idx].clone());
            incomplete.push(IncompleteAlternative {
                id,
                methods,
                totals,
                makespan,
                objective,
                objectives,
                unknown,
            });
            continue;
        }
        if !is_dominated[idx] {
            let (id, methods, totals, makespan, objective, objectives, unknown) =
                alternative(ids[&idx].clone());
            frontier.push(ParetoAlternative {
                id,
                methods,
                totals,
                makespan,
                objective,
                objectives,
                unknown,
            });
            continue;
        }
        // Transitivity on one shared measure set guarantees a frontier
        // dominator: every dominator chain ends at an undominated one that
        // still dominates through the chain.
        let dominated_by: Vec<Dominance> = order
            .iter()
            .filter(|&&a| complete[a] && !is_dominated[a] && dominates(a, idx))
            .map(|&a| dominance_by(a, idx))
            .collect();
        let (id, methods, totals, makespan, objective, objectives, unknown) =
            alternative(ids[&idx].clone());
        dominated.push(DominatedAlternative {
            id,
            methods,
            totals,
            makespan,
            objective,
            objectives,
            unknown,
            dominated_by,
        });
    }
    Ok(ParetoReport {
        compared,
        not_compared,
        frontier,
        dominated,
        incomplete,
        optimality,
        evaluated: found.len(),
    })
}

/// Metrics an assignment carries values for: priced by some chosen method
/// and by every chosen cost-bearing method. A cost-free method with
/// children is a structural decomposition and leaves metrics valued; a
/// leaf or costed method missing a metric makes it unknown, and an
/// activated unresolved task prices nothing, so it makes every metric
/// unknown.
fn valued_metrics(
    by_id: &BTreeMap<&str, &Task>,
    unresolved: &BTreeSet<String>,
    candidate: &Candidate,
) -> BTreeSet<String> {
    let mut valued: BTreeSet<String> = candidate.totals.keys().cloned().collect();
    for (task, method_id) in &candidate.methods {
        let Some(method) = chosen_method(by_id, task, method_id) else {
            continue;
        };
        if method
            .children
            .iter()
            .any(|child| unresolved.contains(child))
        {
            return BTreeSet::new();
        }
        if !method.costs.is_empty() || method.children.is_empty() {
            valued.retain(|m| method.costs.contains_key(m));
        }
    }
    valued
}

/// Value of one valued measure for an assignment: makespan for time,
/// independently-combined risk for `failure_risk`, summed totals else.
/// Values snap to a 1e-6 grid so float noise between equal sums never
/// decides dominance.
fn metric_value(by_id: &BTreeMap<&str, &Task>, candidate: &Candidate, dimension: &str) -> f64 {
    let value = if dimension == "makespan_minutes" {
        candidate.makespan
    } else if dimension == "failure_risk" {
        let residual: f64 = candidate
            .methods
            .iter()
            .filter_map(|(task, method_id)| {
                chosen_method(by_id, task, method_id)?
                    .costs
                    .get("failure_risk")
            })
            .map(|risk| 1.0 - risk.clamp(0.0, 1.0))
            .product();
        1.0 - residual
    } else {
        candidate.totals[dimension]
    };
    (value * 1e6).round() / 1e6
}

/// Comparison vector for one complete assignment in compared order.
fn pareto_vector(
    by_id: &BTreeMap<&str, &Task>,
    candidate: &Candidate,
    compared: &[String],
) -> Vec<f64> {
    compared
        .iter()
        .map(|dimension| metric_value(by_id, candidate, dimension))
        .collect()
}

/// Deterministic order over method maps: sorted `(task, method)` pairs.
fn methods_cmp(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> std::cmp::Ordering {
    let mut av: Vec<(&String, &String)> = a.iter().collect();
    let mut bv: Vec<(&String, &String)> = b.iter().collect();
    av.sort();
    bv.sort();
    av.cmp(&bv)
}

/// Exhaustive branch over methods of activated tasks, collecting every
/// feasible assignment until the time budget expires or
/// [`MAX_PARETO_ASSIGNMENTS`] are collected.
struct ParetoSearch<'a> {
    by_id: &'a BTreeMap<&'a str, &'a Task>,
    root: &'a str,
    constraints: &'a Constraints,
    start: Instant,
    budget: Duration,
    selected: BTreeMap<String, String>,
    queue: Vec<String>,
    found: Vec<Candidate>,
    stopped: bool,
}

impl ParetoSearch<'_> {
    fn run(&mut self, mut index: usize) {
        if self.found.len() >= MAX_PARETO_ASSIGNMENTS || self.start.elapsed() >= self.budget {
            self.stopped = true;
            return;
        }
        let by_id = self.by_id;
        while index < self.queue.len()
            && (self.selected.contains_key(&self.queue[index])
                || by_id[self.queue[index].as_str()].methods.is_empty())
        {
            index += 1;
        }
        if index == self.queue.len() {
            if let Some(scored) = candidate(self.by_id, self.constraints, &self.selected, self.root)
            {
                self.found.push(scored);
            }
            return;
        }
        let task = by_id[self.queue[index].as_str()];
        for method in &task.methods {
            if self
                .constraints
                .forbidden_methods
                .contains(&(task.id.clone(), method.id.clone()))
            {
                continue;
            }
            self.selected.insert(task.id.clone(), method.id.clone());
            let len = self.queue.len();
            self.queue.extend(method.children.iter().cloned());
            self.run(index + 1);
            self.queue.truncate(len);
            self.selected.remove(&task.id);
            if self.stopped {
                break;
            }
        }
    }
}

/// Pick the search incumbent or the greedy fallback, labeled by whether the
/// search finished and every candidate was scored by the exact schedule.
fn settle(
    fastest: Option<Candidate>,
    greedy: Option<Candidate>,
    stopped: bool,
    heuristic: bool,
) -> Result<(Candidate, Optimality), SelectError> {
    match (fastest, greedy) {
        (Some(best), _) if !stopped && !heuristic => Ok((best, Optimality::Optimal)),
        (Some(best), _) if !stopped => Ok((best, Optimality::BestFound { gap_percent: None })),
        (Some(best), Some(g)) if g.faster_than(&best) => Ok((g, Optimality::GreedyOnly)),
        (Some(best), _) => Ok((best, Optimality::BestFound { gap_percent: None })),
        (None, Some(g)) if stopped => Ok((g, Optimality::GreedyOnly)),
        (None, _) if stopped => Err(SelectError::TimeLimit),
        (None, _) => Err(SelectError::Infeasible),
    }
}

/// One feasible method assignment scored for both objectives.
#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    methods: BTreeMap<String, String>,
    totals: BTreeMap<String, f64>,
    objective: f64,
    makespan: f64,
    /// Whether the makespan came from the exact schedule search.
    exact: bool,
}

impl Candidate {
    /// Shorter makespan, then lower weighted effort.
    fn faster_than(&self, other: &Self) -> bool {
        self.makespan < other.makespan - 1e-8
            || ((self.makespan - other.makespan).abs() <= 1e-8
                && self.objective < other.objective - 1e-8)
    }

    /// Lower weighted effort, then shorter makespan.
    fn cheaper_than(&self, other: &Self) -> bool {
        self.objective < other.objective - 1e-8
            || ((self.objective - other.objective).abs() <= 1e-8
                && self.makespan < other.makespan - 1e-8)
    }
}

/// Exhaustive branch over methods of activated tasks only: a task enters
/// the queue when the root or a chosen method requires it.
struct MakespanSearch<'a> {
    by_id: &'a BTreeMap<&'a str, &'a Task>,
    root: &'a str,
    constraints: &'a Constraints,
    start: Instant,
    budget: Duration,
    selected: BTreeMap<String, String>,
    queue: Vec<String>,
    fastest: Option<Candidate>,
    cheapest: Option<Candidate>,
    stopped: bool,
    heuristic: bool,
}

impl MakespanSearch<'_> {
    fn run(&mut self, mut index: usize) {
        if self.start.elapsed() >= self.budget {
            self.stopped = true;
            return;
        }
        let by_id = self.by_id;
        while index < self.queue.len()
            && (self.selected.contains_key(&self.queue[index])
                || by_id[self.queue[index].as_str()].methods.is_empty())
        {
            index += 1;
        }
        if index == self.queue.len() {
            self.evaluate();
            return;
        }
        let task = by_id[self.queue[index].as_str()];
        for method in &task.methods {
            if self
                .constraints
                .forbidden_methods
                .contains(&(task.id.clone(), method.id.clone()))
            {
                continue;
            }
            self.selected.insert(task.id.clone(), method.id.clone());
            let len = self.queue.len();
            self.queue.extend(method.children.iter().cloned());
            self.run(index + 1);
            self.queue.truncate(len);
            self.selected.remove(&task.id);
            if self.stopped {
                break;
            }
        }
    }

    fn evaluate(&mut self) {
        let Some(found) = candidate(self.by_id, self.constraints, &self.selected, self.root) else {
            return;
        };
        self.heuristic |= !found.exact;
        if self
            .fastest
            .as_ref()
            .is_none_or(|old| found.faster_than(old))
        {
            self.fastest = Some(found.clone());
        }
        if self
            .cheapest
            .as_ref()
            .is_none_or(|old| found.cheaper_than(old))
        {
            self.cheapest = Some(found);
        }
    }
}

/// Score a complete assignment, or `None` when it breaks a hard limit or its
/// order edges form a precedence cycle.
fn candidate(
    by_id: &BTreeMap<&str, &Task>,
    constraints: &Constraints,
    methods: &BTreeMap<String, String>,
    root: &str,
) -> Option<Candidate> {
    let mut totals = BTreeMap::new();
    for (task, method_id) in methods {
        for (metric, value) in &chosen_method(by_id, task, method_id)?.costs {
            *totals.entry(metric.clone()).or_insert(0.0) += value;
        }
    }
    if constraints
        .limits
        .iter()
        .any(|(m, l)| totals.get(m).copied().unwrap_or(0.0) > *l + 1e-8)
    {
        return None;
    }
    let schedule = schedule_makespan(by_id, methods, root, &constraints.capacities)?;
    let objective = totals
        .iter()
        .map(|(m, v)| constraints.weights.get(m).copied().unwrap_or(0.0) * v)
        .sum();
    Some(Candidate {
        methods: methods.clone(),
        totals,
        objective,
        makespan: schedule.makespan,
        exact: schedule.exact,
    })
}

fn chosen_method<'a>(
    by_id: &BTreeMap<&str, &'a Task>,
    task: &str,
    method_id: &str,
) -> Option<&'a Method> {
    by_id.get(task)?.methods.iter().find(|m| m.id == method_id)
}

/// Critical-path makespan with P7's leaf semantics, extended with resource
/// caps. A task with a chosen method is a node when that method has no
/// children or its own `duration_minutes`; a node runs after its children's
/// nodes. A sibling order edge `from -> to` makes every node of `to` that is
/// not also a node of `from` wait for `from`'s nodes, with a node-less `from`
/// relaying whatever must finish before it. Nodes holding a limited resource
/// additionally wait for a free slot (one unit each, `floor` of the declared
/// capacity, minimum one). `None` on a precedence cycle.
fn schedule_makespan(
    by_id: &BTreeMap<&str, &Task>,
    chosen: &BTreeMap<String, String>,
    root: &str,
    capacities: &BTreeMap<String, f64>,
) -> Option<ResourceSchedule> {
    let mut schedule = Schedule {
        by_id,
        chosen,
        leaves: BTreeMap::new(),
        durations: Vec::new(),
        demands: Vec::new(),
        keys: Vec::new(),
        edges: BTreeSet::new(),
    };
    schedule.leaves(root)?;
    let Schedule {
        durations,
        demands,
        keys,
        edges,
        ..
    } = schedule;
    resource_constrained_schedule(&durations, &edges, &demands, &keys, capacities)
}

/// One resource-constrained schedule: a start time per node, the span it
/// reaches, and whether that span is proved minimal.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSchedule {
    /// Start time of each node, indexed like the input durations.
    pub starts: Vec<f64>,
    /// Latest finish over all nodes.
    pub makespan: f64,
    /// `false` when the graph was too large for the exact order search and
    /// the span comes from the list-schedule heuristic.
    pub exact: bool,
}

/// Resource-constrained schedule over one duration per node, the precedence
/// edges between them, one demanded-resource set per node, one stable key
/// per node (ties break by key, so callers numbering the same graph
/// differently get the same schedule), and the available capacity per
/// resource (absent resources are unlimited). Uses the serial
/// schedule-generation scheme: exact over every precedence-feasible order for
/// small graphs (`EXACT_SCHEDULE_NODES` or fewer nodes), a
/// deterministic longest-tail-first list schedule beyond that. `None` on a
/// precedence cycle or an out-of-range edge.
///
/// Capacities are instance counts: concurrent holders are capped at
/// `floor(capacity)` with a minimum of one slot, so a fractional declaration
/// still serializes exclusive holders rather than bricking the schedule.
pub fn resource_constrained_schedule(
    durations: &[f64],
    edges: &BTreeSet<(usize, usize)>,
    demands: &[BTreeSet<String>],
    keys: &[String],
    capacities: &BTreeMap<String, f64>,
) -> Option<ResourceSchedule> {
    let n = durations.len();
    let mut pred = vec![Vec::new(); n];
    let mut succ = vec![Vec::new(); n];
    for &(from, to) in edges {
        if from >= n || to >= n {
            return None;
        }
        succ[from].push(to);
        pred[to].push(from);
    }
    let slots: BTreeMap<String, usize> = capacities
        .iter()
        .map(|(name, cap)| (name.clone(), cap.floor().max(1.0) as usize))
        .collect();
    let order = list_order(durations, &pred, &succ, keys)?;
    let (starts, makespan) = serial_schedule(&order, durations, &pred, demands, &slots);
    let mut best = ResourceSchedule {
        starts,
        makespan,
        exact: true,
    };
    // Without a limited holder precedence alone decides, and any serial
    // order already yields the exact longest path.
    let limited = (0..n).any(|i| {
        demands
            .get(i)
            .is_some_and(|need| need.iter().any(|r| slots.contains_key(r.as_str())))
    });
    if !limited {
        return Some(best);
    }
    if n > EXACT_SCHEDULE_NODES {
        best.exact = false;
        return Some(best);
    }
    let mut by_key: Vec<usize> = (0..n).collect();
    by_key.sort_by(|&a, &b| keys.get(a).cmp(&keys.get(b)).then(a.cmp(&b)));
    let mut indegree: Vec<usize> = pred.iter().map(Vec::len).collect();
    let mut order = Vec::with_capacity(n);
    enumerate_orders(
        &mut order,
        &mut indegree,
        &by_key,
        &succ,
        durations,
        &pred,
        demands,
        &slots,
        &mut best,
    );
    Some(best)
}

/// Above this many schedulable nodes the exact order enumeration yields to a
/// deterministic list schedule; small planning graphs stay exact.
const EXACT_SCHEDULE_NODES: usize = 8;

/// Depth-first enumeration of precedence-feasible orders, scoring each with
/// the serial scheme and keeping a strictly shorter one. Ready nodes visit
/// in key order so the search is deterministic.
#[allow(clippy::too_many_arguments)]
fn enumerate_orders(
    order: &mut Vec<usize>,
    indegree: &mut [usize],
    by_key: &[usize],
    succ: &[Vec<usize>],
    durations: &[f64],
    pred: &[Vec<usize>],
    demands: &[BTreeSet<String>],
    slots: &BTreeMap<String, usize>,
    best: &mut ResourceSchedule,
) {
    if order.len() == durations.len() {
        let (starts, makespan) = serial_schedule(order, durations, pred, demands, slots);
        if makespan < best.makespan - 1e-9 {
            best.starts = starts;
            best.makespan = makespan;
        }
        return;
    }
    let ready: Vec<usize> = by_key
        .iter()
        .copied()
        .filter(|&i| indegree[i] == 0 && !order.contains(&i))
        .collect();
    for node in ready {
        for &to in &succ[node] {
            indegree[to] -= 1;
        }
        order.push(node);
        enumerate_orders(
            order, indegree, by_key, succ, durations, pred, demands, slots, best,
        );
        order.pop();
        for &to in &succ[node] {
            indegree[to] += 1;
        }
    }
}

/// Deterministic precedence-feasible order: among ready nodes, the longest
/// remaining path (own duration plus the longest successor chain) first,
/// ties by key then index. `None` on a precedence cycle.
fn list_order(
    durations: &[f64],
    pred: &[Vec<usize>],
    succ: &[Vec<usize>],
    keys: &[String],
) -> Option<Vec<usize>> {
    let n = durations.len();
    let mut outdegree: Vec<usize> = succ.iter().map(Vec::len).collect();
    let mut sinks: Vec<usize> = (0..n).filter(|&i| outdegree[i] == 0).collect();
    let mut tail = vec![0.0f64; n];
    let mut seen = 0;
    while let Some(node) = sinks.pop() {
        seen += 1;
        tail[node] += durations[node];
        for &from in &pred[node] {
            tail[from] = tail[from].max(tail[node]);
            outdegree[from] -= 1;
            if outdegree[from] == 0 {
                sinks.push(from);
            }
        }
    }
    if seen != n {
        return None;
    }
    let mut indegree: Vec<usize> = pred.iter().map(Vec::len).collect();
    let mut ready: Vec<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while !ready.is_empty() {
        let pick = (0..ready.len())
            .min_by(|&a, &b| {
                let (a, b) = (ready[a], ready[b]);
                tail[b]
                    .total_cmp(&tail[a])
                    .then(keys.get(a).cmp(&keys.get(b)))
                    .then(a.cmp(&b))
            })
            .expect("ready is non-empty");
        let node = ready.swap_remove(pick);
        order.push(node);
        for &to in &succ[node] {
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }
    Some(order)
}

/// Serial schedule-generation scheme for one precedence-feasible order: each
/// node starts at the earliest time its predecessors have finished and a
/// slot on every demanded resource is free for its whole duration.
/// Zero-duration nodes hold nothing and start when ready. Returns the start
/// per node and the makespan.
fn serial_schedule(
    order: &[usize],
    durations: &[f64],
    pred: &[Vec<usize>],
    demands: &[BTreeSet<String>],
    slots: &BTreeMap<String, usize>,
) -> (Vec<f64>, f64) {
    let n = durations.len();
    let mut starts = vec![0.0f64; n];
    let mut finish = vec![0.0f64; n];
    // Committed busy intervals per limited resource.
    let mut busy: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut makespan = 0.0f64;
    for &node in order {
        let mut start = pred[node].iter().map(|&p| finish[p]).fold(0.0, f64::max);
        let duration = durations[node];
        if duration > 0.0
            && let Some(need) = demands.get(node)
        {
            start = earliest_slot(start, duration, need, slots, &busy);
            for resource in need {
                if slots.contains_key(resource.as_str()) {
                    busy.entry(resource.clone())
                        .or_default()
                        .push((start, start + duration));
                }
            }
        }
        starts[node] = start;
        finish[node] = start + duration;
        makespan = makespan.max(finish[node]);
    }
    (starts, makespan)
}

/// Earliest `t >= ready` whose whole `[t, t + duration)` fits within every
/// demanded resource's free slots: peak concurrent use over the window stays
/// below capacity. Advances past the earliest-ending overlapping holder while
/// any resource is full somewhere in the window.
fn earliest_slot(
    mut t: f64,
    duration: f64,
    need: &BTreeSet<String>,
    slots: &BTreeMap<String, usize>,
    busy: &BTreeMap<String, Vec<(f64, f64)>>,
) -> f64 {
    loop {
        let end = t + duration;
        let mut advance = None;
        for resource in need {
            let Some(capacity) = slots.get(resource.as_str()).copied() else {
                continue;
            };
            let Some(intervals) = busy.get(resource.as_str()) else {
                continue;
            };
            let overlapping: Vec<(f64, f64)> = intervals
                .iter()
                .copied()
                .filter(|&(s, e)| e > t && s < end)
                .collect();
            // Usage only rises at a holder's start, so the peak sits at the
            // window start or at one of those starts.
            let peak = std::iter::once(t)
                .chain(overlapping.iter().map(|&(s, _)| s).filter(|&s| s > t))
                .map(|p| {
                    overlapping
                        .iter()
                        .filter(|&&(s, e)| s <= p && e > p)
                        .count()
                })
                .max()
                .unwrap_or(0);
            if peak >= capacity {
                let earliest_end = overlapping
                    .iter()
                    .map(|&(_, e)| e)
                    .fold(f64::INFINITY, f64::min);
                advance = Some(advance.map_or(earliest_end, |a: f64| a.min(earliest_end)));
            }
        }
        match advance {
            Some(next) => t = next,
            None => return t,
        }
    }
}

struct Schedule<'a, 'b> {
    by_id: &'b BTreeMap<&'b str, &'a Task>,
    chosen: &'b BTreeMap<String, String>,
    leaves: BTreeMap<String, BTreeSet<usize>>,
    durations: Vec<f64>,
    demands: Vec<BTreeSet<String>>,
    keys: Vec<String>,
    edges: BTreeSet<(usize, usize)>,
}

impl<'a> Schedule<'a, '_> {
    fn leaves(&mut self, id: &str) -> Option<BTreeSet<usize>> {
        if let Some(done) = self.leaves.get(id) {
            return Some(done.clone());
        }
        self.leaves.insert(id.to_owned(), BTreeSet::new());
        let Some(method_id) = self.chosen.get(id) else {
            return Some(BTreeSet::new());
        };
        let method: &'a Method = chosen_method(self.by_id, id, method_id)?;
        let mut children = BTreeMap::new();
        for child in &method.children {
            let leaves = self.leaves(child)?;
            children.insert(child.as_str(), leaves);
        }
        let mut finishes = BTreeMap::new();
        for (from, to) in &method.order_edges {
            let before = finish_leaves(from, method, &children, &mut finishes);
            for &b in children.get(to.as_str()).into_iter().flatten() {
                if !before.contains(&b) {
                    for &a in &before {
                        self.edges.insert((a, b));
                    }
                }
            }
        }
        let mut all: BTreeSet<usize> = children.into_values().flatten().collect();
        let duration = method.costs.get("duration_minutes").copied().unwrap_or(0.0);
        if method.children.is_empty() || duration > 0.0 {
            let node = self.durations.len();
            self.durations.push(duration);
            self.demands
                .push(method.resources.iter().cloned().collect());
            self.keys.push(id.to_owned());
            for &leaf in &all {
                self.edges.insert((leaf, node));
            }
            all.insert(node);
        }
        self.leaves.insert(id.to_owned(), all.clone());
        Some(all)
    }
}

/// Nodes that must finish before `child` counts as finished: its own nodes,
/// or for a node-less child, whatever must finish before it.
fn finish_leaves<'m>(
    child: &'m str,
    method: &'m Method,
    children: &BTreeMap<&str, BTreeSet<usize>>,
    memo: &mut BTreeMap<&'m str, BTreeSet<usize>>,
) -> BTreeSet<usize> {
    if let Some(done) = memo.get(child) {
        return done.clone();
    }
    memo.insert(child, BTreeSet::new());
    let result = match children.get(child) {
        Some(own) if !own.is_empty() => own.clone(),
        _ => {
            let mut relayed = BTreeSet::new();
            for (from, _) in method.order_edges.iter().filter(|(_, to)| to == child) {
                relayed.extend(finish_leaves(from, method, children, memo));
            }
            relayed
        }
    };
    memo.insert(child, result.clone());
    result
}

fn validate_graph(
    by_id: &BTreeMap<&str, &Task>,
    constraints: &Constraints,
) -> Result<(), SelectError> {
    for (metric, value) in constraints.weights.iter().chain(constraints.limits.iter()) {
        if !value.is_finite() || *value < 0.0 {
            return Err(SelectError::Invalid(format!(
                "invalid constraint for `{metric}`"
            )));
        }
    }
    for (resource, capacity) in &constraints.capacities {
        if resource.is_empty() {
            return Err(SelectError::Invalid(
                "resource name must not be empty".into(),
            ));
        }
        if !capacity.is_finite() || *capacity < 0.0 {
            return Err(SelectError::Invalid(format!(
                "invalid capacity for resource `{resource}`"
            )));
        }
    }
    for task in by_id.values() {
        let mut ids = BTreeSet::new();
        for method in &task.methods {
            if !ids.insert(method.id.as_str()) {
                return Err(SelectError::Invalid(format!(
                    "duplicate method id `{}` on `{}`",
                    method.id, task.id
                )));
            }
            for (metric, value) in &method.costs {
                if !value.is_finite() || *value < 0.0 {
                    return Err(SelectError::Invalid(format!(
                        "invalid cost for `{}` metric `{metric}`",
                        method.id
                    )));
                }
            }
            for resource in &method.resources {
                if resource.is_empty() {
                    return Err(SelectError::Invalid(format!(
                        "method `{}` holds a resource with an empty name",
                        method.id
                    )));
                }
            }
            for child in &method.children {
                if !by_id.contains_key(child.as_str()) {
                    return Err(SelectError::Invalid(format!(
                        "method `{}` references missing child `{child}`",
                        method.id
                    )));
                }
            }
            for end in method.order_edges.iter().flat_map(|(a, b)| [a, b]) {
                if !method.children.contains(end) {
                    return Err(SelectError::Invalid(format!(
                        "method `{}` orders `{end}`, which is not one of its children",
                        method.id
                    )));
                }
            }
        }
    }
    Ok(())
}

fn reachable_tasks(by_id: &BTreeMap<&str, &Task>, root: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![root.to_owned()];
    while let Some(id) = stack.pop() {
        if !found.insert(id.clone()) {
            continue;
        }
        if let Some(task) = by_id.get(id.as_str()) {
            for method in &task.methods {
                stack.extend(method.children.iter().cloned());
            }
        }
    }
    found
}

fn greedy_select(
    by_id: &BTreeMap<&str, &Task>,
    reachable: &BTreeSet<String>,
    root: &str,
    c: &Constraints,
) -> Result<Option<Selection>, SelectError> {
    fn best_tree_cost<'a>(
        id: &'a str,
        by_id: &BTreeMap<&'a str, &'a Task>,
        c: &Constraints,
        memo: &mut BTreeMap<String, (f64, String)>,
        active: &mut BTreeSet<String>,
    ) -> Result<(f64, String), SelectError> {
        if let Some(answer) = memo.get(id) {
            return Ok(answer.clone());
        }
        if !active.insert(id.to_owned()) {
            return Err(SelectError::Invalid(format!(
                "selection hierarchy cycle at `{id}`"
            )));
        }
        let Some(task) = by_id.get(id).copied() else {
            return Err(SelectError::Invalid(format!("missing task `{id}`")));
        };
        if task.methods.is_empty() {
            let answer = (0.0, String::new());
            memo.insert(id.to_owned(), answer.clone());
            active.remove(id);
            return Ok(answer);
        }
        let mut options = Vec::new();
        for method in &task.methods {
            if c.forbidden_methods
                .contains(&(task.id.clone(), method.id.clone()))
            {
                continue;
            }
            let mut score: f64 = method
                .costs
                .iter()
                .map(|(k, v)| c.weights.get(k).copied().unwrap_or(0.0) * v)
                .sum();
            for child in &method.children {
                score += best_tree_cost(child, by_id, c, memo, active)?.0;
            }
            if score.is_finite() {
                options.push((score, method.id.clone()));
            }
        }
        active.remove(id);
        options.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let answer = options
            .first()
            .cloned()
            .unwrap_or((f64::INFINITY, String::new()));
        memo.insert(id.to_owned(), answer.clone());
        Ok(answer)
    }
    fn visit<'a>(
        id: &'a str,
        by_id: &BTreeMap<&'a str, &'a Task>,
        choices: &BTreeMap<String, (f64, String)>,
        active: &mut BTreeSet<String>,
        selected: &mut BTreeMap<String, String>,
        costs: &mut BTreeMap<String, f64>,
    ) -> Result<(), SelectError> {
        if !active.insert(id.to_owned()) {
            return Ok(());
        }
        let Some(task) = by_id.get(id).copied() else {
            return Err(SelectError::Invalid(format!("missing task `{id}`")));
        };
        if let Some((_, method_id)) = choices.get(id).filter(|(_, id)| !id.is_empty()) {
            let method = task
                .methods
                .iter()
                .find(|m| &m.id == method_id)
                .expect("greedy choice exists");
            selected.insert(id.to_owned(), method.id.clone());
            for (metric, amount) in &method.costs {
                *costs.entry(metric.clone()).or_default() += amount;
            }
            for child in &method.children {
                visit(child, by_id, choices, active, selected, costs)?;
            }
        }
        Ok(())
    }
    let mut choices = BTreeMap::new();
    best_tree_cost(root, by_id, c, &mut choices, &mut BTreeSet::new())?;
    let mut selected = BTreeMap::new();
    let mut costs = BTreeMap::new();
    if choices
        .get(root)
        .is_some_and(|(_, method)| method.is_empty())
    {
        return Ok(None);
    }
    visit(
        root,
        by_id,
        &choices,
        &mut BTreeSet::new(),
        &mut selected,
        &mut costs,
    )?;
    if c.limits
        .iter()
        .any(|(m, limit)| costs.get(m).copied().unwrap_or(0.0) > *limit + 1e-8)
    {
        return Ok(None);
    }
    Ok(Some(make_selection(
        selected,
        by_id,
        reachable,
        c,
        Optimality::GreedyOnly,
        Duration::ZERO,
    )))
}

fn extract<'a>(
    reachable: &BTreeSet<String>,
    by_id: &BTreeMap<&'a str, &'a Task>,
    chosen: &BTreeMap<(&'a str, &'a str), Variable>,
    solution: &impl Solution,
) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for id in reachable {
        if let Some(task) = by_id.get(id.as_str()) {
            for method in &task.methods {
                if solution.value(chosen[&(id.as_str(), method.id.as_str())]) > 0.5 {
                    result.insert(id.clone(), method.id.clone());
                    break;
                }
            }
        }
    }
    result
}

fn make_selection<'a>(
    selected: BTreeMap<String, String>,
    by_id: &BTreeMap<&'a str, &'a Task>,
    reachable: &BTreeSet<String>,
    c: &Constraints,
    optimality: Optimality,
    time_budget: Duration,
) -> Selection {
    let mut totals = BTreeMap::new();
    for id in reachable {
        if let Some(method_id) = selected.get(id)
            && let Some(method) = by_id
                .get(id.as_str())
                .and_then(|t| t.methods.iter().find(|m| &m.id == method_id))
        {
            for (metric, value) in &method.costs {
                *totals.entry(metric.clone()).or_insert(0.0) += value;
            }
        }
    }
    let objective = totals
        .iter()
        .map(|(m, v)| c.weights.get(m).copied().unwrap_or(0.0) * v)
        .sum();
    Selection {
        methods: selected,
        totals,
        objective,
        optimality,
        time_budget,
        makespan: None,
        explanation: format!(
            "Minimized weighted total effort to {objective:.3}; parallel completion time was not optimized."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn oracle(tasks: &[Task], root: &str) -> f64 {
        let by_id: BTreeMap<_, _> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
        let mut best = f64::INFINITY;
        // Enumerate every assignment. Child activation and shared costs are
        // evaluated by traversing only the selected reachable closure.
        let ids: Vec<_> = tasks.iter().map(|t| t.id.as_str()).collect();
        fn enumerate(
            i: usize,
            ids: &[&str],
            by: &BTreeMap<&str, &Task>,
            chosen: &mut BTreeMap<String, String>,
            root: &str,
            best: &mut f64,
        ) {
            if i == ids.len() {
                let mut needed = BTreeSet::new();
                let mut stack = vec![root.to_owned()];
                while let Some(id) = stack.pop() {
                    if !needed.insert(id.clone()) {
                        continue;
                    }
                    if let Some(method_id) = chosen.get(&id) {
                        let method = by[id.as_str()]
                            .methods
                            .iter()
                            .find(|m| &m.id == method_id)
                            .unwrap();
                        stack.extend(method.children.iter().cloned());
                    } else if !by[id.as_str()].methods.is_empty() {
                        return;
                    }
                }
                let value = needed
                    .iter()
                    .filter_map(|id| {
                        chosen.get(id).map(|method_id| {
                            by[id.as_str()]
                                .methods
                                .iter()
                                .find(|m| &m.id == method_id)
                                .unwrap()
                                .costs
                                .get("cost")
                                .copied()
                                .unwrap_or(0.0)
                        })
                    })
                    .sum();
                *best = best.min(value);
                return;
            }
            let task = by[ids[i]];
            if task.methods.is_empty() {
                enumerate(i + 1, ids, by, chosen, root, best);
                return;
            }
            for method in &task.methods {
                chosen.insert(task.id.clone(), method.id.clone());
                enumerate(i + 1, ids, by, chosen, root, best);
            }
        }
        enumerate(0, &ids, &by_id, &mut BTreeMap::new(), root, &mut best);
        best
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn ilp_matches_exhaustive_optimum(rows in prop::collection::vec(((0u8..20, 0u16..4096), (0u8..20, 0u16..4096)), 1..=12)) {
            let tasks: Vec<_> = rows.iter().enumerate().map(|(i, pair)| {
                let make_method = |j: usize, (cost, mask): &(u8, u16)| Method {
                    id: format!("m{i}_{j}"),
                    costs: BTreeMap::from([("cost".into(), f64::from(*cost))]),
                    children: (i+1..rows.len()).filter(|child| mask & (1 << (child - i - 1)) != 0).map(|child| format!("t{child}")).collect(),
                    order_edges: vec![],
                    resources: vec![],
                };
                Task {
                id: format!("t{i}"),
                methods: vec![make_method(0, &pair.0), make_method(1, &pair.1)],
            }}).collect();
            let result = select(&tasks, "t0", &Constraints { weights: BTreeMap::from([("cost".into(), 1.0)]), ..Constraints::default() }, Duration::from_secs(3)).unwrap();
            prop_assert_eq!(result.optimality, Optimality::Optimal);
            prop_assert!((result.objective - oracle(&tasks, "t0")).abs() < 1e-7);
        }
    }

    #[test]
    fn exact_selection_counts_shared_child_once_and_beats_greedy() {
        let tasks = vec![
            Task {
                id: "root".into(),
                methods: vec![
                    Method {
                        id: "independent".into(),
                        costs: BTreeMap::new(),
                        children: vec!["a1".into(), "a2".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                    Method {
                        id: "shared".into(),
                        costs: BTreeMap::new(),
                        children: vec!["b1".into(), "b2".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                ],
            },
            Task {
                id: "a1".into(),
                methods: vec![Method {
                    id: "per1".into(),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "a2".into(),
                methods: vec![Method {
                    id: "per2".into(),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "b1".into(),
                methods: vec![
                    Method {
                        id: "shared1".into(),
                        costs: BTreeMap::new(),
                        children: vec!["common".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                    Method {
                        id: "fallback1".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
                        order_edges: vec![],
                        resources: vec![],
                    },
                ],
            },
            Task {
                id: "b2".into(),
                methods: vec![
                    Method {
                        id: "shared2".into(),
                        costs: BTreeMap::new(),
                        children: vec!["common".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                    Method {
                        id: "fallback2".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
                        order_edges: vec![],
                        resources: vec![],
                    },
                ],
            },
            Task {
                id: "common".into(),
                methods: vec![Method {
                    id: "once".into(),
                    costs: BTreeMap::from([("cost".into(), 8.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
        ];
        let result = select(
            &tasks,
            "root",
            &Constraints {
                weights: BTreeMap::from([("cost".into(), 1.0)]),
                limits: BTreeMap::new(),
                ..Constraints::default()
            },
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(result.totals["cost"], 8.0);
        assert_eq!(result.optimality, Optimality::Optimal);
    }

    #[test]
    fn hard_budget_excludes_cheaper_objective_method() {
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                Method {
                    id: "cheap-but-over-budget".into(),
                    costs: BTreeMap::from([("money".into(), 11.0), ("quality".into(), 0.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
            ],
        }];
        let result = select(
            &tasks,
            "root",
            &Constraints {
                weights: BTreeMap::from([("quality".into(), 1.0)]),
                limits: BTreeMap::from([("money".into(), 10.0)]),
                ..Constraints::default()
            },
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(result.methods["root"], "feasible");
    }

    #[test]
    fn exact_time_limit_without_feasible_greedy_is_not_reported_infeasible() {
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                Method {
                    id: "cheap-but-over-budget".into(),
                    costs: BTreeMap::from([("money".into(), 11.0), ("quality".into(), 0.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
            ],
        }];
        let result = select(
            &tasks,
            "root",
            &Constraints {
                weights: BTreeMap::from([("quality".into(), 1.0)]),
                limits: BTreeMap::from([("money".into(), 10.0)]),
                ..Constraints::default()
            },
            Duration::ZERO,
        );
        assert_eq!(result, Err(SelectError::TimeLimit));
    }

    #[test]
    fn unneeded_tasks_have_no_chosen_method() {
        let tasks = vec![
            Task {
                id: "root".into(),
                methods: vec![
                    Method {
                        id: "direct".into(),
                        costs: BTreeMap::from([("cost".into(), 1.0)]),
                        children: vec![],
                        order_edges: vec![],
                        resources: vec![],
                    },
                    Method {
                        id: "detour".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec!["side".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                ],
            },
            Task {
                id: "side".into(),
                methods: vec![Method {
                    id: "free".into(),
                    costs: BTreeMap::new(),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
        ];
        let result = select(
            &tasks,
            "root",
            &Constraints {
                weights: BTreeMap::from([("cost".into(), 1.0)]),
                ..Constraints::default()
            },
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(
            result.methods,
            BTreeMap::from([("root".to_owned(), "direct".to_owned())])
        );
    }

    #[test]
    fn exact_time_limit_falls_back_to_feasible_greedy_choice() {
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![Method {
                id: "only-choice".into(),
                costs: BTreeMap::from([("cost".into(), 1.0)]),
                children: vec![],
                order_edges: vec![],
                resources: vec![],
            }],
        }];
        let result = select(
            &tasks,
            "root",
            &Constraints {
                weights: BTreeMap::from([("cost".into(), 1.0)]),
                ..Constraints::default()
            },
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(result.optimality, Optimality::GreedyOnly);
        assert_eq!(result.time_budget, Duration::ZERO);
        assert_eq!(result.methods["root"], "only-choice");
    }

    #[test]
    fn makespan_can_choose_more_total_effort_for_parallel_work() {
        let tasks = vec![
            Task {
                id: "root".into(),
                methods: vec![
                    Method {
                        id: "parallel".into(),
                        costs: BTreeMap::new(),
                        children: vec!["a".into(), "b".into(), "c".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                    Method {
                        id: "serial".into(),
                        costs: BTreeMap::new(),
                        children: vec!["x".into()],
                        order_edges: vec![],
                        resources: vec![],
                    },
                ],
            },
            Task {
                id: "a".into(),
                methods: vec![Method {
                    id: "a".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 20.0),
                        ("effort".into(), 20.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "b".into(),
                methods: vec![Method {
                    id: "b".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 20.0),
                        ("effort".into(), 20.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "c".into(),
                methods: vec![Method {
                    id: "c".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 20.0),
                        ("effort".into(), 20.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "x".into(),
                methods: vec![Method {
                    id: "x".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 20.0),
                        ("effort".into(), 20.0),
                    ]),
                    children: vec!["y".into()],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "y".into(),
                methods: vec![Method {
                    id: "y".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 20.0),
                        ("effort".into(), 20.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
        ];
        let constraints = Constraints {
            weights: BTreeMap::from([("effort".into(), 1.0)]),
            ..Constraints::default()
        };
        let effort = select_with_objective(
            &tasks,
            "root",
            &constraints,
            Duration::from_secs(2),
            Objective::TotalEffort,
        )
        .unwrap();
        let time = select_with_objective(
            &tasks,
            "root",
            &constraints,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(effort.methods["root"], "serial");
        assert_eq!(time.methods["root"], "parallel");
        assert_eq!(time.makespan, Some(20.0));
        assert_eq!(
            time.explanation,
            "Minimized dependency-aware completion time to 20.000 minutes at weighted effort 60.000; the effort-optimal selection takes 40.000 minutes at weighted effort 40.000, so 20.000 more effort buys 20.000 fewer minutes."
        );
    }

    #[test]
    fn makespan_counts_a_shared_child_once_and_respects_dependencies() {
        let tasks = vec![
            Task {
                id: "root".into(),
                methods: vec![Method {
                    id: "both".into(),
                    costs: BTreeMap::new(),
                    children: vec!["left".into(), "right".into()],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "left".into(),
                methods: vec![Method {
                    id: "left".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 3.0)]),
                    children: vec!["shared".into()],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "right".into(),
                methods: vec![Method {
                    id: "right".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 4.0)]),
                    children: vec!["shared".into()],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
            Task {
                id: "shared".into(),
                methods: vec![Method {
                    id: "shared".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                }],
            },
        ];
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints::default(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.totals["duration_minutes"], 12.0);
        assert_eq!(result.makespan, Some(9.0));
    }

    #[test]
    fn makespan_rejects_hard_constraint_infeasibility() {
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![Method {
                id: "too-long".into(),
                costs: BTreeMap::from([("duration_minutes".into(), 8.0)]),
                children: vec![],
                order_edges: vec![],
                resources: vec![],
            }],
        }];
        let constraints = Constraints {
            limits: BTreeMap::from([("duration_minutes".into(), 5.0)]),
            ..Constraints::default()
        };
        assert_eq!(
            select_with_objective(
                &tasks,
                "root",
                &constraints,
                Duration::from_secs(1),
                Objective::Makespan
            ),
            Err(SelectError::Infeasible)
        );
    }

    fn plain(id: &str, children: &[&str], edges: &[(&str, &str)]) -> Method {
        Method {
            id: id.into(),
            costs: BTreeMap::new(),
            children: children.iter().map(|c| (*c).into()).collect(),
            order_edges: edges
                .iter()
                .map(|(a, b)| ((*a).into(), (*b).into()))
                .collect(),
            resources: vec![],
        }
    }

    fn work(id: &str, costs: &[(&str, f64)]) -> Method {
        Method {
            id: id.into(),
            costs: costs.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
            children: vec![],
            order_edges: vec![],
            resources: vec![],
        }
    }

    fn task(id: &str, methods: Vec<Method>) -> Task {
        Task {
            id: id.into(),
            methods,
        }
    }

    fn twenty(id: &str) -> Task {
        task(
            id,
            vec![work(id, &[("duration_minutes", 20.0), ("effort", 20.0)])],
        )
    }

    fn effort_weights() -> Constraints {
        Constraints {
            weights: BTreeMap::from([("effort".into(), 1.0)]),
            ..Constraints::default()
        }
    }

    #[test]
    fn sibling_order_edges_prevent_apparent_parallelism() {
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("parallel", &["a", "b", "c"], &[]),
                    plain("ordered", &["x", "y"], &[("x", "y")]),
                ],
            ),
            twenty("a"),
            twenty("b"),
            twenty("c"),
            twenty("x"),
            twenty("y"),
        ];
        let run = |objective| {
            select_with_objective(
                &tasks,
                "root",
                &effort_weights(),
                Duration::from_secs(2),
                objective,
            )
            .unwrap()
        };
        let effort = run(Objective::TotalEffort);
        let time = run(Objective::Makespan);
        assert_eq!(effort.methods["root"], "ordered");
        assert_eq!(time.methods["root"], "parallel");
        assert_eq!(time.makespan, Some(20.0));
        assert_eq!(time.optimality, Optimality::Optimal);
        assert_eq!(time.totals["effort"], 60.0);
        let only_ordered = Constraints {
            forbidden_methods: BTreeSet::from([("root".into(), "parallel".into())]),
            ..effort_weights()
        };
        let ordered = select_with_objective(
            &tasks,
            "root",
            &only_ordered,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(ordered.makespan, Some(40.0));
    }

    #[test]
    fn a_shared_subtask_decides_between_the_alternatives_using_it() {
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("after-shared", &["shared", "a"], &[("shared", "a")]),
                    plain("beside-shared", &["shared", "b"], &[]),
                ],
            ),
            task(
                "shared",
                vec![
                    work("quick", &[("duration_minutes", 10.0), ("effort", 50.0)]),
                    work("slow", &[("duration_minutes", 30.0)]),
                ],
            ),
            task("a", vec![work("a", &[("duration_minutes", 5.0)])]),
            task("b", vec![work("b", &[("duration_minutes", 25.0)])]),
        ];
        let quick = select_with_objective(
            &tasks,
            "root",
            &effort_weights(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(
            quick.methods,
            BTreeMap::from([
                ("root".to_owned(), "after-shared".to_owned()),
                ("shared".to_owned(), "quick".to_owned()),
                ("a".to_owned(), "a".to_owned()),
            ])
        );
        assert_eq!(quick.makespan, Some(15.0));
        let slow_only = Constraints {
            forbidden_methods: BTreeSet::from([("shared".into(), "quick".into())]),
            ..effort_weights()
        };
        let slow = select_with_objective(
            &tasks,
            "root",
            &slow_only,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(slow.methods["root"], "beside-shared");
        assert_eq!(slow.makespan, Some(30.0));
    }

    #[test]
    fn makespan_ignores_forbidden_methods_on_unchosen_alternatives() {
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("blocked", &["x"], &[]),
                    plain("budgeted", &["y"], &[]),
                ],
            ),
            task("x", vec![work("x", &[])]),
            task(
                "y",
                vec![
                    work("y-pricey", &[("money", 100.0)]),
                    work("y-cheap", &[("money", 1.0), ("effort", 50.0)]),
                ],
            ),
        ];
        let constraints = Constraints {
            limits: BTreeMap::from([("money".into(), 10.0)]),
            forbidden_methods: BTreeSet::from([("x".into(), "x".into())]),
            ..effort_weights()
        };
        let result = select_with_objective(
            &tasks,
            "root",
            &constraints,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.methods["root"], "budgeted");
        assert_eq!(result.methods["y"], "y-cheap");
        assert_eq!(result.optimality, Optimality::Optimal);
    }

    #[test]
    fn makespan_beats_the_effort_greedy_choice_and_proves_it() {
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("blocked", &["x"], &[]),
                    plain("cheap-slow", &["y"], &[]),
                    plain("dear-fast", &["z"], &[]),
                ],
            ),
            task("x", vec![work("x", &[])]),
            task("y", vec![work("y", &[("duration_minutes", 100.0)])]),
            task(
                "z",
                vec![work("z", &[("duration_minutes", 1.0), ("effort", 10.0)])],
            ),
        ];
        let constraints = Constraints {
            forbidden_methods: BTreeSet::from([("x".into(), "x".into())]),
            ..effort_weights()
        };
        let result = select_with_objective(
            &tasks,
            "root",
            &constraints,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.methods["root"], "dear-fast");
        assert_eq!(result.makespan, Some(1.0));
        assert_eq!(result.optimality, Optimality::Optimal);
    }

    #[test]
    fn makespan_without_search_time_returns_the_labeled_greedy_fallback() {
        let tasks = vec![task("root", vec![plain("only", &["a"], &[])]), twenty("a")];
        let result = select_with_objective(
            &tasks,
            "root",
            &effort_weights(),
            Duration::ZERO,
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.optimality, Optimality::GreedyOnly);
        assert_eq!(result.makespan, Some(20.0));
        assert!(result.explanation.starts_with("Feasible fallback"));
    }

    #[test]
    fn ordered_siblings_sharing_a_subtask_wait_only_for_unshared_work() {
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("ordered", &["x", "y"], &[("x", "y")]),
                    work("slow", &[("duration_minutes", 30.0)]),
                ],
            ),
            task("x", vec![plain("x", &["s", "p"], &[])]),
            task("y", vec![plain("y", &["s", "q"], &[])]),
            task("s", vec![work("s", &[("duration_minutes", 10.0)])]),
            task("p", vec![work("p", &[("duration_minutes", 5.0)])]),
            task("q", vec![work("q", &[("duration_minutes", 7.0)])]),
        ];
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints::default(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.methods["root"], "ordered");
        assert_eq!(result.makespan, Some(17.0));
        assert_eq!(result.optimality, Optimality::Optimal);
    }

    #[test]
    fn an_order_edge_from_a_task_without_work_relays_its_predecessors() {
        let tasks = vec![
            task(
                "root",
                vec![plain(
                    "chain",
                    &["first", "idle", "last"],
                    &[("first", "idle"), ("idle", "last")],
                )],
            ),
            task("first", vec![work("first", &[("duration_minutes", 4.0)])]),
            task("idle", vec![]),
            task("last", vec![work("last", &[("duration_minutes", 6.0)])]),
        ];
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints::default(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.makespan, Some(10.0));
    }

    #[test]
    fn pareto_marks_dominated_alternative_with_dominators() {
        // Captain's PC-09 example: A 10 min/$12, B 14/$4, C 12/$7 are each
        // optimal under different tradeoffs, while D 20/$15 is dominated.
        // Fails when D lands on the frontier or names no dominator.
        let priced = |id: &str, minutes: f64, usd: f64| Method {
            id: id.into(),
            costs: BTreeMap::from([
                ("duration_minutes".into(), minutes),
                ("money_usd".into(), usd),
            ]),
            children: vec![],
            order_edges: vec![],
            resources: vec![],
        };
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                priced("a", 10.0, 12.0),
                priced("b", 14.0, 4.0),
                priced("c", 12.0, 7.0),
                priced("d", 20.0, 15.0),
            ],
        }];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
        assert_eq!(report.not_compared, Vec::<String>::new());
        assert_eq!(report.optimality, Optimality::Optimal);
        assert_eq!(report.evaluated, 4);
        let frontier: Vec<&str> = report
            .frontier
            .iter()
            .map(|alt| alt.methods["root"].as_str())
            .collect();
        assert_eq!(frontier, vec!["a", "c", "b"]);
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["root"], "d");
        assert!(report.incomplete.is_empty());
        let frontier_ids: Vec<&str> = report.frontier.iter().map(|alt| alt.id.as_str()).collect();
        assert!(
            report.dominated[0]
                .dominated_by
                .iter()
                .any(|d| frontier_ids.contains(&d.by.as_str())),
            "dominated alternative must name a frontier dominator"
        );
        let by_id: BTreeMap<&str, &str> = report
            .frontier
            .iter()
            .map(|alt| (alt.methods["root"].as_str(), alt.id.as_str()))
            .collect();
        assert!(report.dominated[0].dominated_by.contains(&Dominance {
            by: by_id["a"].to_owned(),
            on: vec!["makespan_minutes".into(), "money_usd".into()],
            unknown: vec![],
        }));
        // Weighted objectives survive beside the frontier for when
        // preferences are clear.
        assert!(report.frontier.iter().all(|alt| alt.objective >= 0.0));
    }

    #[test]
    fn pareto_compares_only_measures_known_for_every_alternative() {
        // `priced` values money, `bare` does not: money is listed as not
        // compared, and the pair still compares on time alone. Fails when
        // money is compared or `bare` is ranked incomplete.
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                Method {
                    id: "priced".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 10.0),
                        ("money_usd".into(), 100.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: "bare".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
            ],
        }];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["makespan_minutes"]);
        assert_eq!(report.not_compared, vec!["money_usd"]);
        assert!(report.incomplete.is_empty());
        assert_eq!(report.frontier.len(), 1);
        assert_eq!(report.frontier[0].methods["root"], "bare");
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["root"], "priced");
        assert_eq!(
            report.dominated[0].dominated_by,
            vec![Dominance {
                by: report.frontier[0].id.clone(),
                on: vec!["makespan_minutes".into()],
                unknown: vec!["money_usd".into()],
            }]
        );
    }

    #[test]
    fn pareto_reports_sparse_reliability_as_not_compared() {
        // The reviewer's case: reliability (`failure_risk`) priced for A
        // only. Time and money are still compared, so D is dominated by A
        // on those two with failure_risk named as not compared. Fails when
        // one sparse metric stops every other comparison.
        let priced = |id: &str, minutes: f64, usd: f64| Method {
            id: id.into(),
            costs: BTreeMap::from([
                ("duration_minutes".into(), minutes),
                ("money_usd".into(), usd),
            ]),
            children: vec![],
            order_edges: vec![],
            resources: vec![],
        };
        let mut risky_a = priced("a", 10.0, 12.0);
        risky_a.costs.insert("failure_risk".into(), 0.05);
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                risky_a,
                priced("b", 14.0, 4.0),
                priced("c", 12.0, 7.0),
                priced("d", 20.0, 15.0),
            ],
        }];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
        assert_eq!(report.not_compared, vec!["failure_risk"]);
        assert!(report.incomplete.is_empty());
        let frontier: Vec<&str> = report
            .frontier
            .iter()
            .map(|alt| alt.methods["root"].as_str())
            .collect();
        assert_eq!(frontier, vec!["a", "c", "b"]);
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["root"], "d");
        let by_a = Dominance {
            by: report.frontier[0].id.clone(),
            on: vec!["makespan_minutes".into(), "money_usd".into()],
            unknown: vec!["failure_risk".into()],
        };
        assert!(
            report.dominated[0].dominated_by.contains(&by_a),
            "{:?}",
            report.dominated[0].dominated_by
        );
    }

    #[test]
    fn pareto_sparse_estimates_never_empty_the_frontier() {
        // Each pair shares exactly one measure, and no measure is known for
        // all three: nothing is compared, so every alternative is complete
        // and the frontier holds all three. Fails on an empty frontier.
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![
                Method {
                    id: "m1".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 1.0),
                        ("failure_risk".into(), 0.2),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: "m2".into(),
                    costs: BTreeMap::from([
                        ("duration_minutes".into(), 2.0),
                        ("money_usd".into(), 1.0),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
                Method {
                    id: "m3".into(),
                    costs: BTreeMap::from([
                        ("money_usd".into(), 2.0),
                        ("failure_risk".into(), 0.1),
                    ]),
                    children: vec![],
                    order_edges: vec![],
                    resources: vec![],
                },
            ],
        }];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(report.compared.is_empty());
        assert_eq!(
            report.not_compared,
            vec!["failure_risk", "makespan_minutes", "money_usd"]
        );
        assert_eq!(report.evaluated, 3);
        assert_eq!(report.frontier.len(), 3);
        assert!(report.dominated.is_empty());
        assert!(report.incomplete.is_empty());
    }

    #[test]
    fn pareto_respects_hard_limits_and_reports_no_valued_objective() {
        // A money cap removes the dear alternative before dominance, so the
        // frontier never offers infeasible work. Fails on an infeasible id.
        let priced = |id: &str, usd: f64| Method {
            id: id.into(),
            costs: BTreeMap::from([("money_usd".into(), usd)]),
            children: vec![],
            order_edges: vec![],
            resources: vec![],
        };
        let tasks = vec![Task {
            id: "root".into(),
            methods: vec![priced("cheap", 4.0), priced("dear", 12.0)],
        }];
        let report = pareto(
            &tasks,
            "root",
            &Constraints {
                limits: BTreeMap::from([("money_usd".into(), 10.0)]),
                ..Constraints::default()
            },
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["money_usd"]);
        assert_eq!(report.frontier.len(), 1);
        assert_eq!(report.frontier[0].methods["root"], "cheap");
        assert!(report.dominated.is_empty());

        // No valued metric anywhere: nothing is compared, so the single
        // alternative is vacuously complete and holds the frontier alone
        // instead of inventing a ranking.
        let bare = vec![Task {
            id: "root".into(),
            methods: vec![Method {
                id: "m".into(),
                costs: BTreeMap::new(),
                children: vec![],
                order_edges: vec![],
                resources: vec![],
            }],
        }];
        let empty = pareto(
            &bare,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(empty.compared.is_empty());
        assert!(empty.not_compared.is_empty());
        assert_eq!(empty.frontier.len(), 1);
        assert!(empty.dominated.is_empty());
        assert!(empty.incomplete.is_empty());
        assert_eq!(empty.evaluated, 1);
    }

    #[test]
    fn pareto_compares_all_alternatives_on_shared_measures_only() {
        // Every assignment prices time and money, but only some price
        // reliability: time and money are compared, reliability is listed
        // as not compared, and dominance stays transitive. Cost-free
        // decomposition `r` must not void a measure by itself. Fails when
        // the sparse reliability column stops the money comparison.
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("r", &["s", "t"], &[]),
                    work("r-alt", &[("money_usd", 10.0), ("duration_minutes", 1.0)]),
                ],
            ),
            task(
                "s",
                vec![
                    work(
                        "s1",
                        &[
                            ("duration_minutes", 5.0),
                            ("money_usd", 5.0),
                            ("failure_risk", 0.2),
                        ],
                    ),
                    work("s2", &[("duration_minutes", 5.0), ("money_usd", 6.0)]),
                ],
            ),
            task(
                "t",
                vec![
                    work(
                        "t1",
                        &[
                            ("duration_minutes", 3.0),
                            ("money_usd", 1.0),
                            ("failure_risk", 0.1),
                        ],
                    ),
                    work("t2", &[("duration_minutes", 4.0), ("money_usd", 2.0)]),
                ],
            ),
        ];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.evaluated, 5);
        assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
        assert_eq!(report.not_compared, vec!["failure_risk"]);
        assert!(report.incomplete.is_empty());
        // `(makespan, money)`: r-alt (1, 10) is fastest, r/s1/t1 (5, 6)
        // cheapest; the (5, 7), (5, 7) and (5, 8) assignments are worse
        // than (5, 6) on money at equal time.
        let frontier: Vec<&str> = report
            .frontier
            .iter()
            .map(|alt| alt.methods["root"].as_str())
            .collect();
        assert_eq!(frontier, vec!["r-alt", "r"]);
        assert_eq!(report.frontier[1].methods["s"], "s1");
        assert_eq!(report.frontier[1].methods["t"], "t1");
        assert_eq!(report.dominated.len(), 3);
        for alt in &report.dominated {
            assert_eq!(alt.methods["root"], "r");
            assert!(
                alt.dominated_by
                    .iter()
                    .any(|d| d.on == vec!["makespan_minutes".to_owned(), "money_usd".to_owned()]),
                "{alt:?}"
            );
        }

        // Fully priced alternatives keep every measure through the same
        // cost-free decomposition.
        let priced = vec![
            task("root", vec![plain("r", &["s"], &[])]),
            task(
                "s",
                vec![
                    work("fast", &[("duration_minutes", 5.0), ("money_usd", 9.0)]),
                    work("slow", &[("duration_minutes", 8.0), ("money_usd", 2.0)]),
                    work("worse", &[("duration_minutes", 9.0), ("money_usd", 9.0)]),
                ],
            ),
        ];
        let report = pareto(
            &priced,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
        assert_eq!(report.frontier.len(), 2);
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["s"], "worse");
    }

    #[test]
    fn pareto_shows_unresolved_work_separately_without_ranking_it() {
        // `split` leaves `b` without a method: its time and cost are unknown,
        // so `split` is incomplete and shown separately, never ranked, while
        // complete `whole` still dominates `whole2`. Done or dropped
        // method-less work stays out of `unresolved` and keeps `split`
        // priced. Fails when `split` is ranked, `whole` is labeled
        // dominated while `b` is open, or `whole2` is not.
        let tasks = vec![
            task(
                "root",
                vec![
                    plain("split", &["a", "b"], &[]),
                    work("whole", &[("duration_minutes", 8.0), ("money_usd", 8.0)]),
                    work("whole2", &[("duration_minutes", 10.0), ("money_usd", 10.0)]),
                ],
            ),
            task(
                "a",
                vec![work("a", &[("duration_minutes", 5.0), ("money_usd", 5.0)])],
            ),
            task("b", vec![]),
        ];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::from(["b".to_owned()]),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.evaluated, 3);
        assert_eq!(report.compared, vec!["makespan_minutes", "money_usd"]);
        assert!(report.not_compared.is_empty());
        assert_eq!(report.frontier.len(), 1);
        assert_eq!(report.frontier[0].methods["root"], "whole");
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["root"], "whole2");
        assert_eq!(
            report.dominated[0].dominated_by,
            vec![Dominance {
                by: report.frontier[0].id.clone(),
                on: vec!["makespan_minutes".into(), "money_usd".into()],
                unknown: vec![],
            }]
        );
        assert_eq!(report.incomplete.len(), 1);
        assert_eq!(report.incomplete[0].methods["root"], "split");
        assert_eq!(
            report.incomplete[0].unknown,
            vec!["makespan_minutes".to_owned(), "money_usd".to_owned()]
        );

        let done = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(done.compared, vec!["makespan_minutes", "money_usd"]);
        assert!(done.incomplete.is_empty());
        assert_eq!(done.frontier.len(), 1);
        assert_eq!(done.frontier[0].methods["root"], "split");
        assert_eq!(done.dominated.len(), 2);
    }

    #[test]
    fn pareto_excludes_internal_metrics_from_dimensions() {
        let tasks = vec![task(
            "root",
            vec![
                work("a", &[("failure_risk", 0.1), ("quality_loss", 0.105)]),
                work("b", &[("failure_risk", 0.3), ("quality_loss", 0.357)]),
            ],
        )];
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &["quality_loss"],
            &BTreeSet::new(),
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(report.compared, vec!["failure_risk"]);
        assert_eq!(report.frontier.len(), 1);
        assert_eq!(
            report.frontier[0].objectives.keys().collect::<Vec<_>>(),
            vec!["failure_risk"]
        );
        assert_eq!(report.dominated.len(), 1);
        assert_eq!(report.dominated[0].methods["root"], "b");
    }

    #[test]
    fn pareto_caps_collected_assignments_and_reports_best_found() {
        // 2^14 feasible assignments exceed the cap; the search must stop
        // there and say the frontier is best-found, not optimal.
        let leaves: Vec<String> = (0..14).map(|n| format!("t{n}")).collect();
        let children: Vec<&str> = leaves.iter().map(String::as_str).collect();
        let mut tasks = vec![task("root", vec![plain("r", &children, &[])])];
        for (n, id) in leaves.iter().enumerate() {
            tasks.push(task(
                id,
                vec![
                    work("fast", &[("duration_minutes", 1.0), ("money_usd", 2.0)]),
                    work(
                        "cheap",
                        &[("duration_minutes", 2.0 + n as f64), ("money_usd", 1.0)],
                    ),
                ],
            ));
        }
        let report = pareto(
            &tasks,
            "root",
            &Constraints::default(),
            &[],
            &BTreeSet::new(),
            Duration::from_secs(600),
        )
        .unwrap();
        assert!(report.evaluated <= MAX_PARETO_ASSIGNMENTS + 1);
        assert_eq!(
            report.optimality,
            Optimality::BestFound { gap_percent: None }
        );
        assert!(
            report.frontier.len() + report.dominated.len() + report.incomplete.len()
                == report.evaluated
        );
    }

    #[test]
    fn timed_out_search_keeps_greedy_when_equally_fast_and_cheaper() {
        let scored = |method: &str, objective: f64| Candidate {
            methods: BTreeMap::from([("root".to_owned(), method.to_owned())]),
            totals: BTreeMap::from([("effort".to_owned(), objective)]),
            objective,
            makespan: 10.0,
            exact: true,
        };
        let incumbent = scored("incumbent", 5.0);
        let greedy = scored("greedy", 3.0);
        assert_eq!(
            settle(Some(incumbent.clone()), Some(greedy.clone()), true, false),
            Ok((greedy.clone(), Optimality::GreedyOnly))
        );
        assert_eq!(
            settle(Some(greedy.clone()), Some(incumbent.clone()), true, false),
            Ok((greedy, Optimality::BestFound { gap_percent: None }))
        );
    }

    fn holding(id: &str, duration: f64, resources: &[&str]) -> Method {
        Method {
            id: id.into(),
            costs: BTreeMap::from([("duration_minutes".into(), duration)]),
            children: vec![],
            order_edges: vec![],
            resources: resources.iter().map(|r| (*r).into()).collect(),
        }
    }

    fn resolve_tasks() -> Vec<Task> {
        vec![
            task(
                "root",
                vec![
                    plain("resolve-heavy", &["r1", "r2"], &[]),
                    plain("offload", &["g1", "g2"], &[]),
                ],
            ),
            task("r1", vec![holding("r1", 20.0, &["resolve"])]),
            task("r2", vec![holding("r2", 20.0, &["resolve"])]),
            task("g1", vec![holding("g1", 20.0, &["resolve"])]),
            task("g2", vec![holding("g2", 20.0, &["gpu"])]),
        ]
    }

    /// Acceptance case 3 from the captain's section 5.1: resource contention
    /// changes the best plan. Without caps both alternatives finish in 20
    /// minutes and the first wins; with one Resolve instance the
    /// resolve-heavy plan serializes to 40 while offloading one task to a
    /// GPU worker stays at 20. Fails on a precedence-only makespan, which
    /// scores both at 20 and never flips.
    #[test]
    fn resource_contention_changes_the_best_plan() {
        let tasks = resolve_tasks();
        let unlimited = select_with_objective(
            &tasks,
            "root",
            &Constraints::default(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(unlimited.methods["root"], "resolve-heavy");
        assert_eq!(unlimited.makespan, Some(20.0));
        assert!(
            unlimited
                .explanation
                .starts_with("Minimized dependency-aware"),
            "unlimited schedule keeps the old label: {}",
            unlimited.explanation
        );
        let limited = select_with_objective(
            &tasks,
            "root",
            &Constraints {
                capacities: BTreeMap::from([("resolve".into(), 1.0), ("gpu".into(), 2.0)]),
                ..Constraints::default()
            },
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(limited.methods["root"], "offload");
        assert_eq!(limited.makespan, Some(20.0));
        assert_eq!(limited.optimality, Optimality::Optimal);
        assert!(
            limited.explanation.starts_with("Minimized resource-aware"),
            "limited schedule names the resource model: {}",
            limited.explanation
        );
        // The loser under contention serializes its two Resolve holders.
        let heavy_only = Constraints {
            capacities: BTreeMap::from([("resolve".into(), 1.0)]),
            forbidden_methods: BTreeSet::from([("root".into(), "offload".into())]),
            ..Constraints::default()
        };
        let heavy = select_with_objective(
            &tasks,
            "root",
            &heavy_only,
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(heavy.methods["root"], "resolve-heavy");
        assert_eq!(heavy.makespan, Some(40.0));
    }

    /// Two exclusive holders serialize while work on other resources still
    /// overlaps them. Fails on a precedence-only schedule, which reports 20.
    #[test]
    fn exclusive_holders_serialize_while_other_work_overlaps() {
        let tasks = vec![
            task("root", vec![plain("all", &["a", "b", "c"], &[])]),
            task("a", vec![holding("a", 20.0, &["resolve"])]),
            task("b", vec![holding("b", 20.0, &["resolve"])]),
            task("c", vec![holding("c", 20.0, &["gpu"])]),
        ];
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints {
                capacities: BTreeMap::from([("resolve".into(), 1.0), ("gpu".into(), 2.0)]),
                ..Constraints::default()
            },
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.makespan, Some(40.0));
        assert_eq!(result.optimality, Optimality::Optimal);
    }

    /// A resource with capacity two runs two holders concurrently; the same
    /// plan under capacity one serializes them. Fails when every capacity is
    /// treated as exclusive.
    #[test]
    fn capacity_above_one_allows_concurrent_holders() {
        let tasks = vec![
            task("root", vec![plain("all", &["a", "b"], &[])]),
            task("a", vec![holding("a", 20.0, &["gpu"])]),
            task("b", vec![holding("b", 20.0, &["gpu"])]),
        ];
        let run = |capacity: f64| {
            select_with_objective(
                &tasks,
                "root",
                &Constraints {
                    capacities: BTreeMap::from([("gpu".into(), capacity)]),
                    ..Constraints::default()
                },
                Duration::from_secs(2),
                Objective::Makespan,
            )
            .unwrap()
        };
        assert_eq!(run(2.0).makespan, Some(20.0));
        assert_eq!(run(1.0).makespan, Some(40.0));
    }

    /// A negative capacity is a malformed constraint, not silent unlimited
    /// parallelism. Fails when validation ignores the capacities map.
    #[test]
    fn invalid_capacity_is_rejected() {
        let tasks = vec![task("root", vec![holding("only", 1.0, &["resolve"])])];
        assert_eq!(
            select_with_objective(
                &tasks,
                "root",
                &Constraints {
                    capacities: BTreeMap::from([("resolve".into(), -1.0)]),
                    ..Constraints::default()
                },
                Duration::from_secs(1),
                Objective::Makespan,
            ),
            Err(SelectError::Invalid(
                "invalid capacity for resource `resolve`".into()
            ))
        );
    }

    /// A resource nobody declared a capacity for never constrains the plan.
    /// Fails when unknown resources block scheduling instead of reading as
    /// unlimited.
    #[test]
    fn undeclared_capacity_reads_as_unlimited() {
        let tasks = vec![
            task("root", vec![plain("all", &["a", "b"], &[])]),
            task("a", vec![holding("a", 20.0, &["resolve"])]),
            task("b", vec![holding("b", 20.0, &["resolve"])]),
        ];
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints::default(),
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.makespan, Some(20.0));
    }

    /// Peak concurrency, not the count of every holder touching the window,
    /// decides whether a slot is free: back-to-back holders on a capacity-2
    /// resource leave one slot open the whole time. Fails when overlaps are
    /// counted over the whole window, which pushes the start to 5.
    #[test]
    fn earliest_slot_counts_peak_concurrency() {
        let slots = BTreeMap::from([("ci".to_owned(), 2)]);
        let busy = BTreeMap::from([("ci".to_owned(), vec![(0.0, 5.0), (5.0, 10.0)])]);
        let need = BTreeSet::from(["ci".to_owned()]);
        assert_eq!(earliest_slot(0.0, 10.0, &need, &slots, &busy), 0.0);
        let full = BTreeMap::from([("ci".to_owned(), vec![(0.0, 5.0), (5.0, 10.0), (2.0, 7.0)])]);
        assert_eq!(earliest_slot(0.0, 10.0, &need, &slots, &full), 7.0);
    }

    /// Graphs past the exact-search size use the list heuristic, so the
    /// makespan is not proved minimal and the selection must not claim it.
    /// Fails when the heuristic span is labeled `optimal`.
    #[test]
    fn heuristic_schedule_is_reported_best_found() {
        let ids: Vec<String> = (0..EXACT_SCHEDULE_NODES + 1)
            .map(|i| format!("t{i}"))
            .collect();
        let children: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut tasks = vec![task("root", vec![plain("all", &children, &[])])];
        for id in &ids {
            tasks.push(task(id, vec![holding(id, 10.0, &["resolve"])]));
        }
        let result = select_with_objective(
            &tasks,
            "root",
            &Constraints {
                capacities: BTreeMap::from([("resolve".into(), 1.0)]),
                ..Constraints::default()
            },
            Duration::from_secs(2),
            Objective::Makespan,
        )
        .unwrap();
        assert_eq!(result.makespan, Some(90.0));
        assert_eq!(
            result.optimality,
            Optimality::BestFound { gap_percent: None }
        );
    }

    /// The schedule depends on node keys, not on how a caller numbered the
    /// nodes, so P6 and P7 agree on the heuristic span. Fails when ties
    /// break by index: here index order runs the short chain head first and
    /// finishes at 40, key and tail order run the long chain first for 30.
    #[test]
    fn schedule_is_independent_of_node_numbering() {
        // a(10, res) -> c(20); b(10, res); edge a -> c. Numbered two ways.
        let n = EXACT_SCHEDULE_NODES + 1;
        let build = |perm: &[usize]| {
            let mut durations = vec![0.0; n];
            let mut demands = vec![BTreeSet::new(); n];
            let mut keys = vec![String::new(); n];
            let spec = [("a", 10.0, true), ("b", 10.0, true), ("c", 20.0, false)];
            for (logical, &(key, duration, holds)) in spec.iter().enumerate() {
                let at = perm[logical];
                durations[at] = duration;
                keys[at] = key.into();
                if holds {
                    demands[at].insert("res".to_owned());
                }
            }
            for logical in spec.len()..n {
                keys[perm[logical]] = format!("z{logical}");
            }
            let edges = BTreeSet::from([(perm[0], perm[2])]);
            let schedule = resource_constrained_schedule(
                &durations,
                &edges,
                &demands,
                &keys,
                &BTreeMap::from([("res".to_owned(), 1.0)]),
            )
            .unwrap();
            assert!(!schedule.exact);
            let by_key: BTreeMap<String, f64> = keys
                .iter()
                .cloned()
                .zip(schedule.starts.iter().copied())
                .collect();
            (schedule.makespan, by_key)
        };
        let forward: Vec<usize> = (0..n).collect();
        let mut reversed = forward.clone();
        reversed.reverse();
        let (span, starts) = build(&forward);
        assert_eq!(span, 30.0);
        assert_eq!(build(&reversed), (span, starts));
    }
}
