//! CLI end-to-end tests: golden snapshots for valid plans, one negative
//! fixture per diagnostic code, plus schema staleness.
//
//! Fixture paths resolve from the workspace root so the asserted `-->`
//! locations stay machine-independent.

use assert_cmd::Command;
use std::path::PathBuf;

/// Workspace root (`crates/pc-cli/../..`), which holds `tests/fixtures`.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Run `pc check <name>` with the fixture directory as cwd so diagnostics
/// print the bare file name instead of an absolute path.
fn check(name: &str) -> assert_cmd::assert::Assert {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    cmd.arg("check").arg(name);
    cmd.assert()
}

#[test]
fn valid_contracts_pass() {
    check("valid-contracts.toml")
        .success()
        .stdout(predicates::str::contains("ok:"));
}

/// Human output for the passing contracts fixture, pinned by snapshot.
#[test]
fn valid_contracts_golden() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    cmd.arg("check").arg("valid-contracts.toml");
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    insta::assert_snapshot!("valid-contracts", String::from_utf8(output.stdout).unwrap());
}

/// The Ren dropped-output case names the lost field and the code.
#[test]
fn dropped_output_golden() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    cmd.arg("check").arg("invalid-E008.toml");
    let output = cmd.output().unwrap();
    assert!(!output.status.success());
    insta::assert_snapshot!("dropped-output", String::from_utf8(output.stderr).unwrap());
}

/// Every diagnostic code has a fixture that fails with exactly that code.
#[test]
fn negative_fixtures_fail_with_their_code() {
    let codes = [
        "E000", "E001", "E002", "E003", "E004", "E005", "E006", "E007", "E008", "E009", "E010",
        "E011", "E012", "E013", "E014", "E015",
    ];
    for code in codes {
        let name = format!("invalid-{code}.toml");
        let mut cmd = Command::cargo_bin("pc").unwrap();
        cmd.current_dir(workspace_root().join("tests").join("fixtures"));
        let output = cmd.arg("check").arg(&name).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{name}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(&format!("error[{code}]")), "{stderr}");
        insta::assert_snapshot!(format!("negative-{code}"), stderr);
    }
}

/// The project's self-plan fixture is a clean, reviewable step-1 plan.
#[test]
fn self_plan_fixture_passes() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root());
    cmd.arg("check").arg("examples/self-plan/plan.toml");
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    insta::assert_snapshot!("self-plan", String::from_utf8(output.stdout).unwrap());
}

#[test]
fn self_plan_compiles_and_explains_accepted_work() {
    let mut compile = Command::cargo_bin("pc").unwrap();
    compile.current_dir(workspace_root());
    compile.arg("compile").arg("examples/self-plan");
    let output = compile.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("makespan 6.167 minutes"), "{text}");
    assert!(text.contains("duration_minutes:"), "{text}");
    assert!(text.contains("critical tasks: run-check\n"), "{text}");
    assert!(
        text.contains("slack run-check: 0.000 minutes (critical)\n"),
        "{text}"
    );

    let mut explain = Command::cargo_bin("pc").unwrap();
    explain.current_dir(workspace_root());
    let output = explain
        .arg("explain")
        .arg("task:step-1")
        .arg("examples/self-plan")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("uses method:check-then-gate")
    );

    let explain = |target: &str| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .current_dir(workspace_root())
            .arg("explain")
            .arg(target)
            .arg("examples/self-plan")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let text = explain("method:plan-steps");
    assert!(
        text.contains(
            "method:plan-steps: assumes (Steps stay independently shippable in report section 4.2 order.)"
        ),
        "{text}"
    );
    let text = explain("run-check");
    assert!(
        text.contains("task:run-check: critical (slack 0.000 minutes; earliest 0.000-6.167, latest 0.000-6.167)"),
        "{text}"
    );
}

/// Parallel critical tasks are listed as a set, never as an invented chain.
#[test]
fn parallel_critical_tasks_are_not_chained() {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root().join("tests").join("fixtures"))
        .arg("compile")
        .arg("valid-parallel-critical.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("makespan 5.000 minutes"), "{text}");
    assert!(text.contains("critical tasks: a, b\n"), "{text}");
    assert!(!text.contains("->"), "{text}");
}

/// Diagnostics for `evidence.jsonl` records point at the log line.
#[test]
fn evidence_log_diagnostics_name_the_log() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = cmd.arg("check").arg("evidence-log").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    insta::assert_snapshot!("evidence-log", String::from_utf8(output.stderr).unwrap());
}

/// `--json` reports carry the code, the subject and the counts.
#[test]
fn json_report_carries_codes_and_counts() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    cmd.arg("check").arg("invalid-E008.toml").arg("--json");
    let output = cmd.output().unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["counts"]["errors"], 1);
    assert_eq!(report["diagnostics"][0]["code"], "E008");
    assert!(
        report["diagnostics"][0]["subject"]
            .as_str()
            .unwrap()
            .contains("voz-mfa")
    );
}

/// The committed schema is exactly what `pc schema` emits.
#[test]
fn committed_schema_is_fresh() {
    let committed =
        std::fs::read_to_string(workspace_root().join("schema").join("plan.schema.json")).unwrap();
    let emitted = serde_json::to_string_pretty(&pc_ir::plan_schema()).unwrap() + "\n";
    assert_eq!(
        committed, emitted,
        "run `pc schema --out schema/plan.schema.json`"
    );
}
