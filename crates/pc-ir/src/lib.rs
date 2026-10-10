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
/// P1/P2 check that the subject exists and units are consistent; P5
/// folds it into capability profiles (see `docs/passes.md`).
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
    /// Optional content hash of the backing artifact.
    pub artifact_hash: Option<String>,
}

/// Build the published JSON Schema for [`PlanDoc`].
#[must_use]
pub fn plan_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(PlanDoc)).unwrap_or(serde_json::Value::Null)
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
