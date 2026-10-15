//! Representative benchmark suite for the planning compiler (PC-16).
//!
//! Plans are generated deterministically in memory: no large fixtures are
//! committed. Each scenario measures parsing, P1-P5 validation, P6
//! selection, P7 scheduling roll-up, P8 frontier ranking, peak RSS,
//! the solver optimality status/gap, and the cost of one small incremental
//! update (append evidence, re-check, re-compile).
//!
//! The suite never optimises anything (that is PC-17) and never writes
//! `metrics.jsonl`: every measurement runs in memory through the same
//! passes the instrumented CLI records.

use pc_ir::{
    Capability, CapabilityKind, Estimate, EstimateProvenance, Evidence, Method, MethodKind,
    MethodStatus, Objective, PlanDoc, PlanMeta, Task, TaskState,
};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Plan shape stressed by a scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScenarioShape {
    /// One root decompose over all leaves, no order edges (max parallelism).
    Flat,
    /// Two-level DAG: the root decomposes into mid tasks that share leaves,
    /// so P6 charges shared descendants once.
    Shared,
    /// One root decompose with a dense but acyclic order-edge set (chain
    /// plus skip-one edges) over its subtasks.
    Dense,
    /// One root decompose whose subtasks each precede the next
    /// `floor(sqrt(n))` subtasks: about `n * sqrt(n)` order edges, denser
    /// than linear.
    Mesh,
}

impl ScenarioShape {
    /// Short label used in scenario names and reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Shared => "shared",
            Self::Dense => "dense",
            Self::Mesh => "mesh",
        }
    }
}

/// One benchmark scenario: leaf count, alternatives per leaf, shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioSpec {
    /// Stable scenario name, e.g. `"flat-100"`.
    pub name: &'static str,
    /// Number of executable leaf tasks.
    pub leaves: usize,
    /// Accepted invoke alternatives per leaf (P6 chooses one).
    pub alternatives: usize,
    /// Plan shape.
    pub shape: ScenarioShape,
    /// True when the scenario is opt-in only (shared machine: the captain
    /// forbids synthetic load by default).
    pub large: bool,
}

/// Scenarios the suite knows, smallest first.
#[must_use]
pub fn all_scenarios() -> Vec<ScenarioSpec> {
    vec![
        ScenarioSpec {
            name: "flat-10",
            leaves: 10,
            alternatives: 3,
            shape: ScenarioShape::Flat,
            large: false,
        },
        ScenarioSpec {
            name: "flat-100",
            leaves: 100,
            alternatives: 3,
            shape: ScenarioShape::Flat,
            large: false,
        },
        ScenarioSpec {
            name: "shared-100",
            leaves: 100,
            alternatives: 3,
            shape: ScenarioShape::Shared,
            large: false,
        },
        ScenarioSpec {
            name: "dense-100",
            leaves: 100,
            alternatives: 3,
            shape: ScenarioShape::Dense,
            large: false,
        },
        ScenarioSpec {
            name: "mesh-100",
            leaves: 100,
            alternatives: 3,
            shape: ScenarioShape::Mesh,
            large: false,
        },
        ScenarioSpec {
            name: "alts-100",
            leaves: 100,
            alternatives: 16,
            shape: ScenarioShape::Flat,
            large: false,
        },
        ScenarioSpec {
            name: "flat-1000",
            leaves: 1000,
            alternatives: 3,
            shape: ScenarioShape::Flat,
            large: false,
        },
        ScenarioSpec {
            name: "shared-1000",
            leaves: 1000,
            alternatives: 3,
            shape: ScenarioShape::Shared,
            large: false,
        },
        ScenarioSpec {
            name: "dense-1000",
            leaves: 1000,
            alternatives: 3,
            shape: ScenarioShape::Dense,
            large: false,
        },
        ScenarioSpec {
            name: "mesh-1000",
            leaves: 1000,
            alternatives: 3,
            shape: ScenarioShape::Mesh,
            large: false,
        },
        ScenarioSpec {
            name: "alts-1000",
            leaves: 1000,
            alternatives: 16,
            shape: ScenarioShape::Flat,
            large: false,
        },
        ScenarioSpec {
            name: "flat-10000",
            leaves: 10_000,
            alternatives: 3,
            shape: ScenarioShape::Flat,
            large: true,
        },
    ]
}

/// Scenarios `pc bench` runs by default: small enough for a shared 24 GB
/// machine. Everything else needs `--all`; `flat-10000` additionally needs
/// `--include-large`.
#[must_use]
pub fn default_scenarios() -> Vec<ScenarioSpec> {
    all_scenarios()
        .into_iter()
        .filter(|spec| spec.leaves <= 100)
        .collect()
}

/// Look up one scenario by name.
#[must_use]
pub fn scenario_by_name(name: &str) -> Option<ScenarioSpec> {
    all_scenarios().into_iter().find(|spec| spec.name == name)
}

/// Minimum number of distinct capabilities the generator cycles through.
/// Bounded so capability count stays flat while task count scales; a
/// scenario with more alternatives per leaf widens the pool to match, so no
/// two alternatives of one leaf invoke the same capability.
pub const CAPABILITY_POOL: usize = 8;

/// Build a deterministic benchmark plan: one root, accepted methods only,
/// empty typed contracts (so E008/E009 are vacuous), unique leaf titles (so
/// P4 merges nothing except the structural sharing the `Shared` shape
/// constructs on purpose).
#[must_use]
pub fn generate(spec: &ScenarioSpec) -> PlanDoc {
    let pool = CAPABILITY_POOL.max(spec.alternatives);
    let mut capabilities = Vec::with_capacity(pool);
    for index in 0..pool {
        let low = 5.0 + (index % 5) as f64;
        let likely = 10.0 + (index % 7) as f64;
        let high = 15.0 + (index % 11) as f64 + likely - 10.0;
        capabilities.push(Capability {
            id: format!("bench-cap-{index}"),
            kind: CapabilityKind::Tool,
            version: "1".into(),
            provider: Some("bench".into()),
            source: Some("generated".into()),
            platform_requirements: Vec::new(),
            licensing_constraints: Vec::new(),
            consumes: Vec::new(),
            produces: Vec::new(),
            estimates: vec![Estimate {
                metric: "duration_minutes".into(),
                low,
                likely,
                high,
                provenance: EstimateProvenance::Declared,
                observations: None,
                window: None,
            }],
            resources: Vec::new(),
        });
    }

    let mut tasks = Vec::new();
    let mut methods = Vec::new();

    for leaf in 0..spec.leaves {
        let id = format!("t-{leaf:06}");
        tasks.push(Task {
            id: id.clone(),
            title: format!("bench leaf {leaf} {}", spec.name),
            state: TaskState::Open,
            consumes: Vec::new(),
            produces: Vec::new(),
            tasks_axi: None,
            beads_jsonl: None,
        });
        for alt in 0..spec.alternatives {
            methods.push(Method {
                id: format!("m-{leaf:06}-{alt}"),
                task: id.clone(),
                kind: MethodKind::Invoke,
                subtasks: Vec::new(),
                order_edges: Vec::new(),
                capability: Some(format!("bench-cap-{}", (leaf + alt) % pool)),
                status: MethodStatus::Accepted,
                provenance: "bench:generator".into(),
                assumptions: Vec::new(),
            });
        }
    }

    // Mid layer for the shared shape: mids tile the leaves every `step`
    // leaves with a `2 * step` window (wrapping), so every leaf sits under
    // at least two mids whenever there is more than one mid.
    let mut root_subtasks: Vec<String> = Vec::new();
    if spec.shape == ScenarioShape::Shared {
        let mids = (spec.leaves / 10).max(1);
        let step = spec.leaves.div_ceil(mids).max(1);
        let window = 2 * step;
        for mid in 0..mids {
            let mid_id = format!("g-{mid:04}");
            let mut subtasks = Vec::new();
            let start = (mid * step) % spec.leaves;
            for offset in 0..window {
                subtasks.push(format!("t-{:06}", (start + offset) % spec.leaves));
            }
            subtasks.sort();
            subtasks.dedup();
            tasks.push(Task {
                id: mid_id.clone(),
                title: format!("bench group {mid} {}", spec.name),
                state: TaskState::Decomposed,
                consumes: Vec::new(),
                produces: Vec::new(),
                tasks_axi: None,
                beads_jsonl: None,
            });
            methods.push(Method {
                id: format!("m-{mid_id}-decomp"),
                task: mid_id.clone(),
                kind: MethodKind::Decompose,
                subtasks,
                order_edges: Vec::new(),
                capability: None,
                status: MethodStatus::Accepted,
                provenance: "bench:generator".into(),
                assumptions: Vec::new(),
            });
            root_subtasks.push(mid_id);
        }
    } else {
        root_subtasks = (0..spec.leaves)
            .map(|leaf| format!("t-{leaf:06}"))
            .collect();
    }

    let mut order_edges = Vec::new();
    if spec.shape == ScenarioShape::Mesh {
        let reach = root_subtasks.len().isqrt().max(1);
        for (index, from) in root_subtasks.iter().enumerate() {
            for to in root_subtasks.iter().skip(index + 1).take(reach) {
                order_edges.push(pc_ir::OrderEdge {
                    from: from.clone(),
                    to: to.clone(),
                });
            }
        }
    }
    if spec.shape == ScenarioShape::Dense {
        // Chain plus skip-one edges: dense but acyclic by construction.
        for window in root_subtasks.windows(2) {
            order_edges.push(pc_ir::OrderEdge {
                from: window[0].clone(),
                to: window[1].clone(),
            });
        }
        for window in root_subtasks.windows(3) {
            order_edges.push(pc_ir::OrderEdge {
                from: window[0].clone(),
                to: window[2].clone(),
            });
        }
    }

    tasks.push(Task {
        id: "root".into(),
        title: format!("bench root {}", spec.name),
        state: TaskState::Decomposed,
        consumes: Vec::new(),
        produces: Vec::new(),
        tasks_axi: None,
        beads_jsonl: None,
    });
    methods.push(Method {
        id: "m-root".into(),
        task: "root".into(),
        kind: MethodKind::Decompose,
        subtasks: root_subtasks,
        order_edges,
        capability: None,
        status: MethodStatus::Accepted,
        provenance: "bench:generator".into(),
        assumptions: Vec::new(),
    });

    let mut weights = std::collections::BTreeMap::new();
    weights.insert("duration_minutes".to_string(), 1.0);

    PlanDoc {
        ir_version: pc_ir::IR_VERSION,
        plan: PlanMeta {
            id: format!("bench-{}", spec.name),
            title: format!("benchmark {}", spec.name),
            description: format!(
                "generated {}-task {} benchmark ({} alternatives per leaf)",
                spec.leaves,
                spec.shape.label(),
                spec.alternatives,
            ),
            external_inputs: Vec::new(),
            tasks_axi_sections: Vec::new(),
        },
        objective: Objective {
            root_task: "root".into(),
            weights,
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            platforms: Vec::new(),
            licenses: Vec::new(),
            acceptance: Vec::new(),
        },
        artifact: Vec::new(),
        resource: Vec::new(),
        task: tasks,
        method: methods,
        capability: capabilities,
        evidence: Vec::new(),
        // No investigations: generated plans measure the execution passes,
        // and the empty default keeps P8 discovery lists deterministically
        // empty across scenarios.
        investigation: Vec::new(),
    }
}

/// One scenario's measured costs. Durations are whole milliseconds; memory
/// is the peak RSS of the process that ran the scenario (see
/// [`peak_rss_bytes`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Scenario name.
    pub scenario: String,
    /// Leaf count requested.
    pub leaves: usize,
    /// Shape label.
    pub shape: String,
    /// Total tasks generated (leaves plus root/mids).
    pub task_count: usize,
    /// Total methods generated.
    pub method_count: usize,
    /// Total capabilities generated.
    pub capability_count: usize,
    /// `pc_ir::from_toml` over the generated TOML.
    pub parse_ms: u64,
    /// P1-P5 `check` over the parsed document.
    pub validation_ms: u64,
    /// Whole `compile_with_objective_and_selection_duration` call.
    pub compile_ms: u64,
    /// P6 selection solver time (inside the compile call).
    pub selection_ms: u64,
    /// Compile time outside P6 (P7 schedule analysis plus roll-ups).
    pub schedule_ms: u64,
    /// P8 `frontier` over the compiled plan.
    pub frontier_ms: u64,
    /// Re-check plus re-compile after appending one evidence record.
    pub incremental_ms: u64,
    /// Peak process RSS over the whole scenario, when available.
    pub memory_bytes: Option<u64>,
    /// P6 optimality status (`optimal`, `best-found`, `greedy-only`).
    pub optimality_status: String,
    /// Solver gap percent, when the solver reports one.
    pub optimality_gap_percent: Option<f64>,
    /// P6 time budget the run preserved.
    pub selection_time_budget_ms: u64,
}

/// Peak resident set size of this process so far, in bytes, from
/// `getrusage(RUSAGE_SELF)`. It never decreases, so callers that want one
/// peak per scenario run each scenario in its own process (`pc bench` does).
/// Returns `None` off Unix or when the probe fails.
#[must_use]
pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(unix)]
    {
        let usage =
            nix::sys::resource::getrusage(nix::sys::resource::UsageWho::RUSAGE_SELF).ok()?;
        let max_rss = u64::try_from(usage.max_rss()).ok()?;
        if cfg!(target_vendor = "apple") {
            Some(max_rss)
        } else {
            max_rss.checked_mul(1024)
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Parse, check, compile and rank one generated scenario, preserving the
/// given P6 solver time budget. Returns an error string when the generated
/// plan fails to check or compile (a generator defect, never a perf signal).
pub fn run_scenario(
    spec: &ScenarioSpec,
    selection_budget: Duration,
) -> Result<BenchmarkResult, String> {
    let doc = generate(spec);
    let task_count = doc.task.len();
    let method_count = doc.method.len();
    let capability_count = doc.capability.len();
    let toml_text = pc_ir::to_toml(&doc).map_err(|e| format!("generator TOML error: {e}"))?;

    let started = Instant::now();
    let parsed: PlanDoc =
        pc_ir::from_toml(&toml_text).map_err(|e| format!("generator parse error: {e}"))?;
    let parse_ms = started.elapsed().as_millis() as u64;

    let started = Instant::now();
    let mut checked = parsed.clone();
    let report = pc_core::passes::check("bench.toml", &toml_text, &mut checked, None);
    let validation_ms = started.elapsed().as_millis() as u64;
    if !report.ok() {
        let first = report
            .diagnostics
            .first()
            .map_or("unknown".to_string(), |d| {
                format!("{} {}", d.code, d.message)
            });
        return Err(format!(
            "generated plan {} failed check: {first}",
            spec.name
        ));
    }

    let started = Instant::now();
    let (compiled, selection) = pc_core::compile_with_objective_and_selection_duration(
        &checked,
        selection_budget,
        pc_core::Objective::TotalEffort,
    )
    .map_err(|e| format!("generated plan {} failed compile: {e}", spec.name))?;
    let compile_ms = started.elapsed().as_millis() as u64;
    let selection_ms = selection.duration.as_millis() as u64;
    let schedule_ms = compile_ms.saturating_sub(selection_ms);

    let started = Instant::now();
    let _frontier = pc_core::frontier(&checked, &compiled);
    let frontier_ms = started.elapsed().as_millis() as u64;

    // Frequent small change: one fresh evidence record on the first used
    // capability, then re-check plus re-compile.
    let started = Instant::now();
    let mut rechecked = parsed;
    if let Some(capability) = doc.capability.first() {
        rechecked.evidence.push(Evidence {
            subject: format!("capability:{}", capability.id),
            metric: "duration_minutes".into(),
            value: 9.5,
            unit: "minutes".into(),
            time: "2026-10-13T12:00:00Z".into(),
            source: "bench".into(),
            basis: Some("absolute".into()),
            artifact_hash: None,
        });
    }
    let re_report = pc_core::passes::check("bench.toml", &toml_text, &mut rechecked, None);
    if !re_report.ok() {
        let first = re_report
            .diagnostics
            .first()
            .map_or("unknown".to_string(), |d| {
                format!("{} {}", d.code, d.message)
            });
        return Err(format!(
            "generated plan {} failed incremental re-check: {first}",
            spec.name
        ));
    }
    let _recompiled = pc_core::compile_with_objective_and_selection_duration(
        &rechecked,
        selection_budget,
        pc_core::Objective::TotalEffort,
    )
    .map_err(|e| {
        format!(
            "generated plan {} failed incremental re-compile: {e}",
            spec.name
        )
    })?;
    let incremental_ms = started.elapsed().as_millis() as u64;
    let memory_bytes = peak_rss_bytes();

    let (optimality_status, optimality_gap_percent) = match selection.optimality {
        pc_core::Optimality::Optimal => ("optimal".to_string(), None),
        pc_core::Optimality::BestFound { gap_percent } => ("best-found".to_string(), gap_percent),
        pc_core::Optimality::GreedyOnly => ("greedy-only".to_string(), None),
    };

    Ok(BenchmarkResult {
        scenario: spec.name.to_string(),
        leaves: spec.leaves,
        shape: spec.shape.label().to_string(),
        task_count,
        method_count,
        capability_count,
        parse_ms,
        validation_ms,
        selection_ms,
        schedule_ms,
        compile_ms,
        frontier_ms,
        incremental_ms,
        memory_bytes,
        optimality_status,
        optimality_gap_percent,
        selection_time_budget_ms: selection_budget.as_millis() as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Defect this would catch: generator emitting a plan P1-P2 refuses
    // (undefined refs, bad shapes, unknown metrics), which would turn every
    // benchmark into an error instead of a measurement.
    #[test]
    fn generated_flat_plan_checks_clean() {
        let spec = ScenarioSpec {
            name: "test-flat",
            leaves: 10,
            alternatives: 2,
            shape: ScenarioShape::Flat,
            large: false,
        };
        let doc = generate(&spec);
        let text = pc_ir::to_toml(&doc).expect("generator TOML serializes");
        let mut parsed: PlanDoc = pc_ir::from_toml(&text).expect("generator TOML parses");
        let report = pc_core::passes::check("bench.toml", &text, &mut parsed, None);
        assert!(
            report.ok(),
            "expected clean check: {:?}",
            report.diagnostics
        );
    }

    // Defect this would catch: a shared-DAG generator whose mid windows
    // stop short of the leaf range, leaving orphan leaves the root never
    // reaches (the scenario would silently measure a smaller plan).
    #[test]
    fn shared_plan_puts_every_leaf_under_two_mids() {
        for name in ["shared-100", "shared-1000"] {
            let spec = scenario_by_name(name).expect("shared scenario exists");
            let doc = generate(&spec);
            let root = doc
                .method
                .iter()
                .find(|method| method.task == "root")
                .expect("root method");
            let mut parents: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for mid in &root.subtasks {
                let method = doc
                    .method
                    .iter()
                    .find(|method| &method.task == mid)
                    .expect("mid method");
                for leaf in &method.subtasks {
                    *parents.entry(leaf.as_str()).or_default() += 1;
                }
            }
            assert_eq!(parents.len(), spec.leaves, "{name}: every leaf reachable");
            assert!(
                parents.values().all(|&count| count >= 2),
                "{name}: every leaf shared by two mids"
            );
        }
    }

    // Defect this would catch: a "denser than linear" scenario that is
    // actually linear, cyclic, or names tasks outside the root's subtasks.
    #[test]
    fn mesh_plan_is_superlinear_and_checks_clean() {
        let spec = scenario_by_name("mesh-100").expect("mesh-100 exists");
        let doc = generate(&spec);
        let root = doc
            .method
            .iter()
            .find(|method| method.task == "root")
            .expect("root method");
        assert!(root.order_edges.len() >= spec.leaves * 9);
        let text = pc_ir::to_toml(&doc).expect("generator TOML serializes");
        let mut parsed: PlanDoc = pc_ir::from_toml(&text).expect("generator TOML parses");
        let report = pc_core::passes::check("bench.toml", &text, &mut parsed, None);
        assert!(
            report.ok(),
            "expected clean check: {:?}",
            report.diagnostics
        );
    }

    // Defect this would catch: a many-alternatives scenario whose
    // alternatives wrap around a smaller capability pool, so one leaf offers
    // cost-tied duplicates instead of distinct choices.
    #[test]
    fn alternatives_of_a_leaf_are_distinct_choices() {
        let spec = scenario_by_name("alts-100").expect("alts-100 exists");
        let doc = generate(&spec);
        let estimates: std::collections::BTreeMap<&str, (u64, u64, u64)> = doc
            .capability
            .iter()
            .map(|capability| {
                let estimate = &capability.estimates[0];
                (
                    capability.id.as_str(),
                    (
                        estimate.low.to_bits(),
                        estimate.likely.to_bits(),
                        estimate.high.to_bits(),
                    ),
                )
            })
            .collect();
        let mut per_leaf: std::collections::BTreeMap<&str, Vec<(u64, u64, u64)>> =
            std::collections::BTreeMap::new();
        for method in doc.method.iter().filter(|m| m.kind == MethodKind::Invoke) {
            let capability = method.capability.as_deref().expect("invoke capability");
            per_leaf
                .entry(method.task.as_str())
                .or_default()
                .push(estimates[capability]);
        }
        for (leaf, mut choices) in per_leaf {
            assert_eq!(choices.len(), spec.alternatives, "{leaf}");
            choices.sort_unstable();
            choices.dedup();
            assert_eq!(
                choices.len(),
                spec.alternatives,
                "{leaf}: duplicate choices"
            );
        }
    }

    // Defect this would catch: nondeterministic generation (hash-map
    // iteration, time input), which would make baselines uncomparable.
    #[test]
    fn generator_is_deterministic() {
        let spec = scenario_by_name("flat-10").expect("flat-10 exists");
        assert_eq!(generate(&spec), generate(&spec));
    }

    // Regression/equivalence check for the lazy source-line lookup used by
    // validation: diagnostics and object counts stay equal for every
    // generated workload, and release solver outcomes match the baseline.
    #[test]
    fn validation_preserves_benchmark_workload_results() {
        for spec in all_scenarios() {
            let generated = generate(&spec);
            let text = pc_ir::to_toml(&generated).expect("generator TOML serializes");
            let mut checked: PlanDoc = pc_ir::from_toml(&text).expect("generator TOML parses");
            let report = pc_core::passes::check("bench.toml", &text, &mut checked, None);

            assert!(
                report.diagnostics.is_empty(),
                "{} diagnostics: {:?}",
                spec.name,
                report.diagnostics
            );
            assert_eq!(
                report.counts.tasks,
                generated.task.len(),
                "{} tasks",
                spec.name
            );
            assert_eq!(
                report.counts.methods,
                generated.method.len(),
                "{} methods",
                spec.name
            );
            assert_eq!(
                report.counts.capabilities,
                generated.capability.len(),
                "{} capabilities",
                spec.name
            );
            assert_eq!(report.counts.errors, 0, "{} errors", spec.name);

            let (compiled, selection) = pc_core::compile_with_objective_and_selection_duration(
                &checked,
                Duration::from_millis(2000),
                pc_core::Objective::TotalEffort,
            )
            .unwrap_or_else(|error| panic!("{} compile: {error}", spec.name));
            assert_eq!(compiled.root_task, "root", "{} root task", spec.name);
            if !cfg!(debug_assertions) {
                let expected_optimality = if spec.large { "greedy-only" } else { "optimal" };
                let actual_optimality = match selection.optimality {
                    pc_core::Optimality::Optimal => "optimal",
                    pc_core::Optimality::BestFound { .. } => "best-found",
                    pc_core::Optimality::GreedyOnly => "greedy-only",
                };
                assert_eq!(
                    actual_optimality, expected_optimality,
                    "{} release selection changed",
                    spec.name
                );
            }
        }
    }

    // Defect this would catch: a runner that reports task counts or solver
    // status inconsistent with the generated plan (wrong doc measured).
    #[test]
    #[ignore = "runs a benchmark scenario; opt in with --ignored"]
    fn runner_reports_consistent_counts() {
        let spec = ScenarioSpec {
            name: "test-flat",
            leaves: 10,
            alternatives: 2,
            shape: ScenarioShape::Flat,
            large: false,
        };
        let expected = generate(&spec);
        let result = run_scenario(&spec, Duration::from_millis(2000)).expect("small scenario runs");
        assert_eq!(result.task_count, expected.task.len());
        assert_eq!(result.method_count, expected.method.len());
        assert_eq!(result.selection_time_budget_ms, 2000);
        assert!(!result.optimality_status.is_empty());
    }
}
