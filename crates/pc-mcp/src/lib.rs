//! Read-only MCP adapter over the planning compiler.
//!
//! This crate owns no passes. Its tool functions replay what
//! `pc check --json`, `pc compile --json`,
//! `pc frontier --json`, `pc context --json`, `pc impact --json` and
//! `pc diff --json` print (the same `pc-core` calls over the
//! same `pc-store` load and `LoadedPlan::check`, including the P3/P4 pass
//! mutations applied before P6). The binary in `src/main.rs` only transports
//! those values over MCP stdio; the CLI contract stays unchanged.
//!
//! This is the read-only phase: write verbs (`propose`, `accept`,
//! `reject`, `record`) are not exposed yet and will be added in a later
//! change, together with their write safeguards.

use std::path::Path;
use std::time::Duration;

/// Default P6 solve budget in milliseconds, matching the CLI default.
pub const DEFAULT_SELECTION_TIME_BUDGET_MS: u64 = 2000;

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
    if !report.ok() {
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
        return Err(message);
    }
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
