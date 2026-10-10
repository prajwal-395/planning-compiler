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

/// Available commands. Later-step commands are added with their passes.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Run passes P1-P4 over a plan file or directory.
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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, json } => run_check(&path, json),
        Commands::Schema { out } => run_schema(out.as_deref()),
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
    for (metric, range) in result.totals {
        println!(
            "{metric}: low {:.3}, likely {:.3}, high {:.3}, expected {:.3}",
            range.low, range.likely, range.high, range.expected
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
        .into_iter()
        .filter(|row| row.subject == target)
        .collect();
    if rows.is_empty() {
        eprintln!("error: no explanation for `{target}` in reachable accepted plan");
        std::process::exit(1);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
    } else {
        for row in rows {
            println!("{}: {} ({})", row.subject, row.decision, row.reason);
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
fn run_schema(out: Option<&std::path::Path>) {
    let schema = pc_ir::plan_schema();
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
