//! Passes P1 (load/resolve), P2 (validate), P3 (match), P4 (share) and P5
//! (estimate).
//!
//! P1 parses the document (done by `pc-store`), rejects a wrong
//! `ir_version` (E000), indexes entities, reports duplicate ids (E001) and
//! canonicalizes evidence units (only the metric's canonical unit is
//! accepted, case-insensitively).
//!
//! P2 runs the semantic checks E002-E016 over the resolved indexes.
//! Only methods with status `accepted` or `derived` participate in hierarchy,
//! contract and compilation checks; `proposed` methods are hypotheses for
//! step 7 and `rejected` methods are ignored by every pass. `dropped` tasks
//! are abandoned work and are skipped by contract and dataflow checks.
//!
//! P3 proposes `derived` invoke methods for `open` tasks whose typed
//! contract is covered by a capability. P4 merges structurally identical
//! tasks by content hash so shared work is counted once downstream.
//! Both run only over clean plans: any diagnostic from P1-P2 (or an
//! unsupported version) leaves the document untouched.
//!
//! Everything here is deterministic: indexes are `BTreeMap`s, diagnostics
//! are sorted by `(code, subject, file, line, message)`, and the same inputs always
//! produce byte-identical reports.

use pc_ir::{
    ContractItem, KNOWN_METRICS, Method, MethodKind, MethodStatus, PlanDoc, TaskState,
    canonical_unit, check_version,
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
    /// Informational contract notes for proposed/derived methods that would
    /// fail E008 if accepted (G2). Same content as the frontier
    /// blocked-alternative entries; never affects `ok()` or the exit code.
    #[serde(default)]
    pub contract_notes: Vec<crate::impact::ContractNote>,
}

impl Report {
    /// True when no error diagnostic was produced.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// A compiler pass in the P1-P9 pipeline. P1-P5 live here; P6-P7 run over
/// the passed document in `compile`, and contributors add later passes as
/// new implementors without touching the manager. See `docs/passes.md` for
/// each pass's contract.
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
/// Runs checks E002-E016 (E000/E001/E014-units belong to P1). Diagnostics
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
        check_capability_constraints(ctx);
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
///
/// Shared with P9 (impact and frontier), which preview contract failures for
/// proposed methods P2 deliberately does not check.
pub(crate) fn missing_fields(need: &[ContractItem], have: &[ContractItem]) -> Vec<String> {
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
        } else if canonical_unit(&e.metric) == Some("ratio") && e.high > 1.0 {
            bad = Some(format!("{} estimates must lie in [0, 1]", e.metric));
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

/// E014: evidence subjects are `capability:`, `method:` or `task:` with a
/// non-empty id, timestamps, values and bases are well-formed, one
/// attributed family (capability and metric after P5 placement, see
/// [`crate::estimate::BasisConflict`]) carries one basis, and units were
/// canonical (P1). Subject ids
/// need not exist: a well-formed subject that names nothing in the plan
/// stages pending evidence P5 holds until the subject is proposed, so
/// evidence can arrive together with the hypothesis it supports instead of
/// failing every command.
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
        if let Err(reason) = pc_ir::check_evidence_subject(&ev.subject) {
            ctx.error_in("P2-validate", "E014", reason, anchor, subject);
            continue;
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
        if !ev.value.is_finite() || ev.value < 0.0 {
            ctx.error_in(
                "P2-validate",
                "E014",
                format!(
                    "evidence for `{}` must have a finite, non-negative value",
                    ev.subject
                ),
                anchor,
                subject.clone(),
            );
        }
        if let Err(reason) = pc_ir::check_evidence_basis(&ev.basis) {
            ctx.error_in("P2-validate", "E014", reason, anchor, subject);
        }
    }
    for conflict in crate::estimate::estimate(ctx.doc).basis_conflicts {
        let anchor = ctx.evidence_anchor(conflict.record);
        let subject = format!("evidence:{}", ctx.doc.evidence[conflict.record].subject);
        ctx.error_in("P2-validate", "E014", conflict.message(), anchor, subject);
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

/// The axes on which the capability conflicts with the objective: its
/// `platform_requirements` (or `licensing_constraints`) are alternatives,
/// so it conflicts only when it lists some and none is declared by the
/// objective. An objective that declares no platforms (or no licences)
/// constrains nothing on that axis.
fn constraint_conflicts(objective: &pc_ir::Objective, cap: &pc_ir::Capability) -> Vec<String> {
    let mut conflicts = Vec::new();
    for (axis, declared, listed) in [
        (
            "platforms",
            &objective.platforms,
            &cap.platform_requirements,
        ),
        ("licences", &objective.licenses, &cap.licensing_constraints),
    ] {
        if !declared.is_empty()
            && !listed.is_empty()
            && !listed.iter().any(|l| declared.contains(l))
        {
            let listed: Vec<String> = listed.iter().map(|l| format!("`{l}`")).collect();
            conflicts.push(format!("{axis} {}", listed.join(", ")));
        }
    }
    conflicts
}

/// E016: every non-rejected invoke method's capability runs on at least
/// one of the objective's declared platforms and under at least one of its
/// accepted licences.
fn check_capability_constraints(ctx: &mut Ctx) {
    let mut conflicts: Vec<(String, String)> = Vec::new();
    for m in &ctx.doc.method {
        if m.kind != MethodKind::Invoke || m.status == MethodStatus::Rejected {
            continue;
        }
        let Some(cap) = m
            .capability
            .as_deref()
            .and_then(|id| ctx.doc.capability.iter().find(|c| c.id == id))
        else {
            continue;
        };
        for conflict in constraint_conflicts(&ctx.doc.objective, cap) {
            conflicts.push((
                m.id.clone(),
                format!(
                    "method `{}` invokes capability `{}` supporting only {conflict}, \
                     none of which the objective declares",
                    m.id, cap.id
                ),
            ));
        }
    }
    for (id, message) in conflicts {
        let line = ctx.locate_id(&id);
        ctx.error("E016", message, line, format!("method:{id}"));
    }
}

/// True when `cap_produces` covers everything `task_produces` promises.
fn capability_covers(task_produces: &[ContractItem], cap_produces: &[ContractItem]) -> bool {
    missing_fields(task_produces, cap_produces).is_empty()
}

/// True when everything `cap_consumes` needs is available from `task_consumes`
/// or plan `external` inputs.
fn inputs_satisfied(
    cap_consumes: &[ContractItem],
    task_consumes: &[ContractItem],
    external: &[ContractItem],
) -> bool {
    let mut available = task_consumes.to_vec();
    available.extend(external.iter().cloned());
    missing_fields(cap_consumes, &available).is_empty()
}

/// P3: match open primitive tasks against capabilities by typed contract.
///
/// For each `open` task with no committed method that promises at least one
/// typed output field, propose a `derived` invoke method for every
/// capability whose contract covers the task's contract: the capability must
/// produce everything the task promises, and everything the capability
/// consumes must come from the task or plan external inputs, and one of its
/// listed platforms and one of its licences must be declared by the
/// objective (E016). The matching is exact typed subset matching; no embeddings or
/// negotiation. Tasks that promise nothing need decomposition, not
/// invocation, so they are skipped: an empty promise is covered by every
/// capability and would only spam vacuous proposals. Proposals that collide with an existing method id, or
/// that duplicate a live invoke of the same capability for the same task,
/// are skipped. Surviving proposals are reported with trace events and carry
/// `derived` status.
pub struct MatchPass;

impl Pass for MatchPass {
    fn name(&self) -> &'static str {
        "P3-match"
    }

    fn run(&self, ctx: &mut Ctx) {
        if !ctx.diagnostics.is_empty() {
            return;
        }
        let external = ctx.doc.plan.external_inputs.clone();
        let mut proposals: Vec<(String, String)> = Vec::new();
        for task in &ctx.doc.task {
            if task.state != TaskState::Open {
                continue;
            }
            if !task.produces.iter().any(|c| !c.fields.is_empty()) {
                continue;
            }
            let has_committed = ctx
                .doc
                .method
                .iter()
                .any(|m| m.task == task.id && is_committed(m.status));
            if has_committed {
                continue;
            }
            for cap in &ctx.doc.capability {
                let already_covered = ctx.doc.method.iter().any(|m| {
                    m.kind == MethodKind::Invoke
                        && m.task == task.id
                        && m.capability.as_deref() == Some(cap.id.as_str())
                        && m.status != MethodStatus::Rejected
                });
                if already_covered {
                    continue;
                }
                if capability_covers(&task.produces, &cap.produces)
                    && inputs_satisfied(&cap.consumes, &task.consumes, &external)
                    && constraint_conflicts(&ctx.doc.objective, cap).is_empty()
                {
                    proposals.push((task.id.clone(), cap.id.clone()));
                }
            }
        }
        proposals.sort();
        proposals.dedup();
        let taken: BTreeSet<String> = ctx.doc.method.iter().map(|m| m.id.clone()).collect();
        for (task_id, cap_id) in proposals {
            let method_id = format!("match-{task_id}-{cap_id}");
            if taken.contains(method_id.as_str()) {
                continue;
            }
            ctx.doc.method.push(Method {
                id: method_id,
                task: task_id.clone(),
                kind: MethodKind::Invoke,
                subtasks: Vec::new(),
                order_edges: Vec::new(),
                capability: Some(cap_id.clone()),
                status: MethodStatus::Derived,
                provenance: "pass:match".into(),
                assumptions: Vec::new(),
            });
            ctx.trace.push(TraceEvent {
                pass: self.name().to_string(),
                subject: format!("task:{task_id}"),
                decision: "derived".to_string(),
                reason: format!("capability `{cap_id}` covers the task contract"),
            });
        }
    }
}

/// Canonical share form of a task's own fields for content hashing.
///
/// Serializes the title, lifecycle state, and consumes/produces as sorted
/// `(artifact, fields)` pairs. The task id is intentionally excluded: two
/// ids denote one shared node only when everything else matches. The title
/// is load-bearing, not decoration: matching on contracts alone wrongly
/// merges distinct work (report section 2.7.1: `transcribe_clip_1` must
/// never equal `transcribe_clip_2`), and collapses plans whose steps share
/// an empty contract but do different work. The state keeps finished or
/// abandoned work from merging into live work and vice versa. A task
/// carrying source passthrough (`tasks_axi` or `beads_jsonl`) is one imported
/// row, which is always distinct work, so its id joins the form and it never
/// shares.
fn canonical_task_form(task: &pc_ir::Task) -> serde_json::Value {
    let mut consumes: Vec<(String, Vec<String>)> = task
        .consumes
        .iter()
        .map(|c| {
            let mut fields = c.fields.clone();
            fields.sort();
            (c.artifact.clone(), fields)
        })
        .collect();
    consumes.sort();
    let mut produces: Vec<(String, Vec<String>)> = task
        .produces
        .iter()
        .map(|p| {
            let mut fields = p.fields.clone();
            fields.sort();
            (p.artifact.clone(), fields)
        })
        .collect();
    produces.sort();
    let mut form = serde_json::json!({
        "title": task.title,
        "state": task.state,
        "consumes": consumes,
        "produces": produces,
    });
    if task.tasks_axi.is_some() || task.beads_jsonl.is_some() {
        form["backlog_row"] = task.id.clone().into();
    }
    form
}

/// Memoized blake3 content hashes over each task's canonical form plus the
/// structure of its committed (accepted/derived) methods: kind, capability,
/// and the hashes of its subtasks and order edges. Two tasks hash equal only
/// when their whole committed subtree is identical, so a merge never drops
/// work. P2 has already proven the committed hierarchy acyclic.
struct ShareHasher<'a> {
    tasks: BTreeMap<&'a str, &'a pc_ir::Task>,
    methods: BTreeMap<&'a str, Vec<&'a Method>>,
    memo: BTreeMap<&'a str, String>,
}

impl<'a> ShareHasher<'a> {
    fn new(doc: &'a PlanDoc) -> Self {
        let tasks = doc.task.iter().map(|t| (t.id.as_str(), t)).collect();
        let mut methods: BTreeMap<&str, Vec<&Method>> = BTreeMap::new();
        for m in &doc.method {
            if is_committed(m.status) {
                methods.entry(m.task.as_str()).or_default().push(m);
            }
        }
        ShareHasher {
            tasks,
            methods,
            memo: BTreeMap::new(),
        }
    }

    fn hash(&mut self, id: &'a str) -> String {
        if let Some(hash) = self.memo.get(id) {
            return hash.clone();
        }
        let Some(task) = self.tasks.get(id).copied() else {
            return String::new();
        };
        let mut forms: Vec<String> = Vec::new();
        for m in self.methods.get(id).cloned().unwrap_or_default() {
            let subtasks: Vec<String> = m.subtasks.iter().map(|s| self.hash(s)).collect();
            let mut order: Vec<(String, String)> = m
                .order_edges
                .iter()
                .map(|e| (self.hash(&e.from), self.hash(&e.to)))
                .collect();
            order.sort();
            order.dedup();
            forms.push(
                serde_json::json!({
                    "kind": m.kind,
                    "capability": m.capability,
                    "subtasks": subtasks,
                    "order": order,
                })
                .to_string(),
            );
        }
        forms.sort();
        forms.dedup();
        let mut form = canonical_task_form(task);
        form["methods"] = forms.into();
        let hash = blake3::hash(form.to_string().as_bytes())
            .to_hex()
            .to_string();
        self.memo.insert(id, hash.clone());
        hash
    }
}

/// Leaves a task may and must expand to in `compile`, over every possible
/// P6 selection. `compile` gives `done`/`dropped` tasks and tasks with no
/// accepted method no leaves, an accepted invoke the task itself, and an
/// accepted decomposition its subtasks' leaves. `may` unions the accepted
/// alternatives and `must` intersects them.
#[derive(Clone, Default)]
struct Leaves {
    may: BTreeSet<String>,
    must: BTreeSet<String>,
}

/// Accepted alternatives per (aliased) task: `None` for an invoke,
/// `Some(subtasks)` for a decomposition.
type Alternatives = BTreeMap<String, Vec<Option<Vec<String>>>>;

fn leaf_set(
    task: &str,
    skipped: &BTreeSet<String>,
    alternatives: &Alternatives,
    memo: &mut BTreeMap<String, Leaves>,
) -> Leaves {
    if let Some(leaves) = memo.get(task) {
        return leaves.clone();
    }
    memo.insert(task.to_string(), Leaves::default());
    let mut may = BTreeSet::new();
    let mut must: Option<BTreeSet<String>> = None;
    if !skipped.contains(task) {
        for alt in alternatives.get(task).into_iter().flatten() {
            let leaves = match alt {
                None => {
                    let own = BTreeSet::from([task.to_string()]);
                    Leaves {
                        may: own.clone(),
                        must: own,
                    }
                }
                Some(subtasks) => {
                    let mut leaves = Leaves::default();
                    for sub in subtasks {
                        let child = leaf_set(sub, skipped, alternatives, memo);
                        leaves.may.extend(child.may);
                        leaves.must.extend(child.must);
                    }
                    leaves
                }
            };
            may.extend(leaves.may);
            must = Some(match must {
                None => leaves.must,
                Some(m) => m.intersection(&leaves.must).cloned().collect(),
            });
        }
    }
    let leaves = Leaves {
        may,
        must: must.unwrap_or_default(),
    };
    memo.insert(task.to_string(), leaves.clone());
    leaves
}

/// Leaves that may and must finish before `child` counts as finished within
/// one method, following compile's `finish_leaves`: a child's own leaves,
/// or for a leafless child, whatever must finish before it.
fn finish_set(
    child: &str,
    edges: &[(String, String)],
    leaves: &BTreeMap<String, Leaves>,
    memo: &mut BTreeMap<String, Leaves>,
) -> Leaves {
    if let Some(done) = memo.get(child) {
        return done.clone();
    }
    memo.insert(child.to_string(), Leaves::default());
    let own = leaves.get(child).cloned().unwrap_or_default();
    let result = if own.must.is_empty() {
        let mut relayed = Leaves::default();
        for (from, _) in edges.iter().filter(|(_, to)| to == child) {
            let before = finish_set(from, edges, leaves, memo);
            relayed.may.extend(before.may);
            relayed.must.extend(before.must);
        }
        if own.may.is_empty() {
            relayed
        } else {
            Leaves {
                may: own.may.union(&relayed.may).cloned().collect(),
                must: BTreeSet::new(),
            }
        }
    } else {
        own
    };
    memo.insert(child.to_string(), result.clone());
    result
}

/// Leaves on a precedence cycle in compile's leaf graph once `alias` is
/// applied, over every possible P6 selection, plus every task's leaves.
///
/// Each accepted decomposition's order edge `from -> to` makes every leaf
/// that may finish before `from` precede every leaf `to` may expand to,
/// skipping target leaves that must already be in the `from` set, as
/// compile skips them. Every edge compile can draw is drawn, so a cycle
/// compile can hit is never missed.
fn leaves_on_order_cycle(
    doc: &PlanDoc,
    alias: &BTreeMap<String, String>,
) -> (BTreeSet<String>, BTreeMap<String, Leaves>) {
    let resolve = |id: &str| alias.get(id).cloned().unwrap_or_else(|| id.to_string());
    let skipped: BTreeSet<String> = doc
        .task
        .iter()
        .filter(|t| matches!(t.state, TaskState::Done | TaskState::Dropped))
        .map(|t| resolve(&t.id))
        .collect();
    let accepted: Vec<&Method> = doc
        .method
        .iter()
        .filter(|m| m.status == MethodStatus::Accepted)
        .collect();
    let mut alternatives: Alternatives = BTreeMap::new();
    for m in &accepted {
        let alt = match m.kind {
            MethodKind::Invoke => None,
            MethodKind::Decompose => Some(m.subtasks.iter().map(|s| resolve(s)).collect()),
        };
        alternatives.entry(resolve(&m.task)).or_default().push(alt);
    }
    let mut leaves: BTreeMap<String, Leaves> = BTreeMap::new();
    for task in &doc.task {
        leaf_set(&resolve(&task.id), &skipped, &alternatives, &mut leaves);
    }
    let mut graph: DiGraph<String, ()> = DiGraph::new();
    let mut nodes: BTreeMap<String, petgraph::graph::NodeIndex> = BTreeMap::new();
    for m in accepted.iter().filter(|m| m.kind == MethodKind::Decompose) {
        let edges: Vec<(String, String)> = m
            .order_edges
            .iter()
            .map(|e| (resolve(&e.from), resolve(&e.to)))
            .filter(|(from, to)| from != to)
            .collect();
        let mut finishes = BTreeMap::new();
        for (from, to) in &edges {
            let before = finish_set(from, &edges, &leaves, &mut finishes);
            for b in leaves.get(to).into_iter().flat_map(|l| &l.may) {
                if before.must.contains(b) {
                    continue;
                }
                for a in before.may.iter().filter(|a| *a != b) {
                    let x = *nodes
                        .entry(a.clone())
                        .or_insert_with(|| graph.add_node(a.clone()));
                    let y = *nodes
                        .entry(b.clone())
                        .or_insert_with(|| graph.add_node(b.clone()));
                    graph.add_edge(x, y, ());
                }
            }
        }
    }
    let mut cyclic = BTreeSet::new();
    for scc in petgraph::algo::tarjan_scc(&graph) {
        if scc.len() > 1 {
            cyclic.extend(scc.into_iter().map(|i| graph[i].clone()));
        }
    }
    (cyclic, leaves)
}

/// True when some live (non-rejected) decomposition's order edges, rewritten
/// through `alias` with self-edges dropped, form a cycle (E007 after merge).
fn method_order_cyclic(doc: &PlanDoc, alias: &BTreeMap<String, String>) -> bool {
    let resolve = |id: &String| alias.get(id).cloned().unwrap_or_else(|| id.clone());
    doc.method
        .iter()
        .filter(|m| m.kind == MethodKind::Decompose && m.status != MethodStatus::Rejected)
        .any(|m| {
            let mut graph: DiGraph<(), ()> = DiGraph::new();
            let mut nodes: BTreeMap<String, petgraph::graph::NodeIndex> = BTreeMap::new();
            for edge in &m.order_edges {
                let (from, to) = (resolve(&edge.from), resolve(&edge.to));
                if from == to {
                    continue;
                }
                let f = *nodes.entry(from).or_insert_with(|| graph.add_node(()));
                let t = *nodes.entry(to).or_insert_with(|| graph.add_node(()));
                graph.add_edge(f, t, ());
            }
            toposort(&graph, None).is_err()
        })
}

/// P4: share structurally identical tasks by content hash.
///
/// Computes a blake3 hash over each task's canonical form (title, state,
/// typed contract) and its committed method subtree (see [`ShareHasher`]).
/// Tasks with the same hash are merged: the smallest id is kept as the
/// canonical node, duplicates are removed, and all references
/// (`method.task`, subtasks, order edges, `objective.root_task`, `task:`
/// evidence subjects) are rewritten through the alias map. Merge groups
/// are tried one at a time in canonical-id order. A group is refused when
/// it would cycle a decomposition's own order edges, or when compile's leaf
/// graph over every selection (see [`leaves_on_order_cycle`]) has a cycle
/// through the canonical task's leaves, or any cycle at all if the
/// canonical task may be leafless and so relay order. Over-approximated
/// cycles only refuse more. The group is then
/// reported as `kept`. Subtask order is preserved (first occurrence
/// wins); only methods that actually reference a duplicate are touched.
/// Self-edges created by a merge are dropped. Methods that become identical
/// on the canonical task are deduplicated, keeping the best status
/// (accepted, then derived, then proposed). Every merge, refusal and
/// removal is reported with a trace event, never silent.
pub struct SharePass;

impl Pass for SharePass {
    fn name(&self) -> &'static str {
        "P4-share"
    }

    fn run(&self, ctx: &mut Ctx) {
        if !ctx.diagnostics.is_empty() {
            return;
        }
        let mut by_hash: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut hasher = ShareHasher::new(ctx.doc);
        for task in &ctx.doc.task {
            by_hash
                .entry(hasher.hash(&task.id))
                .or_default()
                .push(task.id.clone());
        }
        let mut merges: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for mut ids in by_hash.into_values() {
            if ids.len() < 2 {
                continue;
            }
            ids.sort();
            let canonical = ids.remove(0);
            merges.insert(canonical, ids);
        }
        let mut alias: BTreeMap<String, String> = BTreeMap::new();
        for (canonical, duplicates) in std::mem::take(&mut merges) {
            let mut trial = alias.clone();
            trial.extend(duplicates.iter().map(|d| (d.clone(), canonical.clone())));
            let (cyclic, leaves) = leaves_on_order_cycle(ctx.doc, &trial);
            let footprint = leaves.get(&canonical).cloned().unwrap_or_default();
            let touches_cycle = if footprint.must.is_empty() {
                !cyclic.is_empty()
            } else {
                !cyclic.is_disjoint(&footprint.may)
            };
            if !method_order_cyclic(ctx.doc, &trial) && !touches_cycle {
                alias = trial;
                merges.insert(canonical, duplicates);
                continue;
            }
            for dup in duplicates {
                ctx.trace.push(TraceEvent {
                    pass: self.name().to_string(),
                    subject: format!("task:{dup}"),
                    decision: "kept".to_string(),
                    reason: format!("merging into task:{canonical} would create an order cycle"),
                });
            }
        }
        let mut rewritten: BTreeSet<String> = BTreeSet::new();
        for method in &mut ctx.doc.method {
            if let Some(canonical) = alias.get(&method.task).cloned() {
                method.task = canonical;
                rewritten.insert(method.id.clone());
            }
            if method.subtasks.iter().any(|s| alias.contains_key(s)) {
                let mut seen = BTreeSet::new();
                let mut subtasks = Vec::with_capacity(method.subtasks.len());
                for sub in std::mem::take(&mut method.subtasks) {
                    let target = alias.get(&sub).cloned().unwrap_or(sub);
                    if seen.insert(target.clone()) {
                        subtasks.push(target);
                    }
                }
                method.subtasks = subtasks;
            }
            if method
                .order_edges
                .iter()
                .any(|e| alias.contains_key(&e.from) || alias.contains_key(&e.to))
            {
                for edge in &mut method.order_edges {
                    if let Some(canonical) = alias.get(&edge.from).cloned() {
                        edge.from = canonical;
                    }
                    if let Some(canonical) = alias.get(&edge.to).cloned() {
                        edge.to = canonical;
                    }
                }
                method.order_edges.retain(|e| e.from != e.to);
                method
                    .order_edges
                    .sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
                method.order_edges.dedup();
            }
        }
        if let Some(canonical) = alias.get(&ctx.doc.objective.root_task).cloned() {
            ctx.doc.objective.root_task = canonical;
        }
        ctx.doc.task.retain(|t| !alias.contains_key(&t.id));
        let removed = dedup_merged_methods(ctx, &rewritten);
        for evidence in &mut ctx.doc.evidence {
            let target = match split_subject(&evidence.subject) {
                Some(("task", id)) => alias.get(id).map(|c| format!("task:{c}")),
                Some(("method", id)) => removed.get(id).map(|k| format!("method:{k}")),
                _ => None,
            };
            if let Some(subject) = target {
                evidence.subject = subject;
            }
        }
        for (canonical, duplicates) in &merges {
            for dup in duplicates {
                ctx.trace.push(TraceEvent {
                    pass: self.name().to_string(),
                    subject: format!("task:{dup}"),
                    decision: "merged".to_string(),
                    reason: format!("structurally identical to task:{canonical}"),
                });
            }
        }
        ctx.doc.task.sort_by(|a, b| a.id.cmp(&b.id));
    }
}

/// Selection preference among identical methods: accepted, derived, proposed.
fn status_rank(status: MethodStatus) -> u8 {
    match status {
        MethodStatus::Accepted => 0,
        MethodStatus::Derived => 1,
        MethodStatus::Proposed => 2,
        MethodStatus::Rejected => 3,
    }
}

/// Remove live methods made identical by a merge. Only groups containing a
/// method in `rewritten` (whose task was merged away) are touched; the
/// survivor has the best status, then prefers the canonical task's own
/// method, then the smallest id. Rejected methods are left alone. Each
/// removal is traced. Returns removed method id mapped to the survivor.
fn dedup_merged_methods(ctx: &mut Ctx, rewritten: &BTreeSet<String>) -> BTreeMap<String, String> {
    let mut groups: BTreeMap<String, Vec<&Method>> = BTreeMap::new();
    for m in &ctx.doc.method {
        if m.status == MethodStatus::Rejected {
            continue;
        }
        let mut order: Vec<(&str, &str)> = m
            .order_edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect();
        order.sort_unstable();
        let key = serde_json::json!({
            "task": m.task,
            "kind": m.kind,
            "capability": m.capability,
            "subtasks": m.subtasks,
            "order": order,
        })
        .to_string();
        groups.entry(key).or_default().push(m);
    }
    let mut removed: BTreeMap<String, String> = BTreeMap::new();
    for mut group in groups.into_values() {
        if group.len() < 2 || !group.iter().any(|m| rewritten.contains(&m.id)) {
            continue;
        }
        group.sort_by_key(|m| {
            (
                status_rank(m.status),
                rewritten.contains(&m.id),
                m.id.clone(),
            )
        });
        let keep = group[0].id.clone();
        for m in &group[1..] {
            removed.insert(m.id.clone(), keep.clone());
        }
    }
    ctx.doc.method.retain(|m| !removed.contains_key(&m.id));
    for (id, keep) in &removed {
        ctx.trace.push(TraceEvent {
            pass: "P4-share".to_string(),
            subject: format!("method:{id}"),
            decision: "deduplicated".to_string(),
            reason: format!("identical to method:{keep} after merge"),
        });
    }
    removed
}

/// P5: estimate the effective capability profiles from evidence.
///
/// Folds live evidence into per-capability, per-basis profiles (see
/// [`crate::estimate`]) and reports one `measured` trace event per
/// capability, metric and basis with observations, plus `stale`,
/// `contradicts`, `staged` (evidence held with a proposal) and `pending`
/// (evidence for subjects not in the plan) events. P5 never edits the
/// document: declared estimates stay as written, and selection and roll-ups
/// read the effective numbers. It runs after P4, so evidence recorded
/// against a merged-away task already names the surviving task. Like P3 and
/// P4 it runs only over clean plans.
pub struct EstimatePass;

impl Pass for EstimatePass {
    fn name(&self) -> &'static str {
        "P5-estimate"
    }

    fn run(&self, ctx: &mut Ctx) {
        if !ctx.diagnostics.is_empty() {
            return;
        }
        let report = crate::estimate::estimate(ctx.doc);
        for profile in &report.profiles {
            let Some(measured) = &profile.measured else {
                continue;
            };
            let subject = format!("capability:{}", profile.capability);
            let unit = canonical_unit(&profile.metric).unwrap_or("");
            let basis = profile
                .basis
                .as_deref()
                .map_or(String::new(), |b| format!(" [basis {b}]"));
            ctx.trace.push(TraceEvent {
                pass: self.name().to_string(),
                subject: subject.clone(),
                decision: "measured".to_string(),
                reason: format!(
                    "median {:.3} {unit} over {} observations for `{}` (min {:.3}, max {:.3}){basis}",
                    measured.likely,
                    measured.observations,
                    profile.metric,
                    measured.low,
                    measured.high,
                ),
            });
            if measured.stale > 0 {
                ctx.trace.push(TraceEvent {
                    pass: self.name().to_string(),
                    subject: subject.clone(),
                    decision: "stale".to_string(),
                    reason: format!(
                        "{} of {} observations for `{}` are older than {} days before the newest",
                        measured.stale,
                        measured.observations,
                        profile.metric,
                        crate::estimate::STALE_AFTER_DAYS,
                    ),
                });
            }
            if profile.contradicts_declared {
                let declared = profile.declared.unwrap_or(crate::estimate::DeclaredTriple {
                    low: f64::NAN,
                    likely: f64::NAN,
                    high: f64::NAN,
                });
                ctx.trace.push(TraceEvent {
                    pass: self.name().to_string(),
                    subject,
                    decision: "contradicts".to_string(),
                    reason: format!(
                        "measured median {:.3} for `{}` falls outside declared [{:.3}, {:.3}]",
                        measured.likely, profile.metric, declared.low, declared.high,
                    ),
                });
            }
        }
        for staged in &report.staged {
            let basis = staged
                .basis
                .as_deref()
                .map_or(String::new(), |b| format!(" [basis {b}]"));
            let with = if staged.methods.is_empty() {
                format!(
                    "no invoke method proposes capability:{} yet",
                    staged.capability
                )
            } else {
                format!(
                    "staged with {}",
                    staged
                        .methods
                        .iter()
                        .map(|m| format!("method:{m}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            ctx.trace.push(TraceEvent {
                pass: self.name().to_string(),
                subject: format!("capability:{}", staged.capability),
                decision: "staged".to_string(),
                reason: format!(
                    "{} observations for `{}`{basis} {with}: not effective until an invoke method is accepted",
                    staged.observations, staged.metric,
                ),
            });
        }
        for pending in &report.pending {
            let basis = pending
                .basis
                .as_deref()
                .map_or(String::new(), |b| format!(" [basis {b}]"));
            ctx.trace.push(TraceEvent {
                pass: self.name().to_string(),
                subject: pending.subject.clone(),
                decision: "pending".to_string(),
                reason: format!(
                    "{} observations for `{}`{basis} name nothing yet: held until the subject is proposed",
                    pending.observations, pending.metric,
                ),
            });
        }
    }
}

/// Informational contract notes for proposed or derived invoke methods that
/// would fail E008 if accepted (G2): the capability does not cover the
/// task's contract. Ranges over the same pending set as the frontier
/// blocked-alternative entries, via the shared
/// [`crate::frontier::pending_invokes`] and
/// [`crate::impact::contract_note_for`], so anyone reading only `pc check`
/// sees the flag. Never an error: `ok()` stays true and the exit code is
/// unchanged.
fn contract_notes_for_check(doc: &PlanDoc) -> Vec<crate::impact::ContractNote> {
    let live = crate::frontier::live_tasks(doc, &doc.objective.root_task);
    crate::frontier::pending_invokes(doc, &live)
        .into_iter()
        .filter_map(|method| crate::impact::contract_note_for(doc, method))
        .collect()
}

/// Run P1-P5 over a parsed document and build the sorted report.
///
/// `file` is the display path used in diagnostics and `text` the raw source
/// used for line lookup. `evidence_log` anchors records loaded from a JSONL
/// log. Evidence unit case is canonicalized in place, P3 appends `derived`
/// proposals, P4 rewrites shared tasks in place and P5 reports the measured
/// profiles, so callers pass a mutable copy when the file must stay pristine.
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
    let passes: Vec<Box<dyn Pass>> = vec![
        Box::new(ResolvePass),
        Box::new(ValidatePass),
        Box::new(MatchPass),
        Box::new(SharePass),
        Box::new(EstimatePass),
    ];
    for pass in &passes {
        pass.run(&mut ctx);
    }
    let contract_notes = contract_notes_for_check(ctx.doc);
    for note in &contract_notes {
        ctx.trace.push(TraceEvent {
            pass: "check-notes".to_string(),
            subject: format!("method:{}", note.method),
            decision: "note".to_string(),
            reason: note.frontier_reason(),
        });
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
        contract_notes,
    }
}
