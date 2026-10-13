//! Guarded MCP write tools: propose, accept, reject and record.
//!
//! Each test names the defect it would catch. The happy-path test pins the
//! CLI proposal workflow (`propose`, then `accept`) so MCP writes never
//! diverge from `pc propose` / `pc accept` / `pc record`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serialize writes that mutate `PC_MCP_ALLOWED_ROOTS`: the variable is
/// process-global, so tests that need different values must not run
/// concurrently.
static ENV_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

fn lock_env() -> MutexGuard<'static, ()> {
    ENV_GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Point the MCP sandbox at the temp dir (covering every scratch dir) and
/// hold the guard until the end of the test.
fn allow_temp_roots() -> MutexGuard<'static, ()> {
    let guard = lock_env();
    // SAFETY: the guard serializes every mutation of this variable in tests.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", std::env::temp_dir());
    }
    guard
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pc-mcp-write-{}-{}-{name}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_plan(fixture: &str, dir: &Path) -> PathBuf {
    let plan = dir.join("plan.toml");
    std::fs::copy(workspace_root().join("tests/fixtures").join(fixture), &plan).unwrap();
    plan
}

fn patch(value: serde_json::Value) -> pc_ir::Patch {
    serde_json::from_value(value).unwrap()
}

fn evidence(value: serde_json::Value) -> pc_ir::Evidence {
    serde_json::from_value(value).unwrap()
}

/// Round-1 agent patch from the CLI `proposal_loop_agent_play` test.
fn round1_patch() -> pc_ir::Patch {
    patch(serde_json::json!({
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
    }))
}

/// Defect: MCP writes diverge from the CLI proposal workflow.
/// Proposing then accepting through the MCP tools must stage and flip the
/// same method the CLI stages, and the plan must still compile.
#[test]
fn propose_accept_roundtrip_matches_cli_workflow() {
    let _env = allow_temp_roots();
    let dir = scratch("roundtrip");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let (notes, revision) = pc_mcp::propose(&plan, &revision, round1_patch()).unwrap();
    assert!(
        notes.iter().any(|n| n == "proposed method:draft-plan"),
        "{notes:?}"
    );

    let (note, _revision) = pc_mcp::decide(&plan, &revision, "method:draft-plan", true).unwrap();
    assert_eq!(note, "accepted method:draft-plan");

    // The accepted plan still compiles: the write did not corrupt it.
    pc_mcp::compile_plan(&plan, pc_mcp::DEFAULT_SELECTION_TIME_BUDGET_MS).unwrap();
}

/// Defect: lost update overwrites a concurrent edit.
/// A write carrying a stale revision must fail, and the file must keep the
/// first write's content.
#[test]
fn stale_revision_is_a_conflict_not_an_overwrite() {
    let _env = allow_temp_roots();
    let dir = scratch("stale-revision");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);

    let stale = pc_mcp::plan_revision(&plan).unwrap();
    let (_notes, _fresh) = pc_mcp::propose(&plan, &stale, round1_patch()).unwrap();

    // The accept carries the pre-propose revision: the guard must refuse it.
    let err = pc_mcp::decide(&plan, &stale, "draft-plan", true).unwrap_err();
    assert!(err.contains("revision conflict"), "{err}");

    let text = std::fs::read_to_string(&plan).unwrap();
    assert!(text.contains("draft-plan"), "first write must survive");
    assert!(
        text.contains("status = \"proposed\""),
        "refused accept must not flip the method"
    );
}

/// Defect: an MCP write escapes its sandbox.
/// A plan outside `PC_MCP_ALLOWED_ROOTS` must be refused before any write.
#[test]
fn write_outside_allowed_roots_is_refused() {
    let guard = lock_env();
    let allowed = scratch("allowed-roots");
    // SAFETY: the guard serializes every mutation of this variable in tests.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", &allowed);
    }
    let _guard = guard;

    let elsewhere = scratch("elsewhere");
    let plan = copy_plan("valid-proposal-loop.toml", &elsewhere);
    let revision = pc_mcp::plan_revision(&plan).unwrap();

    let before = std::fs::read_to_string(&plan).unwrap();
    let err = pc_mcp::propose(&plan, &revision, round1_patch()).unwrap_err();
    assert!(err.contains("outside PC_MCP_ALLOWED_ROOTS"), "{err}");
    assert_eq!(std::fs::read_to_string(&plan).unwrap(), before);

    // SAFETY: restore the shared value before releasing the guard.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", std::env::temp_dir());
    }
}

/// Defect: an unconfigured server lets agents write anywhere.
/// Without `PC_MCP_ALLOWED_ROOTS` every write tool refuses and names the
/// variable, while read-only tools still work.
#[test]
fn unset_allowed_roots_refuses_every_write() {
    let guard = lock_env();
    // SAFETY: the guard serializes every mutation of this variable in tests.
    unsafe {
        std::env::remove_var("PC_MCP_ALLOWED_ROOTS");
    }
    let _guard = guard;

    let dir = scratch("unset-roots");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);
    let before = std::fs::read_to_string(&plan).unwrap();
    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let record = evidence(serde_json::json!({
        "subject": "method:draft-plan", "metric": "duration_minutes",
        "value": 5.0, "unit": "minutes",
        "time": "2026-10-08T00:00:00Z", "source": "ci",
        "artifact_hash": null
    }));

    let errors = [
        pc_mcp::propose(&plan, &revision, round1_patch()).unwrap_err(),
        pc_mcp::decide(&plan, &revision, "draft-plan", true).unwrap_err(),
        pc_mcp::decide(&plan, &revision, "draft-plan", false).unwrap_err(),
        pc_mcp::record(&plan, &revision, record).unwrap_err(),
    ];
    for err in errors {
        assert!(err.contains("PC_MCP_ALLOWED_ROOTS"), "{err}");
    }
    assert_eq!(std::fs::read_to_string(&plan).unwrap(), before);
    assert!(!dir.join("evidence.jsonl").exists());
    assert!(pc_mcp::check_plan(&plan).unwrap().ok());

    // SAFETY: restore the shared value before releasing the guard.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", std::env::temp_dir());
    }
}

/// Defect: a symlink outside the sandbox redirects the write.
/// A plan reached through a symlink must be written at its real location
/// inside the allowed root; the link and its directory stay untouched.
#[cfg(unix)]
#[test]
fn symlinked_plan_writes_through_to_its_real_file() {
    let guard = lock_env();
    let allowed = scratch("symlink-allowed");
    // SAFETY: the guard serializes every mutation of this variable in tests.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", &allowed);
    }
    let _guard = guard;

    let real = copy_plan("valid-proposal-loop.toml", &allowed);
    let outside = scratch("symlink-outside");
    let link = outside.join("plan.toml");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let revision = pc_mcp::plan_revision(&link).unwrap();
    let (_notes, revision) = pc_mcp::propose(&link, &revision, round1_patch()).unwrap();
    let record = evidence(serde_json::json!({
        "subject": "method:draft-plan", "metric": "duration_minutes",
        "value": 5.0, "unit": "minutes",
        "time": "2026-10-08T00:00:00Z", "source": "ci",
        "artifact_hash": null
    }));
    let (log, line, _revision) = pc_mcp::record(&link, &revision, record).unwrap();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link must not be replaced by a copy"
    );
    assert!(
        std::fs::read_to_string(&real)
            .unwrap()
            .contains("draft-plan")
    );
    assert!(
        !outside.join("evidence.jsonl").exists(),
        "no evidence log may appear outside the allowed root"
    );
    let real_log = allowed.canonicalize().unwrap().join("evidence.jsonl");
    assert_eq!(log, real_log);
    assert_eq!(line, 1);
    let logged = pc_store::load(&real).unwrap();
    assert_eq!(logged.evidence_lines, vec![1]);
    assert!(
        logged
            .doc
            .evidence
            .iter()
            .any(|e| e.subject == "method:draft-plan"),
        "the record must land in the real plan's log"
    );

    // SAFETY: restore the shared value before releasing the guard.
    unsafe {
        std::env::set_var("PC_MCP_ALLOWED_ROOTS", std::env::temp_dir());
    }
}

/// Defect: a guarded write resets the plan file's permissions.
#[cfg(unix)]
#[test]
fn write_preserves_plan_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let _env = allow_temp_roots();
    let dir = scratch("permissions");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);
    std::fs::set_permissions(&plan, std::fs::Permissions::from_mode(0o640)).unwrap();

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    pc_mcp::propose(&plan, &revision, round1_patch()).unwrap();

    let mode = std::fs::metadata(&plan).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640);
}

/// Defect: a writer killed while holding the lock blocks every later write.
/// A leftover lock file holds no OS lock, so writes still proceed, and
/// back-to-back writes never trip over each other's lock file.
#[test]
fn leftover_lock_file_does_not_block_writes() {
    let _env = allow_temp_roots();
    let dir = scratch("leftover-lock");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);
    std::fs::write(dir.join(".plan.toml.lock"), "").unwrap();

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let (_notes, revision) = pc_mcp::propose(&plan, &revision, round1_patch()).unwrap();
    pc_mcp::decide(&plan, &revision, "draft-plan", true).unwrap();
}

/// Defect: a propose inlines sibling evidence, double-counting every
/// observation on the next load.
/// The plan file must keep only inline evidence; log records stay in
/// `evidence.jsonl`.
#[test]
fn propose_does_not_inline_sibling_evidence() {
    let _env = allow_temp_roots();
    let dir = scratch("evidence-inline");
    std::fs::copy(
        workspace_root().join("tests/fixtures/evidence-flip/plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let plan = dir.join("plan.toml");
    let legacy = r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":28.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#;
    std::fs::write(dir.join("evidence.jsonl"), format!("{legacy}\n")).unwrap();

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let grouped = patch(serde_json::json!({
      "add_method": [
        {"id": "via-fast-alt", "task": "job", "kind": "invoke", "capability": "fast@1",
         "status": "proposed", "provenance": "agent:e2e"}
      ]
    }));
    let (_notes, _revision) = pc_mcp::propose(&plan, &revision, grouped).unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.join("evidence.jsonl")).unwrap(),
        format!("{legacy}\n"),
        "the log must be untouched by a plan write"
    );
    let reloaded = pc_store::load(&plan).unwrap();
    assert_eq!(reloaded.evidence_lines.len(), 1);
    assert_eq!(
        reloaded.doc.evidence.len(),
        1,
        "log record must not also be written inline"
    );
}

/// Defect: a write lands on a dirty plan, compounding the breakage.
/// Proposing onto a plan that fails `check` must be refused.
#[test]
fn dirty_plan_refuses_write() {
    let _env = allow_temp_roots();
    let dir = scratch("dirty-plan");
    let plan = copy_plan("invalid-E008.toml", &dir);
    let revision = pc_mcp::plan_revision(&plan).unwrap();

    let err = pc_mcp::propose(&plan, &revision, round1_patch()).unwrap_err();
    assert!(err.contains("fails check"), "{err}");
}

/// Defect: rejecting leaves the method decidable.
/// Rejecting a proposed method must flip it to rejected, after which
/// accepting it fails instead of silently flipping back.
#[test]
fn reject_flips_proposed_method_to_rejected() {
    let _env = allow_temp_roots();
    let dir = scratch("reject");
    let plan = copy_plan("valid-proposal-loop.toml", &dir);

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let (_notes, revision) = pc_mcp::propose(&plan, &revision, round1_patch()).unwrap();
    let (note, revision) = pc_mcp::decide(&plan, &revision, "draft-plan", false).unwrap();
    assert_eq!(note, "rejected method:draft-plan");

    let err = pc_mcp::decide(&plan, &revision, "draft-plan", true).unwrap_err();
    assert!(err.contains("rejected"), "{err}");
}

/// Defect: `record` accepts evidence the CLI would refuse, or drops
/// evidence the CLI would keep.
/// A basis-less record appends and advances the revision; a second record
/// under a conflicting basis names both bases and appends nothing.
#[test]
fn record_validates_like_cli_and_guards_revision() {
    let _env = allow_temp_roots();
    let dir = scratch("record");
    std::fs::copy(
        workspace_root().join("tests/fixtures/evidence-flip/plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    let plan = dir.join("plan.toml");
    let legacy = r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":28.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#;
    std::fs::write(dir.join("evidence.jsonl"), format!("{legacy}\n")).unwrap();

    let revision = pc_mcp::plan_revision(&plan).unwrap();
    let pooled = evidence(serde_json::json!({
        "subject": "capability:fast@1", "metric": "duration_minutes",
        "value": 29.0, "unit": "minutes",
        "time": "2026-10-08T01:00:00Z", "source": "ci",
        "artifact_hash": null
    }));
    let (log, line, next) = pc_mcp::record(&plan, &revision, pooled).unwrap();
    assert_eq!(log, dir.canonicalize().unwrap().join("evidence.jsonl"));
    assert_eq!(line, 2);
    assert_ne!(next, revision, "recording must advance the revision");
    assert_eq!(
        next,
        pc_mcp::plan_revision(&plan).unwrap(),
        "the returned revision is the state just written"
    );

    // A stale revision is a conflict, not an append. The retry pools with
    // the log's basis so only the revision guard can refuse it.
    let retry = evidence(serde_json::json!({
        "subject": "capability:fast@1", "metric": "duration_minutes",
        "value": 30.0, "unit": "minutes",
        "time": "2026-10-08T02:00:00Z", "source": "ci",
        "artifact_hash": null
    }));
    let err = pc_mcp::record(&plan, &revision, retry).unwrap_err();
    assert!(err.contains("revision conflict"), "{err}");

    // A conflicting basis names both bases and appends nothing.
    let conflicting = evidence(serde_json::json!({
        "subject": "capability:fast@1", "metric": "duration_minutes",
        "value": 2.0, "unit": "minutes",
        "time": "2026-10-08T02:00:00Z", "source": "ci",
        "basis": "absolute", "artifact_hash": null
    }));

    // A conflicting basis names both bases and appends nothing.
    let err = pc_mcp::record(&plan, &next, conflicting).unwrap_err();
    assert!(
        err.contains("`absolute` (capability:fast@1)")
            && err.contains("`none` (capability:fast@1)"),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("evidence.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
        "refused record must not append"
    );

    // Unknown metrics never reach the log.
    let bad = evidence(serde_json::json!({
        "subject": "capability:fast@1", "metric": "nope",
        "value": 1.0, "unit": "minutes",
        "time": "2026-10-08T03:00:00Z", "source": "ci",
        "artifact_hash": null
    }));
    let err = pc_mcp::record(&plan, &next, bad).unwrap_err();
    assert!(err.contains("unknown metric"), "{err}");
}
