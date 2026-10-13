//! Revision-guarded plan writes: a write based on a stale load is refused.

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pc-store-revision-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/evidence-flip/plan.toml"),
        dir.join("plan.toml"),
    )
    .unwrap();
    dir
}

/// A write carrying the revision of an earlier load must not overwrite a
/// plan or evidence change made since: the decision was made against the
/// state that load saw.
#[test]
fn write_from_a_stale_load_is_a_conflict() {
    for change_log in [false, true] {
        let dir = scratch(if change_log { "log" } else { "plan" });
        let plan = dir.join("plan.toml");
        let loaded = pc_store::load(&dir).unwrap();

        let record = r#"{"subject":"capability:fast@1","metric":"duration_minutes","value":30.0,"unit":"minutes","time":"2026-10-08T00:00:00Z","source":"ci","artifact_hash":null}"#;
        if change_log {
            std::fs::write(dir.join("evidence.jsonl"), format!("{record}\n")).unwrap();
        } else {
            let mut text = std::fs::read_to_string(&plan).unwrap();
            text.push_str("# concurrent edit\n");
            std::fs::write(&plan, &text).unwrap();
        }
        let before = std::fs::read_to_string(&plan).unwrap();

        let err = pc_store::write_plan_if_revision(&plan, &loaded.revision, &loaded.doc)
            .unwrap_err()
            .to_string();
        assert!(err.contains("revision conflict"), "{err}");
        assert_eq!(std::fs::read_to_string(&plan).unwrap(), before);

        let fresh = pc_store::load(&dir).unwrap();
        assert_ne!(fresh.revision, loaded.revision);
        assert_eq!(fresh.revision, pc_store::plan_revision(&plan).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
