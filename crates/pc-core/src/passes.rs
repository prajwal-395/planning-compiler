//! Passes P1 (load/resolve) and P2 (validate).
//!
//! P1 parses the document (done by `pc-store`), rejects a wrong
//! `ir_version` (E000), indexes entities, reports duplicate ids (E001) and
//! canonicalizes evidence units (only the metric's canonical unit is
//! accepted, case-insensitively).
//!
//! P2 runs the semantic checks E002-E015 over the resolved indexes.
//! Only methods with status `accepted` or `derived` participate in hierarchy,
//! contract and compilation checks; `proposed` methods are hypotheses for
//! step 7 and `rejected` methods are ignored by every pass. `dropped` tasks
//! are abandoned work and are skipped by contract and dataflow checks.
//!
//! Everything here is deterministic: indexes are `BTreeMap`s, diagnostics
//! are sorted by `(code, subject, file, line, message)`, and the same inputs always
//! produce byte-identical reports.

use pc_ir::{
    ContractItem, KNOWN_METRICS, MethodStatus, PlanDoc, TaskState, canonical_unit, check_version,
};
use petgraph::algo::toposort;
use petgraph::graph::DiGraph;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Diagnostic;

/// A trace event: one pass decision about one subject.
///
/// `pc check --json` reports these. v0 records pass boundaries plus one
/// event per diagnostic so the trail is complete from day one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Pass name, e.g. `"P2-validate"`.
    pub pass: String,
    /// Entity the decision is about, e.g. `method:voz-mfa`.
    pub subject: String,
    /// What was decided, e.g. `"error"` or `"ok"`.
    pub decision: String,
    /// Why, usually the diagnostic code or a short reason.
    pub reason: String,
}

/// Entity counts for a checked plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// Number of tasks.
    pub tasks: usize,
    /// Number of methods.
    pub methods: usize,
    /// Number of capabilities.
    pub capabilities: usize,
    /// Number of declared artifact types.
    pub artifacts: usize,
    /// Number of evidence records.
    pub evidence: usize,
    /// Number of error diagnostics.
    pub errors: usize,
}

/// The deterministic result of running P1-P2 over one plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Sorted error diagnostics. Empty means the plan checks clean.
    pub diagnostics: Vec<Diagnostic>,
    /// Entity counts, including the error count.
    pub counts: Counts,
    /// Pass decisions in deterministic order.
    pub trace: Vec<TraceEvent>,
}

impl Report {
    /// True when no error diagnostic was produced.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// A compiler pass in the P1-P9 pipeline. Only P1-P2 exist in v0;
/// contributors add later passes as new implementors without touching the
/// manager. See `docs/passes.md` for each pass's contract.
pub trait Pass {
    /// Short stable name, e.g. `"P2-validate"`.
    fn name(&self) -> &'static str;
    /// Run against the resolve context, appending diagnostics and trace events.
    fn run(&self, ctx: &mut Ctx);
}

/// Mutable state threaded through passes: indexes, source text for line
/// lookup, and the outputs being accumulated.
pub struct Ctx<'a> {
    /// Display path of the plan file, used in diagnostics.
    pub file: &'a str,
    /// Raw source text, used for best-effort line lookup.
    pub text: &'a str,
    /// The plan under check. P1 canonicalizes evidence unit case in place.
    pub doc: &'a mut PlanDoc,
    /// Task ids mapped to their first-seen position.
    pub tasks: BTreeMap<String, usize>,
    /// Method ids mapped to their first-seen position.
    pub methods: BTreeMap<String, usize>,
    /// Capability ids mapped to their first-seen position.
    pub capabilities: BTreeMap<String, usize>,
    /// Artifact names mapped to their first-seen position.
    pub artifacts: BTreeMap<String, usize>,
    /// Resource names mapped to their first-seen position.
    pub resources: BTreeMap<String, usize>,
    /// Accumulated diagnostics.
    pub diagnostics: Vec<Diagnostic>,
    /// Accumulated trace events.
    pub trace: Vec<TraceEvent>,
    /// Origin of the evidence records loaded from a JSONL log, if any.
    pub evidence_log: Option<EvidenceLog<'a>>,
}

/// Where the trailing `lines.len()` records of `PlanDoc::evidence` came
/// from: a JSONL log file, one 1-based line per record.
#[derive(Debug, Clone, Copy)]
pub struct EvidenceLog<'a> {
    /// Display path of the evidence log.
    pub file: &'a str,
    /// 1-based line of each log record, in record order.
    pub lines: &'a [u32],
}

impl<'a> Ctx<'a> {
    /// Push one error diagnostic plus its trace event.
    pub fn error(&mut self, code: &str, message: String, line: Option<u32>, subject: String) {
        self.error_in("P2-validate", code, message, (self.file, line), subject);
    }

    /// Push one error diagnostic anchored at `(file, line)` for `pass`.
    pub fn error_in(
        &mut self,
        pass: &str,
        code: &str,
        message: String,
        (file, line): (&str, Option<u32>),
        subject: String,
    ) {
        self.trace.push(TraceEvent {
            pass: pass.to_string(),
            subject: subject.clone(),
            decision: "error".to_string(),
            reason: code.to_string(),
        });
        self.diagnostics
            .push(Diagnostic::error(code, message, file, line, subject));
    }

    /// File and line of evidence record `i`: its JSONL log line when it was
    /// loaded from the log, otherwise a lookup in the plan file.
    #[must_use]
    pub fn evidence_anchor(&self, i: usize) -> (&'a str, Option<u32>) {
        let total = self.doc.evidence.len();
        if let Some(log) = self.evidence_log {
            let inline = total.saturating_sub(log.lines.len());
            if i >= inline {
                return (log.file, log.lines.get(i - inline).copied());
            }
        }
        (self.file, self.locate(&self.doc.evidence[i].subject))
    }

    /// Best-effort 1-based line number for the first line containing `needle`.
    #[must_use]
    pub fn locate(&self, needle: &str) -> Option<u32> {
        find_line(self.text, needle)
    }

    /// Line lookup for an entity identified by an `id = "<id>"` entry.
    #[must_use]
    pub fn locate_id(&self, id: &str) -> Option<u32> {
        self.locate(&format!("\"{id}\""))
    }
}

/// P1: resolve ids and references, canonicalize evidence units.
///
/// Reports E000 (unsupported `ir_version`), E001 (duplicate ids) and the
/// unit half of E014 (evidence units other than the metric's canonical
/// unit). Everything else is left for P2.
pub struct ResolvePass;

impl Pass for ResolvePass {
    fn name(&self) -> &'static str {
        "P1-resolve"
    }

    fn run(&self, ctx: &mut Ctx) {
        ctx.trace.push(TraceEvent {
            pass: self.name().to_string(),
            subject: "plan".to_string(),
            decision: "started".to_string(),
            reason: "load".to_string(),
        });
        if let Err(e) = check_version(ctx.doc) {
            let line = ctx.locate("ir_version");
            let msg = e.to_string();
            ctx.trace.push(TraceEvent {
                pass: self.name().to_string(),
                subject: "plan".to_string(),
                decision: "error".to_string(),
                reason: "E000".to_string(),
            });
            ctx.diagnostics.push(Diagnostic::error(
                "E000",
                format!("cannot resolve plan: {msg}"),
                ctx.file,
                line,
                "plan".to_string(),
            ));
            return;
        }
        index(
            ctx,
            "task",
            ctx.doc.task.iter().map(|t| t.id.clone()).collect(),
        );
        index(
            ctx,
            "method",
            ctx.doc.method.iter().map(|m| m.id.clone()).collect(),
        );
        index(
            ctx,
            "capability",
            ctx.doc.capability.iter().map(|c| c.id.clone()).collect(),
        );
        index(
            ctx,
            "artifact",
            ctx.doc.artifact.iter().map(|a| a.name.clone()).collect(),
        );
        index(
            ctx,
            "resource",
            ctx.doc.resource.iter().map(|r| r.name.clone()).collect(),
        );
        canonicalize_evidence_units(ctx);
        ctx.trace.push(TraceEvent {
            pass: self.name().to_string(),
            subject: "plan".to_string(),
            decision: "finished".to_string(),
            reason: "resolved".to_string(),
        });
    }
}

/// Best-effort 1-based line number for the first line containing `needle`.
fn find_line(text: &str, needle: &str) -> Option<u32> {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|i| u32::try_from(i + 1).unwrap_or(u32::MAX))
}

/// Insert ids into one namespace index, reporting E001 for repeats.
fn index(ctx: &mut Ctx, namespace: &str, ids: Vec<String>) {
    // Disjoint field borrows so id indexing and line lookup can coexist.
    let text = ctx.text;
    let file = ctx.file;
    let (map, trace): (&mut BTreeMap<String, usize>, &mut Vec<TraceEvent>) = match namespace {
        "task" => (&mut ctx.tasks, &mut ctx.trace),
        "method" => (&mut ctx.methods, &mut ctx.trace),
        "capability" => (&mut ctx.capabilities, &mut ctx.trace),
        "artifact" => (&mut ctx.artifacts, &mut ctx.trace),
        "resource" => (&mut ctx.resources, &mut ctx.trace),
        _ => return,
    };
    let diagnostics = &mut ctx.diagnostics;
    for (position, id) in ids.into_iter().enumerate() {
        if let std::collections::btree_map::Entry::Vacant(slot) = map.entry(id.clone()) {
            slot.insert(position);
        } else {
            let line = find_line(text, &format!("\"{id}\""));
            let subject = format!("{namespace}:{id}");
            trace.push(TraceEvent {
                pass: "P1-resolve".to_string(),
                subject: subject.clone(),
                decision: "error".to_string(),
                reason: "E001".to_string(),
            });
            diagnostics.push(Diagnostic::error(
                "E001",
                format!("duplicate {namespace} id `{id}`"),
                file,
                line,
                subject,
            ));
        }
    }
}

/// Accept only each known metric's canonical unit (case-insensitive) and
/// canonicalize its case in place. Any other unit, including an empty one,
/// is E014 here in P1, because later passes rely on canonical units.
fn canonicalize_evidence_units(ctx: &mut Ctx) {
    let mut bad: Vec<(usize, String, String, String)> = Vec::new();
    for (i, ev) in ctx.doc.evidence.iter_mut().enumerate() {
        if !KNOWN_METRICS.contains(&ev.metric.as_str()) {
            continue;
        }
        let canonical = canonical_unit(&ev.metric).unwrap_or_default().to_string();
        if ev.unit.eq_ignore_ascii_case(&canonical) {
            ev.unit = canonical;
        } else {
            bad.push((i, ev.subject.clone(), ev.metric.clone(), ev.unit.clone()));
        }
    }
    for (i, subject, metric, unit) in bad {
        let expected = canonical_unit(&metric).unwrap_or("?");
        let anchor = ctx.evidence_anchor(i);
        ctx.error_in(
            "P1-resolve",
            "E014",
            format!(
                "evidence for `{subject}` uses unit `{unit}` but metric `{metric}` takes `{expected}`"
            ),
            anchor,
            format!("evidence:{subject}"),
        );
    }
}

/// P2: validate the resolved plan.
///
/// Runs checks E002-E015 (E000/E001/E014-units belong to P1). Diagnostics
/// are sorted before the report is built, so output never depends on input
/// order.
pub struct ValidatePass;

impl Pass for ValidatePass {
    fn name(&self) -> &'static str {
        "P2-validate"
    }

    fn run(&self, ctx: &mut Ctx) {
        // Skip semantic checks when P1 could not resolve the version:
        // every id lookup would be noise on top of E000.
        if ctx.diagnostics.iter().any(|d| d.code == "E000") {
            return;
        }
        check_task_refs(ctx);
        check_capability_refs(ctx);
        check_artifacts(ctx);
        check_hierarchy_cycles(ctx);
        check_order_cycles(ctx);
        check_contracts(ctx);
        check_dataflow(ctx);
        check_metrics(ctx);
        check_estimates(ctx);
        check_resources(ctx);
        check_method_shapes(ctx);
        check_evidence(ctx);
        check_objective(ctx);
    }
}

/// True for methods that participate in hierarchy/contract checks.
fn is_committed(status: MethodStatus) -> bool {
    matches!(status, MethodStatus::Accepted | MethodStatus::Derived)
}

/// True for tasks that contract and dataflow checks apply to.
fn is_live(state: TaskState) -> bool {
    !matches!(state, TaskState::Dropped)
}

/// E002: every task reference points at a declared task.
fn check_task_refs(ctx: &mut Ctx) {
    let mut refs: Vec<(String, String, String)> = Vec::new();
    for m in &ctx.doc.method {
        if m.status == pc_ir::MethodStatus::Rejected {
            continue;
        }
        refs.push((
            format!("method:{}", m.id),
            m.task.clone(),
            format!("method `{}` refers to unknown task `{}`", m.id, m.task),
        ));
        for sub in &m.subtasks {
            refs.push((
                format!("method:{}", m.id),
                sub.clone(),
                format!("method `{}` lists unknown subtask `{}`", m.id, sub),
            ));
        }
        for edge in &m.order_edges {
            refs.push((
                format!("method:{}", m.id),
                edge.from.clone(),
                format!(
                    "method `{}` has an order edge from unknown subtask `{}`",
                    m.id, edge.from
                ),
            ));
            refs.push((
                format!("method:{}", m.id),
                edge.to.clone(),
                format!(
                    "method `{}` has an order edge to unknown subtask `{}`",
                    m.id, edge.to
                ),
            ));
        }
    }
    refs.push((
        "objective".to_string(),
        ctx.doc.objective.root_task.clone(),
        format!(
            "objective root_task refers to unknown task `{}`",
            ctx.doc.objective.root_task
        ),
    ));
    // Order-edge endpoints must also be subtasks of the same method; that
    // membership check lives in E013, existence lives here.
    for (subject, target, message) in refs {
        if !ctx.tasks.contains_key(&target) {
            let line = ctx.locate_id(&target).or_else(|| ctx.locate(&subject));
            ctx.error("E002", message, line, subject);
        }
    }
}

/// E003: every non-rejected invoke method names a declared capability.
fn check_capability_refs(ctx: &mut Ctx) {
    let mut missing: Vec<(String, String)> = Vec::new();
    for m in &ctx.doc.method {
        if m.kind != pc_ir::MethodKind::Invoke || m.status == pc_ir::MethodStatus::Rejected {
            continue;
        }
        if let Some(cap) = &m.capability {
            if !ctx.capabilities.contains_key(cap) {
                missing.push((
                    format!("method:{}", m.id),
                    format!("method `{}` invokes unknown capability `{cap}`", m.id),
                ));
            }
        }
    }
    for (subject, message) in missing {
        let line = ctx.locate(&subject);
        ctx.error("E003", message, line, subject);
    }
}

/// E004/E005: contract items name declared artifacts and declared fields.
fn check_artifacts(ctx: &mut Ctx) {
    let decls: BTreeMap<String, BTreeSet<String>> = ctx
        .doc
        .artifact
        .iter()
        .map(|a| (a.name.clone(), a.fields.iter().cloned().collect()))
        .collect();
    let mut items: Vec<(String, ContractItem, String)> = Vec::new();
    for t in &ctx.doc.task {
        for c in t.consumes.iter().chain(&t.produces) {
            items.push((
                format!("task:{}", t.id),
                c.clone(),
                format!("task `{}`", t.id),
            ));
        }
    }
    for c in &ctx.doc.capability {
        for item in c.consumes.iter().chain(&c.produces) {
            items.push((
                format!("capability:{}", c.id),
                item.clone(),
                format!("capability `{}`", c.id),
            ));
        }
    }
    for item in &ctx.doc.plan.external_inputs {
        items.push((
            "plan".to_string(),
            item.clone(),
            "plan external_inputs".to_string(),
        ));
    }
    for (subject, item, whose) in items {
        let Some(fields) = decls.get(&item.artifact) else {
            let line = ctx.locate(&item.artifact);
            ctx.error(
                "E004",
                format!("{whose} uses undeclared artifact `{}`", item.artifact),
                line,
                subject,
            );
            continue;
        };
        for field in &item.fields {
            if !fields.contains(field) {
                let line = ctx.locate(field);
                ctx.error(
                    "E005",
                    format!("{whose} uses undeclared field `{}.{field}`", item.artifact),
                    line,
                    subject.clone(),
                );
            }
        }
    }
}

/// E006: the committed method hierarchy is acyclic.
///
/// Only `accepted`/`derived` decompose methods add edges; `proposed`
/// methods are hypotheses and `rejected` ones are ignored.
fn check_hierarchy_cycles(ctx: &mut Ctx) {
    let order: Vec<String> = ctx.tasks.keys().cloned().collect();
    let index_of: BTreeMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut graph: DiGraph<(), ()> = DiGraph::new();
    for _ in &order {
        graph.add_node(());
    }
    // Deterministic edge order: methods sorted by id via BTreeMap-free
    // sorted vec, since &mut borrows above are done.
    let mut methods: Vec<(&String, &String, &Vec<String>)> = Vec::new();
    for m in &ctx.doc.method {
        if m.kind == pc_ir::MethodKind::Decompose && is_committed(m.status) {
            methods.push((&m.id, &m.task, &m.subtasks));
        }
    }
    methods.sort();
    for (_, parent, subtasks) in methods {
        let Some(&p) = index_of.get(parent.as_str()) else {
            continue;
        };
        let mut children: Vec<&String> = subtasks.iter().collect();
        children.sort();
        for child in children {
            if let Some(&c) = index_of.get(child.as_str()) {
                graph.add_edge(
                    petgraph::graph::NodeIndex::new(p),
                    petgraph::graph::NodeIndex::new(c),
                    (),
                );
            }
        }
    }
    if let Err(cycle) = toposort(&graph, None) {
        let anchor = order[cycle.node_id().index()].clone();
        let line = ctx.locate_id(&anchor);
        ctx.error(
            "E006",
            format!(
                "method hierarchy contains a cycle involving task `{anchor}`: \
                 acyclic hierarchies keep plan existence decidable"
            ),
            line,
            format!("task:{anchor}"),
        );
    }
}

/// Owned snapshot of one decompose method for cycle checking.
type DecomposeSpec = (String, Vec<String>, Vec<(String, String)>);

/// E007: order edges inside each non-rejected decompose method are acyclic.
fn check_order_cycles(ctx: &mut Ctx) {
    let mut methods: Vec<DecomposeSpec> = Vec::new();
    for m in &ctx.doc.method {
        if m.kind != pc_ir::MethodKind::Decompose || m.status == pc_ir::MethodStatus::Rejected {
            continue;
        }
        methods.push((
            m.id.clone(),
            m.subtasks.clone(),
            m.order_edges
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect(),
        ));
    }
    methods.sort();
    for (id, subtasks, edges) in methods {
        let index_of: BTreeMap<&str, usize> = subtasks
            .iter()
            .enumerate()
            .map(|(i, s)| (s.as_str(), i))
            .collect();
        let mut graph: DiGraph<(), ()> = DiGraph::new();
        for _ in &subtasks {
            graph.add_node(());
        }
        let mut sorted_edges = edges;
        sorted_edges.sort();
        for (from, to) in sorted_edges {
            if let (Some(&f), Some(&t)) = (index_of.get(from.as_str()), index_of.get(to.as_str())) {
                graph.add_edge(
                    petgraph::graph::NodeIndex::new(f),
                    petgraph::graph::NodeIndex::new(t),
                    (),
                );
            }
        }
        if toposort(&graph, None).is_err() {
            let line = ctx.locate_id(&id);
            ctx.error(
                "E007",
                format!("method `{id}` has a cycle in its order edges"),
                line,
                format!("method:{id}"),
            );
        }
    }
}

/// Compare two contracts field by field: every `(artifact, field)` in
/// `need` must appear in `have`. Returns the missing entries.
fn missing_fields(need: &[ContractItem], have: &[ContractItem]) -> Vec<String> {
    let mut have_map: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for item in have {
        have_map
            .entry(item.artifact.as_str())
            .or_default()
            .extend(item.fields.iter().map(String::as_str));
    }
    let mut missing: Vec<String> = Vec::new();
    for item in need {
        let have_fields = have_map.get(item.artifact.as_str());
        let mut fields: Vec<&String> = item.fields.iter().collect();
        fields.sort();
        for field in fields {
            let covered = have_fields.is_some_and(|s| s.contains(field.as_str()));
            if !covered {
                missing.push(format!("{}.{field}", item.artifact));
            }
        }
    }
    missing.sort();
    missing.dedup();
    missing
}

/// E008: typed consumes/produces matching on committed invoke methods.
///
/// The capability must produce everything the task promises downstream,
/// and everything the capability consumes must come from the task's own
/// consumables or plan external inputs.
fn check_contracts(ctx: &mut Ctx) {
    let caps: BTreeMap<String, (Vec<ContractItem>, Vec<ContractItem>)> = ctx
        .doc
        .capability
        .iter()
        .map(|c| (c.id.clone(), (c.produces.clone(), c.consumes.clone())))
        .collect();
    let tasks: BTreeMap<String, (Vec<ContractItem>, Vec<ContractItem>)> = ctx
        .doc
        .task
        .iter()
        .map(|t| (t.id.clone(), (t.produces.clone(), t.consumes.clone())))
        .collect();
    let mut methods: Vec<(String, String, String)> = Vec::new();
    for m in &ctx.doc.method {
        if m.kind == pc_ir::MethodKind::Invoke
            && is_committed(m.status)
            && let Some(cap) = &m.capability
        {
            methods.push((m.id.clone(), m.task.clone(), cap.clone()));
        }
    }
    methods.sort();
    for (id, task_id, cap_id) in methods {
        let (Some((cap_produces, cap_consumes)), Some((task_produces, task_consumes))) =
            (caps.get(&cap_id), tasks.get(&task_id))
        else {
            continue;
        };
        let subject = format!("method:{id}");
        let line = ctx.locate_id(&id);
        for missing in missing_fields(task_produces, cap_produces) {
            ctx.error(
                "E008",
                format!(
                    "method `{id}` cannot satisfy task `{task_id}`: \
                     capability `{cap_id}` does not produce `{missing}`"
                ),
                line,
                subject.clone(),
            );
        }
        let mut available = task_consumes.clone();
        available.extend(ctx.doc.plan.external_inputs.clone());
        for missing in missing_fields(cap_consumes, &available) {
            ctx.error(
                "E008",
                format!(
                    "method `{id}` cannot run: capability `{cap_id}` \
                     consumes `{missing}` which task `{task_id}` does not provide"
                ),
                line,
                subject.clone(),
            );
        }
    }
}

/// E009: every consumed field is produced by another live task or declared
/// as a plan external input. Self-production alone does not satisfy a
/// consumption: something else must provide the field. The check is global
/// in v0 (no ordering-aware dataflow); ordering-aware analysis is deferred
/// past step 1.
fn check_dataflow(ctx: &mut Ctx) {
    let mut external: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for item in &ctx.doc.plan.external_inputs {
        external
            .entry(item.artifact.clone())
            .or_default()
            .extend(item.fields.iter().cloned());
    }
    // Owned snapshot so diagnostics can borrow ctx mutably below.
    let mut tasks: Vec<(String, Vec<ContractItem>)> = ctx
        .doc
        .task
        .iter()
        .filter(|t| is_live(t.state))
        .map(|t| (t.id.clone(), t.consumes.clone()))
        .collect();
    tasks.sort_by(|a, b| a.0.cmp(&b.0));
    let producers: Vec<(String, Vec<ContractItem>)> = ctx
        .doc
        .task
        .iter()
        .filter(|t| is_live(t.state))
        .map(|t| (t.id.clone(), t.produces.clone()))
        .collect();
    for (id, consumes) in &tasks {
        let subject = format!("task:{id}");
        let line = ctx.locate_id(id);
        for item in consumes.iter() {
            let mut fields: Vec<&String> = item.fields.iter().collect();
            fields.sort();
            for field in fields {
                let from_other = producers.iter().any(|(pid, produces)| {
                    pid != id
                        && produces
                            .iter()
                            .any(|p| p.artifact == item.artifact && p.fields.contains(field))
                });
                let from_external = external
                    .get(&item.artifact)
                    .is_some_and(|s| s.contains(field.as_str()));
                if !(from_other || from_external) {
                    ctx.error(
                        "E009",
                        format!(
                            "task `{id}` consumes `{}.{field}` which no \
                             other task produces and no external input provides",
                            item.artifact
                        ),
                        line,
                        subject.clone(),
                    );
                }
            }
        }
    }
}

/// A diagnostic location: display path plus best-effort line.
type Anchor<'a> = (&'a str, Option<u32>);

/// E010: every metric reference names a known metric.
///
/// Covers estimates and evidence. Objective weights are checked under E015
/// so one mistake produces one diagnostic.
fn check_metrics(ctx: &mut Ctx) {
    let mut refs: Vec<(String, String, Anchor<'_>)> = Vec::new();
    for c in &ctx.doc.capability {
        for e in &c.estimates {
            if !KNOWN_METRICS.contains(&e.metric.as_str()) {
                refs.push((
                    format!("capability:{}", c.id),
                    format!("capability `{}` uses unknown metric `{}`", c.id, e.metric),
                    (ctx.file, ctx.locate(&e.metric)),
                ));
            }
        }
    }
    for (i, ev) in ctx.doc.evidence.iter().enumerate() {
        if !KNOWN_METRICS.contains(&ev.metric.as_str()) {
            refs.push((
                format!("evidence:{}", ev.subject),
                format!(
                    "evidence for `{}` uses unknown metric `{}`",
                    ev.subject, ev.metric
                ),
                ctx.evidence_anchor(i),
            ));
        }
    }
    for (subject, message, anchor) in refs {
        ctx.error_in("P2-validate", "E010", message, anchor, subject);
    }
}

/// E011: three-point estimates are ordered, finite, non-negative, and in
/// range; `measured` estimates cite observations.
fn check_estimates(ctx: &mut Ctx) {
    let mut estimates: Vec<(String, pc_ir::Estimate)> = Vec::new();
    for c in &ctx.doc.capability {
        for e in &c.estimates {
            estimates.push((format!("capability:{}", c.id), e.clone()));
        }
    }
    estimates.sort_by(|a, b| a.0.cmp(&b.0));
    for (subject, e) in estimates {
        let line = ctx.locate(&e.metric);
        if !KNOWN_METRICS.contains(&e.metric.as_str()) {
            continue;
        }
        let mut bad: Option<String> = None;
        if ![e.low, e.likely, e.high].iter().all(|v| v.is_finite()) {
            bad = Some("estimates must be finite numbers".to_string());
        } else if e.low > e.likely || e.likely > e.high {
            bad = Some(format!(
                "three-point estimate violates low <= likely <= high \
                 ({} <= {} <= {})",
                e.low, e.likely, e.high
            ));
        } else if e.low < 0.0 {
            bad = Some("estimates must be non-negative".to_string());
        } else if e.metric == "failure_risk" && e.high > 1.0 {
            bad = Some("failure_risk estimates must lie in [0, 1]".to_string());
        } else if e.provenance == pc_ir::EstimateProvenance::Measured
            && e.observations.unwrap_or(0) == 0
        {
            bad = Some("measured estimates must cite observations".to_string());
        }
        if let Some(reason) = bad {
            ctx.error(
                "E011",
                format!("invalid estimate for `{}` on {subject}: {reason}", e.metric),
                line,
                subject,
            );
        }
    }
}

/// E012: capability resources name declared resources.
fn check_resources(ctx: &mut Ctx) {
    let mut refs: Vec<(String, String, String)> = Vec::new();
    for c in &ctx.doc.capability {
        for r in &c.resources {
            refs.push((
                format!("capability:{}", c.id),
                format!("capability `{}` uses undeclared resource `{r}`", c.id),
                r.clone(),
            ));
        }
    }
    refs.sort();
    for (subject, message, target) in refs {
        if !ctx.resources.contains_key(&target) {
            let line = ctx.locate_id(&target).or_else(|| ctx.locate(&subject));
            ctx.error("E012", message, line, subject);
        }
    }
}

/// E013: method shapes match their kind, and order-edge endpoints are
/// subtasks of the same method.
fn check_method_shapes(ctx: &mut Ctx) {
    let mut methods: Vec<pc_ir::Method> = ctx.doc.method.to_vec();
    methods.sort_by(|a, b| a.id.cmp(&b.id));
    for m in methods {
        if m.status == pc_ir::MethodStatus::Rejected {
            continue;
        }
        let subject = format!("method:{}", m.id);
        let line = ctx.locate_id(&m.id);
        match m.kind {
            pc_ir::MethodKind::Invoke => {
                if m.capability.is_none() {
                    ctx.error(
                        "E013",
                        format!("invoke method `{}` names no capability", m.id),
                        line,
                        subject.clone(),
                    );
                }
                if !m.subtasks.is_empty() || !m.order_edges.is_empty() {
                    ctx.error(
                        "E013",
                        format!(
                            "invoke method `{}` must not list subtasks or order edges",
                            m.id
                        ),
                        line,
                        subject.clone(),
                    );
                }
            }
            pc_ir::MethodKind::Decompose => {
                if m.capability.is_some() {
                    ctx.error(
                        "E013",
                        format!("decompose method `{}` must not name a capability", m.id),
                        line,
                        subject.clone(),
                    );
                }
                if m.subtasks.is_empty() {
                    ctx.error(
                        "E013",
                        format!("decompose method `{}` lists no subtasks", m.id),
                        line,
                        subject.clone(),
                    );
                }
                let members: BTreeSet<&str> = m.subtasks.iter().map(String::as_str).collect();
                let mut ends: Vec<(&str, &str)> = m
                    .order_edges
                    .iter()
                    .map(|e| (e.from.as_str(), e.to.as_str()))
                    .collect();
                ends.sort();
                for (from, to) in ends {
                    for end in [from, to] {
                        if ctx.tasks.contains_key(end) && !members.contains(end) {
                            ctx.error(
                                "E013",
                                format!(
                                    "method `{}` orders `{end}`, which is not one of its subtasks",
                                    m.id
                                ),
                                line,
                                subject.clone(),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Split an evidence subject into `(prefix, id)`.
fn split_subject(subject: &str) -> Option<(&str, &str)> {
    subject.split_once(':')
}

/// E014: evidence subjects exist, timestamps are RFC 3339 and values are
/// finite. Units belong to P1.
fn check_evidence(ctx: &mut Ctx) {
    let mut order: Vec<usize> = (0..ctx.doc.evidence.len()).collect();
    order.sort_by(|&a, &b| {
        ctx.doc.evidence[a]
            .subject
            .cmp(&ctx.doc.evidence[b].subject)
    });
    for i in order {
        let ev = ctx.doc.evidence[i].clone();
        let subject = format!("evidence:{}", ev.subject);
        let anchor = ctx.evidence_anchor(i);
        match split_subject(&ev.subject) {
            Some(("capability", id)) if ctx.capabilities.contains_key(id) => {}
            Some(("method", id)) if ctx.methods.contains_key(id) => {}
            Some(("task", id)) if ctx.tasks.contains_key(id) => {}
            _ => {
                ctx.error_in(
                    "P2-validate",
                    "E014",
                    format!(
                        "evidence subject `{}` names nothing: use \
                         capability:<id>, method:<id> or task:<id>",
                        ev.subject
                    ),
                    anchor,
                    subject.clone(),
                );
                continue;
            }
        }
        if !KNOWN_METRICS.contains(&ev.metric.as_str()) {
            continue;
        }
        if chrono::DateTime::parse_from_rfc3339(&ev.time).is_err() {
            ctx.error_in(
                "P2-validate",
                "E014",
                format!(
                    "evidence for `{}` has timestamp `{}` which is not RFC 3339",
                    ev.subject, ev.time
                ),
                anchor,
                subject.clone(),
            );
        }
        if !ev.value.is_finite() {
            ctx.error_in(
                "P2-validate",
                "E014",
                format!("evidence for `{}` has non-finite value", ev.subject),
                anchor,
                subject,
            );
        }
    }
}

/// E015: the objective is well-formed (weights, floors, criteria).
fn check_objective(ctx: &mut Ctx) {
    let obj = ctx.doc.objective.clone();
    let line = ctx.locate("root_task");
    let mut weights: Vec<(&String, &f64)> = obj.weights.iter().collect();
    weights.sort_by(|a, b| a.0.cmp(b.0));
    for (metric, weight) in weights {
        if !KNOWN_METRICS.contains(&metric.as_str()) {
            ctx.error(
                "E015",
                format!("objective weights unknown metric `{metric}`"),
                ctx.locate(metric),
                "objective".to_string(),
            );
        } else if !weight.is_finite() || *weight < 0.0 {
            ctx.error(
                "E015",
                format!("objective weight for `{metric}` must be finite and non-negative"),
                ctx.locate(metric),
                "objective".to_string(),
            );
        }
    }
    if let Some(budget) = obj.budget_usd
        && (!budget.is_finite() || budget < 0.0)
    {
        ctx.error(
            "E015",
            "objective budget_usd must be finite and non-negative".to_string(),
            line,
            "objective".to_string(),
        );
    }
    if let Some(floor) = obj.quality_floor
        && (!floor.is_finite() || floor < 0.0 || floor > 1.0)
    {
        ctx.error(
            "E015",
            "objective quality_floor must lie in [0, 1]".to_string(),
            line,
            "objective".to_string(),
        );
    }
    let mut criteria = obj.acceptance.clone();
    criteria.sort_by(|a, b| {
        format!("{:?}", a.kind)
            .cmp(&format!("{:?}", b.kind))
            .then(a.description.cmp(&b.description))
    });
    for c in criteria {
        match c.kind {
            pc_ir::AcceptanceKind::Manual => {
                if c.description.trim().is_empty() {
                    ctx.error(
                        "E015",
                        "manual acceptance criteria need a description".to_string(),
                        line,
                        "objective".to_string(),
                    );
                }
            }
            pc_ir::AcceptanceKind::Command => {
                if c.command.as_deref().unwrap_or("").trim().is_empty() {
                    ctx.error(
                        "E015",
                        "command acceptance criteria need a command".to_string(),
                        line,
                        "objective".to_string(),
                    );
                }
            }
            pc_ir::AcceptanceKind::Metric => {
                let metric_ok = c
                    .metric
                    .as_deref()
                    .is_some_and(|m| KNOWN_METRICS.contains(&m));
                let op_ok =
                    c.op.as_deref()
                        .is_some_and(|op| matches!(op, "<=" | ">=" | "=="));
                if !metric_ok || !op_ok || c.value.is_none() {
                    ctx.error(
                        "E015",
                        "metric acceptance criteria need a known metric, an op (<=, >=, ==) and a value"
                            .to_string(),
                        line,
                        "objective".to_string(),
                    );
                }
            }
        }
    }
}

/// Run P1 then P2 over a parsed document and build the sorted report.
///
/// `file` is the display path used in diagnostics and `text` the raw source
/// used for line lookup. `evidence_log` anchors records loaded from a JSONL
/// log. Evidence unit case is canonicalized in place, so callers pass a
/// mutable copy when the file must stay pristine.
pub fn check(
    file: &str,
    text: &str,
    doc: &mut PlanDoc,
    evidence_log: Option<EvidenceLog<'_>>,
) -> Report {
    let mut ctx = Ctx {
        file,
        text,
        doc,
        tasks: BTreeMap::new(),
        methods: BTreeMap::new(),
        capabilities: BTreeMap::new(),
        artifacts: BTreeMap::new(),
        resources: BTreeMap::new(),
        diagnostics: Vec::new(),
        trace: Vec::new(),
        evidence_log,
    };
    let passes: Vec<Box<dyn Pass>> = vec![Box::new(ResolvePass), Box::new(ValidatePass)];
    for pass in &passes {
        pass.run(&mut ctx);
    }
    ctx.diagnostics.sort_by(|a, b| {
        a.code
            .cmp(&b.code)
            .then(a.subject.cmp(&b.subject))
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
            .then(a.message.cmp(&b.message))
    });
    ctx.trace.sort_by(|a, b| {
        a.pass
            .cmp(&b.pass)
            .then(a.subject.cmp(&b.subject))
            .then(a.reason.cmp(&b.reason))
            .then(a.decision.cmp(&b.decision))
    });
    let counts = Counts {
        tasks: ctx.doc.task.len(),
        methods: ctx.doc.method.len(),
        capabilities: ctx.doc.capability.len(),
        artifacts: ctx.doc.artifact.len(),
        evidence: ctx.doc.evidence.len(),
        errors: ctx.diagnostics.len(),
    };
    Report {
        diagnostics: std::mem::take(&mut ctx.diagnostics),
        counts,
        trace: std::mem::take(&mut ctx.trace),
    }
}
