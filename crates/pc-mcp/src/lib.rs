//! Read-only MCP adapter over the planning compiler.
//!
//! This crate owns no passes: [`check_plan`], [`compile_plan`] and
//! [`frontier_plan`] replay exactly what `pc check --json`, `pc compile
//! --json` and `pc frontier --json` print (the same `pc-core` calls over the
//! same `pc-store` load and `LoadedPlan::check`, including the P3/P4 pass
//! mutations applied before P6). The binary in `src/main.rs` only transports those
//! values over MCP stdio; the CLI contract stays unchanged.
//!
//! Follow-up (out of scope here): write verbs (`propose`, `accept`,
//! `reject`, `record`) are not exposed; they edit plan files and need a
//! safety review before becoming tools.

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
