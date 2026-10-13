//! `pc-mcp`: MCP server over the planning compiler.
//!
//! Stdio transport. Read-only tools `check`, `compile`, `frontier`, `context`,
//! `impact` and `diff` return the same JSON `pc <verb> --json` prints. Write
//! operations require the current revision token and are refused unless
//! `PC_MCP_ALLOWED_ROOTS` lists the directories the server may mutate.
//! Human approval of `accept` and `reject` is enforced by the MCP host's
//! tool-call confirmation: both are annotated non-read-only, `accept` also
//! destructive, and `accept` requires the agent's explicit `approved=true`.
//! Writes leave a persistent `.plan.toml.lock` beside the plan; ignore it in git.

use pc_mcp::{
    DEFAULT_SELECTION_TIME_BUDGET_MS, check_plan, compile_plan, context_plan, diff_plans,
    frontier_plan, impact_plan,
};
use rmcp::{
    ServiceExt as _,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars::JsonSchema,
    tool, tool_router,
};
use serde::Deserialize;
use std::path::PathBuf;

/// Parameters for the `check` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CheckParams {
    /// Plan file or directory, as passed to `pc check`.
    path: String,
}

/// Parameters for the `compile` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CompileParams {
    /// Plan file or directory, as passed to `pc compile`.
    path: String,
    /// Maximum exact P6 solve time in milliseconds (default 2000, as in the CLI).
    #[schemars(description = "Maximum exact P6 solve time in milliseconds")]
    #[serde(default = "default_budget")]
    selection_time_budget_ms: u64,
}

/// Parameters for the `frontier` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct FrontierParams {
    /// Plan file or directory, as passed to `pc frontier`.
    path: String,
    /// Maximum exact P6 solve time in milliseconds (default 2000, as in the CLI).
    #[schemars(description = "Maximum exact P6 solve time in milliseconds")]
    #[serde(default = "default_budget")]
    selection_time_budget_ms: u64,
}

/// Parameters for the `context` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ContextParams {
    /// Task id, optionally prefixed with `task:`.
    task: String,
    /// Plan file or directory, as passed to `pc context`.
    path: String,
    /// Maximum slice size in tokens (chars/4 approximation; default 2000).
    #[serde(default = "default_context_budget")]
    budget: u64,
}

/// Parameters for the `impact` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ImpactParams {
    /// JSON change object with optional `accept_method` and `add_evidence` arrays.
    change: serde_json::Value,
    /// Plan file or directory, as passed to `pc impact`.
    path: String,
    /// Maximum exact P6 solve time in milliseconds (default 2000, as in the CLI).
    #[serde(default = "default_budget")]
    selection_time_budget_ms: u64,
}

/// Parameters for the `diff` tool.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DiffParams {
    /// First plan file or directory.
    a: String,
    /// Second plan file or directory.
    b: String,
    /// Maximum exact P6 solve time in milliseconds (default 2000, as in the CLI).
    #[serde(default = "default_budget")]
    selection_time_budget_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RevisionParams {
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ProposeParams {
    path: String,
    expected_revision: String,
    patch: serde_json::Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct DecideParams {
    path: String,
    expected_revision: String,
    method: String,
    #[serde(default)]
    approved: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RecordParams {
    path: String,
    expected_revision: String,
    evidence: serde_json::Value,
}

fn default_budget() -> u64 {
    DEFAULT_SELECTION_TIME_BUDGET_MS
}

fn default_context_budget() -> u64 {
    pc_core::DEFAULT_CONTEXT_BUDGET_TOKENS
}

fn ok_json(value: &serde_json::Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(
        serde_json::to_string_pretty(value).unwrap_or_default(),
    )])
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

#[derive(Clone, Default)]
struct PcMcp;

#[tool_router(server_handler)]
impl PcMcp {
    /// Get the opaque revision token required by all write operations.
    #[tool(description = "Return the current revision token for guarded plan writes.")]
    fn revision(&self, Parameters(params): Parameters<RevisionParams>) -> CallToolResult {
        match pc_mcp::plan_revision(&PathBuf::from(params.path)) {
            Ok(revision) => ok_json(&serde_json::json!({"revision": revision})),
            Err(message) => tool_error(message),
        }
    }

    /// Stage a proposed patch after validation.
    #[tool(
        description = "Stage tasks, methods and capabilities as proposed. Requires expected_revision from revision; writes only a clean merged plan."
    )]
    fn propose(&self, Parameters(params): Parameters<ProposeParams>) -> CallToolResult {
        let patch = match serde_json::from_value::<pc_ir::Patch>(params.patch) {
            Ok(patch) => patch,
            Err(e) => return tool_error(format!("cannot parse patch: {e}")),
        };
        match pc_mcp::propose(
            &PathBuf::from(params.path),
            &params.expected_revision,
            patch,
        ) {
            Ok((notes, revision)) => {
                ok_json(&serde_json::json!({"notes": notes, "revision": revision}))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Accept a pending method with an explicit approval acknowledgement.
    #[tool(
        description = "Accept a proposed or derived method. Requires approved=true and the current expected_revision; refused if the merged plan fails check.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    fn accept(&self, Parameters(params): Parameters<DecideParams>) -> CallToolResult {
        if !params.approved {
            return tool_error("accept requires approved=true to confirm the decision");
        }
        match pc_mcp::decide(
            &PathBuf::from(params.path),
            &params.expected_revision,
            &params.method,
            true,
        ) {
            Ok((note, revision)) => {
                ok_json(&serde_json::json!({"note": note, "revision": revision}))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Reject a pending method.
    #[tool(
        description = "Reject a proposed or derived method. Requires the current expected_revision.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    fn reject(&self, Parameters(params): Parameters<DecideParams>) -> CallToolResult {
        match pc_mcp::decide(
            &PathBuf::from(params.path),
            &params.expected_revision,
            &params.method,
            false,
        ) {
            Ok((note, revision)) => {
                ok_json(&serde_json::json!({"note": note, "revision": revision}))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Append validated evidence to the sibling evidence log.
    #[tool(
        description = "Record evidence using the CLI validation rules. Requires the current expected_revision."
    )]
    fn record(&self, Parameters(params): Parameters<RecordParams>) -> CallToolResult {
        let evidence = match serde_json::from_value::<pc_ir::Evidence>(params.evidence) {
            Ok(record) => record,
            Err(e) => return tool_error(format!("cannot parse evidence: {e}")),
        };
        match pc_mcp::record(
            &PathBuf::from(params.path),
            &params.expected_revision,
            evidence,
        ) {
            Ok((log, line, revision)) => {
                ok_json(&serde_json::json!({"log": log, "line": line, "revision": revision}))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Run passes P1-P5 over a plan; same JSON as `pc check --json`.
    #[tool(
        description = "Run pc check over a plan file or directory; returns the same JSON as `pc check --json`."
    )]
    fn check(&self, Parameters(params): Parameters<CheckParams>) -> CallToolResult {
        match check_plan(&PathBuf::from(&params.path)) {
            Ok(report) => {
                ok_json(&serde_json::to_value(&report).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Roll up accepted methods and analyze the critical path; same JSON as `pc compile --json`.
    #[tool(
        description = "Compile a plan; returns the same JSON as `pc compile --json`. Requires a clean `check`."
    )]
    fn compile(&self, Parameters(params): Parameters<CompileParams>) -> CallToolResult {
        match compile_plan(
            &PathBuf::from(&params.path),
            params.selection_time_budget_ms,
        ) {
            Ok(compiled) => {
                ok_json(&serde_json::to_value(&compiled).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Rank unresolved work (P8); same JSON as `pc frontier --json`.
    #[tool(
        description = "Rank unresolved work in a plan; returns the same JSON as `pc frontier --json`. Requires a clean `check`."
    )]
    fn frontier(&self, Parameters(params): Parameters<FrontierParams>) -> CallToolResult {
        match frontier_plan(
            &PathBuf::from(&params.path),
            params.selection_time_budget_ms,
        ) {
            Ok(report) => {
                ok_json(&serde_json::to_value(&report).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Render the compact, budget-fitted context for one task; same JSON as `pc context --json`.
    #[tool(
        description = "Build a budget-fitted context slice for a task; returns the same JSON as `pc context --json`. Requires a clean `check`."
    )]
    fn context(&self, Parameters(params): Parameters<ContextParams>) -> CallToolResult {
        match context_plan(&PathBuf::from(&params.path), &params.task, params.budget) {
            Ok(report) => {
                ok_json(&serde_json::to_value(&report).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Preview a hypothetical change without editing files; same JSON as `pc impact --json`.
    #[tool(
        description = "Preview a structured plan change without writing files; returns the same JSON as `pc impact --json`. Requires a clean `check`."
    )]
    fn impact(&self, Parameters(params): Parameters<ImpactParams>) -> CallToolResult {
        let change = match serde_json::from_value::<pc_core::Change>(params.change) {
            Ok(change) => change,
            Err(error) => return tool_error(format!("cannot parse change: {error}")),
        };
        match impact_plan(
            &PathBuf::from(&params.path),
            &change,
            params.selection_time_budget_ms,
        ) {
            Ok(report) => {
                ok_json(&serde_json::to_value(&report).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }

    /// Compare two plans, preserving structural diffs even when one fails check.
    #[tool(description = "Compare two plans; returns the same JSON as `pc diff --json`.")]
    fn diff(&self, Parameters(params): Parameters<DiffParams>) -> CallToolResult {
        match diff_plans(
            &PathBuf::from(&params.a),
            &PathBuf::from(&params.b),
            params.selection_time_budget_ms,
        ) {
            Ok(report) => {
                ok_json(&serde_json::to_value(&report).unwrap_or(serde_json::Value::Null))
            }
            Err(message) => tool_error(message),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = PcMcp.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
