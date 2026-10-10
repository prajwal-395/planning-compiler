//! `pc`: the planning-compiler CLI (steps 0-1: `check`, `schema`).
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

/// v0 subcommands. `compile`, `frontier`, `explain`, `impact`, `propose`,
/// `record` and `context` arrive in later steps and are out of scope here.
#[derive(Debug, Subcommand)]
enum Commands {
    /// Run passes P1-P2 over a plan file or directory.
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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, json } => run_check(&path, json),
        Commands::Schema { out } => run_schema(out.as_deref()),
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
