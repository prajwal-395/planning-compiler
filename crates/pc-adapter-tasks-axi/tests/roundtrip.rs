//! tasks-axi adapter tests: fixture round-trip plus parse/import/export edges.
//!
//! Fixture paths resolve from the workspace root, like the CLI tests.

use pc_adapter_tasks_axi::{
    Backlog, BacklogItem, export_backlog, import_plan, parse_backlog, render_backlog,
};
use std::path::PathBuf;

/// Workspace root (`crates/pc-adapter-tasks-axi/../..`).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The 10-item synthetic backlog fixture.
fn fixture_text() -> String {
    std::fs::read_to_string(
        workspace_root()
            .join("tests")
            .join("fixtures")
            .join("tasks-axi")
            .join("backlog.md"),
    )
    .unwrap()
}

/// The 10-item subset round-trips into the IR and back byte-identical.
#[test]
fn fixture_round_trip_is_byte_identical() {
    let text = fixture_text();
    let parsed = parse_backlog(&text).unwrap();
    assert_eq!(parsed.items.len(), 10);
    let doc = import_plan(&parsed).unwrap();
    // Ten items plus the import root.
    assert_eq!(doc.task.len(), 11);
    let back = export_backlog(&doc).unwrap();
    assert_eq!(render_backlog(&back), text);
}

/// Import is a fixed point: a second import of the export matches the first.
#[test]
fn fixture_import_is_stable() {
    let text = fixture_text();
    let first = import_plan(&parse_backlog(&text).unwrap()).unwrap();
    let second =
        import_plan(&parse_backlog(&render_backlog(&export_backlog(&first).unwrap())).unwrap())
            .unwrap();
    assert_eq!(first, second);
}

/// Blocked-by edges land in the single root decompose method in file order.
#[test]
fn fixture_blocked_by_maps_to_order_edges() {
    let text = fixture_text();
    let doc = import_plan(&parse_backlog(&text).unwrap()).unwrap();
    assert_eq!(doc.method.len(), 1);
    let method = &doc.method[0];
    assert_eq!(method.task, "backlog-root");
    let edges: Vec<(&str, &str)> = method
        .order_edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    assert!(edges.contains(&("pc-import-axi-a1", "pc-import-axi-a2")));
    assert!(edges.contains(&("pc-import-axi-a2", "pc-import-axi-b1")));
    assert!(edges.contains(&("pc-import-axi-b1", "pc-import-axi-b3")));
    assert!(edges.contains(&("pc-import-axi-b2", "pc-import-axi-b3")));
    assert!(edges.contains(&("pc-import-axi-b3", "pc-import-axi-b5")));
    assert!(edges.contains(&("pc-import-axi-c1", "pc-import-axi-c2")));
    assert!(edges.contains(&("pc-import-axi-c2", "pc-import-axi-c3")));
    assert_eq!(edges.len(), 8);
}

/// Holds, kinds, repos, priorities and date gates ride along as passthrough.
#[test]
fn fixture_unmappable_fields_are_passthrough() {
    let text = fixture_text();
    let doc = import_plan(&parse_backlog(&text).unwrap()).unwrap();
    let task = doc
        .task
        .iter()
        .find(|task| task.id == "pc-import-axi-b3")
        .unwrap();
    let meta = task.tasks_axi.as_ref().unwrap();
    assert_eq!(meta.section, "Queued");
    assert_eq!(meta.repo.as_deref(), Some("planning-compiler"));
    assert_eq!(meta.kind.as_deref(), Some("scout"));
    assert_eq!(meta.hold_reason.as_deref(), Some("waiting on captain"));
    assert_eq!(meta.hold_kind.as_deref(), Some("captain"));
    assert_eq!(meta.hold_until.as_deref(), Some("2026-12-01"));
    // The bare task carries no parentheticals at all.
    let bare = doc
        .task
        .iter()
        .find(|task| task.id == "pc-import-axi-b4")
        .unwrap();
    let bare_meta = bare.tasks_axi.as_ref().unwrap();
    assert_eq!(bare_meta.repo, None);
    assert_eq!(bare_meta.kind, None);
    assert_eq!(bare_meta.priority, None);
    // Done items become done tasks; open items stay open.
    let done = doc
        .task
        .iter()
        .find(|task| task.id == "pc-import-axi-c1")
        .unwrap();
    assert_eq!(done.state, pc_ir::TaskState::Done);
    let open = doc
        .task
        .iter()
        .find(|task| task.id == "pc-import-axi-a1")
        .unwrap();
    assert_eq!(open.state, pc_ir::TaskState::Open);
}

/// Closed-date labels follow the title links: PR merges, report paths report.
#[test]
fn closed_labels_follow_title_links() {
    let backlog = Backlog {
        sections: vec!["Done".to_string()],
        items: ["merged", "reported", "done"]
            .iter()
            .map(|label| BacklogItem {
                id: format!("t-{label}"),
                title: match *label {
                    "merged" => "Ship it https://github.com/o/r/pull/42".to_string(),
                    "reported" => "Write up data/t/report.md".to_string(),
                    _ => "Plain task".to_string(),
                },
                done: true,
                blocked_by: vec![],
                section: "Done".to_string(),
                repo: None,
                kind: None,
                priority: None,
                created: None,
                closed: Some("2026-10-12".to_string()),
                hold_reason: None,
                hold_kind: None,
                hold_until: None,
                body: None,
            })
            .collect(),
    };
    let text = render_backlog(&backlog);
    assert!(text.contains("(merged 2026-10-12)"), "{text}");
    assert!(text.contains("(reported 2026-10-12)"), "{text}");
    assert!(text.contains("(done 2026-10-12)"), "{text}");
    // All three spellings parse back to the same closed date.
    let back = parse_backlog(&text).unwrap();
    assert_eq!(back.items.len(), 3);
    for item in &back.items {
        assert_eq!(item.closed.as_deref(), Some("2026-10-12"));
    }
    assert_eq!(render_backlog(&back), text);
}

/// Empty sections survive the round trip.
#[test]
fn empty_sections_survive() {
    let text =
        "# Backlog\n\n## In flight\n## Queued\n- [ ] t1 - Only task (since 2026-10-12)\n## Done\n";
    let parsed = parse_backlog(text).unwrap();
    assert_eq!(parsed.sections, vec!["In flight", "Queued", "Done"]);
    let doc = import_plan(&parsed).unwrap();
    assert_eq!(
        doc.plan.tasks_axi_sections,
        vec!["In flight", "Queued", "Done"]
    );
    assert_eq!(render_backlog(&export_backlog(&doc).unwrap()), text);
}

/// Titles ending in unknown parentheticals stay titles, not metadata.
#[test]
fn unknown_trailing_parens_stay_in_title() {
    let text = "# Backlog\n\n## Queued\n- [ ] t1 - Pick effort (see notes) (since 2026-10-12)\n- [ ] t2 - Bare parens (here)\n";
    let parsed = parse_backlog(text).unwrap();
    assert_eq!(parsed.items[0].title, "Pick effort (see notes)");
    assert_eq!(parsed.items[0].kind, None);
    // A line with no metadata at all keeps its trailing parens verbatim.
    assert_eq!(parsed.items[1].title, "Bare parens (here)");
    assert_eq!(render_backlog(&parsed), text);
}

/// A non-canonical priority stays in the title instead of re-rendering in
/// its canonical digits, so the line still round-trips byte-identical.
#[test]
fn non_canonical_priority_stays_in_title() {
    let text = "# Backlog\n\n## Queued\n- [ ] t1 - Signed (priority: +1)\n- [ ] t2 - Padded (priority: 01)\n- [ ] t3 - Canonical (priority: -1)\n";
    let parsed = parse_backlog(text).unwrap();
    assert_eq!(parsed.items[0].title, "Signed (priority: +1)");
    assert_eq!(parsed.items[0].priority, None);
    assert_eq!(parsed.items[1].title, "Padded (priority: 01)");
    assert_eq!(parsed.items[1].priority, None);
    assert_eq!(parsed.items[2].title, "Canonical");
    assert_eq!(parsed.items[2].priority, Some(-1));
    let doc = import_plan(&parsed).unwrap();
    assert_eq!(render_backlog(&export_backlog(&doc).unwrap()), text);
}

/// A repeated blocker is refused loudly instead of collapsing on export.
#[test]
fn duplicate_blocker_is_refused() {
    let error = parse_backlog(
        "# Backlog\n\n## Queued\n- [ ] a - First\n- [ ] b - Second blocked-by: a blocked-by: a\n",
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("duplicate `blocked-by: a`"),
        "{error}"
    );
}

/// Out-of-order metadata after a `blocked-by:` entry cannot be title text,
/// so the line is refused loudly.
#[test]
fn out_of_order_metadata_after_blocker_is_refused() {
    let error = parse_backlog(
        "# Backlog\n\n## Queued\n- [ ] b - First\n- [ ] t - Second blocked-by: b (hold: x) (repo: r)\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("(hold: x)"), "{error}");
}

/// A dangling blocker is refused loudly, never dropped.
#[test]
fn dangling_blocker_is_refused() {
    let parsed = parse_backlog(
        "# Backlog\n\n## Queued\n- [ ] t1 - Lonely blocked-by: ghost (since 2026-10-12)\n",
    )
    .unwrap();
    let error = import_plan(&parsed).unwrap_err();
    assert!(error.to_string().contains("ghost"), "{error}");
}

/// An order cycle is refused loudly instead of writing a dirty plan.
#[test]
fn order_cycle_is_refused() {
    let parsed = parse_backlog(
        "# Backlog\n\n## Queued\n- [ ] t1 - First blocked-by: t2 (since 2026-10-12)\n- [ ] t2 - Second blocked-by: t1 (since 2026-10-12)\n",
    )
    .unwrap();
    let error = import_plan(&parsed).unwrap_err();
    assert!(error.to_string().contains("cycle"), "{error}");
}

/// Free-form lines are refused loudly, never silently dropped.
#[test]
fn free_form_lines_are_refused() {
    let error = parse_backlog("# Backlog\n\n## Queued\nJust a note, no task here.\n").unwrap_err();
    assert!(error.to_string().contains("unexpected line"), "{error}");
}

/// Duplicate ids are refused with both line numbers.
#[test]
fn duplicate_ids_are_refused() {
    let error = parse_backlog(
        "# Backlog\n\n## Queued\n- [ ] t1 - First (since 2026-10-12)\n- [ ] t1 - Second (since 2026-10-12)\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("t1"), "{error}");
}

/// An empty backlog imports nothing and refuses loudly.
#[test]
fn empty_backlog_is_refused() {
    let parsed = parse_backlog("# Backlog\n\n## Queued\n").unwrap();
    assert!(parsed.items.is_empty());
    assert!(import_plan(&parsed).is_err());
}

/// Dropped tasks have no backlog counterpart and refuse export loudly.
#[test]
fn dropped_tasks_refuse_export() {
    let text = fixture_text();
    let mut doc = import_plan(&parse_backlog(&text).unwrap()).unwrap();
    doc.task
        .iter_mut()
        .find(|task| task.id == "pc-import-axi-b4")
        .unwrap()
        .state = pc_ir::TaskState::Dropped;
    let error = export_backlog(&doc).unwrap_err();
    assert!(error.to_string().contains("pc-import-axi-b4"), "{error}");
}
