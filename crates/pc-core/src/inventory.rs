//! Capability inventory report: every declared capability with its
//! version, provider, source and P5 evidence coverage.
//!
//! `pc capabilities` renders this over a checked plan so the inventory can
//! be evaluated, not just declared: capabilities with no measurement
//! evidence (live or staged) are flagged `unmeasured`. Rows sort by
//! capability id, so the report is deterministic.

use pc_ir::PlanDoc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One capability's inventory row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryRow {
    /// Capability id.
    pub id: String,
    /// Executor flavour, lowercase.
    pub kind: String,
    /// Capability version.
    pub version: String,
    /// Organization or project that provides the capability, if declared.
    pub provider: Option<String>,
    /// Source locator for the capability record, if declared.
    pub source: Option<String>,
    /// Sorted metrics with live measured evidence.
    pub measured_metrics: Vec<String>,
    /// Live observations folded into the capability's profiles.
    pub observations: u64,
    /// Observations staged with proposals invoking the capability.
    pub staged_observations: u64,
    /// True when the capability has no live or staged measurement evidence.
    pub unmeasured: bool,
}

/// The capability inventory of one plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryReport {
    /// One row per capability, sorted by id.
    pub capabilities: Vec<InventoryRow>,
    /// Number of rows flagged `unmeasured`.
    pub unmeasured: usize,
}

/// Build the inventory report from a checked plan's P5 evidence coverage.
#[must_use]
pub fn inventory(doc: &PlanDoc) -> InventoryReport {
    let estimated = crate::estimate(doc);
    let mut capabilities: Vec<InventoryRow> = doc
        .capability
        .iter()
        .map(|c| {
            let measured: Vec<_> = estimated
                .profiles
                .iter()
                .filter(|p| p.capability == c.id)
                .filter_map(|p| p.measured.map(|m| (p.metric.clone(), m.observations)))
                .collect();
            let observations = measured.iter().map(|(_, n)| n).sum();
            let staged_observations = estimated
                .staged
                .iter()
                .filter(|s| s.capability == c.id)
                .map(|s| s.observations)
                .sum();
            InventoryRow {
                id: c.id.clone(),
                kind: format!("{:?}", c.kind).to_lowercase(),
                version: c.version.clone(),
                provider: c.provider.clone(),
                source: c.source.clone(),
                measured_metrics: measured
                    .into_iter()
                    .map(|(metric, _)| metric)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                observations,
                staged_observations,
                unmeasured: observations == 0 && staged_observations == 0,
            }
        })
        .collect();
    capabilities.sort_by(|a, b| a.id.cmp(&b.id));
    let unmeasured = capabilities.iter().filter(|row| row.unmeasured).count();
    InventoryReport {
        capabilities,
        unmeasured,
    }
}

/// Render the inventory as text: a headline, then one line per capability.
#[must_use]
pub fn render_inventory_text(report: &InventoryReport, display_path: &str) -> String {
    let mut out = format!(
        "capabilities: {} declared, {} unmeasured ({display_path})\n",
        report.capabilities.len(),
        report.unmeasured,
    );
    for row in &report.capabilities {
        let provider = row.provider.as_deref().unwrap_or("-");
        let source = row.source.as_deref().unwrap_or("-");
        let coverage = if row.unmeasured {
            "unmeasured".to_string()
        } else {
            let metrics = if row.measured_metrics.is_empty() {
                "-".to_string()
            } else {
                row.measured_metrics.join(", ")
            };
            format!(
                "measured {metrics}; {} observations, {} staged",
                row.observations, row.staged_observations
            )
        };
        out.push_str(&format!(
            "capability:{} [{} v{}] provider {provider}; source {source}; {coverage}\n",
            row.id, row.kind, row.version,
        ));
    }
    out
}
