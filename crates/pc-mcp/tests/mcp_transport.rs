//! End-to-end test: drive `pc-mcp` over MCP stdio and assert tool results
//! equal the `pc <verb> --json` output for the same plan.

use assert_cmd::Command as AssertCommand;
use rmcp::{
    ServiceExt as _,
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt as _, TokioChildProcess},
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Workspace root (`crates/pc-mcp/../..`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Path to the `pc` CLI binary, freshly built.
///
/// `CARGO_BIN_EXE_<name>` is only set for binaries in the same package as
/// the test, so `pc` is built into the target and profile directories of
/// the current test executable once per test run (a no-op when up to date),
/// so parity never compares against a stale or missing CLI.
fn pc_bin() -> &'static Path {
    static PC: OnceLock<PathBuf> = OnceLock::new();
    PC.get_or_init(build_pc)
}

fn build_pc() -> PathBuf {
    let exe = std::env::current_exe().expect("current test executable");
    let profile_dir = exe
        .parent()
        .and_then(|dir| dir.parent())
        .expect("target profile dir");
    let target_dir = profile_dir.parent().expect("target dir");
    let profile = match profile_dir.file_name().and_then(|name| name.to_str()) {
        Some("debug") => "dev",
        Some(name) => name,
        None => panic!("non-UTF-8 profile dir {}", profile_dir.display()),
    };
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "pc-cli", "--quiet", "--profile", profile])
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(workspace_root())
        .status()
        .expect("cargo build -p pc-cli");
    assert!(status.success(), "failed to build pc CLI for parity test");
    profile_dir.join("pc")
}

/// Run `pc <args>` and return stdout as JSON.
fn pc_json(args: &[&str]) -> serde_json::Value {
    let output = AssertCommand::new(pc_bin()).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "pc {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Spawn `pc-mcp` and return a connected client.
async fn spawn_client() -> rmcp::service::RunningService<rmcp::service::RoleClient, ()> {
    let server_bin = assert_cmd::cargo::cargo_bin("pc-mcp");
    let transport =
        TokioChildProcess::new(tokio::process::Command::new(server_bin).configure(|cmd| {
            cmd.stderr(std::process::Stdio::null());
        }))
        .unwrap();
    ().serve(transport).await.unwrap()
}

/// Call one tool and return the parsed JSON from its first text block.
async fn call_tool_json(
    client: &rmcp::service::Peer<rmcp::service::RoleClient>,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let params = CallToolRequestParams::new(name.to_string())
        .with_arguments(arguments.as_object().unwrap().clone());
    let result = client.call_tool(params).await.unwrap();
    assert!(
        result.is_error != Some(true),
        "{name} tool errored: {result:?}"
    );
    let text = result
        .content
        .iter()
        .find_map(|block| block.as_text().map(|t| t.text.clone()))
        .expect("tool result has a text block");
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn tools_match_cli_json() {
    let client = spawn_client().await;

    // The server exposes exactly the read-only verbs.
    let tools: BTreeSet<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        tools,
        BTreeSet::from([
            "check".to_string(),
            "compile".to_string(),
            "context".to_string(),
            "frontier".to_string(),
            "impact".to_string(),
            "diff".to_string(),
        ]),
        "write verbs stay out of scope"
    );

    for plan in [
        workspace_root()
            .join("tests/fixtures/valid-contracts.toml")
            .display()
            .to_string(),
        workspace_root()
            .join("examples/transcription-substitution/plan.toml")
            .display()
            .to_string(),
    ] {
        let expected_check: serde_json::Value = pc_json(&["check", plan.as_str(), "--json"]);
        let got_check = call_tool_json(&client, "check", serde_json::json!({"path": plan})).await;
        assert_eq!(got_check, expected_check, "check parity for {plan}");

        let expected_compile: serde_json::Value = pc_json(&["compile", plan.as_str(), "--json"]);
        let got_compile =
            call_tool_json(&client, "compile", serde_json::json!({"path": plan})).await;
        assert_eq!(got_compile, expected_compile, "compile parity for {plan}");

        let expected_frontier: serde_json::Value = pc_json(&["frontier", plan.as_str(), "--json"]);
        let got_frontier =
            call_tool_json(&client, "frontier", serde_json::json!({"path": plan})).await;
        assert_eq!(
            got_frontier, expected_frontier,
            "frontier parity for {plan}"
        );

        let expected_context: serde_json::Value =
            pc_json(&["context", "transcribe-footage", plan.as_str(), "--json"]);
        let got_context = call_tool_json(
            &client,
            "context",
            serde_json::json!({"task": "transcribe-footage", "path": plan}),
        )
        .await;
        assert_eq!(got_context, expected_context, "context parity for {plan}");

        let expected_diff: serde_json::Value =
            pc_json(&["diff", plan.as_str(), plan.as_str(), "--json"]);
        let got_diff =
            call_tool_json(&client, "diff", serde_json::json!({"a": plan, "b": plan})).await;
        assert_eq!(got_diff, expected_diff, "diff parity for {plan}");
    }

    let plan = workspace_root().join("examples/transcription-substitution/plan.toml");
    let change_path =
        std::env::temp_dir().join(format!("pc-mcp-impact-{}.json", std::process::id()));
    std::fs::write(&change_path, r#"{"accept_method":["via-hybrid"]}"#).unwrap();
    let expected_impact: serde_json::Value = pc_json(&[
        "impact",
        change_path.to_str().unwrap(),
        plan.to_str().unwrap(),
        "--json",
    ]);
    std::fs::remove_file(change_path).unwrap();
    let got_impact = call_tool_json(
        &client,
        "impact",
        serde_json::json!({
            "path": plan,
            "change": {"accept_method": ["via-hybrid"]}
        }),
    )
    .await;
    assert_eq!(got_impact, expected_impact, "impact parity");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn dirty_plan_is_a_tool_error_not_a_crash() {
    let client = spawn_client().await;
    let plan = workspace_root()
        .join("tests/fixtures/invalid-E008.toml")
        .display()
        .to_string();

    // `check` still returns the report (ok == false), mirroring `pc check
    // --json` which prints JSON before exiting 1.
    let report = call_tool_json(&client, "check", serde_json::json!({"path": plan})).await;
    assert_eq!(report["diagnostics"][0]["code"], "E008");

    // `compile`, `frontier`, `context` and `impact` require a clean check:
    // they report a tool-level error instead of crashing the server.
    for tool in ["compile", "frontier", "context", "impact"] {
        let params = CallToolRequestParams::new(tool).with_arguments(
            serde_json::json!({
                "path": plan,
                "task": "transcribe-footage",
                "change": {},
            })
            .as_object()
            .unwrap()
            .clone(),
        );
        let result = client.call_tool(params).await.unwrap();
        assert_eq!(result.is_error, Some(true), "{tool} must fail cleanly");
        // The server still answers afterwards.
        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 6);
    }

    client.cancel().await.unwrap();
}
