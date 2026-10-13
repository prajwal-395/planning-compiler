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
    /// Minimize dependency-aware completion time using `duration_minutes`.
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

/// Select using either additive total effort or dependency-aware makespan.
/// Makespan selection searches method assignments of activated tasks exactly
/// until the time budget expires; a feasible greedy selection is retained as
/// a fallback.
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
    };
    search.run(0);
    let MakespanSearch {
        fastest,
        cheapest,
        stopped,
        ..
    } = search;
    let cheapest = [cheapest, greedy.clone()]
        .into_iter()
        .flatten()
        .reduce(|a, b| if b.cheaper_than(&a) { b } else { a });
    let (chosen, optimality) = settle(fastest, greedy, stopped)?;
    let cheapest = cheapest.expect("a chosen candidate exists");
    let mut explanation = format!(
        "Minimized dependency-aware completion time to {:.3} minutes at weighted effort {:.3}",
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

/// Pick the search incumbent or the greedy fallback, labeled by whether the
/// search finished.
fn settle(
    fastest: Option<Candidate>,
    greedy: Option<Candidate>,
    stopped: bool,
) -> Result<(Candidate, Optimality), SelectError> {
    match (fastest, greedy) {
        (Some(best), _) if !stopped => Ok((best, Optimality::Optimal)),
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
    let makespan = schedule_makespan(by_id, methods, root)?;
    let objective = totals
        .iter()
        .map(|(m, v)| constraints.weights.get(m).copied().unwrap_or(0.0) * v)
        .sum();
    Some(Candidate {
        methods: methods.clone(),
        totals,
        objective,
        makespan,
    })
}

fn chosen_method<'a>(
    by_id: &BTreeMap<&str, &'a Task>,
    task: &str,
    method_id: &str,
) -> Option<&'a Method> {
    by_id.get(task)?.methods.iter().find(|m| m.id == method_id)
}

/// Critical-path makespan with P7's leaf semantics. A task with a chosen
/// method is a node when that method has no children or its own
/// `duration_minutes`; a node runs after its children's nodes. A sibling
/// order edge `from -> to` makes every node of `to` that is not also a
/// node of `from` wait for `from`'s nodes, with a node-less `from` relaying
/// whatever must finish before it. `None` on a precedence cycle.
fn schedule_makespan(
    by_id: &BTreeMap<&str, &Task>,
    chosen: &BTreeMap<String, String>,
    root: &str,
) -> Option<f64> {
    let mut schedule = Schedule {
        by_id,
        chosen,
        leaves: BTreeMap::new(),
        durations: Vec::new(),
        edges: BTreeSet::new(),
    };
    schedule.leaves(root)?;
    let Schedule {
        durations, edges, ..
    } = schedule;
    let mut successors = vec![Vec::new(); durations.len()];
    let mut indegree = vec![0usize; durations.len()];
    for &(from, to) in &edges {
        successors[from].push(to);
        indegree[to] += 1;
    }
    let mut ready: Vec<_> = (0..durations.len()).filter(|&n| indegree[n] == 0).collect();
    let mut start = vec![0.0f64; durations.len()];
    let mut makespan = 0.0f64;
    let mut seen = 0;
    while let Some(node) = ready.pop() {
        seen += 1;
        let finish = start[node] + durations[node];
        makespan = makespan.max(finish);
        for &to in &successors[node] {
            start[to] = start[to].max(finish);
            indegree[to] -= 1;
            if indegree[to] == 0 {
                ready.push(to);
            }
        }
    }
    (seen == durations.len()).then_some(makespan)
}

struct Schedule<'a, 'b> {
    by_id: &'b BTreeMap<&'b str, &'a Task>,
    chosen: &'b BTreeMap<String, String>,
    leaves: BTreeMap<String, BTreeSet<usize>>,
    durations: Vec<f64>,
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
                    },
                    Method {
                        id: "shared".into(),
                        costs: BTreeMap::new(),
                        children: vec!["b1".into(), "b2".into()],
                        order_edges: vec![],
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
                }],
            },
            Task {
                id: "a2".into(),
                methods: vec![Method {
                    id: "per2".into(),
                    costs: BTreeMap::from([("cost".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
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
                    },
                    Method {
                        id: "fallback1".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
                        order_edges: vec![],
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
                    },
                    Method {
                        id: "fallback2".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec![],
                        order_edges: vec![],
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
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
                    order_edges: vec![],
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
                },
                Method {
                    id: "feasible".into(),
                    costs: BTreeMap::from([("money".into(), 9.0), ("quality".into(), 4.0)]),
                    children: vec![],
                    order_edges: vec![],
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
                    },
                    Method {
                        id: "detour".into(),
                        costs: BTreeMap::from([("cost".into(), 5.0)]),
                        children: vec!["side".into()],
                        order_edges: vec![],
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
                    },
                    Method {
                        id: "serial".into(),
                        costs: BTreeMap::new(),
                        children: vec!["x".into()],
                        order_edges: vec![],
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
                }],
            },
            Task {
                id: "left".into(),
                methods: vec![Method {
                    id: "left".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 3.0)]),
                    children: vec!["shared".into()],
                    order_edges: vec![],
                }],
            },
            Task {
                id: "right".into(),
                methods: vec![Method {
                    id: "right".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 4.0)]),
                    children: vec!["shared".into()],
                    order_edges: vec![],
                }],
            },
            Task {
                id: "shared".into(),
                methods: vec![Method {
                    id: "shared".into(),
                    costs: BTreeMap::from([("duration_minutes".into(), 5.0)]),
                    children: vec![],
                    order_edges: vec![],
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
        }
    }

    fn work(id: &str, costs: &[(&str, f64)]) -> Method {
        Method {
            id: id.into(),
            costs: costs.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
            children: vec![],
            order_edges: vec![],
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
    fn timed_out_search_keeps_greedy_when_equally_fast_and_cheaper() {
        let scored = |method: &str, objective: f64| Candidate {
            methods: BTreeMap::from([("root".to_owned(), method.to_owned())]),
            totals: BTreeMap::from([("effort".to_owned(), objective)]),
            objective,
            makespan: 10.0,
        };
        let incumbent = scored("incumbent", 5.0);
        let greedy = scored("greedy", 3.0);
        assert_eq!(
            settle(Some(incumbent.clone()), Some(greedy.clone()), true),
            Ok((greedy.clone(), Optimality::GreedyOnly))
        );
        assert_eq!(
            settle(Some(greedy.clone()), Some(incumbent.clone()), true),
            Ok((greedy, Optimality::BestFound { gap_percent: None }))
        );
    }
}
