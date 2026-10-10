//! Deterministic compiler passes over Work IR v0.
//!
//! The pipeline runs [`passes::ResolvePass`] (P1) then
//! [`passes::ValidatePass`] (P2) through the [`passes::Pass`] trait and
//! returns a sorted [`passes::Report`]. Later steps add passes without
//! changing this orchestration.

pub mod diagnostic;
pub mod passes;

pub use diagnostic::{Diagnostic, render_human};
pub use passes::{Counts, Ctx, Pass, Report, ResolvePass, TraceEvent, ValidatePass, check};
