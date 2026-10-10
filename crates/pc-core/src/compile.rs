//! Deterministic analysis of the accepted portion of a Work IR plan.
//!
//! Step 2 deliberately does not choose between alternatives. A task is
//! compiled only when it has exactly one accepted method; proposed, derived,
//! and rejected methods are outside this pass. Capability costs come from
//! the P5 effective estimates, so recorded evidence moves selection;
//! every total carries its predicted-versus-measured basis.

use pc_ir::{MethodKind, MethodStatus, PlanDoc};
use pc_select::{
    Constraints as SelectionConstraints, Method as SelectionMethod, Optimality,
    Task as SelectionTask,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// A low, likely, and high estimate, rolled up without collapsing the range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EstimateRange {
    /// Optimistic estimate.
    pub low: f64,
    /// Most likely estimate.
    pub likely: f64,
    /// Pessimistic estimate.
    pub high: f64,
    /// PERT weighted expected estimate, `(low + 4*likely + high) / 6`. For
    /// rolled-up `duration_minutes` it is instead the CPM makespan over each
    /// task's expected duration, so it need not match this range's own PERT.
    pub expected: f64,
}

/// CPM timing for an executable task. Decomposition tasks have no own timing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskTiming {
    /// Earliest start in expected minutes.
    pub earliest_start: f64,
    /// Earliest finish in expected minutes.
    pub earliest_finish: f64,
    /// Latest start without delaying the makespan.
    pub latest_start: f64,
    /// Latest finish without delaying the makespan.
    pub latest_finish: f64,
    /// Slack in expected minutes.
    pub slack: f64,
    /// Whether this task lies on at least one critical path.
    pub critical: bool,
}

/// Predicted-versus-measured summary for one metric: what the plan costs on
/// declared guesses beside what it costs with evidence folded in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricBasis {
    /// Roll-up over declared estimates only: the prediction.
    pub predicted: EstimateRange,
    /// Roll-up over effective (measured where available) estimates: the
    /// number P6 optimized. Always equals `totals[metric]`.
    pub effective: EstimateRange,
    /// Selected leaf tasks with measured data for this metric.
    pub measured_nodes: usize,
    /// Selected leaf tasks contributing to this metric at all.
    pub total_nodes: usize,
}

/// One provenance row: declared guesses beside measured evidence for a
/// capability and metric. This is the provenance report `pc compile --json`
/// carries and `pc explain capability:<id>` renders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceRow {
    /// Capability id.
    pub capability: String,
    /// One known metric.
    pub metric: String,
    /// Declared triple, when the plan states one.
    pub predicted: Option<EstimateRange>,
    /// Measured triple, when attributed evidence exists.
    pub measured: Option<EstimateRange>,
    /// Observation count behind `measured` (zero when declared-only).
    pub observations: u64,
    /// Stale observations behind `measured`.
    pub stale: u64,
    /// True when the measured median falls outside declared `[low, high]`.
    pub contradicts_declared: bool,
    /// The numbers selection and roll-ups used.
    pub effective: EstimateRange,
    /// `"declared"` or `"measured(n)"`.
    pub basis: String,
    /// True when a selected method invokes this capability.
    pub selected: bool,
}

/// Roll-ups and CPM timing for the reachable accepted plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledPlan {
    /// Root task id.
    pub root_task: String,
    /// Effective roll-up ranges by metric, with keys in lexical order.
    pub totals: BTreeMap<String, EstimateRange>,
    /// Predicted-versus-measured basis behind each metric in `totals`.
    pub basis: BTreeMap<String, MetricBasis>,
    /// Per-capability provenance: declared guesses beside measured
    /// evidence. Sorted by `(capability, metric)`.
    pub provenance: Vec<ProvenanceRow>,
    /// `task:<id>` evidence P5 ignored because the task's accepted invoke
    /// methods call no capability or more than one. Sorted by
    /// `(task, metric)`.
    pub unattributed_tasks: Vec<crate::estimate::UnattributedTaskEvidence>,
    /// Expected duration in minutes, equal to the CPM makespan.
    pub makespan_minutes: f64,
    /// Maximum number of simultaneously executable leaf tasks. Tasks with
    /// zero expected duration occupy no time and are not counted.
    pub parallel_width: usize,
    /// CPM details for executable leaf tasks, keyed by task id.
    pub timings: BTreeMap<String, TaskTiming>,
    /// Tasks reachable from the root without exactly one accepted method.
    pub unresolved_tasks: Vec<String>,
    /// Accepted method choices and analysis decisions for `pc explain`.
    pub trace: Vec<CompileTrace>,
    /// P6 optimality status for the selected methods.
    pub selection_status: String,
    /// P6 time budget in milliseconds.
    pub selection_time_budget_ms: u64,
}

/// One stable explanation record from compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileTrace {
    /// Entity subject such as `task:write` or `method:write-via-cli`.
    pub subject: String,
    /// Decision made by the compiler.
    pub decision: String,
    /// Short reason or calculation description.
    pub reason: String,
}

/// Compilation errors that remain after P1-P2 validation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CompileError {
    /// Order edges across methods sharing subtasks form a precedence cycle.
    #[error("tasks {} form a precedence cycle across methods", .tasks.join(", "))]
    PrecedenceCycle {
        /// Executable tasks on or behind the cycle, in lexical order.
        tasks: Vec<String>,
    },
    /// P6 found no method selection satisfying hard constraints.
    #[error("no feasible method selection satisfies the hard constraints")]
    SelectionInfeasible,
    /// P6 failed to solve the method-selection problem.
    #[error("method selection failed: {message}")]
    SelectionFailed {
        /// Solver diagnostic.
        message: String,
    },
}

#[derive(Clone)]
struct Node {
    task: String,
    capability: String,
    estimates: BTreeMap<String, EstimateRange>,
    predicted: BTreeMap<String, EstimateRange>,
    duration: EstimateRange,
    predicted_duration: EstimateRange,
}

struct Walk<'a> {
    tasks: BTreeMap<&'a str, &'a pc_ir::Task>,
    methods: BTreeMap<&'a str, Vec<&'a pc_ir::Method>>,
    effective: BTreeMap<String, BTreeMap<String, EstimateRange>>,
    predicted: BTreeMap<String, BTreeMap<String, EstimateRange>>,
    measured: BTreeSet<(String, String)>,
    nodes: Vec<Node>,
    edges: BTreeSet<(usize, usize)>,
    unresolved: BTreeSet<String>,
    trace: Vec<CompileTrace>,
    leaves: BTreeMap<&'a str, Vec<usize>>,
}

impl<'a> Walk<'a> {
    fn task(&mut self, id: &'a str) -> Result<Vec<usize>, CompileError> {
        if let Some(leaves) = self.leaves.get(id) {
            return Ok(leaves.clone());
        }
        self.leaves.insert(id, Vec::new());
        let leaves = self.expand(id)?;
        self.leaves.insert(id, leaves.clone());
        Ok(leaves)
    }

    fn expand(&mut self, id: &'a str) -> Result<Vec<usize>, CompileError> {
        let Some(task) = self.tasks.get(id).copied() else {
            return Ok(Vec::new());
        };
        if matches!(
            task.state,
            pc_ir::TaskState::Done | pc_ir::TaskState::Dropped
        ) {
            self.trace.push(CompileTrace {
                subject: format!("task:{id}"),
                decision: "skipped".into(),
                reason: format!("task is marked {:?}", task.state),
            });
            return Ok(Vec::new());
        }
        let accepted = self.methods.get(id).map(Vec::as_slice).unwrap_or_default();
        let Some(method) = accepted.first().copied() else {
            self.unresolved.insert(id.to_owned());
            self.trace.push(CompileTrace {
                subject: format!("task:{id}"),
                decision: "unresolved".into(),
                reason: "no accepted method".into(),
            });
            return Ok(Vec::new());
        };
        self.trace.push(CompileTrace {
            subject: format!("task:{id}"),
            decision: "accepted".into(),
            reason: format!("uses method:{}", method.id),
        });
        self.trace.push(CompileTrace {
            subject: format!("method:{}", method.id),
            decision: "included".into(),
            reason: format!("accepted {:?} method", method.kind),
        });
        for assumption in &method.assumptions {
            self.trace.push(CompileTrace {
                subject: format!("method:{}", method.id),
                decision: "assumes".into(),
                reason: assumption.clone(),
            });
        }
        match method.kind {
            MethodKind::Invoke => {
                let capability_id = method.capability.as_deref().unwrap_or_default();
                let estimates = self
                    .effective
                    .get(capability_id)
                    .cloned()
                    .unwrap_or_default();
                let predicted = self
                    .predicted
                    .get(capability_id)
                    .cloned()
                    .unwrap_or_default();
                let duration = estimates.get("duration_minutes").copied().unwrap_or(zero());
                let predicted_duration =
                    predicted.get("duration_minutes").copied().unwrap_or(zero());
                let n = self.nodes.len();
                self.nodes.push(Node {
                    task: task.id.clone(),
                    capability: capability_id.to_string(),
                    estimates,
                    predicted,
                    duration,
                    predicted_duration,
                });
                if let Some(capability) = method.capability.as_deref() {
                    self.trace.push(CompileTrace {
                        subject: format!("capability:{capability}"),
                        decision: "used".into(),
                        reason: format!("invoked by method:{}", method.id),
                    });
                }
                Ok(vec![n])
            }
            MethodKind::Decompose => {
                let mut children = BTreeMap::new();
                for child in &method.subtasks {
                    let leaves = self.task(child)?;
                    children.insert(child.as_str(), leaves);
                }
                let mut finishes = BTreeMap::new();
                for edge in &method.order_edges {
                    let from = finish_leaves(&edge.from, method, &children, &mut finishes);
                    for &b in children.get(edge.to.as_str()).into_iter().flatten() {
                        if !from.contains(&b) {
                            for &a in &from {
                                self.edges.insert((a, b));
                            }
                        }
                    }
                }
                let mut all: Vec<usize> = children.into_values().flatten().collect();
                all.sort_unstable();
                all.dedup();
                Ok(all)
            }
        }
    }
}

/// Leaves that must finish before `child` counts as finished: its own
/// leaves, or for a leafless child, whatever must finish before it.
fn finish_leaves<'m>(
    child: &'m str,
    method: &'m pc_ir::Method,
    children: &BTreeMap<&str, Vec<usize>>,
    memo: &mut BTreeMap<&'m str, BTreeSet<usize>>,
) -> BTreeSet<usize> {
    if let Some(done) = memo.get(child) {
        return done.clone();
    }
    memo.insert(child, BTreeSet::new());
    let own = children.get(child).map(Vec::as_slice).unwrap_or_default();
    let result = if own.is_empty() {
        let mut relayed = BTreeSet::new();
        for edge in method.order_edges.iter().filter(|e| e.to == child) {
            relayed.extend(finish_leaves(&edge.from, method, children, memo));
        }
        relayed
    } else {
        own.iter().copied().collect()
    };
    memo.insert(child, result.clone());
    result
}

/// Analyze the accepted plan rooted at the objective task.
pub fn compile(doc: &PlanDoc) -> Result<CompiledPlan, CompileError> {
    compile_with_budget(doc, std::time::Duration::from_secs(2))
}

/// Analyze the selected plan, allowing callers to configure P6's time limit.
pub fn compile_with_budget(
    doc: &PlanDoc,
    selection_budget: std::time::Duration,
) -> Result<CompiledPlan, CompileError> {
    let accepted: Vec<_> = doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Accepted)
        .collect();
    // P5 folds evidence into effective estimates before selection, so
    // appending evidence can flip the chosen methods. Declared guesses alone
    // no longer decide.
    let estimated = crate::estimate::estimate(doc);
    let eff = estimated.effective_map();
    let mut selection_tasks = Vec::with_capacity(doc.task.len());
    for task in &doc.task {
        let skipped = matches!(
            task.state,
            pc_ir::TaskState::Done | pc_ir::TaskState::Dropped
        );
        let mut methods: Vec<_> = accepted
            .iter()
            .filter(|m| !skipped && m.task == task.id)
            .map(|m| {
                let costs = m
                    .capability
                    .as_deref()
                    .map(|id| {
                        eff.iter()
                            .filter(|((cap, _), _)| cap == id)
                            .map(|((_, metric), e)| (metric.clone(), e.expected))
                            .collect()
                    })
                    .unwrap_or_default();
                SelectionMethod {
                    id: m.id.clone(),
                    costs,
                    children: m.subtasks.clone(),
                }
            })
            .collect();
        methods.sort_by(|a: &SelectionMethod, b| a.id.cmp(&b.id));
        for method in &mut methods {
            method.children.sort();
            method.children.dedup();
        }
        selection_tasks.push(SelectionTask {
            id: task.id.clone(),
            methods,
        });
    }
    selection_tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let mut constraints = SelectionConstraints {
        weights: doc.objective.weights.clone(),
        ..SelectionConstraints::default()
    };
    if let Some(budget) = doc.objective.budget_usd {
        constraints.limits.insert("money_usd".into(), budget);
    }
    if let Some(floor) = doc.objective.quality_floor.filter(|floor| *floor > 0.0) {
        constraints
            .limits
            .insert("quality_loss".into(), -floor.ln());
    }
    let mut ineligible: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    for selection_task in &mut selection_tasks {
        for method in &mut selection_task.methods {
            let Some(source) = doc
                .method
                .iter()
                .find(|candidate| candidate.id == method.id)
            else {
                continue;
            };
            if source.kind != MethodKind::Invoke {
                continue;
            }
            let Some(capability) = source.capability.as_deref() else {
                continue;
            };
            // Presence is effective presence: measured evidence alone makes
            // a capability eligible under a hard constraint.
            if doc.objective.budget_usd.is_some()
                && !eff.contains_key(&(capability.to_string(), "money_usd".to_string()))
            {
                ineligible
                    .entry((selection_task.id.clone(), method.id.clone()))
                    .or_default()
                    .push("missing money_usd estimate under budget_usd");
            }
            if doc.objective.quality_floor.is_some_and(|floor| floor > 0.0) {
                if let Some(risk) = eff
                    .get(&(capability.to_string(), "failure_risk".to_string()))
                    .map(|e| e.expected)
                {
                    method.costs.insert(
                        "quality_loss".into(),
                        if risk >= 1.0 {
                            1_000_000.0
                        } else {
                            -(1.0 - risk).ln()
                        },
                    );
                } else {
                    ineligible
                        .entry((selection_task.id.clone(), method.id.clone()))
                        .or_default()
                        .push("missing failure_risk estimate under quality_floor");
                }
            }
        }
    }
    constraints
        .forbidden_methods
        .extend(ineligible.keys().cloned());
    let selection = pc_select::select(
        &selection_tasks,
        &doc.objective.root_task,
        &constraints,
        selection_budget,
    )
    .map_err(|error| match error {
        pc_select::SelectError::Infeasible => CompileError::SelectionInfeasible,
        other => CompileError::SelectionFailed {
            message: other.to_string(),
        },
    })?;
    let methods: BTreeMap<&str, Vec<_>> = selection
        .methods
        .iter()
        .filter_map(|(task, method)| {
            doc.method
                .iter()
                .find(|m| m.id == *method)
                .map(|m| (task.as_str(), vec![m]))
        })
        .collect();
    let mut walk = Walk {
        tasks: doc.task.iter().map(|t| (t.id.as_str(), t)).collect(),
        methods,
        effective: {
            let mut map: BTreeMap<String, BTreeMap<String, EstimateRange>> = BTreeMap::new();
            for ((cap, metric), e) in &eff {
                map.entry(cap.clone()).or_default().insert(
                    metric.clone(),
                    EstimateRange {
                        low: e.low,
                        likely: e.likely,
                        high: e.high,
                        expected: e.expected,
                    },
                );
            }
            map
        },
        predicted: {
            let mut map: BTreeMap<String, BTreeMap<String, EstimateRange>> = BTreeMap::new();
            for profile in &estimated.profiles {
                if let Some(d) = &profile.declared {
                    map.entry(profile.capability.clone()).or_default().insert(
                        profile.metric.clone(),
                        EstimateRange {
                            low: d.low,
                            likely: d.likely,
                            high: d.high,
                            expected: (d.low + 4.0 * d.likely + d.high) / 6.0,
                        },
                    );
                }
            }
            map
        },
        measured: estimated.measured_set(),
        nodes: Vec::new(),
        edges: BTreeSet::new(),
        unresolved: BTreeSet::new(),
        trace: Vec::new(),
        leaves: BTreeMap::new(),
    };
    walk.task(&doc.objective.root_task)?;
    let Walk {
        nodes,
        edges,
        unresolved,
        mut trace,
        measured,
        ..
    } = walk;
    trace.push(CompileTrace {
        subject: "plan".into(),
        decision: selection_status(&selection.optimality),
        reason: format!(
            "P6 weighted objective {:.6}; {:?} time budget",
            selection.objective, selection.time_budget
        ),
    });
    for method in doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Accepted)
    {
        let Some(chosen) = selection.methods.get(&method.task) else {
            continue;
        };
        if *chosen == method.id {
            continue;
        }
        let reason = match ineligible.get(&(method.task.clone(), method.id.clone())) {
            Some(reasons) => format!("ineligible: {}", reasons.join("; ")),
            None => format!("P6 selected method:{chosen} for task:{}", method.task),
        };
        trace.push(CompileTrace {
            subject: format!("method:{}", method.id),
            decision: "not selected".into(),
            reason,
        });
    }
    // Effective roll-ups drive the plan; predicted (declared-only) roll-ups
    // stay beside them so every total says how much of it rests on guesses.
    let mut totals = rollup(&nodes, |n| &n.estimates);
    let predicted_totals = rollup(&nodes, |n| &n.predicted);
    let (duration, timings, width) = cpm(&nodes, &edges)?;
    let makespan = duration.expected;
    if !nodes.is_empty() {
        totals.insert("duration_minutes".into(), duration);
    }
    // Same graph, declared durations: only the makespan is kept, as the
    // predicted column beside the effective makespan.
    let (predicted_duration, _, _) = cpm_with(&nodes, &edges, |n| &n.predicted_duration)?;
    let mut predicted_full = predicted_totals;
    if !nodes.is_empty() {
        predicted_full.insert("duration_minutes".into(), predicted_duration);
    }
    let mut basis = BTreeMap::new();
    for metric in totals.keys() {
        let total_nodes = nodes
            .iter()
            .filter(|n| n.estimates.contains_key(metric))
            .count();
        let measured_nodes = nodes
            .iter()
            .filter(|n| {
                n.estimates.contains_key(metric)
                    && measured.contains(&(n.capability.clone(), metric.clone()))
            })
            .count();
        basis.insert(
            metric.clone(),
            MetricBasis {
                predicted: predicted_full.get(metric).copied().unwrap_or(zero()),
                effective: totals[metric],
                measured_nodes,
                total_nodes,
            },
        );
    }
    for (task, timing) in &timings {
        trace.push(CompileTrace {
            subject: format!("task:{task}"),
            decision: if timing.critical {
                "critical"
            } else {
                "has slack"
            }
            .into(),
            reason: format!(
                "slack {:.3} minutes; earliest {:.3}-{:.3}, latest {:.3}-{:.3}",
                timing.slack,
                timing.earliest_start,
                timing.earliest_finish,
                timing.latest_start,
                timing.latest_finish
            ),
        });
    }
    trace.push(CompileTrace {
        subject: "plan".into(),
        decision: "analyzed".into(),
        reason: format!("P7 CPM makespan {makespan:.3} minutes; parallel width {width}"),
    });
    // Provenance report: every measured profile plus every used
    // capability's metrics, sorted by (capability, metric) as estimated.
    let selected_caps: BTreeSet<String> = selection
        .methods
        .values()
        .filter_map(|method_id| {
            doc.method
                .iter()
                .find(|m| &m.id == method_id)
                .and_then(|m| m.capability.clone())
        })
        .collect();
    let mut provenance = Vec::new();
    for profile in &estimated.profiles {
        let selected = selected_caps.contains(&profile.capability);
        if profile.measured.is_none() && !selected {
            continue;
        }
        let observations = profile.measured.map_or(0, |m| m.observations);
        provenance.push(ProvenanceRow {
            capability: profile.capability.clone(),
            metric: profile.metric.clone(),
            predicted: profile.declared.map(|d| EstimateRange {
                low: d.low,
                likely: d.likely,
                high: d.high,
                expected: (d.low + 4.0 * d.likely + d.high) / 6.0,
            }),
            measured: profile.measured.map(|m| EstimateRange {
                low: m.low,
                likely: m.likely,
                high: m.high,
                expected: m.expected,
            }),
            observations,
            stale: profile.measured.map_or(0, |m| m.stale),
            contradicts_declared: profile.contradicts_declared,
            effective: profile.effective.map_or(zero(), |e| EstimateRange {
                low: e.low,
                likely: e.likely,
                high: e.high,
                expected: e.expected,
            }),
            basis: if profile.measured.is_some() {
                format!("measured({observations})")
            } else {
                "declared".to_string()
            },
            selected,
        });
    }
    for row in &provenance {
        let Some(measured) = &row.measured else {
            continue;
        };
        let subject = format!("capability:{}", row.capability);
        trace.push(CompileTrace {
            subject: subject.clone(),
            decision: "measured".to_string(),
            reason: format!(
                "P5 median {:.3} over {} observations for `{}` (min {:.3}, max {:.3})",
                measured.likely, row.observations, row.metric, measured.low, measured.high,
            ),
        });
        if row.stale > 0 {
            trace.push(CompileTrace {
                subject: subject.clone(),
                decision: "stale".to_string(),
                reason: format!(
                    "P5 found {} of {} observations for `{}` older than {} days before the newest",
                    row.stale,
                    row.observations,
                    row.metric,
                    crate::estimate::STALE_AFTER_DAYS,
                ),
            });
        }
        if row.contradicts_declared {
            trace.push(CompileTrace {
                subject,
                decision: "contradicts".to_string(),
                reason: format!(
                    "P5 measured median {:.3} for `{}` falls outside the declared range",
                    measured.likely, row.metric,
                ),
            });
        }
    }
    Ok(CompiledPlan {
        root_task: doc.objective.root_task.clone(),
        totals,
        basis,
        provenance,
        unattributed_tasks: estimated.unattributed_tasks,
        makespan_minutes: makespan,
        parallel_width: width,
        timings,
        unresolved_tasks: unresolved.into_iter().collect(),
        trace,
        selection_status: selection_status(&selection.optimality),
        selection_time_budget_ms: selection.time_budget.as_millis() as u64,
    })
}

fn selection_status(status: &Optimality) -> String {
    match status {
        Optimality::Optimal => "optimal".into(),
        Optimality::BestFound {
            gap_percent: Some(gap),
        } => format!("best-found (gap {gap:.2}%)"),
        Optimality::BestFound { gap_percent: None } => "best-found (gap unavailable)".into(),
        Optimality::GreedyOnly => "greedy-only".into(),
    }
}

/// Roll up one estimate map per node across the selected leaves, skipping
/// `duration_minutes` (which takes the CPM makespan instead). Money sums
/// and failure risk combines independent task risks.
fn rollup(
    nodes: &[Node],
    pick: impl Fn(&Node) -> &BTreeMap<String, EstimateRange>,
) -> BTreeMap<String, EstimateRange> {
    let mut totals = BTreeMap::new();
    let metrics: BTreeSet<_> = nodes
        .iter()
        .flat_map(|n| pick(n).keys())
        .filter(|m| *m != "duration_minutes")
        .collect();
    for metric in metrics {
        let vals: Vec<_> = nodes
            .iter()
            .filter_map(|n| pick(n).get(metric).copied())
            .collect();
        let combine = |field: fn(&EstimateRange) -> f64| {
            if metric == "failure_risk" {
                1.0 - vals.iter().map(|v| 1.0 - field(v)).product::<f64>()
            } else {
                vals.iter().map(field).sum()
            }
        };
        let (low, likely, high) = (
            combine(|v| v.low),
            combine(|v| v.likely),
            combine(|v| v.high),
        );
        totals.insert(
            metric.clone(),
            EstimateRange {
                low,
                likely,
                high,
                expected: (low + 4.0 * likely + high) / 6.0,
            },
        );
    }
    totals
}
fn zero() -> EstimateRange {
    EstimateRange {
        low: 0.0,
        likely: 0.0,
        high: 0.0,
        expected: 0.0,
    }
}

fn cpm(
    nodes: &[Node],
    edges: &BTreeSet<(usize, usize)>,
) -> Result<(EstimateRange, BTreeMap<String, TaskTiming>, usize), CompileError> {
    cpm_with(nodes, edges, |n| &n.duration)
}

fn cpm_with(
    nodes: &[Node],
    edges: &BTreeSet<(usize, usize)>,
    pick: impl Fn(&Node) -> &EstimateRange,
) -> Result<(EstimateRange, BTreeMap<String, TaskTiming>, usize), CompileError> {
    let n = nodes.len();
    let mut pred = vec![Vec::new(); n];
    let mut succ = vec![Vec::new(); n];
    for &(a, b) in edges {
        succ[a].push(b);
        pred[b].push(a);
    }
    let mut indegree: Vec<_> = pred.iter().map(Vec::len).collect();
    let mut ready: BTreeSet<(String, usize)> = (0..n)
        .filter(|&i| indegree[i] == 0)
        .map(|i| (nodes[i].task.clone(), i))
        .collect();
    let mut order = Vec::with_capacity(n);
    while let Some((_, i)) = ready.pop_first() {
        order.push(i);
        for &j in &succ[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.insert((nodes[j].task.clone(), j));
            }
        }
    }
    if order.len() != n {
        let mut tasks: Vec<_> = (0..n)
            .filter(|&i| indegree[i] > 0)
            .map(|i| nodes[i].task.clone())
            .collect();
        tasks.sort();
        return Err(CompileError::PrecedenceCycle { tasks });
    }
    let schedule = |duration: fn(&EstimateRange) -> f64| {
        let mut es = vec![0.0; n];
        let mut ef = vec![0.0; n];
        for &i in &order {
            es[i] = pred[i].iter().map(|p| ef[*p]).fold(0.0, f64::max);
            ef[i] = es[i] + duration(pick(&nodes[i]));
        }
        let span = ef.iter().copied().fold(0.0, f64::max);
        (es, ef, span)
    };
    let (_, _, low) = schedule(|d| d.low);
    let (_, _, likely) = schedule(|d| d.likely);
    let (_, _, high) = schedule(|d| d.high);
    let (es, ef, makespan) = schedule(|d| d.expected);
    let duration = EstimateRange {
        low,
        likely,
        high,
        expected: makespan,
    };
    let mut lf = vec![makespan; n];
    let mut ls = vec![0.0; n];
    for &i in order.iter().rev() {
        lf[i] = if succ[i].is_empty() {
            makespan
        } else {
            succ[i].iter().map(|s| ls[*s]).fold(f64::INFINITY, f64::min)
        };
        ls[i] = lf[i] - pick(&nodes[i]).expected;
    }
    let mut timing = BTreeMap::new();
    for i in 0..n {
        let slack = (ls[i] - es[i]).max(0.0);
        timing.insert(
            nodes[i].task.clone(),
            TaskTiming {
                earliest_start: es[i],
                earliest_finish: ef[i],
                latest_start: ls[i],
                latest_finish: lf[i],
                slack,
                critical: slack < 1e-9,
            },
        );
    }
    let mut points = Vec::new();
    for i in 0..n {
        points.push((es[i], 1i32));
        points.push((ef[i], -1));
    }
    points.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut active = 0i32;
    let mut width = 0usize;
    for (_, delta) in points {
        active += delta;
        width = width.max(active.max(0) as usize);
    }
    Ok((duration, timing, width))
}
