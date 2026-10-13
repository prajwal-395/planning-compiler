//! Work IR v0: the typed data model for plans.
//!
//! See `docs/ir.md` for semantics. This crate owns the serde (TOML and JSON)
//! shapes plus the published JSON Schema. It performs no validation beyond
//! structural deserialization; passes P1-P2 live in `pc-core`.
//!
//! Captain's scope adjustments applied to the report's section 3.2 model:
//! - no `human-role` capability kind and no `human_minutes` metric;
//! - contracts are typed consumes/produces matching only;
//! - state is text files, so every type here round-trips through TOML/JSON.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Expected value of [`PlanDoc::ir_version`]. Anything else fails P1 with E000.
pub const IR_VERSION: u32 = 0;

/// Metrics v0 understands. There is deliberately no human-time metric:
/// people are out of scope for v0 (see `docs/ir.md`).
/// `accuracy_distance_from_incumbent` is a relative figure (how far output
/// lands from the incumbent's), never an absolute risk or accuracy, so it
/// stays its own metric rather than folding into `failure_risk`.
pub const KNOWN_METRICS: &[&str] = &[
    "duration_minutes",
    "money_usd",
    "failure_risk",
    "accuracy_distance_from_incumbent",
];

/// Canonical unit for a known metric, the only unit P1 accepts for it.
#[must_use]
pub fn canonical_unit(metric: &str) -> Option<&'static str> {
    match metric {
        "duration_minutes" => Some("minutes"),
        "money_usd" => Some("usd"),
        "failure_risk" | "accuracy_distance_from_incumbent" => Some("ratio"),
        _ => None,
    }
}

/// A plan file: one TOML (`plan.toml`) or JSON (`plan.json`) document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanDoc {
    /// Schema version. Must equal [`IR_VERSION`].
    pub ir_version: u32,
    /// Plan identity plus plan-wide external inputs.
    pub plan: PlanMeta,
    /// Root task, hard constraints, weights, acceptance criteria.
    pub objective: Objective,
    /// Declared artifact types. Defaults to empty.
    #[serde(default)]
    pub artifact: Vec<ArtifactDecl>,
    /// Named capacities (reported against, never scheduled under, in v0).
    #[serde(default)]
    pub resource: Vec<ResourceDecl>,
    /// Needs: what must be true or produced.
    #[serde(default)]
    pub task: Vec<Task>,
    /// Ways to achieve tasks: decompose or invoke. Defaults to empty.
    #[serde(default)]
    pub method: Vec<Method>,
    /// Executors with typed contracts and three-point estimates.
    #[serde(default)]
    pub capability: Vec<Capability>,
    /// Recorded observations, folded into measured profiles by P5
    /// without editing declared estimates. Defaults to empty.
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

/// Plan identity and the fields an external input may satisfy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanMeta {
    /// Stable plan id, e.g. `"self-plan"`.
    pub id: String,
    /// Human title.
    pub title: String,
    /// Longer description, if any.
    #[serde(default)]
    pub description: String,
    /// Artifact fields the world provides, so dataflow need not produce them.
    #[serde(default)]
    pub external_inputs: Vec<ContractItem>,
    /// tasks-axi section names in file order, preserved so export restores
    /// sections even when they hold no items. Empty for plans that did not
    /// come from an import; every pass ignores it.
    #[serde(default)]
    pub tasks_axi_sections: Vec<String>,
}

/// Root task plus constraints, weights and acceptance criteria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    /// Id of the root task.
    pub root_task: String,
    /// Weight per metric for the step-3 additive objective. v0 only validates.
    #[serde(default)]
    pub weights: std::collections::BTreeMap<String, f64>,
    /// Hard budget in USD, if any.
    pub budget_usd: Option<f64>,
    /// Hard deadline (free-form date string in v0), if any.
    pub deadline: Option<String>,
    /// Hard quality floor in `[0, 1]`, if any.
    pub quality_floor: Option<f64>,
    /// Platforms the plan runs on. When non-empty, every non-rejected
    /// invoke's capability must list one of these in `platform_requirements`
    /// when it lists any (E016).
    #[serde(default)]
    pub platforms: Vec<String>,
    /// Licences the plan accepts. When non-empty, every non-rejected
    /// invoke's capability must list one of these in `licensing_constraints`
    /// when it lists any (E016).
    #[serde(default)]
    pub licenses: Vec<String>,
    /// What counts as achieving the objective.
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
}

/// One acceptance criterion on the objective.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    /// How the criterion is checked.
    pub kind: AcceptanceKind,
    /// Human-readable statement. Required for every kind.
    #[serde(default)]
    pub description: String,
    /// Command to run, for `kind = "command"`.
    pub command: Option<String>,
    /// Metric to compare, for `kind = "metric"`.
    pub metric: Option<String>,
    /// Comparison operator (`<=`, `>=`, `==`), for `kind = "metric"`.
    pub op: Option<String>,
    /// Threshold value, for `kind = "metric"`.
    pub value: Option<f64>,
}

/// How an acceptance criterion is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AcceptanceKind {
    /// A person signs off.
    Manual,
    /// A command exits zero.
    Command,
    /// A metric comparison holds.
    Metric,
}

/// A declared artifact type: a name plus its typed fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDecl {
    /// Artifact type name, e.g. `"transcript"`.
    pub name: String,
    /// Field names this artifact type carries.
    pub fields: Vec<String>,
}

/// A named capacity, e.g. `"gpu-1"`. v0 reports usage only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceDecl {
    /// Resource name.
    pub name: String,
    /// Available capacity in `unit`.
    pub capacity: f64,
    /// Capacity unit, free-form in v0.
    #[serde(default)]
    pub unit: String,
}

/// One typed contract entry: an artifact plus the fields used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractItem {
    /// Artifact type name. Must be declared under `artifact`.
    pub artifact: String,
    /// Fields of that artifact. Must be a subset of the declaration.
    pub fields: Vec<String>,
}

/// A need: what must be true or produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// Stable task id.
    pub id: String,
    /// Human title.
    pub title: String,
    /// Lifecycle state. The compiler reads `done`, never sets it.
    pub state: TaskState,
    /// Typed inputs this task needs from upstream or external inputs.
    #[serde(default)]
    pub consumes: Vec<ContractItem>,
    /// Typed outputs this task promises downstream.
    #[serde(default)]
    pub produces: Vec<ContractItem>,
    /// tasks-axi `backlog.md` passthrough: fields with no IR counterpart
    /// (section, repo, kind, priority, holds, date gates, body). No pass reads
    /// its values, so a backlog round-trips byte-identical instead of being
    /// refused or dropped. `None` for tasks that did not come from an import.
    #[serde(default)]
    pub tasks_axi: Option<TasksAxiMeta>,
    /// Original Beads issue JSONL record, preserved verbatim for lossless export.
    /// Its presence also keeps this imported issue distinct under P4 sharing.
    #[serde(default)]
    pub beads_jsonl: Option<String>,
}

/// Opaque tasks-axi `backlog.md` metadata carried through the IR.
///
/// The compiler never reads these fields; P4 only keys on their presence,
/// so every backlog row stays distinct work. The `pc-adapter-tasks-axi`
/// import/export pair maps them to and from the canonical backlog line
/// (`(repo: R)`, `(kind: K)`, `(priority: N)`, `(since D)`, the closed date,
/// `(hold: H)`, `(hold-kind: K)`, `(hold-until: D)`, section and body) so
/// unmappable fields survive untouched.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TasksAxiMeta {
    /// Backlog section the item lived in, e.g. `"Queued"`.
    pub section: String,
    /// `(repo: ...)` value, when present.
    pub repo: Option<String>,
    /// `(kind: ...)` value, when present.
    pub kind: Option<String>,
    /// `(priority: ...)` value, when present.
    pub priority: Option<i64>,
    /// `(since ...)` date (`YYYY-MM-DD`), when present.
    pub created: Option<String>,
    /// Closed date (`YYYY-MM-DD`), when present. Export labels it from the
    /// title: `(merged D)` for a PR link, `(reported D)` for a report path,
    /// `(done D)` otherwise.
    pub closed: Option<String>,
    /// `(hold: ...)` reason, when present.
    pub hold_reason: Option<String>,
    /// `(hold-kind: ...)` value, when present.
    pub hold_kind: Option<String>,
    /// `(hold-until: ...)` date gate, when present.
    pub hold_until: Option<String>,
    /// Indented body text with `\n` line breaks, without trailing newline.
    pub body: Option<String>,
}

/// Lifecycle states for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    /// Not yet broken down.
    Open,
    /// Has an accepted decomposition.
    Decomposed,
    /// Fully reduced to invocations (informational in v0).
    Resolved,
    /// Observed finished. Read from imports, never set by the compiler.
    Done,
    /// Explicitly abandoned.
    Dropped,
}

/// One way to achieve a task.
///
/// `kind` selects the shape: `decompose` uses `subtasks`/`order_edges`,
/// `invoke` uses `capability`. P2 reports E013 when the shape is mixed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Method {
    /// Stable method id, unique among methods.
    pub id: String,
    /// Task this method achieves.
    pub task: String,
    /// Decomposition versus capability invocation.
    pub kind: MethodKind,
    /// Child task ids, for `kind = "decompose"`.
    #[serde(default)]
    pub subtasks: Vec<String>,
    /// Ordering constraints between subtasks, for `kind = "decompose"`.
    #[serde(default)]
    pub order_edges: Vec<OrderEdge>,
    /// Capability id, for `kind = "invoke"`.
    pub capability: Option<String>,
    /// Lifecycle: `accepted`/`derived` methods are checked; only `accepted`
    /// methods are compiled.
    pub status: MethodStatus,
    /// Who proposed it: `human:<name>`, `agent:<name>`, `llm:<model>`,
    /// or `pass:<name>`.
    #[serde(default)]
    pub provenance: String,
    /// Assumptions `explain` must surface (step 2).
    #[serde(default)]
    pub assumptions: Vec<String>,
}

/// Decomposition versus invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    /// Achieve the task via subtasks.
    Decompose,
    /// Achieve the task via a capability.
    Invoke,
}

/// Lifecycle of a method. P2 checks `accepted` and `derived` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MethodStatus {
    /// Committed to the plan.
    Accepted,
    /// Awaiting `accept`/`reject` (step 7).
    Proposed,
    /// Generated by a compiler pass.
    Derived,
    /// Explicitly refused; ignored by every pass.
    Rejected,
}

/// An ordering constraint `from` -> `to` between subtasks of one method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderEdge {
    /// Predecessor subtask id.
    pub from: String,
    /// Successor subtask id.
    pub to: String,
}

/// An executor with a typed contract and three-point estimates.
///
/// `kind` deliberately excludes people: there is no `human-role` in v0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capability {
    /// Stable capability id, e.g. `"whisperx@3"`.
    pub id: String,
    /// Executor flavour. People are out of scope for v0.
    pub kind: CapabilityKind,
    /// Capability version string.
    #[serde(default)]
    pub version: String,
    /// Organization or project that provides this capability.
    #[serde(default)]
    pub provider: Option<String>,
    /// Source locator for the capability record, such as a package URL or repository.
    #[serde(default)]
    pub source: Option<String>,
    /// Platforms the capability runs on, as alternatives; at least one must be
    /// among `objective.platforms` when the plan declares any (E016).
    #[serde(default)]
    pub platform_requirements: Vec<String>,
    /// Licences the capability is available under, as alternatives; at least
    /// one must be among `objective.licenses` when the plan declares any (E016).
    #[serde(default)]
    pub licensing_constraints: Vec<String>,
    /// Typed inputs the capability needs from the invoking task.
    #[serde(default)]
    pub consumes: Vec<ContractItem>,
    /// Typed outputs the capability delivers.
    #[serde(default)]
    pub produces: Vec<ContractItem>,
    /// Three-point estimates per metric.
    #[serde(default)]
    pub estimates: Vec<Estimate>,
    /// Resources this capability draws on.
    #[serde(default)]
    pub resources: Vec<String>,
}

/// Executor flavours v0 models. No human executors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityKind {
    /// A coding agent or harness.
    Agent,
    /// A deterministic tool or model endpoint.
    Tool,
    /// A composed workflow.
    Workflow,
    /// An imported skill (contracts live in a sidecar).
    Skill,
    /// An external service.
    Service,
}

/// A three-point estimate with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Optimistic bound.
    pub low: f64,
    /// Most likely value.
    pub likely: f64,
    /// Pessimistic bound.
    pub high: f64,
    /// Where the numbers came from.
    pub provenance: EstimateProvenance,
    /// Observation count, for `measured` provenance.
    pub observations: Option<u64>,
    /// Evidence window label, e.g. `"30d"`.
    pub window: Option<String>,
}

/// Where an estimate's numbers came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EstimateProvenance {
    /// A guess written by a person or agent.
    Declared,
    /// Derived from evidence (P5 estimators).
    Measured,
}

/// One append-only observation about a capability, method or task.
///
/// P1/P2 check units, timestamps, values, the subject shape (see
/// [`check_evidence_subject`]) and the typed basis (see
/// [`check_evidence_basis`]; P2 also refuses one attributed capability and
/// metric under two bases); P5 folds it into capability profiles (see
/// `docs/passes.md`). A well-formed subject
/// that names nothing in the plan is not an error: P5 holds the record as
/// pending until the subject is proposed, so evidence can be staged
/// together with the hypothesis it supports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    /// `capability:<id>`, `method:<id>` or `task:<id>`.
    pub subject: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Observed value.
    pub value: f64,
    /// Observed unit; P1 accepts only the metric's canonical unit (case-insensitive) and canonicalizes its case.
    pub unit: String,
    /// RFC 3339 timestamp.
    pub time: String,
    /// Where the observation came from.
    pub source: String,
    /// Typed measurement basis: what the value is measured against.
    /// `None` means no basis was recorded (all pre-basis evidence).
    /// `Some` must pass [`check_evidence_basis`].
    #[serde(default)]
    pub basis: Option<String>,
    /// Optional content hash of the backing artifact.
    pub artifact_hash: Option<String>,
}

/// A candidate implementation considered for a planning decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionAlternative {
    /// Stable alternative identifier within the decision.
    pub id: String,
    /// Human-readable description.
    pub description: String,
}

/// A predicted or observed outcome summary, kept distinct by its enclosing event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeSummary {
    /// Human-readable expected or observed result.
    pub description: String,
    /// Metric values, keyed by metric name.
    #[serde(default)]
    pub metrics: std::collections::BTreeMap<String, f64>,
}

/// Common provenance carried by append-only decision and outcome events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventMetadata {
    /// Stable event identifier supplied by the recorder.
    pub id: String,
    /// RFC 3339 event timestamp.
    pub timestamp: String,
    /// Revision of the plan used for the event.
    pub plan_revision: String,
    /// Event producer, such as `pc-cli` or an agent identifier.
    pub source: String,
    /// Event schema version. PC-02 defines version 1.
    pub schema_version: u32,
    /// Referenced task identifiers.
    #[serde(default)]
    pub tasks: Vec<String>,
    /// Referenced capability identifiers.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Measurements captured automatically while the compiler inspects a plan.
/// Missing values mean the operation could not observe that measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompilerRunEvent {
    /// Shared event provenance.
    #[serde(flatten)]
    pub event: EventMetadata,
    /// Operation which produced the observation: `check`, `compile`, `frontier`, or `context`.
    pub operation: String,
    /// File name of the plan the run read; plans sharing a directory share
    /// one log.
    pub plan_file: String,
    /// P6 selection objective (`effort` or `makespan`), when compiled.
    pub selection_objective: Option<String>,
    /// P6 selection time budget in milliseconds, when compiled.
    pub selection_time_budget_ms: Option<u64>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// P1-P5 validation duration in milliseconds.
    pub validation_duration_ms: u64,
    /// Process resident memory in bytes, when available.
    pub memory_bytes: Option<u64>,
    /// Number of tasks in the checked plan.
    pub task_count: usize,
    /// Number of methods in the checked plan.
    pub method_count: usize,
    /// Number of objective constraints (budget, deadline, quality floor,
    /// acceptance criteria) plus order edges of non-rejected methods.
    pub constraint_count: usize,
    /// Number of accepted methods on tasks that offer more than one
    /// accepted method, so each counted method competes in selection.
    pub alternatives_considered: usize,
    /// Number of candidate methods rejected by contract validation.
    pub contract_rejections: usize,
    /// Fraction of plan capabilities with at least one evidence record;
    /// evidence on subjects not in the plan does not count.
    pub evidence_coverage: Option<f64>,
    /// Aggregate expected duration range width in minutes, when compiled.
    pub estimate_uncertainty_minutes: Option<f64>,
    /// Number of recorded decisions whose supporting tasks or capabilities
    /// gained evidence timestamped after the decision. Null when the
    /// decision log cannot be read.
    pub decisions_affected_by_evidence: Option<usize>,
    /// Tasks on the critical path, for compiled operations.
    pub critical_path_tasks: Option<Vec<String>>,
    /// Tasks that entered or left the critical path since the previous
    /// recorded run of the same plan file with the same selection objective
    /// and time budget. Null without a compiled operation or such a run.
    pub critical_path_changes: Option<usize>,
    /// Rendered context size in characters for `context` operations.
    pub context_size_chars: Option<usize>,
    /// Selection solver wall-clock duration in milliseconds.
    pub selection_duration_ms: Option<u64>,
    /// Selection optimality status, when compiled.
    pub optimality_status: Option<String>,
    /// Solver optimality gap in percent, when exposed by the solver.
    pub optimality_gap_percent: Option<f64>,
    /// P8 frontier-ranking wall-clock duration in milliseconds, for
    /// `frontier` operations only.
    pub frontier_duration_ms: Option<u64>,
}

/// A planning decision and its prediction, recorded independently of outcomes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvent {
    /// Shared event provenance.
    #[serde(flatten)]
    pub event: EventMetadata,
    /// Objective the decision addresses.
    pub objective: String,
    /// Revision of the project being planned.
    pub project_revision: String,
    /// Alternatives available at decision time.
    pub alternatives: Vec<DecisionAlternative>,
    /// Selected alternative identifier.
    pub selected_alternative: String,
    /// Alternative the planner intended before seeing the compiler
    /// recommendation, when it was recorded.
    #[serde(default)]
    pub initial_decision: Option<String>,
    /// Compiler recommendation, if one was made.
    pub compiler_recommendation: Option<String>,
    /// Explanation of who or what made the final decision.
    pub decision_provenance: String,
    /// IDs of supporting evidence records.
    #[serde(default)]
    pub supporting_evidence: Vec<String>,
    /// Constraints applied while making the decision.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Predicted result; this is not observed evidence.
    pub predicted_outcome: OutcomeSummary,
    /// Alternative used as the comparison baseline, if applicable.
    pub baseline_alternative: Option<String>,
}

/// An observed result linked to a prior decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeEvent {
    /// Shared event provenance.
    #[serde(flatten)]
    pub event: EventMetadata,
    /// ID of the decision this observation evaluates.
    pub decision_id: String,
    /// Measured result, separate from the decision's prediction.
    pub actual_outcome: OutcomeSummary,
    /// Confidence in the evaluation, when known, in `[0, 1]`.
    pub confidence: Option<f64>,
    /// How the compiler's recommendation affected the decision.
    pub evaluation_status: EvaluationStatus,
    /// Whether using the recommendation helped, assessed independently of
    /// `evaluation_status`; absent when not assessed.
    #[serde(default)]
    pub usefulness: Option<Usefulness>,
}

/// How a compiler recommendation affected a planning decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EvaluationStatus {
    /// The compiler supported the initial decision.
    Confirmed,
    /// The compiler led to a different decision.
    Changed,
    /// The compiler identified a missed issue.
    Corrected,
    /// The compiler identified uncertainty requiring research.
    Investigated,
    /// The recommendation was unsuitable.
    Rejected,
    /// There is insufficient evidence to assess value.
    Inconclusive,
}

impl EvaluationStatus {
    /// Report label for this classification.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "Confirmed",
            Self::Changed => "Changed",
            Self::Corrected => "Corrected",
            Self::Investigated => "Investigated",
            Self::Rejected => "Rejected",
            Self::Inconclusive => "Inconclusive",
        }
    }
}

/// Whether using a compiler recommendation helped, independent of its
/// [`EvaluationStatus`]: a confirmation can be useless and a change harmful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Usefulness {
    /// Using the recommendation improved the outcome or the decision.
    Useful,
    /// Using the recommendation made no material difference.
    Neutral,
    /// Using the recommendation made the outcome worse.
    Harmful,
}

impl Usefulness {
    /// Report label for this assessment, matching its serialized form.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Useful => "useful",
            Self::Neutral => "neutral",
            Self::Harmful => "harmful",
        }
    }
}

/// Check a typed evidence basis: `absolute` for ground-truth measurements,
/// or `distance-from:<target>` for values measured against another
/// capability (for example `distance-from:whisperx@3`). Anything else is
/// refused so a later agent cannot record an absolute-accuracy number where
/// a distance belongs without saying so.
///
/// Only the shape is checked: the target is not required to exist in the
/// plan, because evidence outlives retired incumbents (a fallback capability
/// removed after a substitution must not invalidate the measurements that
/// justified it).
pub fn check_evidence_basis(basis: &Option<String>) -> Result<(), String> {
    let Some(basis) = basis.as_deref() else {
        return Ok(());
    };
    if basis == "absolute" {
        return Ok(());
    }
    if let Some(target) = basis.strip_prefix("distance-from:")
        && !target.is_empty()
        && !target.chars().any(char::is_whitespace)
        && !target.contains(':')
    {
        return Ok(());
    }
    Err(format!(
        "evidence basis `{basis}` must be `absolute` or `distance-from:<capability>`"
    ))
}

/// Check an evidence subject's shape: `capability:<id>`, `method:<id>` or
/// `task:<id>` with a non-empty id. The id need not exist yet (P5 holds such
/// records as pending), but a subject outside these kinds can never be
/// proposed, so it is refused instead of pending forever.
pub fn check_evidence_subject(subject: &str) -> Result<(), String> {
    match subject.split_once(':') {
        Some(("capability" | "method" | "task", id)) if !id.is_empty() => Ok(()),
        _ => Err(format!(
            "evidence subject `{subject}` names nothing: use capability:<id>, method:<id> or task:<id>"
        )),
    }
}

/// Build the published JSON Schema for [`PlanDoc`].
#[must_use]
pub fn plan_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(PlanDoc)).unwrap_or(serde_json::Value::Null)
}

/// An agent patch for `pc propose`: new tasks, methods and capabilities.
///
/// JSON is canonical for agent patches; unknown fields are refused so a
/// typo'd patch never silently applies. Every key is optional and defaults
/// to empty, but a patch that adds nothing is refused by validation.
/// Proposed methods must carry `status = "proposed"`: accepting them is the
/// separate `pc accept` decision, and artifact declarations stay
/// human-maintained, so patches cannot add artifacts or resources.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Patch {
    /// Tasks to add to the plan.
    pub add_task: Vec<Task>,
    /// Methods to add, each with `status = "proposed"`.
    pub add_method: Vec<Method>,
    /// Capabilities to add.
    pub add_capability: Vec<Capability>,
}

/// Build the published JSON Schema for [`Patch`].
#[must_use]
pub fn patch_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Patch)).unwrap_or(serde_json::Value::Null)
}

/// Canonical JSON Schema for [`DecisionEvent`].
#[must_use]
pub fn decision_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(DecisionEvent)).unwrap_or(serde_json::Value::Null)
}

/// Canonical JSON Schema for [`OutcomeEvent`].
#[must_use]
pub fn outcome_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(OutcomeEvent)).unwrap_or(serde_json::Value::Null)
}

/// Canonical JSON Schema for [`CompilerRunEvent`].
#[must_use]
pub fn metrics_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(CompilerRunEvent)).unwrap_or(serde_json::Value::Null)
}

/// Parse a plan from TOML text.
pub fn from_toml(text: &str) -> Result<PlanDoc, toml::de::Error> {
    toml::from_str(text)
}

/// Parse a plan from JSON text.
pub fn from_json(text: &str) -> Result<PlanDoc, serde_json::Error> {
    serde_json::from_str(text)
}

/// Render a plan as pretty JSON (canonical machine interchange form).
pub fn to_json_pretty(doc: &PlanDoc) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(doc)
}

/// Render a plan as TOML (canonical hand-written form).
pub fn to_toml(doc: &PlanDoc) -> Result<String, toml::ser::Error> {
    toml::to_string(doc)
}

/// Errors from light pre-validation helpers in this crate.
#[derive(Debug, thiserror::Error)]
pub enum IrError {
    /// The document declares an unsupported schema version.
    #[error("unsupported ir_version {found}, expected {expected}")]
    UnsupportedVersion {
        /// Version found in the document.
        found: u32,
        /// Version this toolchain reads.
        expected: u32,
    },
}

/// Reject documents whose `ir_version` is not [`IR_VERSION`].
pub fn check_version(doc: &PlanDoc) -> Result<(), IrError> {
    if doc.ir_version != IR_VERSION {
        return Err(IrError::UnsupportedVersion {
            found: doc.ir_version,
            expected: IR_VERSION,
        });
    }
    Ok(())
}
