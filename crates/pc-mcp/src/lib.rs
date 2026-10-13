//! MCP adapter over the planning compiler.
//!
//! This crate owns no passes. Its tool functions replay what
//! `pc check --json`, `pc compile --json`,
//! `pc frontier --json`, `pc context --json`, `pc impact --json` and
//! `pc diff --json` print (the same `pc-core` calls over the
//! same `pc-store` load and `LoadedPlan::check`, including the P3/P4 pass
//! mutations applied before P6). The binary in `src/main.rs` only transports
//! those values over MCP stdio; the CLI contract stays unchanged.
//!
//! Mutations follow the CLI proposal workflow, with revision checks and
//! atomic writes for MCP callers. Writes are refused unless
//! `PC_MCP_ALLOWED_ROOTS` names the directories they may touch. Human
//! approval of `accept` and `reject` is enforced by the MCP host: those tools
//! are annotated as non-read-only (`accept` as destructive) so hosts ask a
//! human before running them, and `accept` also requires the agent's explicit
//! `approved=true`. Writes, including the CLI's, serialize on an OS lock held
//! on a persistent hidden sibling of the plan file (`.plan.toml.lock` for
//! `plan.toml`); it is created on the first write, never removed, and belongs
//! in `.gitignore` (`pc init` adds it).

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default P6 solve budget in milliseconds, matching the CLI default.
pub const DEFAULT_SELECTION_TIME_BUDGET_MS: u64 = 2000;

/// Revision token for a plan's current raw file contents.
pub fn plan_revision(path: &Path) -> Result<String, String> {
    pc_store::plan_revision(path).map_err(|e| e.to_string())
}

fn writable_plan(path: &Path, expected_revision: &str) -> Result<pc_store::LoadedPlan, String> {
    let file = writable_file(path)?;
    let loaded = pc_store::load(&file).map_err(|e| e.to_string())?;
    if loaded.revision != expected_revision {
        return Err(format!(
            "revision conflict: expected {expected_revision}, current {}",
            loaded.revision
        ));
    }
    Ok(loaded)
}

fn writable_file(path: &Path) -> Result<PathBuf, String> {
    let file = pc_store::canonical_plan_file(path).map_err(|e| e.to_string())?;
    let Some(roots) = std::env::var_os("PC_MCP_ALLOWED_ROOTS") else {
        return Err(
            "writes are disabled: set PC_MCP_ALLOWED_ROOTS to the directories the server may modify"
                .to_string(),
        );
    };
    let allowed = std::env::split_paths(&roots)
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| file.starts_with(root));
    if !allowed {
        return Err(format!(
            "write path is outside PC_MCP_ALLOWED_ROOTS: {}",
            file.display()
        ));
    }
    Ok(file)
}

fn clean_report(report: &pc_core::passes::Report) -> Result<(), String> {
    if report.ok() {
        return Ok(());
    }
    let mut message = format!(
        "plan fails check with {} error{}",
        report.counts.errors,
        if report.counts.errors == 1 { "" } else { "s" }
    );
    for diag in report.diagnostics.iter().take(5) {
        message.push_str(&format!(
            "\n{}",
            pc_core::render_human(diag).replace('\n', " ")
        ));
    }
    Err(message)
}

/// Write an edited pristine plan only when its probe copy checks clean.
fn write_checked(loaded: &pc_store::LoadedPlan, edited: pc_ir::PlanDoc) -> Result<String, String> {
    let mut probe = edited.clone();
    // Mirror the CLI probe: pass mutations never reach the file, and the
    // sibling evidence log stays attributed to its lines.
    let evidence_log = loaded
        .evidence_file
        .as_deref()
        .map(|file| pc_core::passes::EvidenceLog {
            file,
            lines: &loaded.evidence_lines,
        });
    let report =
        pc_core::passes::check(&loaded.display_path, &loaded.text, &mut probe, evidence_log);
    clean_report(&report)?;
    // Sibling evidence.jsonl records folded into `loaded.doc` stay in the
    // log: writing them inline would count every observation twice on reload.
    let mut file_doc = edited;
    let inline = file_doc
        .evidence
        .len()
        .saturating_sub(loaded.evidence_lines.len());
    file_doc.evidence.truncate(inline);
    pc_store::write_plan_if_revision(Path::new(&loaded.display_path), &loaded.revision, &file_doc)
        .map_err(|e| e.to_string())
}

/// Stage a validated patch using the pristine plan and CLI-equivalent check.
pub fn propose(
    path: &Path,
    expected_revision: &str,
    patch: pc_ir::Patch,
) -> Result<(Vec<String>, String), String> {
    let loaded = writable_plan(path, expected_revision)?;
    let (_checked, report) = loaded.check();
    clean_report(&report)?;
    // CLI validates against the pristine plan, not the pass-mutated copy.
    pc_core::validate_patch(&loaded.doc, &patch).map_err(|e| format!("invalid patch: {e}"))?;
    let mut edited = loaded.doc.clone();
    let notes = pc_core::apply_patch(&mut edited, patch);
    let revision = write_checked(&loaded, edited)?;
    Ok((notes, revision))
}

/// Accept or reject a pending method after checking the merged plan.
pub fn decide(
    path: &Path,
    expected_revision: &str,
    method: &str,
    accept: bool,
) -> Result<(String, String), String> {
    let loaded = writable_plan(path, expected_revision)?;
    let (checked, report) = loaded.check();
    clean_report(&report)?;
    let id = method.strip_prefix("method:").unwrap_or(method);
    let mut edited = loaded.doc.clone();
    let note = if accept {
        pc_core::accept_method(&mut edited, &checked, id)
    } else {
        pc_core::reject_method(&mut edited, &checked, id)
    }
    .map_err(|e| e.to_string())?;
    let revision = write_checked(&loaded, edited)?;
    Ok((note, revision))
}

/// Record typed evidence, preserving CLI validation and staged-evidence behavior.
pub fn record(
    path: &Path,
    expected_revision: &str,
    record: pc_ir::Evidence,
) -> Result<(PathBuf, u32, String), String> {
    let lock = pc_store::PlanLock::acquire(&writable_file(path)?).map_err(|e| e.to_string())?;
    let loaded = writable_plan(path, expected_revision)?;
    let mut checked_record = record;
    let text = serde_json::to_string(&checked_record).map_err(|e| e.to_string())?;
    checked_record = pc_store::parse_evidence_record(&text).map_err(|e| e.to_string())?;
    let mut with_record = loaded.doc.clone();
    with_record.evidence.push(checked_record.clone());
    if let Some(conflict) = pc_core::estimate(&with_record)
        .basis_conflicts
        .into_iter()
        .find(|c| {
            c.bases
                .iter()
                .any(|(_, subjects)| subjects.contains(&checked_record.subject))
        })
    {
        return Err(conflict.message());
    }
    lock.append_evidence(&checked_record)
        .map_err(|e| e.to_string())
}

/// Run passes P1-P5 over a plan file or directory and return the report.
///
/// Mirrors `pc check --json`: the report is returned even when the plan is
/// dirty (`ok() == false`); only load failures are errors.
pub fn check_plan(path: &Path) -> Result<pc_core::passes::Report, String> {
    let loaded = pc_store::load(path).map_err(|e| format!("error[E000]: {e}"))?;
    Ok(loaded.check().1)
}

/// Load a plan and require it to check clean, applying the same pass
/// mutations (`P3` derived proposals, `P4` share merges) the CLI compiles.
fn load_checked(path: &Path) -> Result<pc_store::LoadedPlan, String> {
    let mut loaded = pc_store::load(path).map_err(|e| format!("error[E000]: {e}"))?;
    let (doc, report) = loaded.check();
    clean_report(&report)?;
    loaded.doc = doc;
    Ok(loaded)
}

/// Compile the accepted plan, mirroring `pc compile --json`.
pub fn compile_plan(
    path: &Path,
    selection_time_budget_ms: u64,
) -> Result<pc_core::CompiledPlan, String> {
    let loaded = load_checked(path)?;
    pc_core::compile_with_budget(&loaded.doc, Duration::from_millis(selection_time_budget_ms))
        .map_err(|e| e.to_string())
}

/// Rank unresolved work, mirroring `pc frontier --json`.
pub fn frontier_plan(
    path: &Path,
    selection_time_budget_ms: u64,
) -> Result<pc_core::FrontierReport, String> {
    let loaded = load_checked(path)?;
    let compiled =
        pc_core::compile_with_budget(&loaded.doc, Duration::from_millis(selection_time_budget_ms))
            .map_err(|e| e.to_string())?;
    Ok(pc_core::frontier(&loaded.doc, &compiled))
}

/// Build a budget-fitted task context, mirroring `pc context --json`.
pub fn context_plan(
    path: &Path,
    task: &str,
    budget_tokens: u64,
) -> Result<pc_core::ContextReport, String> {
    let loaded = load_checked(path)?;
    let compiled = pc_core::compile_with_budget(
        &loaded.doc,
        Duration::from_millis(DEFAULT_SELECTION_TIME_BUDGET_MS),
    )
    .map_err(|e| e.to_string())?;
    let frontier = pc_core::frontier(&loaded.doc, &compiled);
    let task = task.strip_prefix("task:").unwrap_or(task);
    let context =
        pc_core::context(&loaded.doc, &compiled, &frontier, task).map_err(|e| e.to_string())?;
    Ok(pc_core::fit_context(context, budget_tokens))
}

/// Preview a hypothetical change without writing files, mirroring `pc impact --json`.
pub fn impact_plan(
    path: &Path,
    change: &pc_core::Change,
    selection_time_budget_ms: u64,
) -> Result<pc_core::ImpactReport, String> {
    let loaded = load_checked(path)?;
    let report = pc_core::impact(
        &loaded.doc,
        change,
        &loaded.display_path,
        &loaded.text,
        Duration::from_millis(selection_time_budget_ms),
    )
    .map_err(|e| format!("invalid change: {e}"))?;
    if report.clean && report.revised.is_none() {
        return Err(report
            .revised_error
            .clone()
            .unwrap_or_else(|| "recompilation failed".to_string()));
    }
    Ok(report)
}

/// Compare two plans, retaining structural differences for plans that fail check.
pub fn diff_plans(
    a: &Path,
    b: &Path,
    selection_time_budget_ms: u64,
) -> Result<pc_core::DiffReport, String> {
    fn load_side(path: &Path) -> Result<(pc_store::LoadedPlan, pc_core::passes::Report), String> {
        let mut loaded = pc_store::load(path).map_err(|e| format!("error[E000]: {e}"))?;
        let (doc, report) = loaded.check();
        loaded.doc = doc;
        Ok((loaded, report))
    }

    let (a_loaded, a_report) = load_side(a)?;
    let (b_loaded, b_report) = load_side(b)?;
    let budget = Duration::from_millis(selection_time_budget_ms);
    let compile_side = |loaded: &pc_store::LoadedPlan, clean: bool| {
        clean
            .then(|| pc_core::compile_with_budget(&loaded.doc, budget))
            .transpose()
    };
    let a_compiled = compile_side(&a_loaded, a_report.ok()).map_err(|e| e.to_string())?;
    let b_compiled = compile_side(&b_loaded, b_report.ok()).map_err(|e| e.to_string())?;
    Ok(pc_core::diff(
        &a_loaded.doc,
        a_compiled.as_ref(),
        &a_report.diagnostics,
        &b_loaded.doc,
        b_compiled.as_ref(),
        &b_report.diagnostics,
    ))
}
