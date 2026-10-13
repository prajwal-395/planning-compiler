//! `pc`: the planning-compiler CLI.
//!
//! Exit codes follow the workspace contract: `0` means the plan checks
//! clean, `1` means diagnostics were found, `2` means the invocation itself
//! was wrong (reserved for clap usage errors).

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Deterministic compiler for work: validate plans, never execute them.
#[derive(Debug, Parser)]
#[command(name = "pc", version)]
struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    command: Commands,
}

/// Available commands. Step-6 commands rank the frontier and preview and
/// compare plans; the step-7 proposal loop stages agent decompositions and
/// decides them while the core stays model-free.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Scaffold a minimal valid plan directory (a `plan.toml` that passes
    /// `pc check`, and a `.gitignore` entry for the `.plan.toml.lock` that
    /// guarded MCP writes create). Refuses to overwrite an existing plan.
    Init {
        /// Directory to scaffold the plan in. Defaults to `.plan`, which
        /// every other command picks up when run without a path; the
        /// default also refuses when the current directory holds a plan.
        /// The plan file is created as `<dir>/plan.toml`.
        path: Option<PathBuf>,
        /// Emit the created plan path as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run passes P1-P5 over a plan file or directory.
    Check {
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the full report (diagnostics, counts, trace) as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Emit the canonical JSON Schema for plan files.
    Schema {
        /// Write to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Emit the agent-patch schema (`pc propose` input) instead.
        #[arg(long)]
        patch: bool,
        /// Emit the decision-event schema instead of the plan schema.
        #[arg(long, conflicts_with_all = ["patch", "outcome", "metrics"])]
        decision: bool,
        /// Emit the outcome-event schema instead of the plan schema.
        #[arg(long, conflicts_with_all = ["patch", "decision", "metrics"])]
        outcome: bool,
        /// Emit the compiler-run metrics schema instead of the plan schema.
        #[arg(long, conflicts_with_all = ["patch", "decision", "outcome"])]
        metrics: bool,
    },
    /// Roll up accepted methods and analyze the critical path.
    Compile {
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the full compiled plan as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
        /// What P6 minimizes: weighted total effort, or dependency-aware
        /// completion time.
        #[arg(long, value_enum, default_value_t = SelectionObjective::Effort)]
        objective: SelectionObjective,
    },
    /// Explain a task, method, or capability in the accepted plan.
    Explain {
        /// Entity id, optionally prefixed with task:, method:, or capability:.
        target: String,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit matching explanation records as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// Append one evidence observation to the plan's evidence log.
    Record {
        /// JSON file holding one evidence object.
        evidence: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
    },
    /// Append one decision and its prediction to decisions.jsonl.
    Decision {
        /// JSON file holding one decision event.
        event: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when present.
        path: Option<PathBuf>,
    },
    /// Read all decision events as JSON.
    Decisions { path: Option<PathBuf> },
    /// Append one observed result to outcomes.jsonl.
    Outcome {
        /// JSON file holding one outcome event.
        event: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when present.
        path: Option<PathBuf>,
    },
    /// Read all observed outcome events as JSON.
    Outcomes { path: Option<PathBuf> },
    /// Join decisions with observed outcomes and report their evaluation.
    Evaluate {
        /// Only evaluate this decision id; defaults to all decisions.
        #[arg(long)]
        decision: Option<String>,
        /// Plan file or directory. Defaults to `.plan` when present.
        path: Option<PathBuf>,
    },
    /// Summarize decision classifications and outcome coverage.
    Metrics { path: Option<PathBuf> },
    /// Show decision and outcome events in timestamp order.
    History { path: Option<PathBuf> },
    /// Compare predictions with observed outcomes by metric.
    Calibration { path: Option<PathBuf> },
    /// Rank unresolved work (P8): open tasks, pending proposals,
    /// unmeasured critical-path cost and blocked alternatives.
    Frontier {
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the ranked frontier as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// List every capability with its version, provider, source and
    /// evidence coverage, flagging capabilities with no measurement evidence.
    Capabilities {
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the inventory report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Preview a hypothetical change (P9) without editing any files.
    Impact {
        /// JSON file holding the change (accept_method, add_evidence).
        change: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the impact report as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// Compare two plans (P9): structure, selection, totals and diagnostics.
    Diff {
        /// First plan file or directory.
        a: PathBuf,
        /// Second plan file or directory.
        b: PathBuf,
        /// Emit the diff report as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// Stage an agent patch: add tasks, methods and capabilities as
    /// proposed. The merged plan must check clean; nothing is written
    /// otherwise.
    Propose {
        /// JSON file holding the patch (add_task, add_method, add_capability).
        patch: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the staged operations as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Accept one pending method: flip a proposed or derived method to
    /// accepted. Refused when the merged plan would check dirty; preview
    /// such decisions with `pc impact` instead.
    Accept {
        /// Proposed or derived method id, optionally prefixed with method:.
        method: String,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the decision as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reject one pending method: flip a proposed or derived method to
    /// rejected.
    Reject {
        /// Proposed or derived method id, optionally prefixed with method:.
        method: String,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the decision as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Render the compact slice an agent needs to propose the next
    /// decomposition of one task: contracts, neighbours, evidence summary
    /// and frontier entries, fitted to a token budget.
    Context {
        /// Task id, optionally prefixed with task:.
        task: String,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
        /// Emit the fitted slice as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum slice size in tokens (chars/4 approximation).
        #[arg(long, default_value_t = pc_core::DEFAULT_CONTEXT_BUDGET_TOKENS)]
        budget: u64,
    },
    /// Import a tasks-axi `backlog.md` file into a plan directory.
    /// Backlog items become tasks (holds, kinds, repos, priorities, date
    /// gates and bodies ride along as passthrough); `blocked-by:` edges
    /// become order edges. Refuses to overwrite an existing plan.
    ImportTasksAxi {
        /// Backlog file to read.
        backlog: PathBuf,
        /// Plan directory to write `<dir>/plan.toml` into. Defaults to
        /// `.plan`.
        path: Option<PathBuf>,
    },
    /// Export a plan directory back into a tasks-axi `backlog.md` file.
    /// Imported passthrough renders back untouched, so an unedited plan
    /// regenerates its backlog byte-identical.
    ExportTasksAxi {
        /// Backlog file to write (overwritten).
        backlog: PathBuf,
        /// Plan file or directory. Defaults to `.plan` when that directory
        /// holds a plan file, otherwise the current directory.
        path: Option<PathBuf>,
    },
    /// Import Beads issue JSONL into a plan directory.
    ImportBeads {
        /// Beads JSONL file to read.
        jsonl: PathBuf,
        /// Plan directory to write `<dir>/plan.toml` into. Defaults to `.plan`.
        path: Option<PathBuf>,
    },
    /// Export a plan directory to Beads issue JSONL.
    ExportBeads {
        /// JSONL file to write (overwritten).
        jsonl: PathBuf,
        /// Plan file or directory.
        path: Option<PathBuf>,
    },
    /// Reconcile mapped external task state into the plan.
    Sync {
        /// One source to synchronize. Omit with --all.
        #[arg(long, conflicts_with = "all")]
        source: Option<String>,
        /// Synchronize all supported sources.
        #[arg(long)]
        all: bool,
        /// Show the reconciliation without changing plan or sync state.
        #[arg(long)]
        dry_run: bool,
        /// Snapshot file for tasks-axi or Beads (GitHub uses gh when omitted).
        #[arg(long, conflicts_with = "all")]
        input: Option<PathBuf>,
        /// Plan file or directory.
        path: Option<PathBuf>,
    },
}

/// Resolve an optional plan path: an explicit path passes through
/// untouched; otherwise `.plan` when that directory holds a plan file,
/// else `.`.
fn plan_path(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| {
        let default = PathBuf::from(".plan");
        if default.is_dir() && pc_store::resolve_plan_file(&default).is_ok() {
            default
        } else {
            PathBuf::from(".")
        }
    })
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { path, json } => run_init(path, json),
        Commands::Check { path, json } => run_check(&plan_path(path), json),
        Commands::Schema {
            out,
            patch,
            decision,
            outcome,
            metrics,
        } => run_schema(out.as_deref(), patch, decision, outcome, metrics),
        Commands::Compile {
            path,
            json,
            selection_time_budget_ms,
            objective,
        } => run_compile(&plan_path(path), json, selection_time_budget_ms, objective),
        Commands::Explain {
            target,
            path,
            json,
            selection_time_budget_ms,
        } => run_explain(&plan_path(path), &target, json, selection_time_budget_ms),
        Commands::Record { evidence, path } => run_record(&evidence, &plan_path(path)),
        Commands::Decision { event, path } => run_event_record(&event, &plan_path(path), true),
        Commands::Decisions { path } => run_event_read(&plan_path(path), true),
        Commands::Outcome { event, path } => run_event_record(&event, &plan_path(path), false),
        Commands::Outcomes { path } => run_event_read(&plan_path(path), false),
        Commands::Evaluate { decision, path } => {
            run_evaluation(&plan_path(path), decision.as_deref())
        }
        Commands::Metrics { path } => run_planning_metrics(&plan_path(path)),
        Commands::History { path } => run_event_history(&plan_path(path)),
        Commands::Calibration { path } => run_calibration(&plan_path(path)),
        Commands::Frontier {
            path,
            json,
            selection_time_budget_ms,
        } => run_frontier(&plan_path(path), json, selection_time_budget_ms),
        Commands::Capabilities { path, json } => run_capabilities(&plan_path(path), json),
        Commands::Impact {
            change,
            path,
            json,
            selection_time_budget_ms,
        } => run_impact(&change, &plan_path(path), json, selection_time_budget_ms),
        Commands::Diff {
            a,
            b,
            json,
            selection_time_budget_ms,
        } => run_diff(&a, &b, json, selection_time_budget_ms),
        Commands::Propose { patch, path, json } => run_propose(&patch, &plan_path(path), json),
        Commands::Accept { method, path, json } => {
            run_decide(&method, &plan_path(path), json, true)
        }
        Commands::Reject { method, path, json } => {
            run_decide(&method, &plan_path(path), json, false)
        }
        Commands::Context {
            task,
            path,
            json,
            budget,
        } => run_context(&task, &plan_path(path), json, budget),
        Commands::ImportTasksAxi { backlog, path } => run_import_tasks_axi(&backlog, path),
        Commands::ExportTasksAxi { backlog, path } => {
            run_export_tasks_axi(&backlog, &plan_path(path))
        }
        Commands::ImportBeads { jsonl, path } => run_import_beads(&jsonl, path),
        Commands::ExportBeads { jsonl, path } => run_export_beads(&jsonl, &plan_path(path)),
        Commands::Sync {
            source,
            all,
            dry_run,
            input,
            path,
        } => run_sync(source, all, dry_run, input, &plan_path(path)),
    }
}

/// Load and check a plan, exiting on failure. The duration covers the
/// P1-P5 check alone, so it compares with `pc check`'s validation timing.
fn load_checked(
    path: &std::path::Path,
) -> (
    pc_store::LoadedPlan,
    pc_core::passes::Report,
    std::time::Duration,
) {
    let mut loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let validation_started = std::time::Instant::now();
    let (doc, report) = loaded.check();
    let validation_duration = validation_started.elapsed();
    if !report.ok() {
        for diag in &report.diagnostics {
            eprintln!("{}", pc_core::render_human(diag));
        }
        std::process::exit(1);
    }
    // Compile and explain run over the passed document: P3 proposals are
    // visible as derived methods and P4 shares identical subtasks, so P6
    // counts shared work once. P6 still selects among accepted methods only.
    loaded.doc = doc;
    (loaded, report, validation_duration)
}

fn process_memory_bytes() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kib = String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

/// Measurements one CLI operation observed, bundled so the recorder
/// stays under the argument-count lint. Every field the operation could
/// not observe stays `None`.
struct RunObservation<'a> {
    report: &'a pc_core::passes::Report,
    compiled: Option<&'a pc_core::CompiledPlan>,
    selection: Option<&'a pc_core::SelectionObservation>,
    context_size_chars: Option<usize>,
    frontier_duration: Option<std::time::Duration>,
}

fn record_compiler_run(
    loaded: &pc_store::LoadedPlan,
    operation: &str,
    started: std::time::Instant,
    validation_duration: std::time::Duration,
    observation: RunObservation<'_>,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    if std::env::var_os("PC_METRICS").is_some_and(|value| value == "off") {
        return;
    }
    static NEXT_EVENT: AtomicU64 = AtomicU64::new(0);
    let doc = &loaded.doc;
    let RunObservation {
        report,
        compiled,
        selection,
        context_size_chars,
        frontier_duration,
    } = observation;
    let plan_file = std::path::Path::new(&loaded.display_path);
    let critical_path_tasks: Option<Vec<String>> = compiled.map(|result| {
        result
            .timings
            .iter()
            .filter(|(_, timing)| timing.critical)
            .map(|(task, _)| task.clone())
            .collect()
    });
    let critical_path_changes =
        compiled
            .zip(critical_path_tasks.as_ref())
            .and_then(|(result, current)| {
                let previous = pc_store::previous_critical_path(
                    plan_file,
                    &result.selection_objective,
                    result.selection_time_budget_ms,
                )?;
                Some(
                    current
                        .iter()
                        .filter(|task| !previous.contains(task))
                        .count()
                        + previous
                            .iter()
                            .filter(|task| !current.contains(task))
                            .count(),
                )
            });
    let decisions_affected_by_evidence = pc_store::read_decisions(plan_file)
        .ok()
        .map(|decisions| decisions_affected_by_evidence(doc, report, &decisions));
    let uncertainty = compiled.and_then(|result| {
        result
            .totals
            .get("duration_minutes")
            .map(|range| range.high - range.low)
    });
    let coverage = if doc.capability.is_empty() {
        None
    } else {
        let evidenced = doc
            .evidence
            .iter()
            .filter_map(|evidence| evidence.subject.strip_prefix("capability:"))
            .collect::<std::collections::BTreeSet<_>>();
        let covered = doc
            .capability
            .iter()
            .filter(|capability| evidenced.contains(capability.id.as_str()))
            .count();
        Some(covered as f64 / doc.capability.len() as f64)
    };
    let mut accepted_per_task = std::collections::BTreeMap::<&str, usize>::new();
    for method in &doc.method {
        if method.status == pc_ir::MethodStatus::Accepted {
            *accepted_per_task.entry(method.task.as_str()).or_default() += 1;
        }
    }
    let alternatives_considered = accepted_per_task.values().filter(|&&count| count > 1).sum();
    let order_edges: usize = doc
        .method
        .iter()
        .filter(|method| method.status != pc_ir::MethodStatus::Rejected)
        .map(|method| method.order_edges.len())
        .sum();
    let optimality_status = compiled.map(|result| result.selection_status.clone());
    let optimality_gap_percent = selection.and_then(|observation| match observation.optimality {
        pc_core::Optimality::BestFound { gap_percent } => gap_percent,
        pc_core::Optimality::Optimal | pc_core::Optimality::GreedyOnly => None,
    });
    let now = chrono::Utc::now();
    let event = pc_ir::CompilerRunEvent {
        event: pc_ir::EventMetadata {
            id: format!(
                "run-{}-{}-{}",
                now.timestamp_millis(),
                std::process::id(),
                NEXT_EVENT.fetch_add(1, Ordering::Relaxed)
            ),
            timestamp: now.to_rfc3339(),
            plan_revision: blake3::hash(loaded.text.as_bytes()).to_hex().to_string(),
            source: "pc-cli".into(),
            schema_version: 1,
            tasks: doc.task.iter().map(|task| task.id.clone()).collect(),
            capabilities: doc
                .capability
                .iter()
                .map(|capability| capability.id.clone())
                .collect(),
        },
        operation: operation.into(),
        plan_file: plan_file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        selection_objective: compiled.map(|result| result.selection_objective.clone()),
        selection_time_budget_ms: compiled.map(|result| result.selection_time_budget_ms),
        duration_ms: started.elapsed().as_millis() as u64,
        validation_duration_ms: validation_duration.as_millis() as u64,
        memory_bytes: process_memory_bytes(),
        task_count: doc.task.len(),
        method_count: doc.method.len(),
        constraint_count: order_edges
            + doc.objective.acceptance.len()
            + usize::from(doc.objective.budget_usd.is_some())
            + usize::from(doc.objective.deadline.is_some())
            + usize::from(doc.objective.quality_floor.is_some()),
        alternatives_considered,
        contract_rejections: report.contract_notes.len(),
        evidence_coverage: coverage,
        estimate_uncertainty_minutes: uncertainty,
        decisions_affected_by_evidence,
        critical_path_tasks,
        critical_path_changes,
        context_size_chars,
        selection_duration_ms: selection.map(|observation| observation.duration.as_millis() as u64),
        optimality_status,
        optimality_gap_percent,
        frontier_duration_ms: frontier_duration.map(|duration| duration.as_millis() as u64),
    };
    if let Err(error) = pc_store::append_compiler_run(plan_file, &event) {
        eprintln!("warning: could not record compiler metrics: {error}");
    }
}

/// Count recorded decisions whose supporting tasks or capabilities gained
/// evidence after the decision: evidence on one of its tasks, on a
/// non-rejected method of those tasks, or on a capability those methods
/// invoke or the decision names, timestamped after the decision. `doc` is
/// the post-pass plan, so a task P4 merged away is read as its surviving
/// task, whose id the merged task's evidence now names.
fn decisions_affected_by_evidence(
    doc: &pc_ir::PlanDoc,
    report: &pc_core::passes::Report,
    decisions: &[pc_ir::DecisionEvent],
) -> usize {
    let time = |text: &str| chrono::DateTime::parse_from_rfc3339(text).ok();
    let merged: std::collections::BTreeMap<&str, &str> = report
        .trace
        .iter()
        .filter(|event| event.pass == "P4-share" && event.decision == "merged")
        .filter_map(|event| {
            Some((
                event.subject.strip_prefix("task:")?,
                event
                    .reason
                    .strip_prefix("structurally identical to task:")?,
            ))
        })
        .collect();
    decisions
        .iter()
        .filter(|decision| {
            let Some(decided) = time(&decision.event.timestamp) else {
                return false;
            };
            let mut subjects: std::collections::BTreeSet<String> = decision
                .event
                .capabilities
                .iter()
                .map(|capability| format!("capability:{capability}"))
                .collect();
            for task in &decision.event.tasks {
                let task = merged.get(task.as_str()).copied().unwrap_or(task);
                subjects.insert(format!("task:{task}"));
                for method in doc.method.iter().filter(|method| {
                    method.task == task && method.status != pc_ir::MethodStatus::Rejected
                }) {
                    subjects.insert(format!("method:{}", method.id));
                    if let Some(capability) = &method.capability {
                        subjects.insert(format!("capability:{capability}"));
                    }
                }
            }
            doc.evidence.iter().any(|evidence| {
                subjects.contains(&evidence.subject)
                    && time(&evidence.time).is_some_and(|observed| observed > decided)
            })
        })
        .count()
}

/// Why P5 ignored `task:<id>` evidence, and which subjects to use instead.
fn unattributed_reason(entry: &pc_core::UnattributedTaskEvidence) -> String {
    if entry.capabilities.is_empty() {
        return "its accepted invoke methods call no capability".to_string();
    }
    let subjects: Vec<String> = entry
        .methods
        .iter()
        .map(|m| format!("method:{m}"))
        .chain(entry.capabilities.iter().map(|c| format!("capability:{c}")))
        .collect();
    format!(
        "its accepted invoke methods call {} capabilities; record against one of {} instead",
        entry.capabilities.len(),
        subjects.join(", "),
    )
}

/// One text line for ignored `task:<id>` evidence in compile and explain.
fn unattributed_line(entry: &pc_core::UnattributedTaskEvidence) -> String {
    format!(
        "unattributed task:{} {}: {} observation{} ignored ({})",
        entry.task,
        entry.metric,
        entry.observations,
        if entry.observations == 1 { "" } else { "s" },
        unattributed_reason(entry),
    )
}

/// One text line for pending evidence in compile output.
fn pending_line(entry: &pc_core::PendingEvidence) -> String {
    let basis = entry
        .basis
        .as_deref()
        .map_or(String::new(), |b| format!(" [basis {b}]"));
    format!(
        "pending {} {}{}: {} observation{} held (subject not in plan)",
        entry.subject,
        entry.metric,
        basis,
        entry.observations,
        if entry.observations == 1 { "" } else { "s" },
    )
}

/// One text line for the estimated remaining open work in compile output:
/// the frontier's summed weighted cost over estimated open tasks beside
/// the count of open tasks with no estimate, so a zero makespan beside
/// open work no longer reads as done.
fn open_work_line(summary: &pc_core::OpenWorkSummary) -> String {
    let estimated = summary.estimated_tasks.len();
    let unestimated = summary.unestimated_tasks.len();
    format!(
        "open work: estimated weighted cost {:.3} across {} {}; {} open {} unestimated",
        summary.estimated_weighted_cost,
        estimated,
        if estimated == 1 { "task" } else { "tasks" },
        unestimated,
        if unestimated == 1 { "task" } else { "tasks" },
    )
}

/// P6 objective selectable on `pc compile`.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum SelectionObjective {
    /// Minimize weighted total effort, counting shared work once.
    Effort,
    /// Minimize dependency-aware completion time.
    Makespan,
}

fn run_compile(
    path: &std::path::Path,
    json: bool,
    selection_time_budget_ms: u64,
    objective: SelectionObjective,
) {
    let started = std::time::Instant::now();
    let (loaded, report, validation_duration) = load_checked(path);
    let (result, selection) = match pc_core::compile_with_objective_and_selection_duration(
        &loaded.doc,
        std::time::Duration::from_millis(selection_time_budget_ms),
        match objective {
            SelectionObjective::Effort => pc_core::Objective::TotalEffort,
            SelectionObjective::Makespan => pc_core::Objective::Makespan,
        },
    ) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    record_compiler_run(
        &loaded,
        "compile",
        started,
        validation_duration,
        RunObservation {
            report: &report,
            compiled: Some(&result),
            selection: Some(&selection),
            context_size_chars: None,
            frontier_duration: None,
        },
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return;
    }
    println!(
        "compiled: {} ({}); makespan low {:.3}, likely {:.3}, high {:.3}, expected {:.3} (optimistic) minutes; parallel width {}",
        result.root_task,
        loaded.display_path,
        result.makespan_range.low,
        result.makespan_range.likely,
        result.makespan_range.high,
        result.makespan_minutes,
        result.parallel_width
    );
    println!(
        "selection: {} ({} ms budget)",
        result.selection_status, result.selection_time_budget_ms
    );
    if let SelectionObjective::Makespan = objective {
        println!("objective: makespan; {}", result.selection_explanation);
    }
    for (metric, range) in &result.totals {
        let Some(column) = result.basis.get(metric) else {
            continue;
        };
        // Declared-only totals carry no basis; measured totals name every
        // typed basis behind them so a distance never reads as accuracy.
        let bases = if column.measured_nodes == 0 {
            String::new()
        } else {
            format!("; basis {}", column.bases.join(", "))
        };
        // Fully measured totals label their triple as the observed P5
        // range, so a wide machine-contention spread never reads as a point
        // estimate; any measured total names its live observation count.
        let label = if column.measured_nodes > 0 && column.measured_nodes == column.total_nodes {
            "observed range "
        } else {
            ""
        };
        let observations = match (column.measured_nodes, column.measured_observations) {
            (0, _) => String::new(),
            (_, 1) => "; 1 observation".to_string(),
            (_, n) => format!("; {n} observations"),
        };
        println!(
            "{}: {}low {:.3}, likely {:.3}, high {:.3}, expected {:.3} (predicted {:.3}; measured {}/{}{}{})",
            metric_label(metric),
            label,
            range.low,
            range.likely,
            range.high,
            range.expected,
            column.predicted.expected,
            column.measured_nodes,
            column.total_nodes,
            bases,
            observations,
        );
    }
    let mut timed: Vec<_> = result.timings.iter().collect();
    timed.sort_by(|a, b| {
        a.1.earliest_start
            .total_cmp(&b.1.earliest_start)
            .then_with(|| a.0.cmp(b.0))
    });
    let critical: Vec<_> = timed
        .iter()
        .filter(|(_, timing)| timing.critical)
        .map(|(task, _)| task.as_str())
        .collect();
    if !critical.is_empty() {
        println!("critical tasks: {}", critical.join(", "));
    }
    for row in result
        .trace
        .iter()
        .filter(|row| row.decision == "resource-limited")
    {
        println!("resource-limited: {}", row.reason);
    }
    for (task, timing) in timed {
        println!(
            "slack {task}: {:.3} minutes{}",
            timing.slack,
            if timing.critical { " (critical)" } else { "" }
        );
    }
    if !result.unresolved_tasks.is_empty() {
        println!("unresolved: {}", result.unresolved_tasks.join(", "));
    }
    println!("{}", open_work_line(&result.open_work));
    for entry in &result.unattributed_tasks {
        println!("{}", unattributed_line(entry));
    }
    for entry in &result.staged_evidence {
        println!("{}", entry.line());
    }
    for entry in &result.pending_evidence {
        println!("{}", pending_line(entry));
    }
}

fn run_explain(path: &std::path::Path, target: &str, json: bool, selection_time_budget_ms: u64) {
    let (loaded, _, _) = load_checked(path);
    let result = match pc_core::compile_with_budget(
        &loaded.doc,
        std::time::Duration::from_millis(selection_time_budget_ms),
    ) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let target = if target.contains(':') {
        target.to_owned()
    } else {
        format!("task:{target}")
    };
    let mut rows: Vec<_> = result
        .trace
        .iter()
        .filter(|row| row.subject == target)
        .cloned()
        .collect();
    // G3: `explain method:` joins one view from the method's assumptions,
    // its capability's measurement basis and its contract exposure, so
    // verifying a substitution takes one command instead of `explain
    // method` plus `explain capability` plus `pc impact`.
    let method = target.strip_prefix("method:").and_then(|id| {
        loaded
            .doc
            .method
            .iter()
            .find(|candidate| candidate.id == id)
    });
    // Capability targets also render the P5 provenance report: declared
    // guesses beside measured evidence, one row per metric. Method targets
    // join their invoked capability's rows beside their own assumptions.
    let mut provenance: Vec<_> = match target.strip_prefix("capability:") {
        Some(capability) => result
            .provenance
            .iter()
            .filter(|row| row.capability == capability)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    if let Some(method) = method {
        if method.status == pc_ir::MethodStatus::Rejected {
            eprintln!("error: `{target}` is rejected and ignored by every pass");
            std::process::exit(1);
        }
        rows.insert(
            0,
            pc_core::CompileTrace {
                subject: target.clone(),
                decision: "status".to_string(),
                reason: pc_core::status_str(method.status).to_string(),
            },
        );
        if let Some(capability) = method.capability.as_deref() {
            provenance.extend(
                result
                    .provenance
                    .iter()
                    .filter(|row| row.capability == capability)
                    .cloned(),
            );
        }
        // The trace only walks the selected accepted plan, so a proposed or
        // unselected method's hand-written assumptions would otherwise be
        // missing here: carry them beside the measurements.
        for assumption in &method.assumptions {
            if !rows
                .iter()
                .any(|row| row.decision == "assumes" && row.reason == *assumption)
            {
                rows.push(pc_core::CompileTrace {
                    subject: target.clone(),
                    decision: "assumes".to_string(),
                    reason: assumption.clone(),
                });
            }
        }
    }
    let unattributed: Vec<_> = result
        .unattributed_tasks
        .iter()
        .filter(|entry| match target.split_once(':') {
            Some(("task", id)) => entry.task == id,
            Some(("capability", id)) => entry.capabilities.iter().any(|c| c == id),
            _ => false,
        })
        .cloned()
        .collect();
    // Proposed subjects also render the evidence staged with them, which
    // stays out of the effective numbers until the proposal is accepted.
    let staged: Vec<_> = result
        .staged_evidence
        .iter()
        .filter(|entry| match target.split_once(':') {
            Some(("capability", id)) => entry.capability == id,
            Some(("method", id)) => entry.methods.iter().any(|m| m == id),
            _ => false,
        })
        .cloned()
        .collect();
    // Contract exposure: the same typed note P8 ranks as a blocked
    // alternative and P9 previews, so the consumer list sits beside the
    // assumptions and measurements. Empty when the contract holds.
    let contract_notes: Vec<_> = match method {
        Some(found) => pc_core::contract_note_for(&loaded.doc, found)
            .into_iter()
            .collect(),
        None => Vec::new(),
    };
    // R5: `explain task:` joins the task's selection, its slack/criticality
    // and the frontier entries touching it, reusing the `pc context`
    // assembly so both commands filter frontier identically. Other subject
    // kinds are untouched: their output stays byte-identical.
    let task_context = target.strip_prefix("task:").and_then(|id| {
        let frontier_report = pc_core::frontier(&loaded.doc, &result);
        pc_core::context(&loaded.doc, &result, &frontier_report, id).ok()
    });
    let timing = task_context
        .as_ref()
        .and_then(|ctx| result.timings.get(&ctx.task.id));
    if rows.is_empty()
        && provenance.is_empty()
        && unattributed.is_empty()
        && staged.is_empty()
        && contract_notes.is_empty()
        && task_context.as_ref().is_none_or(|ctx| {
            ctx.task.selected_method.is_none() && timing.is_none() && ctx.frontier.is_empty()
        })
    {
        eprintln!("error: no explanation for `{target}` in reachable accepted plan");
        std::process::exit(1);
    }
    if json {
        let mut view = serde_json::json!({
            "rows": rows,
            "provenance": provenance,
            "unattributed_tasks": unattributed,
            "staged_evidence": staged,
            "contract_notes": contract_notes,
        });
        if let Some(ctx) = &task_context {
            view["selection"] = serde_json::json!(ctx.task.selected_method);
            view["timing"] = serde_json::json!(timing);
            view["frontier"] = serde_json::json!(ctx.frontier);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&view).unwrap_or_default()
        );
    } else {
        let selection_stated = rows
            .iter()
            .any(|row| matches!(row.decision.as_str(), "accepted" | "unresolved" | "skipped"));
        for row in rows {
            println!("{}: {} ({})", row.subject, row.decision, row.reason);
        }
        if let Some(ctx) = &task_context {
            if !selection_stated {
                match &ctx.task.selected_method {
                    Some(chosen) => println!("{target}: selected (uses method:{chosen})"),
                    None => {
                        println!("{target}: not selected (unreachable from the objective root)")
                    }
                }
            }
            for item in &ctx.frontier {
                println!("{}", item.line());
            }
        }
        for row in &provenance {
            println!("{}", provenance_line(row));
        }
        for entry in &unattributed {
            println!("{}", unattributed_line(entry));
        }
        for entry in &staged {
            println!("{}", entry.line());
        }
        for note in &contract_notes {
            println!("{}", contract_note_line(note));
        }
    }
}

/// One text line for a P5 provenance row: declared guesses beside measured
/// evidence. Shared by capability explanations and the joined (G3) method
/// view so the measurement basis reads identically in both.
fn provenance_line(row: &pc_core::ProvenanceRow) -> String {
    let predicted = row.predicted.map_or("none".to_string(), |p| {
        format!("{:.3}/{:.3}/{:.3}", p.low, p.likely, p.high)
    });
    let measured = row
        .measured
        .map_or("no measured evidence".to_string(), |m| {
            let stale = if row.stale > 0 {
                format!(", {} stale", row.stale)
            } else {
                String::new()
            };
            let sources = if row.sources.is_empty() {
                String::new()
            } else {
                format!(", sources: {}", row.sources.join(", "))
            };
            format!(
                "{:.3}/{:.3}/{:.3} (n={}{stale}{sources})",
                m.low, m.likely, m.high, row.observations,
            )
        });
    let contradicts = if row.contradicts_declared {
        " [contradicts declared]"
    } else {
        ""
    };
    // Declared-only rows carry no measurement to type; measured
    // rows always name their basis (`none` when the evidence
    // predates basis typing) so a distance never reads as accuracy.
    let basis = match (&row.measured, &row.measurement_basis) {
        (Some(_), Some(b)) => format!(" [basis {b}]"),
        (Some(_), None) => " [basis none]".to_string(),
        (None, _) => String::new(),
    };
    format!(
        "provenance {} {}: predicted {predicted} vs measured {measured}{contradicts}{basis}",
        row.capability, row.metric,
    )
}

/// One text line for a typed contract violation: the dropped fields with
/// the live tasks consuming them. Shared by `pc check` notes (G2), impact
/// previews, and the joined (G3) method view so the exposure reads
/// identically in all of them.
fn contract_note_line(note: &pc_core::ContractNote) -> String {
    let mut missing = note.missing_outputs.clone();
    missing.extend(note.missing_inputs.iter().cloned());
    let consumers = if note.consumers.is_empty() {
        "no live task consumes them".to_string()
    } else {
        format!(
            "consumed by {}",
            note.consumers
                .iter()
                .map(|c| format!("task:{c}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "contract method:{}: would drop {} ({consumers})",
        note.method,
        missing.join(", "),
    )
}

/// Handler for `pc record`.
fn run_record(evidence_path: &std::path::Path, path: &std::path::Path) {
    let text = match std::fs::read_to_string(evidence_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!(
                "error[E014]: cannot read `{}`: {e}",
                evidence_path.display()
            );
            std::process::exit(1);
        }
    };
    let record = match pc_store::parse_evidence_record(&text) {
        Ok(record) => record,
        Err(e) => {
            let code = if matches!(e, pc_store::EvidenceRecordError::UnknownMetric(_)) {
                "E010"
            } else {
                "E014"
            };
            eprintln!("error[{code}]: {e}");
            std::process::exit(1);
        }
    };
    let lock = pc_store::PlanLock::acquire(path).unwrap_or_else(|e| {
        eprintln!("error[E000]: {e}");
        std::process::exit(1);
    });
    let loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let mut with_record = loaded.doc.clone();
    with_record.evidence.push(record.clone());
    if let Some(conflict) = pc_core::estimate(&with_record)
        .basis_conflicts
        .into_iter()
        .find(|c| {
            c.bases
                .iter()
                .any(|(_, subjects)| subjects.contains(&record.subject))
        })
    {
        eprintln!("error[E014]: {}", conflict.message());
        std::process::exit(1);
    }
    match lock.append_evidence(&record) {
        Ok((log, line, _revision)) => {
            println!(
                "recorded {} {}={}{} to {}:{line}",
                record.subject,
                record.metric,
                record.value,
                record.unit,
                log.display(),
            );
            // The probe holds only the new record, so every warning below
            // describes this record, never the rest of the log. Unknown
            // subjects are staged as pending evidence, not refused: the
            // hypothesis they support may be proposed after they are
            // recorded.
            let mut probe = loaded.doc;
            probe.evidence = vec![record];
            let report =
                pc_core::passes::check(&loaded.display_path, &loaded.text, &mut probe, None);
            if !report.ok() {
                return;
            }
            let estimated = pc_core::estimate(&probe);
            for entry in &estimated.pending {
                eprintln!(
                    "warning: evidence subject `{}` names nothing yet; P5 holds it as pending until the subject is proposed",
                    entry.subject,
                );
            }
            for entry in &estimated.staged {
                let with = if entry.methods.is_empty() {
                    format!("capability:{}", entry.capability)
                } else {
                    entry
                        .methods
                        .iter()
                        .map(|m| format!("method:{m}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                eprintln!(
                    "warning: P5 holds this evidence as staged with {with}: it stays out of the effective numbers until an invoke method is accepted",
                );
            }
            for entry in estimated.unattributed_tasks {
                eprintln!(
                    "warning: P5 will not attribute task:{} evidence ({})",
                    entry.task,
                    unattributed_reason(&entry),
                );
            }
        }
        Err(e) => {
            eprintln!("error[E014]: {e}");
            std::process::exit(1);
        }
    }
}

fn run_event_record(input: &std::path::Path, path: &std::path::Path, decision: bool) {
    let text = std::fs::read_to_string(input).unwrap_or_else(|e| {
        eprintln!("error: cannot read `{}`: {e}", input.display());
        std::process::exit(1);
    });
    let loaded = pc_store::load(path).unwrap_or_else(|e| {
        eprintln!("error[E000]: {e}");
        std::process::exit(1);
    });
    let plan_file = std::path::PathBuf::from(&loaded.display_path);
    let result = if decision {
        serde_json::from_str::<pc_ir::DecisionEvent>(&text)
            .map_err(|e| e.to_string())
            .and_then(|event| {
                pc_store::append_decision(&plan_file, &event)
                    .map(|(p, n)| (p, n, event.event.id))
                    .map_err(|e| e.to_string())
            })
    } else {
        serde_json::from_str::<pc_ir::OutcomeEvent>(&text)
            .map_err(|e| e.to_string())
            .and_then(|event| {
                pc_store::append_outcome(&plan_file, &event)
                    .map(|(p, n)| (p, n, event.event.id))
                    .map_err(|e| e.to_string())
            })
    };
    match result {
        Ok((log, line, id)) => println!("recorded {id} to {}:{line}", log.display()),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn run_event_read(path: &std::path::Path, decision: bool) {
    let loaded = pc_store::load(path).unwrap_or_else(|e| {
        eprintln!("error[E000]: {e}");
        std::process::exit(1);
    });
    let plan_file = std::path::PathBuf::from(&loaded.display_path);
    let result = if decision {
        pc_store::read_decisions(&plan_file).and_then(|events| {
            serde_json::to_string_pretty(&events).map_err(|e| pc_store::LoadError::Unparsable {
                path: "decisions.jsonl".into(),
                message: e.to_string(),
            })
        })
    } else {
        pc_store::read_outcomes(&plan_file).and_then(|events| {
            serde_json::to_string_pretty(&events).map_err(|e| pc_store::LoadError::Unparsable {
                path: "outcomes.jsonl".into(),
                message: e.to_string(),
            })
        })
    };
    match result {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn event_records(
    path: &std::path::Path,
) -> Result<(Vec<pc_ir::DecisionEvent>, Vec<pc_ir::OutcomeEvent>), pc_store::LoadError> {
    let loaded = pc_store::load(path)?;
    let plan_file = std::path::PathBuf::from(loaded.display_path);
    Ok((
        pc_store::read_decisions(&plan_file)?,
        pc_store::read_outcomes(&plan_file)?,
    ))
}

/// Instant of an event; unparsable timestamps (only possible in a
/// hand-edited log) order before every valid one.
fn event_instant(event: &pc_ir::EventMetadata) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(&event.timestamp).ok()
}

/// Latest outcome per decision by event instant, ties going to the later
/// log entry, so re-evaluating a decision replaces its earlier outcome in
/// every report instead of being counted again.
fn latest_outcomes(
    outcomes: &[pc_ir::OutcomeEvent],
) -> std::collections::BTreeMap<&str, &pc_ir::OutcomeEvent> {
    let mut latest = std::collections::BTreeMap::<&str, &pc_ir::OutcomeEvent>::new();
    for outcome in outcomes {
        let replace = latest
            .get(outcome.decision_id.as_str())
            .is_none_or(|current| event_instant(&current.event) <= event_instant(&outcome.event));
        if replace {
            latest.insert(outcome.decision_id.as_str(), outcome);
        }
    }
    latest
}

fn classification(outcome: Option<&&pc_ir::OutcomeEvent>) -> &'static str {
    outcome.map_or("Inconclusive", |o| o.evaluation_status.label())
}

fn usefulness(outcome: Option<&&pc_ir::OutcomeEvent>) -> &'static str {
    outcome
        .and_then(|o| o.usefulness)
        .map_or("not-assessed", pc_ir::Usefulness::label)
}

fn run_evaluation(path: &std::path::Path, decision_id: Option<&str>) {
    let result = event_records(path).map(|(decisions, outcomes)| {
        let latest_by_decision = latest_outcomes(&outcomes);
        let rows: Vec<_> = decisions.iter().filter(|d| decision_id.is_none_or(|id| d.event.id == id)).map(|d| {
            let latest = latest_by_decision.get(d.event.id.as_str());
            let comparisons: Vec<_> = latest.into_iter().flat_map(|o| d.predicted_outcome.metrics.iter().filter_map(move |(metric, predicted)| {
                o.actual_outcome.metrics.get(metric).map(|actual| serde_json::json!({"metric":metric,"predicted":pc_core::json::round3(*predicted),"actual":pc_core::json::round3(*actual),"absolute_error":pc_core::json::round3((actual-predicted).abs()),"status":"measured"}))
            })).collect();
            let agrees = |a: &Option<String>, b: &Option<String>| a.as_ref().zip(b.as_ref()).map(|(a, b)| a == b);
            let selected = Some(d.selected_alternative.clone());
            serde_json::json!({
                "decision_id":d.event.id, "objective":d.objective, "initial_decision":d.initial_decision,
                "initial_decision_status":if d.initial_decision.is_some() { "recorded" } else { "not-recorded" },
                "recommendation":d.compiler_recommendation, "recommendation_reason":d.supporting_evidence,
                "final_decision":d.selected_alternative, "alternatives":d.alternatives,
                "recommendation_matches_initial":agrees(&d.compiler_recommendation, &d.initial_decision),
                "final_matches_recommendation":agrees(&selected, &d.compiler_recommendation),
                "final_matches_initial":agrees(&selected, &d.initial_decision),
                "classification":classification(latest), "usefulness":usefulness(latest),
                "outcome":latest.map(|o| &o.actual_outcome),
                "comparisons":comparisons,
                "counterfactual": {"alternative":d.baseline_alternative,"value":null,"status":"estimated","note":"No observed baseline outcome is recorded."},
                "planning_cost": {"value":null,"status":"unknown","note":"Planning cost is not recorded in decision/outcome events; PC-03 instrumentation is not yet merged."}
            })
        }).collect();
        serde_json::json!({"evaluations":rows})
    });
    print_event_report(result);
}

fn run_planning_metrics(path: &std::path::Path) {
    let result = event_records(path).map(|(decisions, outcomes)| {
        // Count each decision once by its latest outcome. Unevaluated
        // decisions count as Inconclusive and not-assessed.
        let latest_by_decision = latest_outcomes(&outcomes);
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        let mut usefulness_counts = std::collections::BTreeMap::<String, usize>::new();
        for decision in &decisions {
            let latest = latest_by_decision.get(decision.event.id.as_str());
            *counts.entry(classification(latest).into()).or_default() += 1;
            *usefulness_counts.entry(usefulness(latest).into()).or_default() += 1;
        }
        serde_json::json!({"decision_count":decisions.len(),"outcome_count":outcomes.len(),"evaluated_decision_count":latest_by_decision.len(),"unevaluated_decision_count":decisions.len().saturating_sub(latest_by_decision.len()),"classifications":counts,"usefulness":usefulness_counts})
    });
    print_event_report(result);
}

fn run_event_history(path: &std::path::Path) {
    let result = event_records(path).map(|(decisions, outcomes)| {
        let mut rows: Vec<_> = decisions
            .into_iter()
            .map(|e| {
                (
                    event_instant(&e.event),
                    serde_json::json!({"type":"decision","timestamp":e.event.timestamp,"event":e}),
                )
            })
            .chain(outcomes.into_iter().map(|e| {
                (
                    event_instant(&e.event),
                    serde_json::json!({"type":"outcome","timestamp":e.event.timestamp,"event":e}),
                )
            }))
            .collect();
        rows.sort_by_key(|(instant, _)| *instant);
        serde_json::json!({"history":rows.into_iter().map(|(_, row)| row).collect::<Vec<_>>()})
    });
    print_event_report(result);
}

/// Calibration pairs each decision's prediction with its latest outcome
/// only, like `evaluate` and `metrics`, so a re-evaluated decision weighs
/// the same as any other.
fn run_calibration(path: &std::path::Path) {
    let result = event_records(path).map(|(decisions, outcomes)| {
        let latest_by_decision = latest_outcomes(&outcomes);
        let mut values = std::collections::BTreeMap::<String, Vec<(f64,f64)>>::new();
        for d in &decisions { if let Some(o) = latest_by_decision.get(d.event.id.as_str()) {
            for (metric,predicted) in &d.predicted_outcome.metrics { if let Some(actual) = o.actual_outcome.metrics.get(metric) { values.entry(metric.clone()).or_default().push((*predicted,*actual)); } }
        }}
        let metrics: serde_json::Map<String, serde_json::Value> = values.into_iter().map(|(metric,pairs)| {
            let count=pairs.len(); let mae=pc_core::json::round3(pairs.iter().map(|(p,a)|(p-a).abs()).sum::<f64>() / count as f64);
            (metric, serde_json::json!({"count":count,"mean_absolute_error":mae,"status":"measured"}))
        }).collect();
        serde_json::json!({"metrics":metrics})
    });
    print_event_report(result);
}

fn print_event_report(result: Result<serde_json::Value, pc_store::LoadError>) {
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("error[E000]: {error}");
            std::process::exit(1);
        }
    }
}

/// Plan id for a fresh scaffold: the target directory's name, sanitized
/// to a stable id (`cargo init` names the package after the directory).
/// Falls back to `my-plan` for `.`, `/`, and names with no usable characters.
fn scaffold_plan_id(dir: &std::path::Path) -> String {
    let raw = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut id = String::new();
    for ch in raw.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch);
        } else if ch == '-' || ch == '_' {
            id.push('-');
        } else if id.ends_with('-') || id.is_empty() {
            continue;
        } else {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_string();
    if id.is_empty() {
        "my-plan".to_string()
    } else {
        id
    }
}

/// Minimal valid plan: one open root task, no methods or capabilities.
/// `pc check` passes on it; the agent proposal loop (`propose`/`accept`)
/// grows it from here.
fn scaffold_doc(plan_id: &str) -> pc_ir::PlanDoc {
    pc_ir::PlanDoc {
        ir_version: pc_ir::IR_VERSION,
        plan: pc_ir::PlanMeta {
            id: plan_id.to_string(),
            title: "My plan".to_string(),
            description: String::new(),
            external_inputs: Vec::new(),
            tasks_axi_sections: Vec::new(),
        },
        objective: pc_ir::Objective {
            root_task: "start".to_string(),
            weights: std::collections::BTreeMap::from([("duration_minutes".to_string(), 1.0)]),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            platforms: Vec::new(),
            licenses: Vec::new(),
            acceptance: vec![pc_ir::AcceptanceCriterion {
                kind: pc_ir::AcceptanceKind::Manual,
                description: "Someone signs off.".to_string(),
                command: None,
                metric: None,
                op: None,
                value: None,
            }],
        },
        artifact: Vec::new(),
        resource: Vec::new(),
        task: vec![pc_ir::Task {
            id: "start".to_string(),
            title: "Start".to_string(),
            state: pc_ir::TaskState::Open,
            consumes: Vec::new(),
            produces: Vec::new(),
            tasks_axi: None,
            beads_jsonl: None,
        }],
        method: Vec::new(),
        capability: Vec::new(),
        evidence: Vec::new(),
        investigation: Vec::new(),
    }
}

/// Resolve the directory a new plan is written to (default `.plan`),
/// exiting when it is a file or a plan already exists there.
fn new_plan_dir(path: Option<PathBuf>) -> PathBuf {
    let defaulted = path.is_none();
    let path = path.unwrap_or_else(|| PathBuf::from(".plan"));
    if path.is_file() {
        eprintln!(
            "error: cannot write a plan in `{}`: not a directory",
            path.display()
        );
        std::process::exit(1);
    }
    // Refuse to overwrite: an existing plan file under the target directory
    // wins over a new plan, so a stray command never clobbers work. A
    // defaulted `.plan` also guards the current directory, whose plan a new
    // `.plan/` would otherwise shadow for bare commands.
    let mut guarded = vec![path.as_path()];
    if defaulted {
        guarded.push(std::path::Path::new("."));
    }
    for dir in guarded {
        if let Ok(existing) = pc_store::resolve_plan_file(dir) {
            eprintln!(
                "error: plan already exists at `{}`: refusing to overwrite",
                existing.display()
            );
            std::process::exit(1);
        }
    }
    path
}

/// Handler for `pc init`.
fn run_init(path: Option<PathBuf>, json: bool) {
    let path = new_plan_dir(path);
    let path = path.as_path();
    if let Err(e) = std::fs::create_dir_all(path) {
        eprintln!("error[E000]: cannot create `{}`: {e}", path.display());
        std::process::exit(1);
    }
    let plan_id = scaffold_plan_id(path);
    let doc = scaffold_doc(&plan_id);
    let text = match pc_ir::to_toml(&doc) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error[E000]: cannot render scaffold plan: {e}");
            std::process::exit(1);
        }
    };
    let file = path.join("plan.toml");
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
    match written {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            eprintln!(
                "error: plan already exists at `{}`: refusing to overwrite",
                file.display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("error[E000]: cannot write `{}`: {e}", file.display());
            std::process::exit(1);
        }
    }
    let ignore = path.join(".gitignore");
    if let Err(e) = ensure_ignored(&ignore, ".plan.toml.lock") {
        eprintln!("warning: cannot update `{}`: {e}", ignore.display());
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": file.display().to_string(),
            }))
            .unwrap_or_default()
        );
        return;
    }
    println!("initialized plan at {}", file.display());
}

/// Append `entry` to a `.gitignore` unless a line already matches it.
fn ensure_ignored(ignore: &std::path::Path, entry: &str) -> std::io::Result<()> {
    let existing = match std::fs::read_to_string(ignore) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ignore)
        .and_then(|mut f| {
            std::io::Write::write_all(&mut f, format!("{separator}{entry}\n").as_bytes())
        })
}

/// Handler for `pc check`.
fn run_check(path: &std::path::Path, json: bool) {
    let started = std::time::Instant::now();
    let mut loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let validation_started = std::time::Instant::now();
    let (doc, report) = loaded.check();
    let validation_duration = validation_started.elapsed();
    loaded.doc = doc;
    record_compiler_run(
        &loaded,
        "check",
        started,
        validation_duration,
        RunObservation {
            report: &report,
            compiled: None,
            selection: None,
            context_size_chars: None,
            frontier_duration: None,
        },
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        for diag in &report.diagnostics {
            eprintln!("{}", pc_core::render_human(diag));
        }
        let counts = &report.counts;
        if report.ok() {
            println!(
                "ok: {} tasks, {} methods, {} capabilities, {} artifacts, {} evidence ({})",
                counts.tasks,
                counts.methods,
                counts.capabilities,
                counts.artifacts,
                counts.evidence,
                loaded.display_path,
            );
        } else {
            eprintln!(
                "found {} error{}",
                counts.errors,
                if counts.errors == 1 { "" } else { "s" }
            );
        }
        for note in &report.contract_notes {
            println!("{}", contract_note_line(note));
        }
    }
    if !report.ok() {
        std::process::exit(1);
    }
}

/// Handler for `pc schema`.
fn run_schema(
    out: Option<&std::path::Path>,
    patch: bool,
    decision: bool,
    outcome: bool,
    metrics: bool,
) {
    let schema = if decision {
        pc_ir::decision_schema()
    } else if outcome {
        pc_ir::outcome_schema()
    } else if metrics {
        pc_ir::metrics_schema()
    } else if patch {
        pc_ir::patch_schema()
    } else {
        pc_ir::plan_schema()
    };
    let text = serde_json::to_string_pretty(&schema).unwrap_or_default() + "\n";
    if let Some(path) = out {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
            std::process::exit(1);
        }
        if let Err(e) = std::fs::write(path, &text) {
            eprintln!("error[E000]: cannot write `{}`: {e}", path.display());
            std::process::exit(1);
        }
    } else {
        print!("{text}");
    }
}

/// Human label for a totals metric: relative metrics name their basis so
/// they never read as absolute figures.
fn metric_label(metric: &str) -> String {
    if metric == "accuracy_distance_from_incumbent" {
        format!("{metric} (distance from the incumbent, not absolute accuracy)")
    } else {
        metric.to_string()
    }
}

/// Handler for `pc capabilities`.
fn run_capabilities(path: &std::path::Path, json: bool) {
    let (loaded, _, _) = load_checked(path);
    let report = pc_core::inventory(&loaded.doc);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return;
    }
    print!(
        "{}",
        pc_core::render_inventory_text(&report, &loaded.display_path)
    );
}

/// Handler for `pc frontier`.
fn run_frontier(path: &std::path::Path, json: bool, selection_time_budget_ms: u64) {
    let started = std::time::Instant::now();
    let (loaded, report, validation_duration) = load_checked(path);
    let (compiled, selection) = match pc_core::compile_with_objective_and_selection_duration(
        &loaded.doc,
        std::time::Duration::from_millis(selection_time_budget_ms),
        pc_core::Objective::TotalEffort,
    ) {
        Ok(compiled) => compiled,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let frontier_started = std::time::Instant::now();
    let frontier_report = pc_core::frontier(&loaded.doc, &compiled);
    let frontier_duration = frontier_started.elapsed();
    record_compiler_run(
        &loaded,
        "frontier",
        started,
        validation_duration,
        RunObservation {
            report: &report,
            compiled: Some(&compiled),
            selection: Some(&selection),
            context_size_chars: None,
            frontier_duration: Some(frontier_duration),
        },
    );
    let report = frontier_report;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return;
    }
    println!(
        "execution frontier: {} ranked, {} unestimated, {} blocked ({})",
        report.items.len(),
        report.unestimated.len(),
        report.blocked.len(),
        loaded.display_path,
    );
    let print_item = |item: &pc_core::FrontierItem| {
        let mut flags = String::new();
        if item.critical {
            flags.push_str(" (critical)");
        }
        if !item.ready {
            flags.push_str(" (blocked)");
        }
        if item.tied {
            flags.push_str(" (tied)");
        }
        let score = item
            .score
            .map(|score| format!("{score:.3} "))
            .unwrap_or_default();
        println!(
            "{score}{} [{}]{}: {}",
            item.subject, item.kind, flags, item.reason,
        );
    };
    report.items.iter().for_each(print_item);
    if !report.unestimated.is_empty() {
        println!(
            "unestimated (no share of open acceptable weighted cost; structural order, not ranked against the list above):"
        );
        report.unestimated.iter().for_each(print_item);
    }
    if !report.blocked.is_empty() {
        println!(
            "blocked (fails typed contract matching, accepting would be E008; no share, not ranked):"
        );
        report.blocked.iter().for_each(print_item);
    }
    println!(
        "discovery frontier: {} investigation(s)",
        report.discovery.len()
    );
    for item in &report.discovery {
        println!(
            "{} [critical={}, uncertainty={:.3}, alternatives={}, downstream decisions={}, investigation cost={}, material-change likelihood={}]: {}",
            item.subject,
            item.critical,
            item.uncertainty,
            item.alternatives,
            item.downstream_decisions,
            item.investigation_cost
                .map_or_else(|| "unknown".to_string(), |v| format!("{v:.3}")),
            item.material_change_likelihood
                .map_or_else(|| "unknown".to_string(), |v| format!("{v:.3}")),
            item.question,
        );
        if let Some(voi) = &item.value_of_information {
            let methods = if voi.switch_candidates.is_empty() {
                voi.preferred_method.clone()
            } else {
                voi.switch_candidates.join(", ")
            };
            println!(
                "  value of information: EVSI [{:.3}, {:.3}] -> {} ({methods}): {}",
                voi.evsi_low, voi.evsi_high, voi.conclusion, voi.explanation,
            );
        }
        if let Some(reason) = &item.value_of_information_unknown {
            println!("  value of information: unknown ({reason})");
        }
    }
}

/// Handler for `pc impact`.
fn run_impact(
    change_path: &std::path::Path,
    path: &std::path::Path,
    json: bool,
    selection_time_budget_ms: u64,
) {
    let (loaded, _, _) = load_checked(path);
    let text = match std::fs::read_to_string(change_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", change_path.display());
            std::process::exit(1);
        }
    };
    let change: pc_core::Change = match serde_json::from_str(&text) {
        Ok(change) => change,
        Err(e) => {
            eprintln!(
                "error: cannot parse `{}` as a change: {e}",
                change_path.display()
            );
            std::process::exit(1);
        }
    };
    let report = match pc_core::impact(
        &loaded.doc,
        &change,
        &loaded.display_path,
        &loaded.text,
        std::time::Duration::from_millis(selection_time_budget_ms),
    ) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("error: invalid change: {e}");
            std::process::exit(1);
        }
    };
    if report.clean && report.revised.is_none() {
        eprintln!(
            "error: {}",
            report
                .revised_error
                .as_deref()
                .unwrap_or("recompilation failed")
        );
        std::process::exit(1);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return;
    }
    println!(
        "impact: {} on {}",
        change_path.display(),
        loaded.display_path
    );
    for note in &report.applied {
        println!("applied: {note}");
    }
    if report.affected_tasks.is_empty() {
        println!("affected tasks: none");
    } else {
        println!("affected tasks: {}", report.affected_tasks.join(", "));
    }
    for diag in &report.diagnostics {
        eprintln!("{}", pc_core::render_human(diag));
    }
    for note in &report.contract_notes {
        println!("{}", contract_note_line(note));
    }
    for change in &report.selection_changes {
        println!(
            "selection {}: {} -> {}",
            change.task,
            change.before.as_deref().unwrap_or("(unneeded)"),
            change.after.as_deref().unwrap_or("(unneeded)"),
        );
    }
    for delta in &report.totals_delta {
        if delta.delta == 0.0 {
            continue;
        }
        println!(
            "{}: expected {:.3} -> {:.3} ({:+.3})",
            metric_label(&delta.metric),
            delta.before,
            delta.after,
            delta.delta,
        );
    }
    if let Some(delta) = report.makespan_delta {
        if delta == 0.0 {
            return;
        }
        let (before, after) = report
            .baseline
            .as_ref()
            .zip(report.revised.as_ref())
            .map(|(a, b)| (a.makespan_minutes, b.makespan_minutes))
            .unwrap_or((0.0, 0.0));
        println!("makespan: {before:.3} -> {after:.3} minutes ({delta:+.3})");
    }
}

/// Load one diff side: parse errors exit 1, but diagnostics are data and
/// stay in the report so failing plans can still be compared.
fn load_side(path: &std::path::Path) -> (pc_store::LoadedPlan, pc_core::passes::Report) {
    let loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let (doc, report) = loaded.check();
    let mut loaded = loaded;
    loaded.doc = doc;
    (loaded, report)
}

/// Handler for `pc diff`.
fn run_diff(a: &std::path::Path, b: &std::path::Path, json: bool, selection_time_budget_ms: u64) {
    let budget = std::time::Duration::from_millis(selection_time_budget_ms);
    let (loaded_a, report_a) = load_side(a);
    let (loaded_b, report_b) = load_side(b);
    let compile_side = |loaded: &pc_store::LoadedPlan, clean: bool| {
        if !clean {
            return None;
        }
        match pc_core::compile_with_budget(&loaded.doc, budget) {
            Ok(compiled) => Some(compiled),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    };
    let compiled_a = compile_side(&loaded_a, report_a.ok());
    let compiled_b = compile_side(&loaded_b, report_b.ok());
    let report = pc_core::diff(
        &loaded_a.doc,
        compiled_a.as_ref(),
        &report_a.diagnostics,
        &loaded_b.doc,
        compiled_b.as_ref(),
        &report_b.diagnostics,
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return;
    }
    println!(
        "diff: {} vs {}",
        loaded_a.display_path, loaded_b.display_path
    );
    let mut empty = true;
    let mut section = |label: &str, values: &[String]| {
        if !values.is_empty() {
            empty = false;
            println!("{label}: {}", values.join(", "));
        }
    };
    section("tasks added", &report.tasks_added);
    section("tasks removed", &report.tasks_removed);
    section("tasks changed", &report.tasks_changed);
    section("capabilities added", &report.capabilities_added);
    section("capabilities removed", &report.capabilities_removed);
    section("capabilities changed", &report.capabilities_changed);
    section("methods added", &report.methods_added);
    section("methods removed", &report.methods_removed);
    for change in &report.method_status_changes {
        empty = false;
        println!(
            "method status {}: {} -> {}",
            change.method, change.before, change.after,
        );
    }
    for change in &report.method_content_changes {
        empty = false;
        println!("method content changed: {}", change.method);
    }
    for change in &report.selection_changes {
        empty = false;
        println!(
            "selection {}: {} -> {}",
            change.task,
            change.before.as_deref().unwrap_or("(unneeded)"),
            change.after.as_deref().unwrap_or("(unneeded)"),
        );
    }
    if let (Some(before), Some(after), Some(delta)) = (
        report.makespan_before,
        report.makespan_after,
        report.makespan_delta,
    ) {
        // Zero movement is not a difference: identical plans (and
        // coincidentally equal makespans) report nothing here.
        if delta != 0.0 {
            empty = false;
            println!("makespan: {before:.3} -> {after:.3} minutes ({delta:+.3})");
        }
    }
    for delta in &report.totals_delta {
        if delta.delta == 0.0 {
            continue;
        }
        empty = false;
        // Like the movement itself, the bases behind each side render only
        // when they moved: identical bases on both sides add no signal.
        let bases = if delta.basis_before == delta.basis_after {
            String::new()
        } else {
            format!(" [basis: {}]", pc_core::delta_bases(delta))
        };
        println!(
            "{}: expected {:.3} -> {:.3} ({:+.3}){bases}",
            metric_label(&delta.metric),
            delta.before,
            delta.after,
            delta.delta,
        );
    }
    for diag in &report.diagnostics_added {
        empty = false;
        println!(
            "diagnostic added: {}",
            pc_core::render_human(diag).replace('\n', " ")
        );
    }
    for diag in &report.diagnostics_removed {
        empty = false;
        println!(
            "diagnostic removed: {}",
            pc_core::render_human(diag).replace('\n', " ")
        );
    }
    if empty {
        println!("no differences");
    }
}

/// Load the pristine plan for the step-7 decision commands.
///
/// Unlike [`load_checked`], the document is returned exactly as parsed: P3
/// derived proposals, P4 share merges and P1 unit canonicalization are pass
/// mutations and must never be written back to the file.
fn load_pristine(path: &std::path::Path) -> pc_store::LoadedPlan {
    match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    }
}

/// Refuse a write when the plan under edit is already dirty.
fn require_clean(report: &pc_core::passes::Report) {
    if report.ok() {
        return;
    }
    for diag in &report.diagnostics {
        eprintln!("{}", pc_core::render_human(diag));
    }
    eprintln!(
        "found {} error{}",
        report.counts.errors,
        if report.counts.errors == 1 { "" } else { "s" }
    );
    std::process::exit(1);
}

/// Probe-check a merged edit and refuse the write when it is dirty.
///
/// `edited` is the pristine plan plus the staged decision; the probe runs on
/// a copy so pass mutations never reach the file. Diagnostics ARE the
/// finding for a refused decision, exactly as with `pc impact`.
fn require_merged_clean(loaded: &pc_store::LoadedPlan, edited: &pc_ir::PlanDoc) {
    let mut probed = edited.clone();
    let evidence_log = loaded
        .evidence_file
        .as_deref()
        .map(|file| pc_core::passes::EvidenceLog {
            file,
            lines: &loaded.evidence_lines,
        });
    let report = pc_core::passes::check(
        &loaded.display_path,
        &loaded.text,
        &mut probed,
        evidence_log,
    );
    require_clean(&report);
}

/// Write the edited pristine plan back to the file it was loaded from.
///
/// The trailing evidence records `pc_store::load` folded in from the sibling
/// `evidence.jsonl` stay in the log: writing them inline would count every
/// observation twice on the next load.
fn write_edited(loaded: &pc_store::LoadedPlan, edited: &pc_ir::PlanDoc) {
    let plan_file = std::path::PathBuf::from(&loaded.display_path);
    let mut file_doc = edited.clone();
    let inline = file_doc
        .evidence
        .len()
        .saturating_sub(loaded.evidence_lines.len());
    file_doc.evidence.truncate(inline);
    if let Err(e) = pc_store::write_plan_if_revision(&plan_file, &loaded.revision, &file_doc) {
        eprintln!("error[E000]: {e}");
        std::process::exit(1);
    }
}

/// Print staged decisions: bare notes in text, the notes plus the plan in JSON.
fn print_decision(notes: &[String], plan: &str, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "applied": notes,
                "plan": plan,
            }))
            .unwrap_or_default()
        );
        return;
    }
    for note in notes {
        println!("{note}");
    }
}

/// Handler for `pc propose`.
fn run_propose(patch_path: &std::path::Path, path: &std::path::Path, json: bool) {
    let text = match std::fs::read_to_string(patch_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error: cannot read `{}`: {e}", patch_path.display());
            std::process::exit(1);
        }
    };
    let patch: pc_ir::Patch = match serde_json::from_str(&text) {
        Ok(patch) => patch,
        Err(e) => {
            eprintln!(
                "error: cannot parse `{}` as a patch: {e}",
                patch_path.display()
            );
            std::process::exit(1);
        }
    };
    let loaded = load_pristine(path);
    let (_, report) = loaded.check();
    require_clean(&report);
    if let Err(e) = pc_core::validate_patch(&loaded.doc, &patch) {
        eprintln!("error: invalid patch: {e}");
        std::process::exit(1);
    }
    let mut edited = loaded.doc.clone();
    let applied = pc_core::apply_patch(&mut edited, patch);
    require_merged_clean(&loaded, &edited);
    write_edited(&loaded, &edited);
    print_decision(&applied, &loaded.display_path, json);
}

/// Handler for `pc accept` and `pc reject`.
fn run_decide(id: &str, path: &std::path::Path, json: bool, accept: bool) {
    let loaded = load_pristine(path);
    let (checked_doc, report) = loaded.check();
    require_clean(&report);
    let id = id.strip_prefix("method:").unwrap_or(id);
    let mut edited = loaded.doc.clone();
    let outcome = if accept {
        pc_core::accept_method(&mut edited, &checked_doc, id)
    } else {
        pc_core::reject_method(&mut edited, &checked_doc, id)
    };
    let note = match outcome {
        Ok(note) => note,
        Err(pc_core::ImpactError::UnknownMethod { id }) => {
            eprintln!("error: unknown method `{id}`");
            std::process::exit(1);
        }
        Err(pc_core::ImpactError::NotAcceptable { id, status }) => {
            let (verb, past) = if accept {
                ("accept", "accepted")
            } else {
                ("reject", "rejected")
            };
            eprintln!(
                "error: cannot {verb} method `{id}` with status `{status}` (only proposed or derived methods can be {past})"
            );
            std::process::exit(1);
        }
    };
    require_merged_clean(&loaded, &edited);
    write_edited(&loaded, &edited);
    print_decision(&[note], &loaded.display_path, json);
}

/// Handler for `pc context`.
fn run_context(task: &str, path: &std::path::Path, json: bool, budget: u64) {
    let started = std::time::Instant::now();
    let (loaded, report, validation_duration) = load_checked(path);
    let (compiled, selection) = match pc_core::compile_with_objective_and_selection_duration(
        &loaded.doc,
        std::time::Duration::from_millis(2000),
        pc_core::Objective::TotalEffort,
    ) {
        Ok(compiled) => compiled,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let frontier_report = pc_core::frontier(&loaded.doc, &compiled);
    let task = task.strip_prefix("task:").unwrap_or(task);
    let full = match pc_core::context(&loaded.doc, &compiled, &frontier_report, task) {
        Ok(full) => full,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let fitted = pc_core::fit_context(full, budget);
    let rendered = pc_core::render_context_text(&fitted);
    record_compiler_run(
        &loaded,
        "context",
        started,
        validation_duration,
        RunObservation {
            report: &report,
            compiled: Some(&compiled),
            selection: Some(&selection),
            context_size_chars: Some(rendered.chars().count()),
            frontier_duration: None,
        },
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&fitted).unwrap_or_default()
        );
        return;
    }
    print!("{rendered}");
}

/// Handler for `pc import-tasks-axi`.
fn run_import_tasks_axi(backlog_path: &std::path::Path, path: Option<PathBuf>) {
    let file = new_plan_dir(path).join("plan.toml");
    let text = match std::fs::read_to_string(backlog_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("error[E000]: cannot read `{}`: {e}", backlog_path.display());
            std::process::exit(1);
        }
    };
    let parsed = match pc_adapter_tasks_axi::parse_backlog(&text) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("error: cannot import `{}`: {e}", backlog_path.display());
            std::process::exit(1);
        }
    };
    let doc = match pc_adapter_tasks_axi::import_plan(&parsed) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("error: cannot import `{}`: {e}", backlog_path.display());
            std::process::exit(1);
        }
    };
    // The import promises a clean plan: prove it before writing anything.
    let rendered = match pc_ir::to_toml(&doc) {
        Ok(rendered) => rendered,
        Err(e) => {
            eprintln!("error[E000]: cannot render imported plan: {e}");
            std::process::exit(1);
        }
    };
    let mut probed = doc.clone();
    let report = pc_core::passes::check(&file.display().to_string(), &rendered, &mut probed, None);
    if !report.ok() {
        for diag in &report.diagnostics {
            eprintln!("{}", pc_core::render_human(diag));
        }
        eprintln!("error: imported plan fails check; refusing to write");
        std::process::exit(1);
    }
    if let Some(parent) = file.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&file, &rendered) {
        eprintln!("error[E000]: cannot write `{}`: {e}", file.display());
        std::process::exit(1);
    }
    println!(
        "imported {} tasks from {} to {}",
        parsed.items.len(),
        backlog_path.display(),
        file.display(),
    );
}

/// Handler for `pc export-tasks-axi`.
fn run_export_tasks_axi(backlog_path: &std::path::Path, path: &std::path::Path) {
    // Export reads the pristine plan: P3/P4 pass mutations must never leak
    // into the backlog (a merged-away task is still a backlog item).
    let loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let backlog = match pc_adapter_tasks_axi::export_backlog(&loaded.doc) {
        Ok(backlog) => backlog,
        Err(e) => {
            eprintln!("error: cannot export `{}`: {e}", loaded.display_path);
            std::process::exit(1);
        }
    };
    let text = pc_adapter_tasks_axi::render_backlog(&backlog);
    if let Some(parent) = backlog_path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(backlog_path, &text) {
        eprintln!(
            "error[E000]: cannot write `{}`: {e}",
            backlog_path.display()
        );
        std::process::exit(1);
    }
    println!(
        "exported {} tasks from {} to {}",
        backlog.items.len(),
        loaded.display_path,
        backlog_path.display(),
    );
}

/// Handler for `pc import-beads`.
fn run_import_beads(input: &std::path::Path, path: Option<PathBuf>) {
    let file = new_plan_dir(path).join("plan.toml");
    let text = std::fs::read_to_string(input).unwrap_or_else(|e| {
        eprintln!("error[E000]: cannot read `{}`: {e}", input.display());
        std::process::exit(1);
    });
    let pc_adapter_beads::Import { doc, unlinked } = pc_adapter_beads::import_jsonl(&text)
        .unwrap_or_else(|e| {
            eprintln!("error: cannot import `{}`: {e}", input.display());
            std::process::exit(1);
        });
    let rendered = pc_ir::to_toml(&doc).unwrap_or_else(|e| {
        eprintln!("error[E000]: cannot render imported plan: {e}");
        std::process::exit(1);
    });
    let mut probe = doc.clone();
    let report = pc_core::passes::check(&file.display().to_string(), &rendered, &mut probe, None);
    if !report.ok() {
        for diag in &report.diagnostics {
            eprintln!("{}", pc_core::render_human(diag));
        }
        eprintln!("error: imported plan fails check; refusing to write");
        std::process::exit(1);
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
            std::process::exit(1);
        });
    }
    std::fs::write(&file, rendered).unwrap_or_else(|e| {
        eprintln!("error[E000]: cannot write `{}`: {e}", file.display());
        std::process::exit(1);
    });
    for dep in &unlinked {
        println!(
            "unlinked dependency: `{}` blocks on `{}`, which is not in the file; kept in passthrough with no order edge",
            dep.issue, dep.depends_on
        );
    }
    println!(
        "imported {} Beads issues to {}",
        doc.task.len() - 1,
        file.display()
    );
}

/// Handler for `pc export-beads`.
fn run_export_beads(output: &std::path::Path, path: &std::path::Path) {
    let loaded = pc_store::load(path).unwrap_or_else(|e| {
        eprintln!("error[E000]: {e}");
        std::process::exit(1);
    });
    let text = pc_adapter_beads::export_jsonl(&loaded.doc).unwrap_or_else(|e| {
        eprintln!("error: cannot export `{}`: {e}", loaded.display_path);
        std::process::exit(1);
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
            std::process::exit(1);
        });
    }
    std::fs::write(output, text).unwrap_or_else(|e| {
        eprintln!("error[E000]: cannot write `{}`: {e}", output.display());
        std::process::exit(1);
    });
    println!(
        "exported Beads issues from {} to {}",
        loaded.display_path,
        output.display()
    );
}

fn run_sync(
    source: Option<String>,
    all: bool,
    dry_run: bool,
    input: Option<PathBuf>,
    path: &std::path::Path,
) {
    use pc_sync::{Mapping, Source, State};
    use std::process::Command;
    if !all && source.is_none() {
        eprintln!("pc sync requires --source <github|tasks-axi|beads> or --all");
        std::process::exit(2);
    }
    let plan_file = pc_store::resolve_plan_file(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let dir = plan_file.parent().unwrap_or(std::path::Path::new("."));
    let state_file = dir.join("sync-state.json");
    let map_file = dir.join("sync-map.json");
    let mut sync: State = if state_file.exists() {
        serde_json::from_slice(&std::fs::read(&state_file).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", state_file.display());
            std::process::exit(1)
        }))
        .unwrap_or_else(|e| {
            eprintln!("cannot parse {}: {e}", state_file.display());
            std::process::exit(1)
        })
    } else {
        State::default()
    };
    let mappings: Vec<Mapping> = if map_file.exists() {
        serde_json::from_slice(&std::fs::read(&map_file).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", map_file.display());
            std::process::exit(1)
        }))
        .unwrap_or_else(|e| {
            eprintln!("cannot parse {}: {e}", map_file.display());
            std::process::exit(1)
        })
    } else {
        Vec::new()
    };
    let sources = if all {
        vec![Source::TasksAxi, Source::Beads, Source::Github]
    } else {
        vec![match source.as_deref().unwrap_or("") {
            "github" => Source::Github,
            "tasks-axi" => Source::TasksAxi,
            "beads" => Source::Beads,
            x => {
                eprintln!("unknown sync source `{x}`");
                std::process::exit(2)
            }
        }]
    };
    let mut loaded = pc_store::load(path).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let mut had_conflict = false;
    let mut plan_changed = false;
    let mut reconciled = false;
    for src in sources {
        let snapshot = match src {
            Source::TasksAxi => Some("tasks-axi.backlog.md"),
            Source::Beads => Some("beads.jsonl"),
            Source::Github => None,
        }
        .map(|name| dir.join("sources").join(name));
        if all {
            let skip = if !mappings.iter().any(|m| m.source == src) {
                Some("no mappings in sync-map.json".to_owned())
            } else {
                snapshot
                    .as_ref()
                    .filter(|file| !file.exists())
                    .map(|file| format!("no snapshot at {}", file.display()))
            };
            if let Some(reason) = skip {
                println!("{src:?}: skipped, {reason}");
                continue;
            }
        }
        let raw = if let Some(file) = input.as_ref().or(snapshot.as_ref()) {
            std::fs::read_to_string(file).unwrap_or_else(|e| {
                eprintln!("cannot read {}: {e}", file.display());
                std::process::exit(1)
            })
        } else {
            let repo = Command::new("gh")
                .args([
                    "repo",
                    "view",
                    "--json",
                    "nameWithOwner",
                    "--jq",
                    ".nameWithOwner",
                ])
                .output()
                .unwrap_or_else(|e| {
                    eprintln!("cannot run gh: {e}");
                    std::process::exit(1)
                });
            if !repo.status.success() {
                eprintln!(
                    "gh repo view failed: {}",
                    String::from_utf8_lossy(&repo.stderr)
                );
                std::process::exit(1);
            }
            let repo = String::from_utf8_lossy(&repo.stdout).trim().to_owned();
            let result = Command::new("gh")
                .args([
                    "api",
                    "--paginate",
                    "--slurp",
                    &format!("repos/{repo}/issues?state=all&per_page=100"),
                ])
                .output()
                .unwrap_or_else(|e| {
                    eprintln!("cannot run gh api: {e}");
                    std::process::exit(1)
                });
            if !result.status.success() {
                eprintln!("gh api failed: {}", String::from_utf8_lossy(&result.stderr));
                std::process::exit(1);
            }
            String::from_utf8_lossy(&result.stdout).into_owned()
        };
        let records = pc_sync::parse_snapshot(src, &raw).unwrap_or_else(|e| {
            eprintln!("cannot parse {src:?} snapshot: {e}");
            std::process::exit(1)
        });
        let preview = pc_sync::reconcile(&mut loaded.doc, &mappings, &mut sync, src, &records)
            .unwrap_or_else(|e| {
                eprintln!("sync failed: {e}");
                std::process::exit(1)
            });
        println!(
            "{src:?}: {} changed, {} unchanged, {} conflicts, {} unmapped",
            preview.changed.len(),
            preview.unchanged.len(),
            preview.conflicts.len(),
            preview.unmapped.len()
        );
        reconciled = true;
        plan_changed |= !preview.changed.is_empty();
        for conflict in preview.conflicts {
            had_conflict = true;
            println!(
                "conflict {:?}/{} -> {}: {}",
                conflict.source, conflict.external_id, conflict.task_id, conflict.reason
            );
        }
    }
    if !dry_run && reconciled {
        if plan_changed {
            write_edited(&loaded, &loaded.doc);
        }
        std::fs::write(&state_file, serde_json::to_vec_pretty(&sync).unwrap()).unwrap_or_else(
            |e| {
                eprintln!("cannot write {}: {e}", state_file.display());
                std::process::exit(1)
            },
        );
    }
    if had_conflict {
        std::process::exit(1);
    }
}
