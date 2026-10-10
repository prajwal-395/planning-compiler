//! P5: estimate — fold evidence into capability profiles.
//!
//! Evidence never edits declared estimates. Instead, every capability and
//! metric gets an *effective* three-point estimate: the measured triple
//! (min/median/max over attributed observations) when evidence exists,
//! otherwise the declared triple. Selection (P6) and roll-ups run on the
//! effective numbers; the predicted (declared) numbers stay visible beside
//! them so every total says how much of it rests on guesses.
//!
//! Attribution is by subject: `capability:<id>` measures that capability
//! directly, `method:<id>` measures the capability its invoke method calls,
//! and `task:<id>` measures the one capability the task's accepted invoke
//! methods call. A task whose accepted invoke methods call several
//! capabilities is ambiguous (the record cannot say which alternative ran),
//! so its evidence counts as unattributed. P5 runs after P4, so evidence
//! recorded against a merged-away task already names the surviving task and
//! counts toward the same capability.
//!
//! Statistics are deterministic: values sort ascending, the median is the
//! middle value (the mean of the two middles for even counts), and the
//! spread is min/max. There is no wall-clock input: staleness is relative
//! to the newest observation for the same capability and metric (older than
//! [`STALE_AFTER_DAYS`] days), and contradiction means the measured median
//! falls outside the declared `[low, high]`. The v0 window is all records;
//! time-windowed estimators are deferred.

use std::collections::{BTreeMap, BTreeSet};

use pc_ir::{EstimateProvenance, KNOWN_METRICS, PlanDoc};
use serde::{Deserialize, Serialize};

/// Observations older than this many days before the newest observation for
/// the same capability and metric count as stale.
pub const STALE_AFTER_DAYS: i64 = 90;

/// One attributed observation: its timestamp (when it parses) and value.
type Observation = (Option<chrono::DateTime<chrono::FixedOffset>>, f64);

/// Observations grouped by `(capability, metric)`, in sorted order.
type ObservationGroups = BTreeMap<(String, String), Vec<Observation>>;

/// The declared triple for one capability and metric, if the plan states one.
///
/// When several estimates name the same metric, the first in document order
/// wins; P2 does not forbid duplicates, so the rule is documented here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeclaredTriple {
    /// Optimistic bound.
    pub low: f64,
    /// Most likely value.
    pub likely: f64,
    /// Pessimistic bound.
    pub high: f64,
}

/// One measured triple derived from attributed evidence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeasuredTriple {
    /// Minimum observed value.
    pub low: f64,
    /// Median observed value.
    pub likely: f64,
    /// Maximum observed value.
    pub high: f64,
    /// PERT expected value, `(low + 4*likely + high) / 6`.
    pub expected: f64,
    /// Observation count behind this triple.
    pub observations: u64,
    /// Observations older than [`STALE_AFTER_DAYS`] days before the newest.
    pub stale: u64,
}

/// The numbers selection and roll-ups actually use for one capability and
/// metric, plus where they came from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EffectiveEstimate {
    /// Optimistic bound.
    pub low: f64,
    /// Most likely value.
    pub likely: f64,
    /// Pessimistic bound.
    pub high: f64,
    /// PERT expected value, `(low + 4*likely + high) / 6`.
    pub expected: f64,
    /// Whether the numbers were guessed or derived from evidence.
    pub provenance: EstimateProvenance,
    /// Observation count: measured `n`, or the declared estimate's own
    /// count (zero when it cites none).
    pub observations: u64,
}

/// One capability and metric: declared guesses beside measured evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityProfile {
    /// Capability id.
    pub capability: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Declared triple, when the plan states one.
    pub declared: Option<DeclaredTriple>,
    /// Measured triple, when attributed evidence exists.
    pub measured: Option<MeasuredTriple>,
    /// The numbers to use: measured when present, declared otherwise.
    pub effective: Option<EffectiveEstimate>,
    /// True when the measured median falls outside declared `[low, high]`.
    pub contradicts_declared: bool,
}

/// `task:<id>` evidence P5 could not attribute: the task's accepted invoke
/// methods call no capability, or more than one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnattributedTaskEvidence {
    /// Task id named by the records.
    pub task: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Records ignored for this task and metric.
    pub observations: u64,
    /// Capabilities the task's accepted invoke methods call, sorted.
    pub capabilities: Vec<String>,
    /// The task's accepted invoke methods, sorted: the precise subjects to
    /// record against instead.
    pub methods: Vec<String>,
}

/// The deterministic result of running P5 over one plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateReport {
    /// One profile per capability and metric that has a declared estimate,
    /// attributed evidence, or both. Sorted by `(capability, metric)`.
    pub profiles: Vec<CapabilityProfile>,
    /// Records with a known metric that named no measurable capability:
    /// unknown subjects, decompose methods, tasks with no accepted invoke
    /// method, and tasks whose accepted invoke methods call more than one
    /// capability. (Unknown metrics and P1/P2 failures belong to
    /// diagnostics, not here.)
    pub unattributed: u64,
    /// The `task:<id>` part of `unattributed`, grouped by task and metric.
    /// Sorted by `(task, metric)`.
    pub unattributed_tasks: Vec<UnattributedTaskEvidence>,
}

impl EstimateReport {
    /// Effective numbers keyed by `(capability, metric)`, for P6 costs and
    /// roll-ups. Entries without either a declared estimate or evidence are
    /// absent; callers fall back to zero as before P5.
    #[must_use]
    pub fn effective_map(&self) -> BTreeMap<(String, String), EffectiveEstimate> {
        self.profiles
            .iter()
            .filter_map(|p| {
                p.effective
                    .map(|e| ((p.capability.clone(), p.metric.clone()), e))
            })
            .collect()
    }

    /// Capabilities with measured data for `(capability, metric)`.
    #[must_use]
    pub fn measured_set(&self) -> BTreeSet<(String, String)> {
        self.profiles
            .iter()
            .filter(|p| p.measured.is_some())
            .map(|p| (p.capability.clone(), p.metric.clone()))
            .collect()
    }
}

/// Median of an ascending slice: the middle value, or the mean of the two
/// middles for even counts. The slice must be non-empty.
fn median(sorted: &[f64]) -> f64 {
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

/// Fold a plan's evidence into per-capability profiles.
///
/// Pure and deterministic: groups iterate in sorted order, values sort
/// ascending, and timestamps parse from the records themselves (records
/// with an unparseable timestamp still count, but never count as stale).
#[must_use]
pub fn estimate(doc: &PlanDoc) -> EstimateReport {
    let caps: BTreeMap<&str, &pc_ir::Capability> =
        doc.capability.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut method_cap: BTreeMap<&str, &str> = BTreeMap::new();
    for m in &doc.method {
        if m.kind == pc_ir::MethodKind::Invoke {
            if let Some(cap) = m.capability.as_deref() {
                method_cap.insert(m.id.as_str(), cap);
            }
        }
    }
    let mut task_caps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut task_methods: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for m in &doc.method {
        if m.kind == pc_ir::MethodKind::Invoke
            && m.status == pc_ir::MethodStatus::Accepted
            && let Some(cap) = m.capability.as_deref()
        {
            task_caps.entry(m.task.as_str()).or_default().insert(cap);
            task_methods
                .entry(m.task.as_str())
                .or_default()
                .insert(m.id.as_str());
        }
    }
    let mut groups: ObservationGroups = BTreeMap::new();
    let mut unattributed: u64 = 0;
    let mut unattributed_task_counts: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    for ev in &doc.evidence {
        if !KNOWN_METRICS.contains(&ev.metric.as_str()) {
            continue;
        }
        let time = chrono::DateTime::parse_from_rfc3339(&ev.time).ok();
        let targets: Vec<String> = match ev.subject.split_once(':') {
            Some(("capability", id)) if caps.contains_key(id) => vec![id.to_string()],
            Some(("method", id)) => match method_cap.get(id) {
                Some(cap) => vec![(*cap).to_string()],
                None => vec![],
            },
            Some(("task", id)) => match task_caps.get(id) {
                Some(set) if set.len() == 1 => set.iter().map(ToString::to_string).collect(),
                _ => vec![],
            },
            _ => vec![],
        };
        if targets.is_empty() {
            unattributed += 1;
            if let Some(("task", id)) = ev.subject.split_once(':') {
                *unattributed_task_counts
                    .entry((id, ev.metric.as_str()))
                    .or_default() += 1;
            }
        }
        for cap in targets {
            groups
                .entry((cap, ev.metric.clone()))
                .or_default()
                .push((time, ev.value));
        }
    }
    let mut keys: BTreeSet<(String, String)> = groups.keys().cloned().collect();
    for c in &doc.capability {
        for e in &c.estimates {
            if KNOWN_METRICS.contains(&e.metric.as_str()) {
                keys.insert((c.id.clone(), e.metric.clone()));
            }
        }
    }
    let mut profiles = Vec::with_capacity(keys.len());
    for (capability, metric) in keys {
        let declared = doc
            .capability
            .iter()
            .find(|c| c.id == capability)
            .and_then(|c| c.estimates.iter().find(|e| e.metric == metric))
            .map(|e| DeclaredTriple {
                low: e.low,
                likely: e.likely,
                high: e.high,
            });
        let declared_observations = doc
            .capability
            .iter()
            .find(|c| c.id == capability)
            .and_then(|c| c.estimates.iter().find(|e| e.metric == metric))
            .and_then(|e| e.observations)
            .unwrap_or(0);
        let measured = groups
            .get(&(capability.clone(), metric.clone()))
            .map(|records| {
                let mut values: Vec<f64> = records.iter().map(|(_, v)| *v).collect();
                values.sort_by(f64::total_cmp);
                let likely = median(&values);
                let (low, high) = (values[0], values[values.len() - 1]);
                let newest = records.iter().filter_map(|(t, _)| *t).max();
                let stale = match newest {
                    Some(max) => {
                        let cutoff = max - chrono::Duration::days(STALE_AFTER_DAYS);
                        records
                            .iter()
                            .filter(|(t, _)| t.is_some_and(|t| t < cutoff))
                            .count() as u64
                    }
                    None => 0,
                };
                MeasuredTriple {
                    low,
                    likely,
                    high,
                    expected: (low + 4.0 * likely + high) / 6.0,
                    observations: values.len() as u64,
                    stale,
                }
            });
        let contradicts_declared = match (declared, measured) {
            (Some(d), Some(m)) => m.likely < d.low || m.likely > d.high,
            _ => false,
        };
        let effective = match (declared, measured) {
            (_, Some(m)) => Some(EffectiveEstimate {
                low: m.low,
                likely: m.likely,
                high: m.high,
                expected: m.expected,
                provenance: EstimateProvenance::Measured,
                observations: m.observations,
            }),
            (Some(d), None) => Some(EffectiveEstimate {
                low: d.low,
                likely: d.likely,
                high: d.high,
                expected: (d.low + 4.0 * d.likely + d.high) / 6.0,
                provenance: EstimateProvenance::Declared,
                observations: declared_observations,
            }),
            (None, None) => None,
        };
        profiles.push(CapabilityProfile {
            capability,
            metric,
            declared,
            measured,
            effective,
            contradicts_declared,
        });
    }
    let names = |set: Option<&BTreeSet<&str>>| -> Vec<String> {
        set.into_iter().flatten().map(ToString::to_string).collect()
    };
    let unattributed_tasks = unattributed_task_counts
        .into_iter()
        .map(|((task, metric), observations)| UnattributedTaskEvidence {
            task: task.to_string(),
            metric: metric.to_string(),
            observations,
            capabilities: names(task_caps.get(task)),
            methods: names(task_methods.get(task)),
        })
        .collect();
    EstimateReport {
        profiles,
        unattributed,
        unattributed_tasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(cap: &str, declared: (f64, f64, f64)) -> PlanDoc {
        PlanDoc {
            ir_version: 0,
            plan: pc_ir::PlanMeta {
                id: "p".into(),
                title: "t".into(),
                description: String::new(),
                external_inputs: vec![],
            },
            objective: pc_ir::Objective {
                root_task: "root".into(),
                weights: Default::default(),
                budget_usd: None,
                deadline: None,
                quality_floor: None,
                acceptance: vec![],
            },
            artifact: vec![],
            resource: vec![],
            task: vec![],
            method: vec![],
            capability: vec![pc_ir::Capability {
                id: cap.into(),
                kind: pc_ir::CapabilityKind::Tool,
                version: "1".into(),
                consumes: vec![],
                produces: vec![],
                estimates: vec![pc_ir::Estimate {
                    metric: "duration_minutes".into(),
                    low: declared.0,
                    likely: declared.1,
                    high: declared.2,
                    provenance: EstimateProvenance::Declared,
                    observations: None,
                    window: None,
                }],
                resources: vec![],
            }],
            evidence: vec![],
        }
    }

    fn ev(subject: &str, value: f64, time: &str) -> pc_ir::Evidence {
        pc_ir::Evidence {
            subject: subject.into(),
            metric: "duration_minutes".into(),
            value,
            unit: "minutes".into(),
            time: time.into(),
            source: "test".into(),
            artifact_hash: None,
        }
    }

    #[test]
    fn measured_triple_is_min_median_max() {
        let mut doc = doc_with("c@1", (1.0, 2.0, 3.0));
        for v in [30.0, 10.0, 20.0] {
            doc.evidence
                .push(ev("capability:c@1", v, "2026-10-01T00:00:00Z"));
        }
        let report = estimate(&doc);
        assert_eq!(report.profiles.len(), 1);
        let measured = report.profiles[0].measured.unwrap();
        assert_eq!(measured.low, 10.0);
        assert_eq!(measured.likely, 20.0);
        assert_eq!(measured.high, 30.0);
        assert_eq!(measured.observations, 3);
        let eff = report.profiles[0].effective.unwrap();
        assert_eq!(eff.provenance, EstimateProvenance::Measured);
        assert_eq!(eff.observations, 3);
    }

    #[test]
    fn even_counts_average_the_two_middles() {
        let mut doc = doc_with("c@1", (1.0, 2.0, 3.0));
        for v in [10.0, 20.0, 30.0, 40.0] {
            doc.evidence
                .push(ev("capability:c@1", v, "2026-10-01T00:00:00Z"));
        }
        let measured = estimate(&doc).profiles[0].measured.unwrap();
        assert_eq!(measured.likely, 25.0);
    }

    #[test]
    fn declared_survives_without_evidence() {
        let doc = doc_with("c@1", (1.0, 2.0, 3.0));
        let profile = &estimate(&doc).profiles[0];
        assert!(profile.measured.is_none());
        let eff = profile.effective.unwrap();
        assert_eq!(eff.provenance, EstimateProvenance::Declared);
        assert_eq!(eff.expected, (1.0 + 8.0 + 3.0) / 6.0);
    }
}
