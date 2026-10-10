//! Deterministic analysis of the accepted portion of a Work IR plan.
//!
//! Step 2 deliberately does not choose between alternatives. A task is
//! compiled only when it has exactly one accepted method; proposed, derived,
//! and rejected methods are outside this pass.

use pc_ir::{Estimate, MethodKind, MethodStatus, PlanDoc};
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

/// Roll-ups and CPM timing for the reachable accepted plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledPlan {
    /// Root task id.
    pub root_task: String,
    /// Roll-up ranges by metric, with keys in lexical order.
    pub totals: BTreeMap<String, EstimateRange>,
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
    /// More than one accepted method exists for a reachable task.
    #[error("task `{task}` has multiple accepted methods; step 2 does not select alternatives")]
    Ambiguous {
        /// Task with multiple accepted methods.
        task: String,
    },
    /// Order edges across methods sharing subtasks form a precedence cycle.
    #[error("tasks {} form a precedence cycle across methods", .tasks.join(", "))]
    PrecedenceCycle {
        /// Executable tasks on or behind the cycle, in lexical order.
        tasks: Vec<String>,
    },
}

#[derive(Clone)]
struct Node {
    task: String,
    estimates: BTreeMap<String, EstimateRange>,
    duration: EstimateRange,
}

struct Walk<'a> {
    tasks: BTreeMap<&'a str, &'a pc_ir::Task>,
    methods: BTreeMap<&'a str, Vec<&'a pc_ir::Method>>,
    caps: BTreeMap<&'a str, &'a pc_ir::Capability>,
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
        if accepted.len() > 1 {
            return Err(CompileError::Ambiguous { task: id.into() });
        }
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
                let estimates = method
                    .capability
                    .as_deref()
                    .and_then(|c| self.caps.get(c).copied())
                    .map(|c| {
                        c.estimates
                            .iter()
                            .map(|e| (e.metric.clone(), range(e)))
                            .collect::<BTreeMap<_, _>>()
                    })
                    .unwrap_or_default();
                let duration = estimates.get("duration_minutes").copied().unwrap_or(zero());
                let n = self.nodes.len();
                self.nodes.push(Node {
                    task: task.id.clone(),
                    estimates,
                    duration,
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
    let mut methods: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for m in doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Accepted)
    {
        methods.entry(m.task.as_str()).or_default().push(m);
    }
    let mut walk = Walk {
        tasks: doc.task.iter().map(|t| (t.id.as_str(), t)).collect(),
        methods,
        caps: doc.capability.iter().map(|c| (c.id.as_str(), c)).collect(),
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
        ..
    } = walk;
    let mut totals = BTreeMap::new();
    let metrics: BTreeSet<_> = nodes
        .iter()
        .flat_map(|n| n.estimates.keys())
        .filter(|m| *m != "duration_minutes")
        .collect();
    for metric in metrics {
        let vals: Vec<_> = nodes
            .iter()
            .filter_map(|n| n.estimates.get(metric).copied())
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
    let (duration, timings, width) = cpm(&nodes, &edges)?;
    let makespan = duration.expected;
    if !nodes.is_empty() {
        totals.insert("duration_minutes".into(), duration);
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
    Ok(CompiledPlan {
        root_task: doc.objective.root_task.clone(),
        totals,
        makespan_minutes: makespan,
        parallel_width: width,
        timings,
        unresolved_tasks: unresolved.into_iter().collect(),
        trace,
    })
}

fn range(e: &Estimate) -> EstimateRange {
    EstimateRange {
        low: e.low,
        likely: e.likely,
        high: e.high,
        expected: (e.low + 4.0 * e.likely + e.high) / 6.0,
    }
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
            ef[i] = es[i] + duration(&nodes[i].duration);
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
        ls[i] = lf[i] - nodes[i].duration.expected;
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
