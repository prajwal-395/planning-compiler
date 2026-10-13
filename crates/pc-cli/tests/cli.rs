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

#[test]
fn decision_and_outcome_commands_append_and_read_distinct_records() {
    let root = std::env::temp_dir().join(format!(
        "pc-cli-events-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(
        workspace_root().join("tests/fixtures/valid-contracts.toml"),
        root.join("plan.toml"),
    )
    .unwrap();
    let event_dir = root.join("input");
    std::fs::create_dir(&event_dir).unwrap();
    let decision = serde_json::json!({
        "id":"d-1", "timestamp":"2026-10-13T12:00:00Z", "plan_revision":"r1", "source":"test", "schema_version":1,
        "tasks":["render"], "capabilities":[], "objective":"Improve rendering", "project_revision":"abc",
        "alternatives":[{"id":"batch","description":"Batch"},{"id":"current","description":"Current"}], "selected_alternative":"batch",
        "initial_decision":"current", "compiler_recommendation":"batch", "decision_provenance":"accepted recommendation", "supporting_evidence":[],
        "constraints":[], "predicted_outcome":{"description":"20 min","metrics":{"duration_minutes":20.0}}, "baseline_alternative":"current"
    });
    let outcome = serde_json::json!({
        "id":"o-1", "timestamp":"2026-10-13T13:00:00Z", "plan_revision":"r1", "source":"benchmark", "schema_version":1,
        "tasks":["render"], "capabilities":[], "decision_id":"d-1",
        "actual_outcome":{"description":"24 min","metrics":{"duration_minutes":24.0}}, "confidence":0.9, "evaluation_status":"confirmed", "usefulness":"neutral"
    });
    let decision_path = event_dir.join("decision.json");
    let outcome_path = event_dir.join("outcome.json");
    std::fs::write(&decision_path, serde_json::to_vec(&decision).unwrap()).unwrap();
    std::fs::write(&outcome_path, serde_json::to_vec(&outcome).unwrap()).unwrap();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("decision")
        .arg(&decision_path)
        .arg(&root)
        .assert()
        .success();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("decisions")
        .arg(&root)
        .assert()
        .success()
        .stdout(predicates::str::contains("predicted_outcome"));
    Command::cargo_bin("pc")
        .unwrap()
        .arg("outcome")
        .arg(&outcome_path)
        .arg(&root)
        .assert()
        .success();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("outcomes")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rows: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(rows[0]["decision_id"], "d-1");
    assert_eq!(
        rows[0]["actual_outcome"]["metrics"]["duration_minutes"],
        24.0
    );
    let evaluated = Command::cargo_bin("pc")
        .unwrap()
        .arg("evaluate")
        .arg("--decision")
        .arg("d-1")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let evaluated: serde_json::Value = serde_json::from_slice(&evaluated).unwrap();
    assert_eq!(evaluated["evaluations"][0]["classification"], "Confirmed");
    assert_eq!(evaluated["evaluations"][0]["usefulness"], "neutral");
    assert_eq!(evaluated["evaluations"][0]["initial_decision"], "current");
    assert_eq!(
        evaluated["evaluations"][0]["recommendation_matches_initial"],
        false
    );
    assert_eq!(
        evaluated["evaluations"][0]["final_matches_recommendation"],
        true
    );
    assert_eq!(evaluated["evaluations"][0]["final_matches_initial"], false);
    assert_eq!(
        evaluated["evaluations"][0]["comparisons"][0]["status"],
        "measured"
    );
    assert_eq!(
        evaluated["evaluations"][0]["counterfactual"]["status"],
        "estimated"
    );
    // A bare path with no --decision flag evaluates every decision: the
    // path must not be mistaken for a decision id (regression on the old
    // two-positional `evaluate [id] [path]` shape).
    let evaluated_all = Command::cargo_bin("pc")
        .unwrap()
        .arg("evaluate")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let evaluated_all: serde_json::Value = serde_json::from_slice(&evaluated_all).unwrap();
    assert_eq!(evaluated_all["evaluations"].as_array().unwrap().len(), 1);
    assert_eq!(
        evaluated_all["evaluations"][0]["planning_cost"]["status"],
        "unknown"
    );
    let metrics = Command::cargo_bin("pc")
        .unwrap()
        .arg("metrics")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let metrics: serde_json::Value = serde_json::from_slice(&metrics).unwrap();
    assert_eq!(metrics["classifications"]["Confirmed"], 1);
    let calibration = Command::cargo_bin("pc")
        .unwrap()
        .arg("calibration")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let calibration: serde_json::Value = serde_json::from_slice(&calibration).unwrap();
    assert_eq!(
        calibration["metrics"]["duration_minutes"]["mean_absolute_error"],
        4.0
    );
    let history = Command::cargo_bin("pc")
        .unwrap()
        .arg("history")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let history: serde_json::Value = serde_json::from_slice(&history).unwrap();
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
    // A second outcome for the same decision re-evaluates it: metrics must
    // count the decision once (latest wins) while still counting both
    // outcome events. Counting outcomes would report Confirmed 1 and
    // Rejected 1 for a single decision.
    let outcome2 = serde_json::json!({
        "id":"o-2", "timestamp":"2026-10-13T14:00:00Z", "plan_revision":"r1", "source":"benchmark", "schema_version":1,
        "tasks":["render"], "capabilities":[], "decision_id":"d-1",
        "actual_outcome":{"description":"26 min","metrics":{"duration_minutes":26.0}}, "confidence":0.5, "evaluation_status":"rejected"
    });
    let outcome2_path = event_dir.join("outcome2.json");
    std::fs::write(&outcome2_path, serde_json::to_vec(&outcome2).unwrap()).unwrap();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("outcome")
        .arg(&outcome2_path)
        .arg(&root)
        .assert()
        .success();
    let metrics2 = Command::cargo_bin("pc")
        .unwrap()
        .arg("metrics")
        .arg(&root)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let metrics2: serde_json::Value = serde_json::from_slice(&metrics2).unwrap();
    assert_eq!(metrics2["decision_count"], 1);
    assert_eq!(metrics2["outcome_count"], 2);
    assert_eq!(metrics2["evaluated_decision_count"], 1);
    assert_eq!(metrics2["classifications"]["Rejected"], 1);
    assert!(metrics2["classifications"].get("Confirmed").is_none());
    assert_eq!(metrics2["usefulness"]["not-assessed"], 1);
    // o-3 is logged last and its local time reads later than o-2's, but
    // 20:30+07:00 is 13:30Z, before o-2 at 14:00Z: o-2 stays the latest
    // outcome everywhere, and history orders by instant, not by text.
    let outcome3 = serde_json::json!({
        "id":"o-3", "timestamp":"2026-10-13T20:30:00+07:00", "plan_revision":"r1", "source":"benchmark", "schema_version":1,
        "tasks":["render"], "capabilities":[], "decision_id":"d-1",
        "actual_outcome":{"description":"30 min","metrics":{"duration_minutes":30.0}}, "confidence":0.5, "evaluation_status":"changed"
    });
    let outcome3_path = event_dir.join("outcome3.json");
    std::fs::write(&outcome3_path, serde_json::to_vec(&outcome3).unwrap()).unwrap();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("outcome")
        .arg(&outcome3_path)
        .arg(&root)
        .assert()
        .success();
    let report = |command: &str| -> serde_json::Value {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg(command)
            .arg(&root)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice(&output).unwrap()
    };
    let evaluated3 = report("evaluate");
    assert_eq!(evaluated3["evaluations"][0]["classification"], "Rejected");
    assert_eq!(
        evaluated3["evaluations"][0]["comparisons"][0]["actual"],
        26.0
    );
    assert_eq!(report("metrics")["classifications"]["Rejected"], 1);
    let calibration3 = report("calibration");
    assert_eq!(calibration3["metrics"]["duration_minutes"]["count"], 1);
    assert_eq!(
        calibration3["metrics"]["duration_minutes"]["mean_absolute_error"],
        6.0
    );
    let order: Vec<_> = report("history")["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["event"]["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(order, ["d-1", "o-1", "o-3", "o-2"]);
    // A misspelled status is refused instead of silently counting as
    // Inconclusive.
    let misspelled = serde_json::json!({
        "id":"o-4", "timestamp":"2026-10-13T15:00:00Z", "plan_revision":"r1", "source":"benchmark", "schema_version":1,
        "tasks":["render"], "capabilities":[], "decision_id":"d-1",
        "actual_outcome":{"description":"x","metrics":{}}, "confidence":null, "evaluation_status":"confirm"
    });
    let misspelled_path = event_dir.join("outcome4.json");
    std::fs::write(&misspelled_path, serde_json::to_vec(&misspelled).unwrap()).unwrap();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("outcome")
        .arg(&misspelled_path)
        .arg(&root)
        .assert()
        .failure()
        .stderr(predicates::str::contains("unknown variant `confirm`"));
    assert_eq!(report("metrics")["outcome_count"], 3);
    std::fs::remove_dir_all(root).unwrap();
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

/// The capability inventory lists versions with evidence coverage: without
/// this, a provider bump or an unmeasured capability would ship silently.
#[test]
fn capabilities_lists_versions_and_evidence_coverage() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = cmd
        .arg("capabilities")
        .arg("valid-contracts.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("capability:whisperx@3"), "{text}");
    assert!(text.contains("unmeasured"), "{text}");

    let mut json = Command::cargo_bin("pc").unwrap();
    json.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = json
        .arg("capabilities")
        .arg("valid-contracts.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let caps = report["capabilities"].as_array().unwrap();
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0]["id"], "whisperx@3");
    assert_eq!(caps[0]["version"], "3");
    assert!(caps[0]["unmeasured"].as_bool().unwrap());
}

/// Step 4: duplicates merge by content hash and covering capabilities are
/// proposed as derived methods, pinned by snapshot.
#[test]
fn match_share_golden() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root().join("tests").join("fixtures"));
    cmd.arg("check").arg("valid-match-share.toml");
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    insta::assert_snapshot!("match-share", String::from_utf8(output.stdout).unwrap());

    // The trace names the Substitute (derived proposal) and Share (merge)
    // decisions; the same-title, different-contract task is never merged.
    let mut json = Command::cargo_bin("pc").unwrap();
    json.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = json
        .arg("check")
        .arg("valid-match-share.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["counts"]["tasks"], 3);
    assert_eq!(report["counts"]["methods"], 2);
    let trace = report["trace"].as_array().unwrap();
    let has = |pass: &str, subject: &str, decision: &str| {
        trace
            .iter()
            .any(|e| e["pass"] == pass && e["subject"] == subject && e["decision"] == decision)
    };
    assert!(has("P3-match", "task:ingest-a", "derived"));
    assert!(has("P4-share", "task:ingest-b", "merged"));
    assert!(!has("P4-share", "task:ingest-c", "merged"));

    // Compile runs over the shared document: the merged task is gone from
    // the unresolved list, so P6 counts its work once.
    let mut compile = Command::cargo_bin("pc").unwrap();
    compile.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = compile
        .arg("compile")
        .arg("valid-match-share.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("unresolved: ingest-a, ingest-c\n"), "{text}");
    assert!(!text.contains("ingest-b"), "{text}");
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
        "E011", "E012", "E013", "E014", "E015", "E016",
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

/// The project's self-plan fixture is the step-8 dogfood plan: steps 0-7
/// done, step 8 in progress, post-MVP open.
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
    // Every finished step is a done leaf and no remaining task has an
    // accepted method, so there is no executable work to roll up: makespan
    // 0 with no metric totals, and the open post-MVP and PC work listed.
    // Continuous validation is an ongoing process outside the
    // decomposition, so it is never unresolved development work.
    assert!(
        text.contains(
            "makespan low 0.000, likely 0.000, high 0.000, expected 0.000 (optimistic) minutes"
        ),
        "{text}"
    );
    assert!(text.contains("parallel width 0"), "{text}");
    assert!(!text.contains("duration_minutes:"), "{text}");
    assert!(text.contains("unresolved: PC-02, PC-03"), "{text}");
    assert!(text.contains("post-mvp-11-mining"), "{text}");
    assert!(!text.contains("continuous-validation"), "{text}");
    assert!(!text.contains("post-mvp-01-tasks-axi"), "{text}");

    let mut explain = Command::cargo_bin("pc").unwrap();
    explain.current_dir(workspace_root());
    let output = explain
        .arg("explain")
        .arg("task:step-8")
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
            .contains("uses method:step-8-plan")
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
            "method:plan-steps: assumes (Steps stay independently shippable in report section 4.2 order; PC backlog hard dependencies follow section 11"
        ),
        "{text}"
    );
    let text = explain("method:step-8-plan");
    assert!(
        text.contains("method:step-8-plan: assumes (Retrospective accepted as step 8's first proof point (3/3 pass with documented nuances).)"),
        "{text}"
    );
    let text = explain("task:PC-06");
    assert!(
        text.contains("task:PC-06: unresolved (no accepted method)"),
        "{text}"
    );

    // The P5 provenance report for the shared implementation capability
    // rests on the eight measured PR open-to-merge times (basis absolute),
    // recorded as a pipeline-time baseline; no open task estimates through it.
    let text = explain("capability:crewmate@1");
    assert!(
        text.contains("measured (P5 median 4.341 over 8 observations for `duration_minutes` (min 2.883, max 16.650) [basis absolute])"),
        "{text}"
    );
    assert!(
        text.contains("provenance crewmate@1 duration_minutes: predicted 2.000/5.000/15.000 vs measured 2.883/4.341/16.650 (n=8"),
        "{text}"
    );
    assert!(text.contains("[basis absolute]"), "{text}");

    // The frontier ranks estimated proposals by their share of the open
    // acceptable weighted cost (scheduling before store, pricey backends
    // before cheap ones), each followed by its open task. Unestimated work
    // is a separate structural group, never ranked against costed work.
    // No waiting period gates anything: post-MVP work is ready alongside the
    // PC lane heads, and only the PC backlog's explicit dependencies block.
    let frontier = |json: bool| {
        let mut cmd = Command::cargo_bin("pc").unwrap();
        cmd.current_dir(workspace_root())
            .arg("frontier")
            .arg("examples/self-plan");
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let report: serde_json::Value = serde_json::from_str(&frontier(true)).unwrap();
    // PC-10 defect: execution candidates alone hide plan investigations, and
    // absent IR fields must not be turned into invented cost/probability.
    let discovery = report["discovery"].as_array().unwrap();
    assert!(
        !discovery.is_empty(),
        "proposed alternatives should create investigation work: {report}"
    );
    assert!(
        discovery
            .iter()
            .any(|item| item["alternatives"].as_u64().unwrap_or(0) > 0),
        "{report}"
    );
    assert!(
        discovery
            .iter()
            .all(|item| item["investigation_cost"].is_null()
                && item["material_change_likelihood"].is_null()),
        "unknown discovery quantities must remain unknown: {report}"
    );
    let subjects = |group: &str| -> Vec<String> {
        report[group]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["subject"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        subjects("items"),
        [
            "method:sched-via-aries",
            "task:post-mvp-08-scheduling",
            "method:store-via-postgres",
            "task:post-mvp-09-sqlite",
            "method:sched-via-pumpkin",
            "method:store-via-sqlite",
        ],
        "{report}"
    );
    assert_eq!(
        subjects("unestimated"),
        [
            "task:post-mvp-03-taskmaster",
            "task:post-mvp-04-skill-scan",
            "task:post-mvp-06-proposer",
            "task:post-mvp-07-pareto",
            "task:post-mvp-10-rewrite",
            "task:post-mvp-11-mining",
            "task:PC-02",
            "task:PC-05",
            "task:PC-07",
            "task:PC-10",
            "task:PC-12",
            "task:PC-14",
            "task:PC-03",
            "task:PC-04",
            "task:PC-06",
            "task:PC-08",
            "task:PC-09",
            "task:PC-11",
            "task:PC-13",
            "task:PC-15",
            "task:PC-16",
            "task:PC-17",
            "task:PC-18",
        ],
        "{report}"
    );
    let ready: Vec<String> = report["unestimated"]
        .as_array()
        .unwrap()
        .iter()
        .chain(report["items"].as_array().unwrap())
        .filter(|item| item["ready"] == true)
        .map(|item| item["subject"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ready,
        [
            "task:post-mvp-03-taskmaster",
            "task:post-mvp-04-skill-scan",
            "task:post-mvp-06-proposer",
            "task:post-mvp-07-pareto",
            "task:post-mvp-10-rewrite",
            "task:post-mvp-11-mining",
            "task:PC-02",
            "task:PC-05",
            "task:PC-07",
            "task:PC-10",
            "task:PC-12",
            "task:PC-14",
            "method:sched-via-aries",
            "task:post-mvp-08-scheduling",
            "method:store-via-postgres",
            "task:post-mvp-09-sqlite",
            "method:sched-via-pumpkin",
            "method:store-via-sqlite",
        ],
        "{report}"
    );
    // Genuine ties say so instead of hiding behind subject order, and
    // blocked work names what blocks it: the live Ren cycle (PC-06) waits
    // only on decision recording, never on step-8 or a waiting period.
    let text = frontier(false);
    assert!(
        text.contains("frontier: 6 ranked, 23 unestimated, 0 blocked"),
        "{text}"
    );
    assert!(
        text.contains("task:PC-06 [open-task] (critical) (blocked) (tied)"),
        "{text}"
    );
    assert!(text.contains("blocked by PC-02;"), "{text}");
    assert!(!text.contains("blocked by step-8"), "{text}");
    assert!(!text.contains("continuous-validation"), "{text}");
    assert!(
        text.contains("weighted expected cost 190.217 of 335.323 open"),
        "{text}"
    );
}

/// Item pc-open-work-headline: `pc compile` on the self-plan states the
/// estimated remaining open work beside the zero makespan, in text and
/// `--json`, without changing the makespan or its meaning.
#[test]
fn self_plan_compile_reports_open_work() {
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
    // The zero is correct per the IR (unresolved tasks roll zero work), so
    // the open-work line must sit beside it: estimated frontier work plus
    // the unestimated count, never a changed makespan.
    assert!(
        text.contains(
            "makespan low 0.000, likely 0.000, high 0.000, expected 0.000 (optimistic) minutes"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "open work: estimated weighted cost 335.323 across 2 tasks; 23 open tasks unestimated"
        ),
        "{text}"
    );

    let mut json = Command::cargo_bin("pc").unwrap();
    json.current_dir(workspace_root());
    let output = json
        .arg("compile")
        .arg("examples/self-plan")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["makespan_minutes"], serde_json::json!(0.0));
    let open = &report["open_work"];
    let cost = open["estimated_weighted_cost"].as_f64().unwrap();
    assert!(
        (cost - 335.323).abs() < 0.01,
        "estimated open cost {cost} should match the frontier open total"
    );
    assert_eq!(
        open["estimated_tasks"],
        serde_json::json!(["post-mvp-08-scheduling", "post-mvp-09-sqlite"]),
        "{open}"
    );
    let unestimated: Vec<String> = open["unestimated_tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(unestimated.len(), 23, "{open}");
    assert!(unestimated.contains(&"PC-06".to_string()), "{open}");
    assert!(
        !unestimated.contains(&"continuous-validation".to_string()),
        "{open}"
    );
    assert!(
        unestimated.contains(&"post-mvp-11-mining".to_string()),
        "{open}"
    );
    // Every unresolved task is accounted for exactly once.
    let mut accounted: Vec<serde_json::Value> = open["estimated_tasks"]
        .as_array()
        .unwrap()
        .iter()
        .chain(open["unestimated_tasks"].as_array().unwrap().iter())
        .cloned()
        .collect();
    let mut unresolved: Vec<serde_json::Value> =
        report["unresolved_tasks"].as_array().unwrap().clone();
    accounted.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    unresolved.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    assert_eq!(accounted, unresolved, "{open}");
}

/// The next dogfood decision (accepting a pending backend proposal) keeps
/// the self-plan feasible: every proposed capability declares the cost and
/// risk the objective's budget and quality floor require.
#[test]
fn self_plan_stays_feasible_after_accepting_each_proposal() {
    for method in [
        "sched-via-pumpkin",
        "sched-via-aries",
        "store-via-sqlite",
        "store-via-postgres",
    ] {
        let dir = scratch_dir(method);
        for file in ["plan.toml", "evidence.jsonl"] {
            std::fs::copy(
                workspace_root()
                    .join("examples")
                    .join("self-plan")
                    .join(file),
                dir.join(file),
            )
            .unwrap();
        }
        pc_ok(&dir, &["accept", method]);
        let text = pc_ok(&dir, &["compile"]);
        assert!(text.contains("selection: optimal"), "{method}: {text}");
        pc_ok(&dir, &["frontier"]);
    }
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
    assert!(
        text.contains(
            "makespan low 5.000, likely 5.000, high 5.000, expected 5.000 (optimistic) minutes"
        ),
        "{text}"
    );
    assert!(text.contains("critical tasks: a, b\n"), "{text}");
    assert!(!text.contains("->"), "{text}");
}

/// Compile reports the low/likely/high makespan range in text and JSON,
/// matching the duration total and labelling the PERT expectation optimistic.
/// Fails on the old single-value `makespan <expected> minutes` output.
#[test]
fn compile_reports_makespan_range() {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root().join("tests").join("fixtures"))
        .arg("compile")
        .arg("evidence-flip")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(
            "makespan low 9.000, likely 10.000, high 11.000, expected 10.000 (optimistic) minutes"
        ),
        "{text}"
    );

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root().join("tests").join("fixtures"))
        .arg("compile")
        .arg("evidence-flip")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["makespan_range"],
        serde_json::json!({"low": 9.0, "likely": 10.0, "high": 11.0, "expected": 10.0}),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        value["makespan_range"], value["totals"]["duration_minutes"],
        "makespan range matches the duration total, which already carried a range"
    );
    assert_eq!(
        value["makespan_minutes"],
        value["makespan_range"]["expected"]
    );
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

/// The committed patch schema is exactly what `pc schema --patch` emits.
#[test]
fn patch_schema_is_fresh() {
    let committed =
        std::fs::read_to_string(workspace_root().join("schema").join("patch.schema.json")).unwrap();
    let emitted = serde_json::to_string_pretty(&pc_ir::patch_schema()).unwrap() + "\n";
    assert_eq!(
        committed, emitted,
        "run `pc schema --patch --out schema/patch.schema.json`"
    );

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("schema")
        .arg("--patch")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("add_task"), "{text}");
    assert!(text.contains("add_method"), "{text}");
    assert!(text.contains("add_capability"), "{text}");
}

#[test]
fn decision_and_outcome_schemas_are_fresh() {
    for (name, emitted) in [
        ("decision.schema.json", pc_ir::decision_schema()),
        ("outcome.schema.json", pc_ir::outcome_schema()),
    ] {
        let committed =
            std::fs::read_to_string(workspace_root().join("schema").join(name)).unwrap();
        assert_eq!(
            committed,
            serde_json::to_string_pretty(&emitted).unwrap() + "\n",
            "regenerate {name} with pc schema"
        );
    }
}

/// Step 5: appending evidence flips a selection. The fixture's declared
/// guesses prefer via-fast; its evidence log measures fast as slow and slow
/// as quick, so compiling the directory picks via-slow while the bare plan
/// still picks via-fast.
#[test]
fn evidence_flips_selection() {
    let explain = |dir: &std::path::Path| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("explain")
            .arg("task:job")
            .arg(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    // The bare plan on its own: copy just plan.toml elsewhere, because the
    // fixture directory's sibling evidence.jsonl always loads with it.
    let bare_dir = scratch_dir("bare");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        bare_dir.join("plan.toml"),
    )
    .unwrap();
    let bare = explain(&bare_dir);
    assert!(bare.contains("uses method:via-fast"), "{bare}");
    std::fs::remove_dir_all(&bare_dir).unwrap();
    let measured = explain(
        &workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip"),
    );
    assert!(measured.contains("uses method:via-slow"), "{measured}");
}

/// Step 5: the provenance report shows predicted guesses beside measured
/// evidence, pinned by snapshot for both compile and explain.
#[test]
fn evidence_provenance_golden() {
    let mut compile = Command::cargo_bin("pc").unwrap();
    compile.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = compile
        .arg("compile")
        .arg("evidence-flip")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!(
        "evidence-provenance-compile",
        String::from_utf8(output.stdout).unwrap()
    );

    let mut explain = Command::cargo_bin("pc").unwrap();
    explain.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = explain
        .arg("explain")
        .arg("capability:fast@1")
        .arg("evidence-flip")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!(
        "evidence-provenance-explain",
        String::from_utf8(output.stdout).unwrap()
    );

    let mut explain = Command::cargo_bin("pc").unwrap();
    explain.current_dir(workspace_root().join("tests").join("fixtures"));
    let output = explain
        .arg("explain")
        .arg("capability:fast@1")
        .arg("evidence-flip")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["rows"].as_array().unwrap().len(), 2);
    let provenance = value["provenance"].as_array().unwrap();
    assert_eq!(provenance.len(), 1);
    assert_eq!(provenance[0]["capability"], "fast@1");
    assert_eq!(provenance[0]["observations"], 3);
    assert_eq!(provenance[0]["contradicts_declared"], true);
    insta::assert_snapshot!(
        "evidence-provenance-explain-json",
        String::from_utf8(output.stdout).unwrap()
    );
}

/// Step 6: `pc frontier` ranks open tasks, pending proposals and blocked
/// alternatives on the transcription fixture, pinned by snapshot.
#[test]
fn frontier_golden() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root());
    let output = cmd
        .arg("frontier")
        .arg("examples/transcription-substitution/plan.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    insta::assert_snapshot!("frontier", String::from_utf8(output.stdout).unwrap());

    // `--json` carries scores and kinds.
    let json = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("frontier")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(json.status.success());
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    // The only costed work is the E008-blocked hybrid, which can never be
    // accepted: it is listed once in the blocked group with no share or
    // score, and the open task has no estimate to rank.
    assert!(report["items"].as_array().unwrap().is_empty());
    let unestimated = report["unestimated"].as_array().unwrap();
    assert_eq!(unestimated.len(), 1);
    assert_eq!(unestimated[0]["kind"], "open-task");
    let blocked = report["blocked"].as_array().unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["subject"], "method:via-hybrid");
    assert_eq!(blocked[0]["kind"], "blocked-alternative");
    assert!(blocked[0]["share"].is_null() && blocked[0]["score"].is_null());

    // The frontier is deterministic: two runs give byte-identical JSON.
    let again = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("frontier")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(json.stdout, again.stdout);
}

/// An E008-blocked method never ranks against acceptable work: with a
/// cheap contract-clean proposal elsewhere, the blocked hybrid still sits
/// once in the blocked group with no share, and no ranked share exceeds 1.
#[test]
fn blocked_alternatives_stay_out_of_the_ranking() {
    let dir = scratch_dir("frontier-blocked");
    let source = workspace_root()
        .join("examples")
        .join("transcription-substitution");
    std::fs::copy(source.join("evidence.jsonl"), dir.join("evidence.jsonl")).unwrap();
    let mut plan = std::fs::read_to_string(source.join("plan.toml")).unwrap();
    plan.push_str(
        r#"
[[capability]]
id = "picker@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 4.0, likely = 5.0, high = 6.0, provenance = "declared" }]

[[method]]
id = "pick-lines"
task = "select-lines"
kind = "invoke"
capability = "picker@1"
status = "proposed"
provenance = "agent:test"
"#,
    );
    std::fs::write(dir.join("plan.toml"), plan).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&pc_ok(&dir, &["frontier", "--json"])).unwrap();
    let subjects = |group: &str| -> Vec<String> {
        report[group]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["subject"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        subjects("items"),
        ["method:pick-lines", "task:select-lines"],
        "{report}"
    );
    assert_eq!(subjects("blocked"), ["method:via-hybrid"], "{report}");
    assert!(report["blocked"][0]["share"].is_null(), "{report}");
    assert!(
        report["blocked"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("transcript.segment_confidence (consumed by select-lines)"),
        "{report}"
    );
    for item in report["items"].as_array().unwrap() {
        assert!(item["share"].as_f64().unwrap() <= 1.0, "{report}");
    }
}

/// Step 6, criterion (a): `pc impact` with the naive accept flags the lost
/// `segment_confidence` output and names its consumer, without editing files.
#[test]
fn impact_flags_dropped_output_with_consumers() {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("impact")
        .arg("examples/transcription-substitution/changes/accept-hybrid.json")
        .arg("examples/transcription-substitution/plan.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the diagnostics are the finding, not a failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("applied: accepted method:via-hybrid"),
        "{stdout}"
    );
    assert!(
        stdout.contains("affected tasks: transcribe-footage, select-lines, ship-reels"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "contract method:via-hybrid: would drop transcript.segment_confidence (consumed by task:select-lines)"
        ),
        "{stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("error[E008]"), "{stderr}");
    assert!(stderr.contains("transcript.segment_confidence"), "{stderr}");

    // The JSON report carries the same contract note structurally.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("impact")
        .arg("examples/transcription-substitution/changes/accept-hybrid.json")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(
        report["applied"],
        serde_json::json!(["accepted method:via-hybrid"])
    );
    assert_eq!(
        report["affected_tasks"],
        serde_json::json!(["transcribe-footage", "select-lines", "ship-reels"])
    );
    let notes = report["contract_notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["method"], "via-hybrid");
    assert_eq!(
        notes[0]["missing_outputs"],
        serde_json::json!(["transcript.segment_confidence"])
    );
    assert_eq!(notes[0]["consumers"], serde_json::json!(["select-lines"]));
}

/// Gap G2: `pc check` flags a proposed contract violation informationally.
/// The synthetic transcription fixture stays `ok` (exit 0) but reports the
/// same dropped field and consumer the frontier blocked-alternative and the
/// impact contract note name.
#[test]
fn check_flags_proposed_contract_violation_without_failing() {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("check")
        .arg("examples/transcription-substitution/plan.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "check notes never fail the plan: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ok:"), "{stdout}");
    assert!(
        stdout.contains(
            "contract method:via-hybrid: would drop transcript.segment_confidence (consumed by task:select-lines)"
        ),
        "{stdout}"
    );
    assert!(
        String::from_utf8(output.stderr).unwrap().is_empty(),
        "notes go to stdout, errors to stderr"
    );

    // `--json` carries the same note structurally, beside an empty
    // diagnostics list so `ok` stays true.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("check")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["diagnostics"].as_array().unwrap().len(), 0);
    let notes = report["contract_notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["method"], "via-hybrid");
    assert_eq!(notes[0]["task"], "transcribe-footage");
    assert_eq!(notes[0]["capability"], "voz-hybrid@1");
    assert_eq!(
        notes[0]["missing_outputs"],
        serde_json::json!(["transcript.segment_confidence"])
    );
    assert_eq!(notes[0]["consumers"], serde_json::json!(["select-lines"]));
    let trace = report["trace"].as_array().unwrap();
    assert!(
        trace.iter().any(|e| e["pass"] == "check-notes"
            && e["subject"] == "method:via-hybrid"
            && e["decision"] == "note"),
        "notes are traced"
    );
}

/// Step 6: `pc impact` refuses changes that name nothing or recommit work.
#[test]
fn impact_rejects_bad_changes() {
    let dir = scratch_dir("impact-bad");
    let change = dir.join("change.json");
    let run = |body: &str| {
        std::fs::write(&change, body).unwrap();
        Command::cargo_bin("pc")
            .unwrap()
            .current_dir(workspace_root())
            .arg("impact")
            .arg(&change)
            .arg("examples/transcription-substitution/plan.toml")
            .output()
            .unwrap()
    };
    let output = run(r#"{"accept_method": ["ghost"]}"#);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("invalid change: change names unknown method `ghost`")
    );
    let output = run(r#"{"accept_method": ["via-whisperx"]}"#);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("only proposed or derived methods can be accepted")
    );
    let output = run(r#"{"accept_method": "via-hybrid"}"#);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("cannot parse")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 6: `pc impact` keeps accuracy distance apart from failure risk and
/// labels it as a distance from the incumbent in the totals movement.
#[test]
fn impact_labels_accuracy_distance() {
    let dir = scratch_dir("impact-distance");
    std::fs::copy(
        workspace_root().join("examples/transcription-substitution/decided.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let evidence: Vec<String> = [
        ("duration_minutes", 3.0, "minutes"),
        ("duration_minutes", 4.0, "minutes"),
        ("duration_minutes", 5.0, "minutes"),
        ("accuracy_distance_from_incumbent", 0.08, "ratio"),
        ("accuracy_distance_from_incumbent", 0.12, "ratio"),
    ]
    .iter()
    .map(|(metric, value, unit)| {
        format!(
            r#"{{"subject":"capability:voz-hybrid@1","metric":"{metric}","value":{value},"unit":"{unit}","time":"2026-09-11T00:00:00Z","source":"distance-from-incumbent","artifact_hash":null}}"#
        )
    })
    .collect();
    let change = dir.join("change.json");
    std::fs::write(
        &change,
        format!(r#"{{"add_evidence": [{}]}}"#, evidence.join(",")),
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("impact")
        .arg(&change)
        .arg(dir.join("plan.toml"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("selection transcribe-footage: via-whisperx -> via-hybrid"),
        "{text}"
    );
    assert!(
        text.contains(
            "accuracy_distance_from_incumbent (distance from the incumbent, not absolute accuracy): expected 0.000 -> 0.100 (+0.100)"
        ),
        "{text}"
    );
    assert!(!text.contains("failure_risk"), "{text}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 6: `pc diff` between the before and decided plans shows the status
/// flip, the selection flip and the makespan movement, pinned by snapshot.
#[test]
fn diff_before_after_decision() {
    let mut cmd = Command::cargo_bin("pc").unwrap();
    cmd.current_dir(workspace_root());
    let output = cmd
        .arg("diff")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("examples/transcription-substitution/decided.toml")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("method status via-hybrid: proposed -> accepted"),
        "{text}"
    );
    assert!(
        text.contains("selection transcribe-footage: via-whisperx -> via-hybrid"),
        "{text}"
    );
    assert!(
        text.contains("makespan: 12.000 -> 4.000 minutes (-8.000)"),
        "{text}"
    );
    assert!(
        text.contains(
            "accuracy_distance_from_incumbent (distance from the incumbent, not absolute accuracy): expected 0.000 -> 0.100 (+0.100)"
        ),
        "{text}"
    );
    assert!(!text.contains("failure_risk"), "{text}");
    insta::assert_snapshot!("transcription-diff", text);

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("diff")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("examples/transcription-substitution/decided.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["method_status_changes"],
        serde_json::json!([{"method": "via-hybrid", "before": "proposed", "after": "accepted"}])
    );
    assert_eq!(report["makespan_before"], serde_json::json!(12.0));
    assert_eq!(report["makespan_after"], serde_json::json!(4.0));
    assert_eq!(report["makespan_delta"], serde_json::json!(-8.0));

    // Identical plans report no differences.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("diff")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("examples/transcription-substitution/plan.toml")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("no differences")
    );
}

/// Step 6 proof target: the synthetic transcription-substitution fixture
/// passes report criteria (a)-(c).
#[test]
fn transcription_substitution_criteria() {
    // (b) P6 prefers the hybrid on recorded duration evidence: the decided
    // plan compiles to via-hybrid with evidence, and falls back to
    // via-whisperx on declared guesses alone.
    let explain = |target: &str, plan: &std::path::Path| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("explain")
            .arg(target)
            .arg(plan)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let decided = workspace_root().join("examples/transcription-substitution/decided.toml");
    let text = explain("task:transcribe-footage", &decided);
    assert!(text.contains("uses method:via-hybrid"), "{text}");

    // The bare decided plan without its evidence log still prefers the
    // incumbent on declared guesses, so the flip above is evidence-driven.
    let bare_dir = scratch_dir("decided-bare");
    std::fs::copy(&decided, bare_dir.join("plan.toml")).unwrap();
    let bare = explain("task:transcribe-footage", &bare_dir.join("plan.toml"));
    assert!(bare.contains("uses method:via-whisperx"), "{bare}");
    std::fs::remove_dir_all(&bare_dir).unwrap();

    // (b) continued: hybrid accuracy is labelled as distance from the
    // incumbent, never as absolute accuracy.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("explain")
        .arg("capability:voz-hybrid@1")
        .arg(&decided)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let provenance = value["provenance"].as_array().unwrap();
    let distance = provenance
        .iter()
        .find(|row| row["metric"] == "accuracy_distance_from_incumbent")
        .expect("accuracy distance provenance");
    assert_eq!(distance["observations"], 2);
    assert_eq!(
        distance["sources"],
        serde_json::json!(["distance-from-incumbent"])
    );
    let risk = provenance
        .iter()
        .find(|row| row["metric"] == "failure_risk")
        .expect("failure_risk provenance");
    assert_eq!(risk["basis"], "declared", "{risk}");
    assert_eq!(risk["measurement_basis"], serde_json::Value::Null, "{risk}");
    assert_eq!(risk["observations"], 0, "{risk}");
    let text = explain("method:via-hybrid", &decided);
    assert!(
        text.contains("measured as distance from the incumbent, not as absolute accuracy"),
        "{text}"
    );

    // (c) Every trade-off the #1179 ruling names is present in pc explain:
    // speed, distance from WhisperX, fallback triggers, confidence gap.
    for phrase in [
        "Far faster",
        "distance from WhisperX",
        "fallback triggers",
        "confidence gap is made LOUD",
    ] {
        assert!(text.contains(phrase), "missing `{phrase}` in:\n{text}");
    }
}

/// Gap G3: `explain method:` joins one view from the method's assumptions,
/// its capability's measurement basis and its contract exposure, so one
/// command carries what used to take `explain method` plus `explain
/// capability` plus `pc impact`. Synthetic fixture only; no Ren figures.
#[test]
fn explain_method_joins_provenance_and_contract() {
    let explain = |target: &str, plan: &std::path::Path, json: bool| {
        let mut cmd = Command::cargo_bin("pc").unwrap();
        cmd.arg("explain").arg(target).arg(plan);
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let decided = workspace_root().join("examples/transcription-substitution/decided.toml");
    let proposed = workspace_root().join("examples/transcription-substitution/plan.toml");

    // Accepted method: assumptions beside the invoked capability's live
    // measured-vs-predicted provenance with sources and basis labels.
    let text = explain("method:via-hybrid", &decided, false);
    assert!(
        text.starts_with("method:via-hybrid: status (accepted)\n"),
        "{text}"
    );
    assert!(text.contains("method:via-hybrid: assumes ("), "{text}");
    assert!(
        text.contains(
            "provenance voz-hybrid@1 duration_minutes: predicted 13.000/15.000/19.000 vs measured 3.000/4.000/5.000 (n=3, sources: reel-eval) [contradicts declared] [basis none]"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "provenance voz-hybrid@1 accuracy_distance_from_incumbent: predicted none vs measured 0.080/0.100/0.120 (n=2, sources: distance-from-incumbent) [basis none]"
        ),
        "{text}"
    );
    // The decided contract holds the hybrid's outputs, so no exposure line.
    assert!(!text.contains("contract method:via-hybrid"), "{text}");

    // Proposed method: the same assumptions (the trace only walks the
    // accepted plan, so these were missing before the join) beside the
    // staged holdings and the contract exposure, worded exactly as the
    // impact preview and frontier entry word it.
    let text = explain("method:via-hybrid", &proposed, false);
    assert!(
        text.starts_with("method:via-hybrid: status (proposed)\n"),
        "{text}"
    );
    assert!(text.contains("method:via-hybrid: assumes ("), "{text}");
    assert!(
        text.contains(
            "staged capability:voz-hybrid@1 duration_minutes: 3 observations staged for proposal method:via-hybrid, not counted until accepted"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "contract method:via-hybrid: would drop transcript.segment_confidence (consumed by task:select-lines)"
        ),
        "{text}"
    );

    // The JSON view carries the same join structurally.
    let value: serde_json::Value =
        serde_json::from_str(&explain("method:via-hybrid", &decided, true)).unwrap();
    let provenance = value["provenance"].as_array().unwrap();
    assert_eq!(provenance.len(), 3);
    assert!(
        provenance
            .iter()
            .all(|row| row["capability"] == "voz-hybrid@1"),
        "{provenance:?}"
    );
    assert_eq!(value["contract_notes"].as_array().unwrap().len(), 0);
    let value: serde_json::Value =
        serde_json::from_str(&explain("method:via-hybrid", &proposed, true)).unwrap();
    let rows = value["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["decision"], "status");
    assert_eq!(rows[0]["reason"], "proposed");
    let notes = value["contract_notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["method"], "via-hybrid");
    assert_eq!(
        notes[0]["missing_outputs"],
        serde_json::json!(["transcript.segment_confidence"])
    );
    assert_eq!(notes[0]["consumers"], serde_json::json!(["select-lines"]));

    // A rejected method is ignored by every pass, so the joined view refuses
    // it instead of surfacing its contract exposure as a live concern.
    let dir = scratch_dir("explain-rejected");
    std::fs::copy(&proposed, dir.join("plan.toml")).unwrap();
    assert_eq!(
        pc_ok(&dir, &["reject", "via-hybrid"]),
        "rejected method:via-hybrid\n"
    );
    let stderr = pc_fails(&dir, &["explain", "method:via-hybrid"]);
    assert!(
        stderr.contains("`method:via-hybrid` is rejected and ignored by every pass"),
        "{stderr}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// R5: `explain task:` joins the task's selection, its slack/criticality
/// timing and the frontier entries touching it (the same entries `pc
/// context` assembles), with text and `--json` agreeing. Other subject
/// kinds are out of scope: their output stays byte-identical.
#[test]
fn explain_task_shows_selection_timing_and_frontier() {
    let explain = |target: &str, plan: &std::path::Path, json: bool| {
        let mut cmd = Command::cargo_bin("pc").unwrap();
        cmd.arg("explain").arg(target).arg(plan);
        if json {
            cmd.arg("--json");
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let context = |task: &str, plan: &std::path::Path| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("context")
            .arg(task)
            .arg(plan)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let decided = workspace_root().join("examples/transcription-substitution/decided.toml");
    let proposed = workspace_root().join("examples/transcription-substitution/plan.toml");

    // Accepted critical leaf: the trace rows carry selection and timing,
    // each stated exactly once in text.
    let text = explain("task:transcribe-footage", &decided, false);
    let count = |text: &str, needle: &str| text.matches(needle).count();
    assert_eq!(
        count(
            &text,
            "task:transcribe-footage: accepted (uses method:via-hybrid)"
        ),
        1,
        "{text}"
    );
    assert_eq!(count(&text, "via-hybrid"), 1, "{text}");
    assert_eq!(
        count(
            &text,
            "task:transcribe-footage: critical (slack 0.000 minutes;"
        ),
        1,
        "{text}"
    );
    assert_eq!(count(&text, "slack"), 1, "{text}");
    let value: serde_json::Value =
        serde_json::from_str(&explain("task:transcribe-footage", &decided, true)).unwrap();
    assert_eq!(value["selection"], "via-hybrid");
    assert_eq!(value["timing"]["critical"], true);
    assert_eq!(value["timing"]["slack"], 0.0);
    assert_eq!(value["frontier"].as_array().unwrap().len(), 0);
    // Text and JSON agree on the timing numbers.
    assert!(
        text.contains(&format!(
            "earliest {:.3}-{:.3}",
            value["timing"]["earliest_start"].as_f64().unwrap(),
            value["timing"]["earliest_finish"].as_f64().unwrap(),
        )),
        "{text}"
    );

    // Unresolved open task: the frontier entry touching it renders beside
    // the unresolved row, identically to `pc context`.
    let text = explain("task:emit-confidence-notice", &decided, false);
    assert_eq!(
        count(
            &text,
            "task:emit-confidence-notice: unresolved (no accepted method)"
        ),
        1,
        "{text}"
    );
    assert_eq!(count(&text, "unresolved"), 1, "{text}");
    let context_text = context("emit-confidence-notice", &decided);
    let context_frontier: Vec<&str> = context_text
        .lines()
        .filter(|line| line.starts_with("frontier "))
        .collect();
    assert_eq!(context_frontier.len(), 1);
    assert!(
        text.contains(context_frontier[0]),
        "explain misses context frontier line:\n{text}\n{context_text}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&explain("task:emit-confidence-notice", &decided, true)).unwrap();
    assert_eq!(value["selection"], serde_json::Value::Null);
    assert_eq!(value["timing"], serde_json::Value::Null);
    let frontier = value["frontier"].as_array().unwrap();
    assert_eq!(frontier.len(), 1);
    assert_eq!(frontier[0]["subject"], "task:emit-confidence-notice");
    assert_eq!(frontier[0]["kind"], "open-task");
    // The text frontier line carries the same reason the JSON does.
    assert!(
        text.contains(frontier[0]["reason"].as_str().unwrap()),
        "{text}"
    );

    // A task touched by a blocked proposal surfaces the blocked
    // alternative, worded exactly as the context entry words it.
    let text = explain("task:transcribe-footage", &proposed, false);
    assert!(
        text.contains(
            "frontier method:via-hybrid [blocked-alternative] (critical): accepting it would drop transcript.segment_confidence (consumed by select-lines)"
        ),
        "{text}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&explain("task:transcribe-footage", &proposed, true)).unwrap();
    let frontier = value["frontier"].as_array().unwrap();
    assert!(
        frontier
            .iter()
            .any(|item| item["subject"] == "method:via-hybrid"
                && item["kind"] == "blocked-alternative"),
        "{frontier:?}"
    );

    // Out of scope guard: method and capability views carry no task join.
    let value: serde_json::Value =
        serde_json::from_str(&explain("method:via-hybrid", &decided, true)).unwrap();
    for key in ["selection", "timing", "frontier"] {
        assert_eq!(value.get(key), None, "{key} leaked into method view");
    }
    let value: serde_json::Value =
        serde_json::from_str(&explain("capability:whisperx@3", &decided, true)).unwrap();
    for key in ["selection", "timing", "frontier"] {
        assert_eq!(value.get(key), None, "{key} leaked into capability view");
    }

    // Unknown tasks still fail like before.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("explain")
        .arg("task:nope")
        .arg(&decided)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("no explanation for `task:nope` in reachable accepted plan"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A scratch plan directory for `pc record` tests, under the system temp
/// dir so the worktree stays clean.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pc-record-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Copy the proposal-loop fixture to a scratch dir for the step-7 tests,
/// which edit the plan file.
fn loop_scratch(name: &str) -> PathBuf {
    let dir = scratch_dir(name);
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("valid-proposal-loop.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    dir
}

/// Run `pc` with `dir` as the plan path and return stdout, asserting success.
fn pc_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .args(args)
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "pc {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Run `pc` with `dir` as the plan path and return stderr, asserting failure.
fn pc_fails(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::cargo_bin("pc")
        .unwrap()
        .args(args)
        .arg(dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "pc {args:?}");
    String::from_utf8(output.stderr).unwrap()
}

/// Write a patch body to the scratch dir and return its path.
fn write_patch(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

/// Step 7: an end-to-end script that plays an agent. Round 1 decomposes an
/// open task (propose, recompile, accept); round 2 binds the leaves to
/// capabilities and compiles to a fully selected plan.
#[test]
fn proposal_loop_agent_play() {
    let dir = loop_scratch("agent-play");
    let plan = dir.join("plan.toml");

    // Round 1: the agent decomposes `draft` into outline-first-then-write.
    let patch = write_patch(
        &dir,
        "round1.json",
        r#"{
          "add_task": [
            {"id": "outline", "title": "Outline", "state": "open",
             "produces": [{"artifact": "doc", "fields": ["notes"]}]},
            {"id": "write", "title": "Write", "state": "open",
             "consumes": [{"artifact": "doc", "fields": ["notes"]}],
             "produces": [{"artifact": "doc", "fields": ["text"]}] }
          ],
          "add_method": [
            {"id": "draft-plan", "task": "draft", "kind": "decompose",
             "subtasks": ["outline", "write"],
             "order_edges": [{"from": "outline", "to": "write"}],
             "status": "proposed", "provenance": "agent:e2e",
             "assumptions": ["Outline first, then write to its notes."]}
          ]
        }"#,
    );
    let proposed = pc_ok(&dir, &["propose", patch.to_str().unwrap()]);
    assert!(
        proposed.contains("proposed method:draft-plan\n"),
        "{proposed}"
    );
    assert!(proposed.contains("proposed task:outline\n"), "{proposed}");
    assert!(proposed.contains("proposed task:write\n"), "{proposed}");

    // Recompile: selection still runs over accepted methods only, so the
    // staged work changes nothing yet — but the plan still compiles, the
    // frontier ranks the pending proposal, and the context already shows
    // the proposed children.
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("unresolved: draft, review\n"),
        "{compiled}"
    );
    let frontier = pc_ok(&dir, &["frontier"]);
    assert!(frontier.contains("method:draft-plan"), "{frontier}");
    let slice = pc_ok(&dir, &["context", "draft"]);
    assert!(slice.contains("child outline: Outline [open]"), "{slice}");
    assert!(slice.contains("child write: Write [open]"), "{slice}");

    // Accept the decomposition. The plan stays clean: the hierarchy is
    // acyclic and every consumed field is still produced.
    let decided = pc_ok(&dir, &["accept", "draft-plan"]);
    assert_eq!(decided, "accepted method:draft-plan\n");
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("unresolved: outline, review, write\n"),
        "{compiled}"
    );

    // Round 2: bind the leaves. New capabilities arrive in the same patch as
    // the invoke methods that call them.
    let patch = write_patch(
        &dir,
        "round2.json",
        r#"{
          "add_capability": [
            {"id": "noter@1", "kind": "tool", "version": "1",
             "produces": [{"artifact": "doc", "fields": ["notes"]}],
             "estimates": [{"metric": "duration_minutes", "low": 1.0, "likely": 2.0, "high": 3.0, "provenance": "declared"}]},
            {"id": "reviewer@1", "kind": "tool", "version": "1",
             "consumes": [{"artifact": "doc", "fields": ["text"]}],
             "estimates": [{"metric": "duration_minutes", "low": 1.0, "likely": 1.0, "high": 1.0, "provenance": "declared"}]}
          ],
          "add_method": [
            {"id": "outline-run", "task": "outline", "kind": "invoke", "capability": "noter@1",
             "status": "proposed", "provenance": "agent:e2e"},
            {"id": "write-run", "task": "write", "kind": "invoke", "capability": "writer@1",
             "status": "proposed", "provenance": "agent:e2e"},
            {"id": "review-run", "task": "review", "kind": "invoke", "capability": "reviewer@1",
             "status": "proposed", "provenance": "agent:e2e"}
          ]
        }"#,
    );
    let proposed = pc_ok(&dir, &["propose", patch.to_str().unwrap()]);
    assert!(
        proposed.contains("proposed capability:noter@1\n"),
        "{proposed}"
    );
    for method in ["outline-run", "write-run", "review-run"] {
        assert_eq!(
            pc_ok(&dir, &["accept", method]),
            format!("accepted method:{method}\n")
        );
    }

    // Fully selected: outline runs, then write (2.0 + 4.0 expected minutes),
    // with review in parallel.
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains(
            "makespan low 3.000, likely 6.000, high 9.000, expected 6.000 (optimistic) minutes"
        ),
        "{compiled}"
    );
    assert!(!compiled.contains("unresolved:"), "{compiled}");
    let json = pc_ok(&dir, &["compile", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["selection"]["draft"], "draft-plan");
    assert_eq!(value["selection"]["outline"], "outline-run");
    assert_eq!(value["selection"]["write"], "write-run");
    assert_eq!(value["selection"]["review"], "review-run");

    // The file holds exactly the staged and decided work: P3 derived ids
    // were never written back.
    let check = pc_ok(&dir, &["check", "--json"]);
    let report: serde_json::Value = serde_json::from_str(&check).unwrap();
    assert_eq!(report["counts"]["tasks"], 5);
    assert_eq!(report["counts"]["methods"], 5);
    assert_eq!(report["counts"]["capabilities"], 3);
    let text = std::fs::read_to_string(&plan).unwrap();
    assert!(!text.contains("match-"), "{text}");

    insta::assert_snapshot!("proposal-loop-context", slice);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 7: `pc propose` refuses invalid patches and leaves the file
/// byte-identical.
#[test]
fn propose_refuses_invalid_patches() {
    let dir = loop_scratch("propose-bad");
    let plan = dir.join("plan.toml");
    let before = std::fs::read_to_string(&plan).unwrap();
    let propose = |name: &str, body: &str| {
        let patch = write_patch(&dir, name, body);
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("propose")
            .arg(&patch)
            .arg(&dir)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{name}");
        String::from_utf8(output.stderr).unwrap()
    };

    let stderr = propose("empty.json", "{}");
    assert!(
        stderr.contains("invalid patch: patch adds nothing"),
        "{stderr}"
    );
    let stderr = propose(
        "unknown-task.json",
        r#"{"add_method": [
            {"id": "m", "task": "ghost", "kind": "invoke", "capability": "writer@1",
             "status": "proposed", "provenance": "agent:e2e"}
        ]}"#,
    );
    assert!(
        stderr.contains("invalid patch: method `m` names unknown task `ghost`"),
        "{stderr}"
    );
    let stderr = propose(
        "committed.json",
        r#"{"add_method": [
            {"id": "m", "task": "draft", "kind": "invoke", "capability": "writer@1",
             "status": "accepted", "provenance": "agent:e2e"}
        ]}"#,
    );
    assert!(stderr.contains("must be proposed in a patch"), "{stderr}");
    let stderr = propose(
        "unknown-field.json",
        r#"{"add_task": [{"id": "a", "title": "A", "state": "open",
            "produces": [{"artifact": "doc", "fields": ["ghost"]}]}]}"#,
    );
    assert!(
        stderr.contains("uses unknown field `doc.ghost`"),
        "{stderr}"
    );
    let stderr = propose("not-json.json", "{");
    assert!(stderr.contains("cannot parse"), "{stderr}");

    // Every refusal above wrote nothing.
    assert_eq!(std::fs::read_to_string(&plan).unwrap(), before);

    // A valid patch applies once; proposing it again collides with the plan.
    let patch = write_patch(
        &dir,
        "once.json",
        r#"{"add_task": [{"id": "solo", "title": "Solo", "state": "open"}]}"#,
    );
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("propose")
        .arg(&patch)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("propose")
        .arg(&patch)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("task `solo` already exists in the plan"),
        "{stderr}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 7: `pc accept` and `pc reject` refuse unknown, committed and
/// contract-breaking decisions; `reject` and derived accepts stick.
#[test]
fn decide_refuses_bad_decisions() {
    let dir = loop_scratch("decide-bad");
    let stderr = pc_fails(&dir, &["accept", "ghost"]);
    assert!(stderr.contains("unknown method `ghost`"), "{stderr}");
    let stderr = pc_fails(&dir, &["accept", "root-all"]);
    assert!(
        stderr.contains("only proposed or derived methods can be accepted"),
        "{stderr}"
    );
    let stderr = pc_fails(&dir, &["reject", "root-all"]);
    assert!(
        stderr.contains("only proposed or derived methods can be rejected"),
        "{stderr}"
    );

    // A P3 derived proposal materializes into the file as accepted, and the
    // matcher does not propose it again.
    assert_eq!(
        pc_ok(&dir, &["accept", "match-draft-writer@1"]),
        "accepted method:match-draft-writer@1\n"
    );
    let text = std::fs::read_to_string(dir.join("plan.toml")).unwrap();
    assert!(text.contains("match-draft-writer@1"), "{text}");
    let explained = pc_ok(&dir, &["explain", "task:draft"]);
    assert!(
        explained.contains("uses method:match-draft-writer@1"),
        "{explained}"
    );
    std::fs::remove_dir_all(&dir).unwrap();

    // Accepting a substitution that drops a contracted output is refused
    // with the fresh E008 diagnostics, and the file is untouched.
    let dir = scratch_dir("decide-lossy");
    std::fs::copy(
        workspace_root().join("examples/transcription-substitution/plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let before = std::fs::read_to_string(dir.join("plan.toml")).unwrap();
    let stderr = pc_fails(&dir, &["accept", "via-hybrid"]);
    assert!(stderr.contains("error[E008]"), "{stderr}");
    assert!(stderr.contains("transcript.segment_confidence"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("plan.toml")).unwrap(),
        before
    );

    // Rejecting the same proposal sticks: accepting it afterwards is refused.
    assert_eq!(
        pc_ok(&dir, &["reject", "via-hybrid"]),
        "rejected method:via-hybrid\n"
    );
    let stderr = pc_fails(&dir, &["accept", "via-hybrid"]);
    assert!(
        stderr.contains("only proposed or derived methods can be accepted"),
        "{stderr}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 7: a decision on a plan with a sibling evidence log never copies the
/// log's observations into the plan file, so P5 counts each one once.
#[test]
fn decide_keeps_evidence_log_out_of_the_plan() {
    let dir = scratch_dir("decide-evidence");
    let example = workspace_root().join("examples/transcription-substitution");
    for name in ["plan.toml", "evidence.jsonl"] {
        std::fs::copy(example.join(name), dir.join(name)).unwrap();
    }
    let log = std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap();
    let evidence = |dir: &std::path::Path| -> Vec<String> {
        pc_ok(dir, &["context", "transcribe-footage"])
            .lines()
            .filter(|line| line.starts_with("evidence "))
            .map(str::to_string)
            .collect()
    };
    let whisperx =
        "evidence whisperx@3 duration_minutes: 11.000/12.000/13.000 (n=3) [basis none]".to_string();
    assert!(evidence(&dir).contains(&whisperx), "{:?}", evidence(&dir));

    assert_eq!(
        pc_ok(&dir, &["reject", "via-hybrid"]),
        "rejected method:via-hybrid\n"
    );
    let plan = pc_ir::from_toml(&std::fs::read_to_string(dir.join("plan.toml")).unwrap()).unwrap();
    assert!(plan.evidence.is_empty(), "{:?}", plan.evidence);
    assert_eq!(
        std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap(),
        log
    );
    assert!(evidence(&dir).contains(&whisperx), "{:?}", evidence(&dir));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 7: the loop works on JSON plans too, preserving the on-disk format.
#[test]
fn json_plan_round_trips_propose_and_accept() {
    let dir = scratch_dir("json-loop");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("proposal-loop-json")
            .join("plan.json"),
        dir.join("plan.json"),
    )
    .unwrap();
    let patch = write_patch(
        &dir,
        "patch.json",
        r#"{"add_task": [{"id": "solo", "title": "Solo", "state": "open"}],
            "add_method": [
            {"id": "draft-plan", "task": "draft", "kind": "decompose",
             "subtasks": ["solo"], "status": "proposed", "provenance": "agent:e2e"}
        ]}"#,
    );
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("propose")
        .arg(&patch)
        .arg(&dir)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["applied"],
        serde_json::json!(["proposed method:draft-plan", "proposed task:solo",])
    );
    assert!(value["plan"].as_str().unwrap().ends_with("plan.json"));

    assert_eq!(
        pc_ok(&dir, &["accept", "draft-plan"]),
        "accepted method:draft-plan\n"
    );
    let text = std::fs::read_to_string(dir.join("plan.json")).unwrap();
    assert!(text.contains("\"id\": \"draft-plan\""), "{text}");
    assert!(!text.contains("match-"), "{text}");
    pc_ok(&dir, &["check"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Step 7: `pc context` renders the slice for one task and fits it to the
/// token budget.
#[test]
fn context_budget_fits_and_truncates() {
    let plan = "examples/transcription-substitution/plan.toml";
    let run = |args: &[&str]| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .current_dir(workspace_root())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let text = run(&["context", "transcribe-footage", plan]);
    assert!(
        text.contains("task transcribe-footage: Transcribe footage [resolved]"),
        "{text}"
    );
    assert!(text.contains("method via-hybrid [proposed"), "{text}");
    assert!(
        text.contains("parent ship-reels (Ship the reels) via reels-pipeline [accepted]"),
        "{text}"
    );
    assert!(
        text.contains("frontier method:via-hybrid [blocked-alternative]"),
        "{text}"
    );
    insta::assert_snapshot!("proposal-loop-slice", text);

    let json = run(&["context", "transcribe-footage", plan, "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["subject"], "task:transcribe-footage");
    assert_eq!(value["truncated"], false);
    assert_eq!(value["budget_tokens"], 2000);
    assert_eq!(value["methods"].as_array().unwrap().len(), 2);
    assert_eq!(value["capabilities"].as_array().unwrap().len(), 2);

    // No budget limit is the same slice as the default budget.
    let huge = run(&[
        "context",
        "transcribe-footage",
        plan,
        "--json",
        "--budget",
        "1000000",
    ]);
    let huge_value: serde_json::Value = serde_json::from_str(&huge).unwrap();
    assert_eq!(huge_value["truncated"], false);
    assert_eq!(huge_value["methods"], value["methods"]);

    // A tiny budget keeps the focal task and drops everything else.
    let tiny = run(&[
        "context",
        "transcribe-footage",
        plan,
        "--json",
        "--budget",
        "10",
    ]);
    let tiny_value: serde_json::Value = serde_json::from_str(&tiny).unwrap();
    assert_eq!(tiny_value["truncated"], true);
    assert_eq!(tiny_value["task"]["id"], "transcribe-footage");
    assert!(tiny_value["methods"].as_array().unwrap().is_empty());
    assert!(tiny_value["frontier"].as_array().unwrap().is_empty());

    // A mid budget is a strict prefix of the full slice.
    let mid = run(&[
        "context",
        "transcribe-footage",
        plan,
        "--json",
        "--budget",
        "300",
    ]);
    let mid_value: serde_json::Value = serde_json::from_str(&mid).unwrap();
    assert_eq!(mid_value["truncated"], true);
    assert!(mid_value["tokens"].as_u64().unwrap() <= 300);
    assert!(!mid_value["methods"].as_array().unwrap().is_empty());

    // Deterministic: two runs give byte-identical JSON.
    let again = run(&[
        "context",
        "transcribe-footage",
        plan,
        "--json",
        "--budget",
        "300",
    ]);
    assert_eq!(mid, again);

    // Unknown tasks fail like `pc explain` on missing targets.
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("context")
        .arg("ghost")
        .arg(plan)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("plan declares no task `ghost`")
    );
}

/// Step 5: `pc record` appends one validated observation to the sibling
/// evidence log, and the next compile sees it.
#[test]
fn record_appends_evidence_and_moves_compilation() {
    let dir = scratch_dir("flip");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":30.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    )
    .unwrap();

    let selected = |dir: &std::path::Path| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("explain")
            .arg("task:job")
            .arg(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    assert!(
        selected(&dir).contains("uses method:via-fast"),
        "declared guesses prefer fast"
    );

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&observation)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("recorded capability:fast@1"), "{text}");
    assert!(text.contains(":1"), "{text}");
    let log = std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 1);
    let record: serde_json::Value = serde_json::from_str(log.lines().next().unwrap()).unwrap();
    assert_eq!(record["value"], 30.0);

    let text = selected(&dir);
    assert!(
        text.contains("uses method:via-slow"),
        "one recording flips the selection: {text}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc record` appends a `task:<id>` observation for a task with several
/// accepted capabilities but warns, and compile and explain list it as
/// unattributed instead of silently dropping it.
#[test]
fn ambiguous_task_evidence_warns_and_surfaces_as_unattributed() {
    let dir = scratch_dir("ambiguous");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let pc = |args: &[&str]| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .args(args)
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    let record = |subject: &str| {
        let observation = dir.join("observation.json");
        std::fs::write(
            &observation,
            format!(
                r#"{{"subject":"{subject}","metric":"duration_minutes","value":30.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}}"#
            ),
        )
        .unwrap();
        pc(&["record", observation.to_str().unwrap()])
    };

    let (stdout, stderr) = record("capability:fast@1");
    assert!(stdout.contains("recorded capability:fast@1"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");

    let (stdout, stderr) = record("task:job");
    assert!(stdout.contains("recorded task:job"), "{stdout}");
    assert_eq!(
        stderr,
        "warning: P5 will not attribute task:job evidence (its accepted invoke methods call 2 capabilities; record against one of method:via-fast, method:via-slow, capability:fast@1, capability:slow@1 instead)\n"
    );
    let log = std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 2);

    let line = "unattributed task:job duration_minutes: 1 observation ignored (its accepted invoke methods call 2 capabilities; record against one of method:via-fast, method:via-slow, capability:fast@1, capability:slow@1 instead)\n";
    let (compiled, _) = pc(&["compile"]);
    assert!(compiled.ends_with(line), "{compiled}");
    let (explained, _) = pc(&["explain", "task:job"]);
    assert!(explained.ends_with(line), "{explained}");
    let (explained, _) = pc(&["explain", "capability:slow@1"]);
    assert!(explained.ends_with(line), "{explained}");

    let expected = serde_json::json!([{
        "task": "job",
        "metric": "duration_minutes",
        "observations": 1,
        "capabilities": ["fast@1", "slow@1"],
        "methods": ["via-fast", "via-slow"],
    }]);
    let (json, _) = pc(&["compile", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["unattributed_tasks"], expected);
    let (json, _) = pc(&["explain", "task:job", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["unattributed_tasks"], expected);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc record` starts a new line when the existing log lacks a trailing
/// newline, so the log stays parseable and the reported line is right.
#[test]
fn record_appends_after_a_log_without_trailing_newline() {
    let fixture = workspace_root()
        .join("tests")
        .join("fixtures")
        .join("evidence-flip");
    let dir = scratch_dir("no-newline");
    std::fs::copy(fixture.join("plan.toml"), dir.join("plan.toml")).unwrap();
    let log = std::fs::read_to_string(fixture.join("evidence.jsonl")).unwrap();
    std::fs::write(dir.join("evidence.jsonl"), log.trim_end()).unwrap();
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":31.0,"unit":"minutes","time":"2026-10-08T03:00:00Z","source":"ci","artifact_hash":null}"#,
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&observation)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.trim_end().ends_with("evidence.jsonl:7"), "{stdout}");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("check")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap();
    assert_eq!(log.lines().count(), 7);
    let last: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(last["value"], 31.0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The `pc record` warning judges attribution on the checked plan, as
/// compile does: here P4 merges `b` into `a` and keeps both capabilities
/// accepted, so `task:b` evidence is ambiguous even though the raw plan
/// gives `b` a single accepted capability.
#[test]
fn record_warns_about_ambiguity_after_p4_merges() {
    let dir = scratch_dir("merged");
    std::fs::write(
        dir.join("plan.toml"),
        r#"ir_version = 0

[plan]
id = "merged"
title = "Merged ambiguity"

[objective]
root_task = "root"

[[artifact]]
name = "clip"
fields = ["frames"]

[[task]]
id = "root"
title = "Root"
state = "decomposed"

[[task]]
id = "a"
title = "Ingest"
state = "resolved"
produces = [{ artifact = "clip", fields = ["frames"] }]

[[task]]
id = "b"
title = "Ingest"
state = "resolved"
produces = [{ artifact = "clip", fields = ["frames"] }]

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["a", "b"]
status = "accepted"
provenance = "human:test"

[[method]]
id = "a-x"
task = "a"
kind = "invoke"
capability = "x@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "a-y"
task = "a"
kind = "invoke"
capability = "y@1"
status = "derived"
provenance = "human:test"

[[method]]
id = "b-y"
task = "b"
kind = "invoke"
capability = "y@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "b-x"
task = "b"
kind = "invoke"
capability = "x@1"
status = "derived"
provenance = "human:test"

[[capability]]
id = "x@1"
kind = "tool"
version = "1"
produces = [{ artifact = "clip", fields = ["frames"] }]
estimates = [{ metric = "duration_minutes", low = 1.0, likely = 2.0, high = 3.0, provenance = "declared" }]

[[capability]]
id = "y@1"
kind = "tool"
version = "1"
produces = [{ artifact = "clip", fields = ["frames"] }]
estimates = [{ metric = "duration_minutes", low = 1.0, likely = 2.0, high = 3.0, provenance = "declared" }]
"#,
    )
    .unwrap();
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"task:b","metric":"duration_minutes","value":30.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&observation)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("warning: P5 will not attribute task:a evidence"),
        "{stderr}"
    );

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("compile")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("unattributed task:a duration_minutes"),
        "{stdout}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc record` refuses unknown metrics, wrong units, bad timestamps,
/// malformed subjects and bases, and negative values instead of appending
/// them. Well-formed unknown subjects are staged as pending evidence, not
/// refused.
#[test]
fn record_rejects_invalid_evidence() {
    let dir = scratch_dir("invalid");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let record = |name: &str, body: &str| {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("record")
            .arg(&path)
            .arg(&dir)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{name}");
        String::from_utf8(output.stderr).unwrap()
    };
    let stderr = record(
        "metric.json",
        r#"{"subject":"capability:fast@1","metric":"vibes","value":1.0,"unit":"x","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    );
    assert!(stderr.contains("error[E010]"), "{stderr}");
    let stderr = record(
        "unit.json",
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":1.0,"unit":"hours","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    );
    assert!(stderr.contains("error[E014]"), "{stderr}");
    let stderr = record(
        "time.json",
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"yesterday","source":"ci","artifact_hash":null}"#,
    );
    assert!(stderr.contains("error[E014]"), "{stderr}");
    for subject in ["capabilty:fast@1", "fast@1", "capability:"] {
        let stderr = record(
            "subject.json",
            &format!(
                r#"{{"subject":"{subject}","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}}"#
            ),
        );
        assert!(
            stderr.contains("error[E014]") && stderr.contains("names nothing"),
            "{subject}: {stderr}"
        );
    }
    let stderr = record(
        "basis.json",
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","basis":"vibes","artifact_hash":null}"#,
    );
    assert!(stderr.contains("error[E014]"), "{stderr}");
    let stderr = record(
        "negative.json",
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":-100.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    );
    assert!(stderr.contains("error[E014]"), "{stderr}");
    assert!(
        !dir.join("evidence.jsonl").exists(),
        "nothing invalid was appended"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// G1: `pc record` stages evidence for subjects that name nothing yet as
/// pending instead of refusing it with E014.
#[test]
fn record_accepts_unknown_subjects_as_pending() {
    let dir = scratch_dir("pending");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"capability:ghost@9","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&observation)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("names nothing yet; P5 holds it as pending"),
        "{stderr}"
    );
    assert!(dir.join("evidence.jsonl").exists());
    // The staged record checks clean and moves no numbers.
    let check = pc_ok(&dir, &["check"]);
    assert!(check.contains("ok:"), "{check}");
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("pending capability:ghost@9 duration_minutes: 1 observation held"),
        "{compiled}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

const HYPOTHESIS_PLAN: &str = r#"ir_version = 0

[plan]
id = "hypothesis"
title = "Hypothesis"

[objective]
root_task = "job"
weights = { duration_minutes = 1.0 }

[[task]]
id = "job"
title = "Do the job"
state = "resolved"

[[method]]
id = "via-old"
task = "job"
kind = "invoke"
capability = "old@1"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "old@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 4.0, likely = 5.0, high = 6.0, provenance = "declared" }]
"#;

/// G4: one capability and metric carries one basis. `pc record` refuses a
/// record whose basis differs from the log's, naming both bases, and `pc
/// check` refuses a hand-edited log that mixes them.
#[test]
fn record_refuses_mixed_basis_for_one_subject_and_metric() {
    let dir = scratch_dir("basis-conflict");
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("evidence-flip")
            .join("plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let legacy = r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#;
    std::fs::write(dir.join("evidence.jsonl"), format!("{legacy}\n")).unwrap();
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":2.0,"unit":"minutes","time":"2026-10-08T01:00:00Z","source":"ci","basis":"absolute","artifact_hash":null}"#,
    )
    .unwrap();
    let stderr = pc_fails(&dir, &["record", observation.to_str().unwrap()]);
    assert!(
        stderr.contains("error[E014]")
            && stderr.contains("`absolute` (capability:fast@1) and `none` (capability:fast@1)")
            && stderr.contains("distinct metric"),
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap(),
        format!("{legacy}\n"),
        "nothing was appended"
    );
    // A basis-less record pools with the basis-less log.
    std::fs::write(&observation, legacy).unwrap();
    pc_ok(&dir, &["record", observation.to_str().unwrap()]);
    // A hand-edited log mixing bases fails check the same way.
    std::fs::write(
        dir.join("evidence.jsonl"),
        format!(
            "{legacy}\n{}\n",
            legacy.replace(
                r#""artifact_hash""#,
                r#""basis":"distance-from:slow@1","artifact_hash""#
            )
        ),
    )
    .unwrap();
    let stderr = pc_fails(&dir, &["check"]);
    assert!(
        stderr.contains("error[E014]")
            && stderr.contains(
                "`distance-from:slow@1` (capability:fast@1) and `none` (capability:fast@1)"
            ),
        "{stderr}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

const HYPOTHESIS_PATCH: &str = r#"{
  "add_capability": [
    {"id": "new@1", "kind": "tool", "version": "1",
     "estimates": [{"metric": "duration_minutes", "low": 8.0, "likely": 10.0, "high": 12.0, "provenance": "declared"}]}
  ],
  "add_method": [
    {"id": "via-new", "task": "job", "kind": "invoke", "capability": "new@1",
     "status": "proposed", "provenance": "agent:e2e"}
  ]
}"#;

const HYPOTHESIS_OBSERVATION: &str = r#"{"subject":"capability:new@1","metric":"duration_minutes","value":2.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#;

/// G1 end to end: evidence recorded against a proposed capability is
/// staged (never moves the accepted totals), goes live on accept, and
/// stays staged on reject.
#[test]
fn proposal_evidence_lifecycle() {
    let dir = scratch_dir("hypothesis");
    std::fs::write(dir.join("plan.toml"), HYPOTHESIS_PLAN).unwrap();
    let patch = write_patch(&dir, "patch.json", HYPOTHESIS_PATCH);
    let observation = dir.join("observation.json");
    std::fs::write(&observation, HYPOTHESIS_OBSERVATION).unwrap();

    let proposed = pc_ok(&dir, &["propose", patch.to_str().unwrap()]);
    assert!(proposed.contains("proposed method:via-new"), "{proposed}");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&observation)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("staged with method:via-new"), "{stderr}");

    // Staged: the incumbent still holds at its declared 5 minutes.
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("expected 5.000 (predicted 5.000; measured 0/1)"),
        "{compiled}"
    );
    assert!(
        compiled.contains(
            "staged capability:new@1 duration_minutes: 1 observation staged for proposal method:via-new, not counted until accepted"
        ),
        "{compiled}"
    );
    // The agent reviewing the proposal sees the evidence behind it.
    let context = pc_ok(&dir, &["context", "job"]);
    assert!(
        context.contains(
            "staged capability:new@1 duration_minutes: 1 observation staged for proposal method:via-new, not counted until accepted"
        ),
        "{context}"
    );
    let context_json: serde_json::Value =
        serde_json::from_str(&pc_ok(&dir, &["context", "job", "--json"])).unwrap();
    assert_eq!(
        context_json["staged"][0]["capability"], "new@1",
        "{context_json}"
    );
    assert_eq!(
        context_json["staged"][0]["methods"][0], "via-new",
        "{context_json}"
    );
    for target in ["capability:new@1", "method:via-new"] {
        let explained = pc_ok(&dir, &["explain", target]);
        assert!(
            explained.contains("staged for proposal method:via-new, not counted until accepted"),
            "{target}: {explained}"
        );
        let explained: serde_json::Value =
            serde_json::from_str(&pc_ok(&dir, &["explain", target, "--json"])).unwrap();
        assert_eq!(
            explained["staged_evidence"][0]["observations"], 1,
            "{target}: {explained}"
        );
    }

    // Accept: the staged record goes live and the selection flips to the
    // measured 2 minutes (declared alone would say 10).
    let accepted = pc_ok(&dir, &["accept", "via-new"]);
    assert!(accepted.contains("accepted method:via-new"), "{accepted}");
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled
            .contains("expected 2.000 (predicted 10.000; measured 1/1; basis none; 1 observation)"),
        "{compiled}"
    );
    assert!(!compiled.contains("staged capability:new@1"), "{compiled}");
    std::fs::remove_dir_all(&dir).unwrap();

    // Reject: the hypothesis dies but its records stay staged, and the
    // accepted totals never moved.
    let dir = scratch_dir("hypothesis-reject");
    std::fs::write(dir.join("plan.toml"), HYPOTHESIS_PLAN).unwrap();
    let patch = write_patch(&dir, "patch.json", HYPOTHESIS_PATCH);
    let observation = dir.join("observation.json");
    std::fs::write(&observation, HYPOTHESIS_OBSERVATION).unwrap();
    pc_ok(&dir, &["propose", patch.to_str().unwrap()]);
    pc_ok(&dir, &["record", observation.to_str().unwrap()]);
    let rejected = pc_ok(&dir, &["reject", "via-new"]);
    assert!(rejected.contains("rejected method:via-new"), "{rejected}");
    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("expected 5.000 (predicted 5.000; measured 0/1)"),
        "{compiled}"
    );
    assert!(
        compiled.contains(
            "staged capability:new@1 duration_minutes: 1 observation staged with no proposal for capability:new@1 yet, not counted until accepted"
        ),
        "{compiled}"
    );
    // A live observation recorded beside the staged one warns about
    // nothing: warnings describe the new record, never the rest of the log.
    let live = dir.join("live.json");
    std::fs::write(
        &live,
        HYPOTHESIS_OBSERVATION.replace("capability:new@1", "capability:old@1"),
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("record")
        .arg(&live)
        .arg(&dir)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("warning"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// G5 end to end: fully measured compile totals label their triple as the
/// observed P5 range with its observation count, so a wide
/// machine-contention spread never reads as a point estimate.
/// Declared-only totals are unchanged.
#[test]
fn compile_totals_quote_measured_spread_as_range() {
    let dir = scratch_dir("observed-range");
    std::fs::write(
        dir.join("plan.toml"),
        r#"ir_version = 0

[plan]
id = "spread"
title = "Spread"

[objective]
root_task = "job"
weights = { duration_minutes = 1.0 }

[[task]]
id = "job"
title = "Do the job"
state = "resolved"

[[method]]
id = "via-c"
task = "job"
kind = "invoke"
capability = "c@1"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "c@1"
kind = "tool"
version = "1"
estimates = [
  { metric = "duration_minutes", low = 10.0, likely = 12.0, high = 16.0, provenance = "declared" },
  { metric = "failure_risk", low = 0.05, likely = 0.10, high = 0.20, provenance = "declared" },
]
"#,
    )
    .unwrap();
    // Two synthetic observations with a 4x spread (machine contention, not
    // a configuration difference): the median alone would hide it.
    std::fs::write(
        dir.join("evidence.jsonl"),
        "{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":2.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T00:00:00Z\",\"source\":\"ci\",\"artifact_hash\":null}\n{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":8.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T01:00:00Z\",\"source\":\"ci\",\"artifact_hash\":null}\n",
    )
    .unwrap();

    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("duration_minutes: observed range low 2.000, likely 5.000, high 8.000, expected 5.000 (predicted 12.333; measured 1/1; basis none; 2 observations)"),
        "{compiled}"
    );
    // The declared-only metric line is unchanged: no label, no count.
    let risk_line = compiled
        .lines()
        .find(|line| line.starts_with("failure_risk:"))
        .unwrap();
    assert!(risk_line.starts_with("failure_risk: low "), "{risk_line}");
    assert!(risk_line.ends_with("measured 0/1)"), "{risk_line}");

    let json: serde_json::Value =
        serde_json::from_str(&pc_ok(&dir, &["compile", "--json"])).unwrap();
    assert_eq!(
        json["basis"]["duration_minutes"]["measured_observations"], 2,
        "{json}"
    );
    assert_eq!(
        json["basis"]["failure_risk"]["measured_observations"], 0,
        "{json}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// G5 partial case: when only some nodes are measured the bounds mix in
/// declared guesses, so the triple is never labelled as observed; the
/// observation count still names the measured part.
#[test]
fn compile_totals_never_label_partly_declared_bounds_observed() {
    let dir = scratch_dir("partly-observed");
    std::fs::write(
        dir.join("plan.toml"),
        r#"ir_version = 0

[plan]
id = "partly"
title = "Partly"

[objective]
root_task = "root"
weights = { duration_minutes = 1.0 }

[[task]]
id = "root"
title = "Root"
state = "decomposed"

[[task]]
id = "a"
title = "Measured step"
state = "resolved"

[[task]]
id = "b"
title = "Declared step"
state = "resolved"

[[method]]
id = "root-all"
task = "root"
kind = "decompose"
subtasks = ["a", "b"]
status = "accepted"
provenance = "human:test"

[[method]]
id = "via-c"
task = "a"
kind = "invoke"
capability = "c@1"
status = "accepted"
provenance = "human:test"

[[method]]
id = "via-d"
task = "b"
kind = "invoke"
capability = "d@1"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "c@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 10.0, likely = 12.0, high = 16.0, provenance = "declared" }]

[[capability]]
id = "d@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 10.0, likely = 12.0, high = 16.0, provenance = "declared" }]
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("evidence.jsonl"),
        "{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":2.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T00:00:00Z\",\"source\":\"ci\",\"artifact_hash\":null}\n{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":8.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T01:00:00Z\",\"source\":\"ci\",\"artifact_hash\":null}\n",
    )
    .unwrap();

    let compiled = pc_ok(&dir, &["compile"]);
    let line = compiled
        .lines()
        .find(|line| line.starts_with("duration_minutes:"))
        .unwrap();
    assert!(!line.contains("observed"), "{line}");
    assert!(
        line.ends_with("(predicted 12.333; measured 1/2; basis none; 2 observations)"),
        "{line}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc init` scaffolds a minimal valid plan that passes `pc check`.
#[test]
fn init_scaffolds_minimal_valid_plan() {
    let parent = scratch_dir("init-scaffold");
    let dir = parent.join("new-plan");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("initialized plan at"), "{stdout}");
    assert!(stdout.contains("plan.toml"), "{stdout}");

    // The scaffold parses as a plan and checks clean.
    let text = std::fs::read_to_string(dir.join("plan.toml")).unwrap();
    let doc = pc_ir::from_toml(&text).unwrap();
    assert_eq!(doc.task.len(), 1);
    assert_eq!(doc.objective.root_task, "start");
    let check = pc_ok(&dir, &["check"]);
    assert!(check.contains("ok: 1 tasks"), "{check}");

    // `--json` emits the created plan path.
    let other = parent.join("json-plan");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&other)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["plan"].as_str().unwrap().ends_with("plan.toml"),
        "{value}"
    );
    let check = pc_ok(&other, &["check"]);
    assert!(check.contains("ok: 1 tasks"), "{check}");
    std::fs::remove_dir_all(&parent).unwrap();
}

/// A bare `pc init` scaffolds `.plan/`, and a bare `pc check` finds it.
#[test]
fn bare_init_then_bare_check_uses_dot_plan() {
    let dir = scratch_dir("init-bare");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&dir)
        .arg("init")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.join(".plan").join("plan.toml").is_file());
    assert!(!dir.join("plan.toml").exists());

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&dir)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ok: 1 tasks"), "{stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A bare `pc init` beside an existing `./plan.toml` refuses rather than
/// creating a `.plan/` that would shadow it.
#[test]
fn bare_init_refuses_beside_root_plan() {
    let dir = scratch_dir("init-bare-root");
    let first = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(first.status.success());
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&dir)
        .arg("init")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("plan.toml"), "{stderr}");
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    assert!(!dir.join(".plan").exists(), "refusal wrote nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Bare commands fall back to `./plan.toml` when `.plan/` holds no plan.
#[test]
fn bare_check_ignores_empty_dot_plan() {
    let dir = scratch_dir("check-empty-dot-plan");
    let first = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(first.status.success());
    std::fs::create_dir(dir.join(".plan")).unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&dir)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ok: 1 tasks"), "{stdout}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc init` refuses to overwrite an existing plan and writes nothing.
#[test]
fn init_refuses_to_overwrite_existing_plan() {
    let dir = scratch_dir("init-refusal");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let before = std::fs::read_to_string(dir.join("plan.toml")).unwrap();

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already exists"), "{stderr}");
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join("plan.toml")).unwrap(),
        before,
        "refusal wrote nothing"
    );

    // A sibling `plan.json` counts as an existing plan too.
    let other = scratch_dir("init-refusal-json");
    std::fs::write(other.join("plan.json"), "{\"ir_version\": 0}").unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("init")
        .arg(&other)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("already exists"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&other).unwrap();
}
/// G4 end to end: the typed basis surfaces in compile, explain and diff;
/// subjects measuring one capability pool under one basis, and a record
/// mixing in another basis is refused naming both subjects.
#[test]
fn evidence_basis_renderings() {
    let dir = scratch_dir("basis");
    std::fs::write(
        dir.join("plan.toml"),
        r#"ir_version = 0

[plan]
id = "basis"
title = "Basis"

[objective]
root_task = "job"
weights = { duration_minutes = 1.0 }

[[task]]
id = "job"
title = "Do the job"
state = "resolved"

[[method]]
id = "via-c"
task = "job"
kind = "invoke"
capability = "c@1"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "c@1"
kind = "tool"
version = "1"
estimates = [{ metric = "duration_minutes", low = 4.0, likely = 5.0, high = 6.0, provenance = "declared" }]
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("evidence.jsonl"),
        "{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":10.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T00:00:00Z\",\"source\":\"ci\",\"basis\":\"distance-from:old@1\",\"artifact_hash\":null}\n{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":12.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T01:00:00Z\",\"source\":\"ci\",\"basis\":\"distance-from:old@1\",\"artifact_hash\":null}\n{\"subject\":\"method:via-c\",\"metric\":\"duration_minutes\",\"value\":20.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T02:00:00Z\",\"source\":\"ci\",\"basis\":\"distance-from:old@1\",\"artifact_hash\":null}\n{\"subject\":\"method:via-c\",\"metric\":\"duration_minutes\",\"value\":21.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T03:00:00Z\",\"source\":\"ci\",\"basis\":\"distance-from:old@1\",\"artifact_hash\":null}\n{\"subject\":\"method:via-c\",\"metric\":\"duration_minutes\",\"value\":22.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T04:00:00Z\",\"source\":\"ci\",\"basis\":\"distance-from:old@1\",\"artifact_hash\":null}\n",
    )
    .unwrap();

    let compiled = pc_ok(&dir, &["compile"]);
    assert!(
        compiled.contains("measured 1/1; basis distance-from:old@1; 5 observations)"),
        "{compiled}"
    );
    let explained = pc_ok(&dir, &["explain", "capability:c@1"]);
    assert!(
        explained.contains("(n=5, sources: ci) [contradicts declared] [basis distance-from:old@1]"),
        "{explained}"
    );

    // A basis-less record for the same capability through another subject
    // is refused, naming both bases and their subjects.
    let observation = dir.join("observation.json");
    std::fs::write(
        &observation,
        r#"{"subject":"method:via-c","metric":"duration_minutes","value":30.0,"unit":"minutes","time":"2026-10-08T05:00:00Z","source":"ci","artifact_hash":null}"#,
    )
    .unwrap();
    let stderr = pc_fails(&dir, &["record", observation.to_str().unwrap()]);
    assert!(
        stderr.contains("error[E014]")
            && stderr.contains("`capability:c@1`")
            && stderr.contains(
                "`distance-from:old@1` (capability:c@1, method:via-c) and `none` (method:via-c)"
            ),
        "{stderr}"
    );

    // Diff against a sibling plan measured without a basis: the totals
    // move and the basis movement renders beside them.
    let other = scratch_dir("basis-other");
    std::fs::copy(dir.join("plan.toml"), other.join("plan.toml")).unwrap();
    std::fs::write(
        other.join("evidence.jsonl"),
        "{\"subject\":\"capability:c@1\",\"metric\":\"duration_minutes\",\"value\":30.0,\"unit\":\"minutes\",\"time\":\"2026-10-08T00:00:00Z\",\"source\":\"ci\",\"artifact_hash\":null}\n",
    )
    .unwrap();
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("diff")
        .arg(&dir)
        .arg(&other)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let diff = String::from_utf8(output.stdout).unwrap();
    assert!(
        diff.contains("[basis: distance-from:old@1 -> none]"),
        "{diff}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_dir_all(&other).unwrap();
}

/// D3: `compile`, `frontier`, `impact` and `diff --json` round floats to the
/// text renderer's 3-decimal precision. Regression on
/// examples/transcription-substitution, where `failure_risk.low` printed as
/// 0.050000000000000044 and a diff's `failure_risk.before` as
/// 0.10833333333333332.
#[test]
fn json_floats_round_to_text_precision() {
    fn assert_rounded(value: &serde_json::Value, ctx: &str) {
        match value {
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    let rounded = (f * 1000.0).round() / 1000.0;
                    assert!(
                        (f - rounded).abs() < f64::EPSILON,
                        "{ctx}: float {f} carries more than 3 decimals"
                    );
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_rounded(item, ctx);
                }
            }
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    assert_rounded(item, &format!("{ctx}.{key}"));
                }
            }
            _ => {}
        }
    }

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("compile")
        .arg("examples/transcription-substitution")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(
        !raw.contains("0.050000000000000044"),
        "unrounded float leaked into compile --json:\n{raw}"
    );
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(value["totals"]["failure_risk"]["low"], 0.05, "{value}");
    assert_eq!(value["totals"]["failure_risk"]["likely"], 0.1, "{value}");
    assert_eq!(value["totals"]["failure_risk"]["high"], 0.2, "{value}");
    assert_eq!(
        value["totals"]["failure_risk"]["expected"], 0.108,
        "{value}"
    );
    assert_rounded(&value, "compile");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("frontier")
        .arg("examples/transcription-substitution")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(
        !raw.contains("0.3333333333333333"),
        "unrounded float leaked into frontier --json:\n{raw}"
    );
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let entry = &value["unestimated"][0];
    assert_eq!(entry["score"], 0.333, "{value}");
    assert_eq!(entry["share"], 0.667, "{value}");
    assert_rounded(&value, "frontier");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("diff")
        .arg("examples/transcription-substitution/decided.toml")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let delta = |metric: &str| {
        value["totals_delta"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["metric"] == metric)
            .unwrap_or_else(|| panic!("no {metric} delta in {value}"))
            .clone()
    };
    let accuracy = delta("accuracy_distance_from_incumbent");
    assert_eq!(accuracy["before"], 0.1, "{value}");
    assert_eq!(accuracy["delta"], -0.1, "{value}");
    assert_eq!(delta("failure_risk")["before"], 0.108, "{value}");
    assert_rounded(&value, "diff");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(workspace_root())
        .arg("impact")
        .arg("examples/transcription-substitution/changes/accept-hybrid.json")
        .arg("examples/transcription-substitution/plan.toml")
        .arg("--json")
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_rounded(&value, "impact");
}

/// Copy the tasks-axi 10-item backlog fixture to a scratch dir.
fn tasks_axi_scratch(name: &str) -> PathBuf {
    let dir = scratch_dir(name);
    std::fs::copy(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("tasks-axi")
            .join("backlog.md"),
        dir.join("backlog.md"),
    )
    .unwrap();
    dir
}

/// The 10-item backlog subset round-trips through the IR byte-identical:
/// import, clean check, then export regenerates the exact input bytes.
#[test]
fn tasks_axi_round_trip_is_byte_identical() {
    let dir = tasks_axi_scratch("tasks-axi-roundtrip");
    let backlog = dir.join("backlog.md");
    let plan = dir.join("plan");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(&backlog)
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("imported 10 tasks"), "{stdout}");
    assert!(plan.join("plan.toml").is_file());

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("check")
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ok: 11 tasks"), "{stdout}");

    let exported = dir.join("roundtrip.md");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("export-tasks-axi")
        .arg(&exported)
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("exported 10 tasks"), "{stdout}");

    assert_eq!(
        std::fs::read(&backlog).unwrap(),
        std::fs::read(&exported).unwrap(),
        "export must regenerate the backlog byte-identical"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc import-tasks-axi` refuses to overwrite an existing plan.
#[test]
fn tasks_axi_import_refuses_overwrite() {
    let dir = tasks_axi_scratch("tasks-axi-overwrite");
    let backlog = dir.join("backlog.md");
    let plan = dir.join("plan");

    let first = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(&backlog)
        .arg(&plan)
        .output()
        .unwrap();
    assert!(first.status.success());
    let before = std::fs::read(plan.join("plan.toml")).unwrap();

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(&backlog)
        .arg(&plan)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    assert_eq!(
        std::fs::read(plan.join("plan.toml")).unwrap(),
        before,
        "refusal wrote nothing"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc import-tasks-axi` refuses a non-backlog file loudly and writes nothing.
#[test]
fn tasks_axi_import_refuses_garbage() {
    let dir = scratch_dir("tasks-axi-garbage");
    let backlog = dir.join("backlog.md");
    std::fs::write(
        &backlog,
        "# Backlog\n\n## Queued\nJust a note, no task here.\n",
    )
    .unwrap();
    let plan = dir.join("plan");

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(&backlog)
        .arg(&plan)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unexpected line"), "{stderr}");
    assert!(!plan.join("plan.toml").exists(), "refusal wrote nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc export-tasks-axi` refuses dropped tasks loudly: tasks-axi has no
/// counterpart for them, so they are never silently skipped.
#[test]
fn tasks_axi_export_refuses_dropped() {
    let dir = scratch_dir("tasks-axi-dropped");
    let plan = dir.join("plan");
    std::fs::create_dir_all(&plan).unwrap();
    std::fs::write(
        plan.join("plan.toml"),
        "ir_version = 0\n\n[plan]\nid = \"x\"\ntitle = \"x\"\n\n[objective]\nroot_task = \"backlog-root\"\n\n[[task]]\nid = \"backlog-root\"\ntitle = \"Backlog\"\nstate = \"decomposed\"\n\n[[task]]\nid = \"t1\"\ntitle = \"Gone\"\nstate = \"dropped\"\n\n[task.tasks_axi]\nsection = \"Queued\"\n\n[[method]]\nid = \"backlog-decompose\"\ntask = \"backlog-root\"\nkind = \"decompose\"\nsubtasks = [\"t1\"]\nstatus = \"accepted\"\n",
    )
    .unwrap();

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("export-tasks-axi")
        .arg(dir.join("backlog.md"))
        .arg(&plan)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("t1"), "{stderr}");
    assert!(stderr.contains("dropped"), "{stderr}");
    assert!(!dir.join("backlog.md").exists(), "refusal wrote nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Every backlog row is distinct work: two rows with the same title and
/// state but different repos and holds stay two tasks through P4, so both
/// reach compile and the frontier.
#[test]
fn tasks_axi_same_title_rows_stay_distinct() {
    let dir = scratch_dir("tasks-axi-same-title");
    let backlog = dir.join("backlog.md");
    std::fs::write(
        &backlog,
        "# Backlog\n\n## Queued\n- [ ] a - Bump deps (repo: x)\n- [ ] b - Bump deps (repo: y) (hold: waiting)\n",
    )
    .unwrap();
    let plan = dir.join("plan");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(&backlog)
        .arg(&plan)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json = |verb: &str| -> serde_json::Value {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg(verb)
            .arg(&plan)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };

    let compiled = json("compile");
    assert_eq!(
        compiled["unresolved_tasks"],
        serde_json::json!(["a", "b"]),
        "{compiled}"
    );
    let frontier = json("frontier");
    let subjects: Vec<&str> = frontier["unestimated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["subject"].as_str().unwrap())
        .collect();
    assert_eq!(subjects, ["task:a", "task:b"], "{frontier}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `pc import-tasks-axi` refuses any existing plan file in the target
/// directory, not just `plan.toml`, so it never shadows a `plan.json`.
#[test]
fn tasks_axi_import_refuses_existing_json_plan() {
    let dir = tasks_axi_scratch("tasks-axi-json-plan");
    let plan = dir.join("plan");
    std::fs::create_dir_all(&plan).unwrap();
    std::fs::write(plan.join("plan.json"), "{}").unwrap();

    let output = Command::cargo_bin("pc")
        .unwrap()
        .arg("import-tasks-axi")
        .arg(dir.join("backlog.md"))
        .arg(&plan)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    assert!(stderr.contains("plan.json"), "{stderr}");
    assert!(!plan.join("plan.toml").exists(), "refusal wrote nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A bare `pc import-tasks-axi` refuses when the current directory already
/// holds a plan, which a new `.plan/` would shadow for bare commands.
#[test]
fn tasks_axi_import_default_refuses_cwd_plan() {
    let dir = tasks_axi_scratch("tasks-axi-cwd-plan");
    std::fs::write(dir.join("plan.toml"), "ir_version = 0\n").unwrap();

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&dir)
        .arg("import-tasks-axi")
        .arg("backlog.md")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("refusing to overwrite"), "{stderr}");
    assert!(!dir.join(".plan").exists(), "refusal wrote nothing");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Synthetic documented Beads JSONL round-trips through the CLI and IR.
#[test]
fn beads_jsonl_round_trip_is_byte_identical() {
    let dir = scratch_dir("beads-roundtrip");
    let input = workspace_root().join("tests/fixtures/beads/issues.jsonl");
    let output = dir.join("roundtrip.jsonl");
    let plan = dir.join("plan");
    Command::cargo_bin("pc")
        .unwrap()
        .arg("import-beads")
        .arg(&input)
        .arg(&plan)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "unlinked dependency: `bd-e5f6` blocks on `external:infra:ci-runner`, which is not in the file; kept in passthrough with no order edge\n",
        ))
        .stdout(predicates::str::contains(
            "unlinked dependency: `bd-e5f6` blocks on `bd-gone`, which is not in the file; kept in passthrough with no order edge\n",
        ))
        .stdout(predicates::str::contains("imported 3 Beads issues"));
    Command::cargo_bin("pc")
        .unwrap()
        .arg("check")
        .arg(&plan)
        .assert()
        .success();
    Command::cargo_bin("pc")
        .unwrap()
        .arg("export-beads")
        .arg(&output)
        .arg(&plan)
        .assert()
        .success();
    assert_eq!(
        std::fs::read(input).unwrap(),
        std::fs::read(output).unwrap()
    );
}

/// `pc compile --objective makespan` trades total work for elapsed time
/// and explains the trade; effort stays the default objective.
#[test]
fn compile_objective_flag_switches_between_effort_and_makespan() {
    let compile = |objective: &[&str]| {
        let output = Command::cargo_bin("pc")
            .unwrap()
            .arg("compile")
            .arg(
                workspace_root()
                    .join("tests")
                    .join("fixtures")
                    .join("makespan-tradeoff"),
            )
            .arg("--json")
            .args(objective)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let effort = compile(&[]);
    assert_eq!(effort["selection_objective"], "effort");
    assert_eq!(effort["selection"]["deliver"], "chain");
    assert_eq!(effort["makespan_minutes"], 40.0);
    assert_eq!(effort, compile(&["--objective", "effort"]));

    let fast = compile(&["--objective", "makespan"]);
    assert_eq!(fast["selection_objective"], "makespan");
    assert_eq!(fast["selection_status"], "optimal");
    assert_eq!(fast["selection"]["deliver"], "fan-out");
    assert_eq!(fast["makespan_minutes"], 20.0);
    assert_eq!(
        fast["selection_explanation"],
        "Minimized dependency-aware completion time to 20.000 minutes at weighted effort 60.000; \
the effort-optimal selection takes 40.000 minutes at weighted effort 40.000, so 20.000 more \
effort buys 20.000 fewer minutes."
    );
}

/// `pc capabilities` lists every capability with version, provider, source
/// and evidence coverage, flagging those with no measurement evidence.
#[test]
fn capabilities_report_flags_unmeasured_inventory() {
    let fixtures = workspace_root().join("tests").join("fixtures");
    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&fixtures)
        .arg("capabilities")
        .arg("capability-inventory")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "capabilities: 3 declared, 1 unmeasured (capability-inventory/plan.toml)\n\
         capability:idle@1 [service v2] provider Example Org; source -; unmeasured\n\
         capability:measured@1 [tool v1] provider Example Org; source pkg:cargo/measured@1; \
         measured duration_minutes, money_usd; 2 observations, 0 staged\n\
         capability:staged@1 [agent v1] provider -; source -; measured -; 0 observations, 1 staged\n"
    );

    let output = Command::cargo_bin("pc")
        .unwrap()
        .current_dir(&fixtures)
        .arg("capabilities")
        .arg("capability-inventory")
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["unmeasured"], 1);
    let rows = json["capabilities"].as_array().unwrap();
    let ids: Vec<&str> = rows.iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["idle@1", "measured@1", "staged@1"]);
    assert_eq!(rows[0]["unmeasured"], true);
    assert_eq!(rows[1]["provider"], "Example Org");
    assert_eq!(rows[1]["source"], "pkg:cargo/measured@1");
    assert_eq!(rows[1]["observations"], 2);
    assert_eq!(rows[1]["unmeasured"], false);
    assert_eq!(rows[2]["staged_observations"], 1);
    assert_eq!(rows[2]["unmeasured"], false);
}
