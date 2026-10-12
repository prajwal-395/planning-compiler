//! Synthetic Beads JSONL import/export acceptance tests.

use pc_adapter_beads::{UnlinkedDependency, export_jsonl, import_jsonl};

#[test]
fn documented_issue_fields_and_unknown_fields_round_trip_verbatim() {
    let input = include_str!("../../../tests/fixtures/beads/issues.jsonl");
    let import = import_jsonl(input).expect("synthetic Beads JSONL imports");
    let doc = &import.doc;
    assert_eq!(doc.task.len(), 4);
    assert_eq!(doc.task[1].state, pc_ir::TaskState::Open);
    assert_eq!(doc.task[2].state, pc_ir::TaskState::Done);
    assert!(
        doc.task
            .iter()
            .skip(1)
            .all(|task| task.beads_jsonl.is_some())
    );
    assert_eq!(
        doc.method[0].order_edges,
        vec![
            pc_ir::OrderEdge {
                from: "bd-a1b2".into(),
                to: "bd-c3d4".into()
            },
            pc_ir::OrderEdge {
                from: "bd-c3d4".into(),
                to: "bd-e5f6".into()
            }
        ]
    );
    assert_eq!(
        import.unlinked,
        vec![
            UnlinkedDependency {
                issue: "bd-e5f6".into(),
                depends_on: "external:infra:ci-runner".into()
            },
            UnlinkedDependency {
                issue: "bd-e5f6".into(),
                depends_on: "bd-gone".into()
            }
        ]
    );
    assert_eq!(export_jsonl(doc).unwrap(), input);
}

#[test]
fn malformed_jsonl_and_duplicate_ids_are_rejected() {
    assert!(import_jsonl("{bad}\n").is_err());
    assert!(
        import_jsonl("{\"id\":\"bd-a\",\"title\":\"one\"}\n{\"id\":\"bd-a\",\"title\":\"two\"}\n")
            .is_err()
    );
}

fn fixture_doc() -> pc_ir::PlanDoc {
    import_jsonl(include_str!("../../../tests/fixtures/beads/issues.jsonl"))
        .unwrap()
        .doc
}

fn rows(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn renamed_issue_is_rewritten_in_every_dependency_and_reimports() {
    let mut doc = fixture_doc();
    for task in &mut doc.task {
        if task.id == "bd-a1b2" {
            task.id = "bd-x".into();
        }
    }
    for edge in &mut doc.method[0].order_edges {
        if edge.from == "bd-a1b2" {
            edge.from = "bd-x".into();
        }
    }
    doc.method[0].subtasks[0] = "bd-x".into();
    let exported = export_jsonl(&doc).unwrap();
    let rows = rows(&exported);
    assert_eq!(rows[0]["id"], "bd-x");
    assert_eq!(
        rows[1]["dependencies"],
        serde_json::json!([{"issue_id":"bd-c3d4","depends_on_id":"bd-x","type":"blocks"}])
    );
    assert_eq!(rows[2]["dependencies"][3]["depends_on_id"], "bd-x");
    let reimported = import_jsonl(&exported).unwrap();
    assert_eq!(
        reimported.doc.method[0].order_edges,
        doc.method[0].order_edges
    );
    assert_eq!(export_jsonl(&reimported.doc).unwrap(), exported);
}

#[test]
fn order_edge_edits_rebuild_blocks_dependencies_and_keep_unlinked_ones() {
    let mut doc = fixture_doc();
    doc.method[0].order_edges = vec![pc_ir::OrderEdge {
        from: "bd-a1b2".into(),
        to: "bd-e5f6".into(),
    }];
    let exported = export_jsonl(&doc).unwrap();
    let input = include_str!("../../../tests/fixtures/beads/issues.jsonl");
    let lines: Vec<&str> = exported.lines().collect();
    assert_eq!(lines[0], input.lines().next().unwrap());
    let rows = rows(&exported);
    assert_eq!(rows[1]["dependencies"], serde_json::json!([]));
    let targets: Vec<(&str, &str)> = rows[2]["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|dep| {
            (
                dep["depends_on_id"].as_str().unwrap(),
                dep["type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        targets,
        vec![
            ("external:infra:ci-runner", "blocks"),
            ("bd-gone", "blocks"),
            ("bd-a1b2", "related"),
            ("bd-a1b2", "blocks"),
        ]
    );
    let reimported = import_jsonl(&exported).unwrap();
    assert_eq!(
        reimported.doc.method[0].order_edges,
        doc.method[0].order_edges
    );
    assert_eq!(reimported.unlinked.len(), 2);
}

#[test]
fn states_without_a_beads_status_are_refused_and_open_done_edits_export() {
    for state in [
        pc_ir::TaskState::Dropped,
        pc_ir::TaskState::Resolved,
        pc_ir::TaskState::Decomposed,
    ] {
        let mut doc = fixture_doc();
        doc.task[2].state = state;
        assert!(matches!(
            export_jsonl(&doc),
            Err(pc_adapter_beads::AdapterError::UnsupportedState(id, _)) if id == "bd-c3d4"
        ));
    }
    let mut doc = fixture_doc();
    doc.task[2].state = pc_ir::TaskState::Open;
    doc.task[1].state = pc_ir::TaskState::Done;
    let rows = rows(&export_jsonl(&doc).unwrap());
    assert_eq!(rows[0]["status"], "closed");
    assert_eq!(rows[1]["status"], "open");
}
