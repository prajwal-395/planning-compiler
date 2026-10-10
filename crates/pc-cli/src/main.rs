//! `pc`: the planning-compiler CLI.
//!
//! Exit codes follow the workspace contract: `0` means the plan checks
//! clean, `1` means diagnostics were found, `2` means the invocation itself
//! was wrong (reserved for clap usage errors).

use clap::{Parser, Subcommand};
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
    /// Run passes P1-P5 over a plan file or directory.
    Check {
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
    },
    /// Roll up accepted methods and analyze the critical path.
    Compile {
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit the full compiled plan as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// Explain a task, method, or capability in the accepted plan.
    Explain {
        /// Entity id, optionally prefixed with task:, method:, or capability:.
        target: String,
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Rank unresolved work (P8): open tasks, pending proposals,
    /// unmeasured critical-path cost and blocked alternatives.
    Frontier {
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit the ranked frontier as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum exact P6 solve time in milliseconds.
        #[arg(long, default_value_t = 2000)]
        selection_time_budget_ms: u64,
    },
    /// Preview a hypothetical change (P9) without editing any files.
    Impact {
        /// JSON file holding the change (accept_method, add_evidence).
        change: PathBuf,
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit the decision as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reject one pending method: flip a proposed or derived method to
    /// rejected.
    Reject {
        /// Proposed or derived method id, optionally prefixed with method:.
        method: String,
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
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
        /// Plan file or directory. Defaults to the current directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Emit the fitted slice as JSON.
        #[arg(long)]
        json: bool,
        /// Maximum slice size in tokens (chars/4 approximation).
        #[arg(long, default_value_t = pc_core::DEFAULT_CONTEXT_BUDGET_TOKENS)]
        budget: u64,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, json } => run_check(&path, json),
        Commands::Schema { out, patch } => run_schema(out.as_deref(), patch),
        Commands::Compile {
            path,
            json,
            selection_time_budget_ms,
        } => run_compile(&path, json, selection_time_budget_ms),
        Commands::Explain {
            target,
            path,
            json,
            selection_time_budget_ms,
        } => run_explain(&path, &target, json, selection_time_budget_ms),
        Commands::Record { evidence, path } => run_record(&evidence, &path),
        Commands::Frontier {
            path,
            json,
            selection_time_budget_ms,
        } => run_frontier(&path, json, selection_time_budget_ms),
        Commands::Impact {
            change,
            path,
            json,
            selection_time_budget_ms,
        } => run_impact(&change, &path, json, selection_time_budget_ms),
        Commands::Diff {
            a,
            b,
            json,
            selection_time_budget_ms,
        } => run_diff(&a, &b, json, selection_time_budget_ms),
        Commands::Propose { patch, path, json } => run_propose(&patch, &path, json),
        Commands::Accept { method, path, json } => run_decide(&method, &path, json, true),
        Commands::Reject { method, path, json } => run_decide(&method, &path, json, false),
        Commands::Context {
            task,
            path,
            json,
            budget,
        } => run_context(&task, &path, json, budget),
    }
}

fn load_checked(path: &std::path::Path) -> (pc_store::LoadedPlan, pc_core::passes::Report) {
    let mut loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let mut doc = loaded.doc.clone();
    let evidence_log = loaded
        .evidence_file
        .as_deref()
        .map(|file| pc_core::passes::EvidenceLog {
            file,
            lines: &loaded.evidence_lines,
        });
    let report = pc_core::passes::check(&loaded.display_path, &loaded.text, &mut doc, evidence_log);
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
    (loaded, report)
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

fn run_compile(path: &std::path::Path, json: bool, selection_time_budget_ms: u64) {
    let (loaded, _) = load_checked(path);
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
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
        return;
    }
    println!(
        "compiled: {} ({}); makespan {:.3} minutes; parallel width {}",
        result.root_task, loaded.display_path, result.makespan_minutes, result.parallel_width
    );
    println!(
        "selection: {} ({} ms budget)",
        result.selection_status, result.selection_time_budget_ms
    );
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
        println!(
            "{}: low {:.3}, likely {:.3}, high {:.3}, expected {:.3} (predicted {:.3}; measured {}/{}{})",
            metric_label(metric),
            range.low,
            range.likely,
            range.high,
            range.expected,
            column.predicted.expected,
            column.measured_nodes,
            column.total_nodes,
            bases,
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
    let (loaded, _) = load_checked(path);
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
    let rows: Vec<_> = result
        .trace
        .iter()
        .filter(|row| row.subject == target)
        .cloned()
        .collect();
    // Capability targets also render the P5 provenance report: declared
    // guesses beside measured evidence, one row per metric.
    let provenance: Vec<_> = match target.strip_prefix("capability:") {
        Some(capability) => result
            .provenance
            .iter()
            .filter(|row| row.capability == capability)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
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
    if rows.is_empty() && provenance.is_empty() && unattributed.is_empty() && staged.is_empty() {
        eprintln!("error: no explanation for `{target}` in reachable accepted plan");
        std::process::exit(1);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "rows": rows,
                "provenance": provenance,
                "unattributed_tasks": unattributed,
                "staged_evidence": staged,
            }))
            .unwrap_or_default()
        );
    } else {
        for row in rows {
            println!("{}: {} ({})", row.subject, row.decision, row.reason);
        }
        for row in provenance {
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
            println!(
                "provenance {} {}: predicted {predicted} vs measured {measured}{contradicts}{basis}",
                row.capability, row.metric,
            );
        }
        for entry in &unattributed {
            println!("{}", unattributed_line(entry));
        }
        for entry in &staged {
            println!("{}", entry.line());
        }
    }
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
    let plan_file = std::path::PathBuf::from(&loaded.display_path);
    match pc_store::append_evidence(&plan_file, &record) {
        Ok((log, line)) => {
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
/// Handler for `pc check`.
fn run_check(path: &std::path::Path, json: bool) {
    let loaded = match pc_store::load(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("error[E000]: {e}");
            std::process::exit(1);
        }
    };
    let mut doc = loaded.doc;
    let evidence_log = loaded
        .evidence_file
        .as_deref()
        .map(|file| pc_core::passes::EvidenceLog {
            file,
            lines: &loaded.evidence_lines,
        });
    let report = pc_core::passes::check(&loaded.display_path, &loaded.text, &mut doc, evidence_log);
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
    }
    if !report.ok() {
        std::process::exit(1);
    }
}

/// Handler for `pc schema`.
fn run_schema(out: Option<&std::path::Path>, patch: bool) {
    let schema = if patch {
        pc_ir::patch_schema()
    } else {
        pc_ir::plan_schema()
    };
    let text = serde_json::to_string_pretty(&schema).unwrap_or_default() + "\n";
    if let Some(path) = out {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("error[E000]: cannot create `{}`: {e}", parent.display());
                std::process::exit(1);
            }
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

/// Handler for `pc frontier`.
fn run_frontier(path: &std::path::Path, json: bool, selection_time_budget_ms: u64) {
    let (loaded, _) = load_checked(path);
    let compiled = match pc_core::compile_with_budget(
        &loaded.doc,
        std::time::Duration::from_millis(selection_time_budget_ms),
    ) {
        Ok(compiled) => compiled,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let report = pc_core::frontier(&loaded.doc, &compiled);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
        return;
    }
    println!(
        "frontier: {} item{} ({})",
        report.items.len(),
        if report.items.len() == 1 { "" } else { "s" },
        loaded.display_path,
    );
    for item in &report.items {
        println!(
            "{:.3} {} [{}]{}: {}",
            item.score,
            item.subject,
            item.kind,
            if item.critical { " (critical)" } else { "" },
            item.reason,
        );
    }
}

/// Handler for `pc impact`.
fn run_impact(
    change_path: &std::path::Path,
    path: &std::path::Path,
    json: bool,
    selection_time_budget_ms: u64,
) {
    let (loaded, _) = load_checked(path);
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
        println!(
            "contract method:{}: would drop {} ({consumers})",
            note.method,
            missing.join(", "),
        );
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
    let mut doc = loaded.doc.clone();
    let evidence_log = loaded
        .evidence_file
        .as_deref()
        .map(|file| pc_core::passes::EvidenceLog {
            file,
            lines: &loaded.evidence_lines,
        });
    let report = pc_core::passes::check(&loaded.display_path, &loaded.text, &mut doc, evidence_log);
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

/// Probe-check a copy of the pristine plan.
///
/// Returns the checked copy (carrying the P3 derived proposals `accept` and
/// `reject` can materialize) plus its report. Step-7 edits require a clean
/// plan first: a dirty plan means a hand edit or an older write broke it,
/// and layering a decision on top would only hide the breakage.
fn checked_copy(loaded: &pc_store::LoadedPlan) -> (pc_ir::PlanDoc, pc_core::passes::Report) {
    let mut probed = loaded.doc.clone();
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
    (probed, report)
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
    if let Err(e) = pc_store::write_plan(&plan_file, &file_doc) {
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
    let (_, report) = checked_copy(&loaded);
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
    let (checked_doc, report) = checked_copy(&loaded);
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
    let (loaded, _) = load_checked(path);
    let compiled =
        match pc_core::compile_with_budget(&loaded.doc, std::time::Duration::from_millis(2000)) {
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
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&fitted).unwrap_or_default()
        );
        return;
    }
    print!("{}", pc_core::render_context_text(&fitted));
}
