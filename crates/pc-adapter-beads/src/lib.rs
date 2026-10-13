//! Beads issue JSONL import/export adapter.
//!
//! Each issue becomes a distinct IR task. Original JSONL records are carried
//! verbatim in task passthrough, preserving fields the IR does not model.

use pc_ir::{
    AcceptanceCriterion, AcceptanceKind, IR_VERSION, Method, MethodKind, MethodStatus, Objective,
    OrderEdge, PlanDoc, PlanMeta, Task, TaskState,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Root task used to group imported issues.
pub const ROOT_TASK_ID: &str = "beads-root";

#[derive(Debug, Error, PartialEq, Eq)]
/// Import/export failures for a Beads JSONL document.
pub enum AdapterError {
    /// A line is not valid JSON.
    #[error("JSONL line {line}: {message}")]
    /// Line number and JSON parser message.
    Json {
        /// 1-based JSONL line number.
        line: usize,
        /// JSON parser diagnostic.
        message: String,
    },
    /// A required string field is missing or empty.
    #[error("JSONL line {line}: issue needs non-empty string `{field}`")]
    /// Line number and missing field name.
    MissingField {
        /// 1-based JSONL line number.
        line: usize,
        /// Required field name.
        field: &'static str,
    },
    /// Two rows have the same issue id.
    #[error("duplicate Beads issue id `{0}`")]
    DuplicateId(String),
    /// The input contains no issue rows.
    #[error("no Beads issue records")]
    Empty,
    /// Export encountered a task without its original source row.
    #[error("task `{0}` has no Beads passthrough record")]
    MissingPassthrough(String),
    /// Export encountered a task state with no Beads status.
    #[error(
        "task `{0}` is {1}: Beads has no counterpart status, set it open or done before export"
    )]
    UnsupportedState(String, &'static str),
}

/// A `blocks` dependency whose target is not a row in the imported file
/// (cross-repo `external:` references, filtered exports, deleted issues). It
/// stays in the issue's passthrough record and adds no IR order edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnlinkedDependency {
    /// Issue that declares the dependency.
    pub issue: String,
    /// `depends_on_id` absent from the file.
    pub depends_on: String,
}

/// An imported plan plus the dependencies it could not link.
#[derive(Debug, Clone)]
pub struct Import {
    /// Plan built from the issue rows.
    pub doc: PlanDoc,
    /// Blocking dependencies kept only in passthrough, in file order.
    pub unlinked: Vec<UnlinkedDependency>,
}

#[derive(Debug, Clone)]
struct Record {
    id: String,
    title: String,
    status: String,
    raw: String,
    value: Value,
}

/// Parse Beads export JSONL and build a plan. All source fields remain in the
/// raw record so export can reproduce them without interpreting their schema.
pub fn import_jsonl(input: &str) -> Result<Import, AdapterError> {
    let mut rows = Vec::new();
    let mut ids = BTreeSet::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|e| AdapterError::Json {
            line: index + 1,
            message: e.to_string(),
        })?;
        let get = |field: &'static str| {
            value
                .get(field)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or(AdapterError::MissingField {
                    line: index + 1,
                    field,
                })
        };
        let id = get("id")?;
        let title = get("title")?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("open")
            .to_owned();
        if !ids.insert(id.clone()) {
            return Err(AdapterError::DuplicateId(id));
        }
        rows.push(Record {
            id,
            title,
            status,
            raw: line.to_owned(),
            value,
        });
    }
    if rows.is_empty() {
        return Err(AdapterError::Empty);
    }
    let mut tasks = vec![Task {
        id: ROOT_TASK_ID.into(),
        title: "Beads issues".into(),
        state: TaskState::Decomposed,
        consumes: vec![],
        produces: vec![],
        tasks_axi: None,
        beads_jsonl: None,
    }];
    let mut subtasks = Vec::new();
    let mut edges = Vec::new();
    let mut unlinked = Vec::new();
    for row in &rows {
        subtasks.push(row.id.clone());
        tasks.push(Task {
            id: row.id.clone(),
            title: row.title.clone(),
            state: if row.status == "closed" {
                TaskState::Done
            } else {
                TaskState::Open
            },
            consumes: vec![],
            produces: vec![],
            tasks_axi: None,
            beads_jsonl: Some(row.raw.clone()),
        });
        if let Some(deps) = row.value.get("dependencies").and_then(Value::as_array) {
            for dep in deps {
                let Some(target) = dep.get("depends_on_id").and_then(Value::as_str) else {
                    continue;
                };
                let kind = dep.get("type").and_then(Value::as_str).unwrap_or("blocks");
                if kind != "blocks" {
                    continue;
                }
                if ids.contains(target) {
                    edges.push(OrderEdge {
                        from: target.into(),
                        to: row.id.clone(),
                    });
                } else {
                    unlinked.push(UnlinkedDependency {
                        issue: row.id.clone(),
                        depends_on: target.into(),
                    });
                }
            }
        }
    }
    let doc = PlanDoc { ir_version: IR_VERSION, plan: PlanMeta { id: "beads-import".into(), title: "Beads issues".into(), description: "Imported Beads issue JSONL; original records are preserved in beads_jsonl passthrough.".into(), external_inputs: vec![], tasks_axi_sections: vec![] }, objective: Objective { root_task: ROOT_TASK_ID.into(), weights: BTreeMap::new(), budget_usd: None, deadline: None, quality_floor: None, platforms: vec![], licenses: vec![], acceptance: vec![AcceptanceCriterion { kind: AcceptanceKind::Manual, description: "Issues round-trip to Beads JSONL.".into(), command: None, metric: None, op: None, value: None }] }, artifact: vec![], resource: vec![], task: tasks, method: vec![Method { id: "beads-issues".into(), task: ROOT_TASK_ID.into(), kind: MethodKind::Decompose, subtasks, order_edges: edges, capability: None, status: MethodStatus::Accepted, provenance: "agent:pc-adapter-beads".into(), assumptions: vec!["Beads issue fields are preserved verbatim; blocks dependencies map to IR order edges.".into()] }], capability: vec![], evidence: vec![] };
    Ok(Import { doc, unlinked })
}

/// Export the original issue records in their imported order.
///
/// Rows whose id, title, open/done state and blocking dependencies match the
/// IR are emitted byte-identical. Otherwise the record is patched: renamed ids
/// are rewritten in every dependency that names them, and `blocks`
/// dependencies between issues in the file rebuild from the non-rejected
/// decompose methods' order edges. Dependencies on issues outside the file
/// are carried through untouched. States with no Beads status are refused.
pub fn export_jsonl(doc: &PlanDoc) -> Result<String, AdapterError> {
    let mut blockers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for method in &doc.method {
        if method.status == MethodStatus::Rejected || method.kind != MethodKind::Decompose {
            continue;
        }
        for edge in &method.order_edges {
            let entry = blockers.entry(edge.to.as_str()).or_default();
            if !entry.contains(&edge.from.as_str()) {
                entry.push(edge.from.as_str());
            }
        }
    }
    let mut rows = Vec::new();
    let mut renamed = BTreeMap::new();
    for task in &doc.task {
        if task.id == ROOT_TASK_ID {
            continue;
        }
        let raw = task
            .beads_jsonl
            .as_deref()
            .ok_or_else(|| AdapterError::MissingPassthrough(task.id.clone()))?;
        let value: Value = serde_json::from_str(raw).map_err(|e| AdapterError::Json {
            line: 0,
            message: e.to_string(),
        })?;
        let original_id = value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        renamed.insert(original_id.clone(), task.id.clone());
        rows.push((task, raw, value, original_id));
    }
    let mut out = Vec::new();
    for (task, raw, original, original_id) in rows {
        let mut value = original.clone();
        let original_state = if value.get("status").and_then(Value::as_str) == Some("closed") {
            TaskState::Done
        } else {
            TaskState::Open
        };
        if task.state != original_state {
            value["status"] = match task.state {
                TaskState::Done => "closed",
                TaskState::Open => "open",
                TaskState::Decomposed => {
                    return Err(AdapterError::UnsupportedState(
                        task.id.clone(),
                        "decomposed",
                    ));
                }
                TaskState::Resolved => {
                    return Err(AdapterError::UnsupportedState(task.id.clone(), "resolved"));
                }
                TaskState::Dropped => {
                    return Err(AdapterError::UnsupportedState(task.id.clone(), "dropped"));
                }
            }
            .into();
        }
        value["id"] = task.id.clone().into();
        value["title"] = task.title.clone().into();
        let wanted = blockers.get(task.id.as_str()).cloned().unwrap_or_default();
        let mut linked = Vec::new();
        let mut deps = Vec::new();
        for dep in original
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mut dep = dep.clone();
            if dep.get("issue_id").and_then(Value::as_str) == Some(original_id.as_str()) {
                dep["issue_id"] = task.id.clone().into();
            }
            let target = dep.get("depends_on_id").and_then(Value::as_str);
            if let Some(target) = target.and_then(|t| renamed.get(t)) {
                dep["depends_on_id"] = target.clone().into();
                if dep.get("type").and_then(Value::as_str).unwrap_or("blocks") == "blocks" {
                    if !wanted.contains(&target.as_str()) {
                        continue;
                    }
                    linked.push(target.clone());
                }
            }
            deps.push(dep);
        }
        for from in wanted {
            if !linked.iter().any(|known| known == from) {
                deps.push(serde_json::json!({
                    "issue_id": task.id,
                    "depends_on_id": from,
                    "type": "blocks",
                }));
            }
        }
        if original.get("dependencies").is_some_and(Value::is_array) || !deps.is_empty() {
            value["dependencies"] = deps.into();
        }
        if value == original {
            out.push(raw.to_owned());
        } else {
            out.push(serde_json::to_string(&value).expect("JSON value always serializes"));
        }
    }
    if out.is_empty() {
        return Err(AdapterError::Empty);
    }
    Ok(format!("{}\n", out.join("\n")))
}
