//! Deterministic compiler passes over Work IR v0.
//!
//! The pipeline runs [`passes::ResolvePass`] (P1), [`passes::ValidatePass`]
//! (P2), [`passes::MatchPass`] (P3), [`passes::SharePass`] (P4) and
//! [`passes::EstimatePass`] (P5) through the [`passes::Pass`] trait and
//! returns a sorted [`passes::Report`]. Selection (P6) and schedule analysis
//! (P7) run over the passed document in [`compile_with_budget`], using the
//! P5 effective estimates from [`estimate`]. Step 6 adds the [`frontier`]
//! ranking pass (P8) and the [`impact`] preview and comparison passes (P9)
//! as plain functions over checked plans, following the analysis-pass rule
//! in `CONTRIBUTING.md`.

pub mod compile;
pub mod diagnostic;
pub mod estimate;
pub mod frontier;
pub mod impact;
pub mod json;
pub mod passes;
pub mod propose;

pub use compile::{
    CompileError, CompileTrace, CompiledPlan, EstimateRange, MetricBasis, OpenWorkSummary,
    ProvenanceRow, TaskTiming, compile, compile_with_budget,
};
pub use diagnostic::{Diagnostic, render_human};
pub use estimate::{
    BasisConflict, EstimateReport, PendingEvidence, StagedEvidence, UnattributedTaskEvidence,
    basis_name, estimate,
};
pub use frontier::{DiscoveryItem, FrontierItem, FrontierReport, frontier, open_work};
pub use impact::{
    Change, ContractNote, DiffReport, ImpactError, ImpactReport, contract_note_for, delta_bases,
    diff, impact, status_str,
};
pub use passes::{
    Counts, Ctx, EstimatePass, MatchPass, Pass, Report, ResolvePass, SharePass, TraceEvent,
    ValidatePass, check,
};
pub use propose::{
    CapabilityView, ContextError, ContextReport, DEFAULT_CONTEXT_BUDGET_TOKENS, MATCH_ID_PREFIX,
    MethodView, ParentView, PatchError, TaskView, accept_method, apply_patch, context,
    estimate_tokens, fit_context, method_kind_str, reject_method, render_context_text,
    task_state_str, validate_patch,
};
