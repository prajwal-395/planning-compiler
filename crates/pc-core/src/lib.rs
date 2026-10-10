//! Deterministic compiler passes over Work IR v0.
//!
//! The pipeline runs [`passes::ResolvePass`] (P1), [`passes::ValidatePass`]
//! (P2), [`passes::MatchPass`] (P3), [`passes::SharePass`] (P4) and
//! [`passes::EstimatePass`] (P5) through the [`passes::Pass`] trait and
//! returns a sorted [`passes::Report`]. Selection (P6) and schedule analysis
//! (P7) run over the passed document in [`compile_with_budget`], using the
//! P5 effective estimates from [`estimate`]. Later steps add passes without
//! changing this orchestration.

pub mod compile;
pub mod diagnostic;
pub mod estimate;
pub mod passes;

pub use compile::{
    CompileError, CompileTrace, CompiledPlan, EstimateRange, MetricBasis, ProvenanceRow,
    TaskTiming, compile, compile_with_budget,
};
pub use diagnostic::{Diagnostic, render_human};
pub use estimate::{EstimateReport, UnattributedTaskEvidence, estimate};
pub use passes::{
    Counts, Ctx, EstimatePass, MatchPass, Pass, Report, ResolvePass, SharePass, TraceEvent,
    ValidatePass, check,
};
