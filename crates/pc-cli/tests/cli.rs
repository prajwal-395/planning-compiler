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
    // Step 5 folds the two run-check observations (3.0 via the task, 4.0 via
    // the capability) into a measured (3.0, 3.5, 4.0) triple, so the makespan
    // is the measured 3.5 minutes, not the declared 6.167.
    assert!(text.contains("makespan 3.500 minutes"), "{text}");
    assert!(text.contains("duration_minutes:"), "{text}");
    assert!(text.contains("(predicted 6.167; measured 1/1)"), "{text}");
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
        text.contains("task:run-check: critical (slack 0.000 minutes; earliest 0.000-3.500, latest 0.000-3.500)"),
        "{text}"
    );

    // The P5 provenance report names the measured capability behind the
    // makespan: two observations, none stale, agreeing with the guess.
    let text = explain("capability:cargo-test@1");
    assert!(
        text.contains("provenance cargo-test@1 duration_minutes: predicted 2.000/5.000/15.000 vs measured 3.000/3.500/4.000 (n=2, sources: ci)"),
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
    let items = report["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let scores: Vec<f64> = items
        .iter()
        .map(|item| item["score"].as_f64().unwrap())
        .collect();
    let mut ranked = scores.clone();
    ranked.sort_by(|a, b| b.total_cmp(a));
    assert_eq!(scores, ranked);
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| item["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"open-task"));
    assert!(kinds.contains(&"proposal"));
    assert!(kinds.contains(&"blocked-alternative"));

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
        stdout.contains("affected tasks: select-lines, ship-reels, transcribe-footage"),
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
        serde_json::json!(["select-lines", "ship-reels", "transcribe-footage"])
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
/// negative values and unknown subjects instead of appending them.
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
    let stderr = record(
        "subject.json",
        r#"{"subject":"capability:ghost@9","metric":"duration_minutes","value":1.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#,
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
