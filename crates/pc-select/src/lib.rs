//! Deterministic alternative-method selection (P6).
//!
//! Tasks are OR variables and methods are AND variables. A selected method
//! activates each child task; each task's cost is therefore counted once even
//! when multiple selected methods share it.

use good_lp::{
    Expression, ProblemVariables, ResolutionError, Solution, SolutionStatus, SolverModel, Variable,
    WithTimeLimit, default_solver, variable,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
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
            for child in &method.children {
                if !by_id.contains_key(child.as_str()) {
                    return Err(SelectError::Invalid(format!(
                        "method `{}` references missing child `{child}`",
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
                    },
                    Method {
                        id: "shared".into(),
                        costs: BTreeMap::new(),
                        children: vec!["b1".into(), "b2".into()],
                    },
                ],
            },
            Task {
                id: "a1".into(),
                methods: vec![Method {
                    id: "per1".into(),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                }],
            },
            Task {
                id: "a2".into(),
                methods: vec![Method {
                    id: "per2".into(),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                }],
            },
            Task {
                id: "b1".into(),
                methods: vec![
                    Method {
                        id: "shared1".into(),
                        costs: BTreeMap::new(),
                        children: vec!["common".into()],
                    },
                    Method {
                        id: "fallback1".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
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
                    },
                    Method {
                        id: "fallback2".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
                    },
                ],
            },
            Task {
                id: "common".into(),
                methods: vec![Method {
                    id: "once".into(),
                    costs: BTreeMap::from([("cost".into(), 8.0)]),
                    children: vec![],
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
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
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
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
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
                    },
                    Method {
                        id: "detour".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec!["side".into()],
                    },
                ],
            },
            Task {
                id: "side".into(),
                methods: vec![Method {
                    id: "free".into(),
                    costs: BTreeMap::new(),
                    children: vec![],
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
}
