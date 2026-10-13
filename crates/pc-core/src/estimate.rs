//! P5: estimate — fold evidence into capability profiles.
//!
//! Evidence never edits declared estimates. Instead, every capability,
//! metric and typed basis gets an *effective* three-point estimate: the
//! measured triple (min/median/max over attributed observations) when live
//! evidence exists, otherwise the declared triple. Selection (P6) and
//! roll-ups run on the effective numbers; the predicted (declared) numbers
//! stay visible beside them so every total says how much of it rests on
//! guesses.
//!
//! Attribution is by subject: `capability:<id>` measures that capability
//! directly, `method:<id>` measures the capability its invoke method calls,
//! and `task:<id>` measures the one capability the task's accepted invoke
//! methods call. A task whose accepted invoke methods call several
//! capabilities is ambiguous (the record cannot say which alternative ran),
//! so its evidence counts as unattributed. A well-formed subject that names
//! nothing in the plan is not an error: the record counts as pending until the subject
//! is proposed, so evidence can be staged together with the hypothesis it
//! supports. P5 runs after P4, so evidence recorded against a merged-away
//! task already names the surviving task and counts toward the same
//! capability.
//!
//! Evidence staged with a proposal never moves the accepted plan's numbers:
//! a record is live only when its capability is invoked by at least one
//! accepted method, whichever subject (`capability:`, `method:` or `task:`)
//! named it. Records for a capability only proposed, derived or rejected
//! methods invoke are held as staged evidence, labelled with the proposals
//! currently invoking that capability: accepting any method that invokes it
//! makes them live with no record movement, rejecting keeps them staged.
//! Pending records become staged or live automatically when their subject
//! arrives and is decided.
//!
//! One attributed family (capability and metric after placement, live or
//! staged, or the subject itself while pending) carries one basis. A mix,
//! including a basis-less record beside a typed one, is reported in
//! [`EstimateReport::basis_conflicts`] and refused by P2 and `pc record`,
//! so observations measured against different things are never pooled,
//! never resolved by majority, and never drop a family from selection.
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

/// One attributed observation: its timestamp (when it parses), value,
/// source and typed basis. Sources travel into profiles so provenance can
/// name the measurement basis; the basis travels with them so observations
/// with different bases are never pooled.
type Observation = (
    Option<chrono::DateTime<chrono::FixedOffset>>,
    f64,
    String,
    Option<String>,
);

/// Live observations grouped by `(capability, metric, basis)`, in sorted
/// order. The basis key is the recorded basis, or empty for evidence with
/// no basis field.
type ObservationGroups = BTreeMap<(String, String, String), Vec<Observation>>;

/// One attributed family's first basis, the index of its first record with
/// a different basis, and every basis with the subjects that recorded it.
type FamilyBases = (
    Option<String>,
    Option<usize>,
    BTreeMap<String, BTreeSet<String>>,
);

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

/// One capability, metric and typed basis: declared guesses beside measured
/// evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityProfile {
    /// Capability id.
    pub capability: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Typed measurement basis behind `measured`: `None` for evidence with
    /// no basis field (all pre-basis evidence).
    pub basis: Option<String>,
    /// Declared triple, when the plan states one.
    pub declared: Option<DeclaredTriple>,
    /// Measured triple, when live attributed evidence exists for this basis.
    pub measured: Option<MeasuredTriple>,
    /// The numbers to use: measured when present, declared otherwise.
    pub effective: Option<EffectiveEstimate>,
    /// True when the measured median falls outside declared `[low, high]`.
    pub contradicts_declared: bool,
    /// Sorted unique evidence sources behind `measured` (empty when
    /// declared-only).
    pub sources: Vec<String>,
}

/// Evidence staged with a proposal: attributed to a capability no accepted
/// method invokes yet, so held out of the effective numbers until a method
/// is accepted. Accepting a proposed or derived method for the capability
/// makes these records live with no record movement; rejecting it keeps
/// them staged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedEvidence {
    /// Capability id the records measure.
    pub capability: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Typed measurement basis, when recorded.
    pub basis: Option<String>,
    /// Records held for this capability, metric and basis.
    pub observations: u64,
    /// Sorted proposed or derived invoke methods referencing the
    /// capability: the live proposals this evidence is staged with. Empty
    /// when no invoke method proposes the capability yet.
    pub methods: Vec<String>,
}

impl StagedEvidence {
    /// One text line naming the proposal these records are staged for and
    /// saying they are not counted until it is accepted.
    #[must_use]
    pub fn line(&self) -> String {
        let basis = self
            .basis
            .as_deref()
            .map_or(String::new(), |b| format!(" [basis {b}]"));
        let held = format!(
            "{} observation{}",
            self.observations,
            if self.observations == 1 { "" } else { "s" }
        );
        let proposal = if self.methods.is_empty() {
            format!(
                "staged with no proposal for capability:{} yet",
                self.capability
            )
        } else {
            format!(
                "staged for proposal {}",
                self.methods
                    .iter()
                    .map(|m| format!("method:{m}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        format!(
            "staged capability:{} {}{basis}: {held} {proposal}, not counted until accepted",
            self.capability, self.metric,
        )
    }
}

/// Evidence whose subject names nothing in the plan: accepted, never an
/// error, and held until the subject is proposed. Proposing the subject
/// stages these records with the proposal; accepting the proposal makes
/// them live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEvidence {
    /// Subject as recorded, e.g. `capability:voz-hybrid@1`.
    pub subject: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Typed measurement basis, when recorded.
    pub basis: Option<String>,
    /// Records held for this subject, metric and basis.
    pub observations: u64,
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

/// One attributed family whose records carry more than one basis: P2 and
/// `pc record` refuse it (E014), naming every basis and the subjects that
/// recorded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisConflict {
    /// `capability:<id>` for live or staged records, or the subject itself
    /// for pending records.
    pub family: String,
    /// One of [`KNOWN_METRICS`].
    pub metric: String,
    /// Each basis (see [`basis_name`]) with the sorted subjects that
    /// recorded it, sorted by basis.
    pub bases: Vec<(String, Vec<String>)>,
    /// Index into the plan's evidence of the first record whose basis
    /// differs from the family's first record.
    pub record: usize,
}

impl BasisConflict {
    /// The E014 message: every basis with its contributing subjects.
    #[must_use]
    pub fn message(&self) -> String {
        let bases = self
            .bases
            .iter()
            .map(|(basis, subjects)| format!("`{basis}` ({})", subjects.join(", ")))
            .collect::<Vec<_>>()
            .join(" and ");
        format!(
            "evidence for `{}` metric `{}` mixes bases {bases}: one capability and metric carries one basis; record the other basis under a distinct metric",
            self.family, self.metric,
        )
    }
}

/// The deterministic result of running P5 over one plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimateReport {
    /// One profile per capability, metric and basis that has a declared
    /// estimate, live attributed evidence, or both. Sorted by
    /// `(capability, metric, basis)` with the missing basis first.
    pub profiles: Vec<CapabilityProfile>,
    /// Records with a known metric that named no live measurable
    /// capability: decompose methods and tasks with no accepted invoke
    /// method, or tasks whose accepted invoke methods call more than one
    /// capability. (Unknown metrics and P1/P2 failures belong to
    /// diagnostics, not here; unknown subjects are [`Self::pending`],
    /// proposal-staged records are [`Self::staged`].)
    pub unattributed: u64,
    /// The `task:<id>` part of `unattributed`, grouped by task and metric.
    /// Sorted by `(task, metric)`.
    pub unattributed_tasks: Vec<UnattributedTaskEvidence>,
    /// Evidence staged with proposals, grouped by capability, metric and
    /// basis. Sorted by `(capability, metric, basis)`.
    pub staged: Vec<StagedEvidence>,
    /// Evidence for subjects not in the plan, grouped by subject, metric
    /// and basis. Sorted by `(subject, metric, basis)`.
    pub pending: Vec<PendingEvidence>,
    /// Attributed families mixing bases. Sorted by `(family, metric)`.
    pub basis_conflicts: Vec<BasisConflict>,
}

/// Display name for a basis group: the recorded basis, or `"none"` when
/// the evidence carries no basis field.
#[must_use]
pub fn basis_name(basis: &Option<String>) -> String {
    basis.clone().unwrap_or_else(|| "none".to_string())
}

impl EstimateReport {
    /// Effective numbers keyed by `(capability, metric)`, for P6 costs and
    /// roll-ups. Entries without either a declared estimate or live
    /// evidence are absent; callers fall back to zero as before P5.
    /// Staged and pending records never enter this map.
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

    /// Capabilities with live measured data for `(capability, metric)`.
    /// Staged and pending records never enter this set.
    #[must_use]
    pub fn measured_set(&self) -> BTreeSet<(String, String)> {
        self.profiles
            .iter()
            .filter(|p| p.measured.is_some())
            .map(|p| (p.capability.clone(), p.metric.clone()))
            .collect()
    }

    /// Sorted unique typed bases with live measured data for `(capability,
    /// metric)`, with the missing basis reported as `"none"`. Empty when
    /// nothing is measured.
    #[must_use]
    pub fn measured_bases(&self, capability: &str, metric: &str) -> Vec<String> {
        self.profiles
            .iter()
            .filter(|p| p.capability == capability && p.metric == metric && p.measured.is_some())
            .map(|p| basis_name(&p.basis))
            .collect::<BTreeSet<_>>()
            .into_iter()
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
/// Only live records (measuring a capability at least one accepted method
/// invokes) fold into profiles; staged and pending records are reported
/// separately and never move the effective numbers.
#[must_use]
pub fn estimate(doc: &PlanDoc) -> EstimateReport {
    let caps: BTreeMap<&str, &pc_ir::Capability> =
        doc.capability.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut method_cap: BTreeMap<&str, &str> = BTreeMap::new();
    for m in &doc.method {
        if m.kind == pc_ir::MethodKind::Invoke
            && let Some(cap) = m.capability.as_deref()
        {
            method_cap.insert(m.id.as_str(), cap);
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
    // Capabilities invoked by at least one accepted method: the only ones
    // whose evidence is live. Everything attributed elsewhere is staged
    // with its proposal.
    let live_caps: BTreeSet<&str> = task_caps.values().flatten().copied().collect();
    // Live proposals per capability: the proposed or derived invoke methods
    // staged evidence is attributed to.
    let mut proposal_methods: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for m in &doc.method {
        if m.kind == pc_ir::MethodKind::Invoke
            && matches!(
                m.status,
                pc_ir::MethodStatus::Proposed | pc_ir::MethodStatus::Derived
            )
            && let Some(cap) = m.capability.as_deref()
        {
            proposal_methods
                .entry(cap)
                .or_default()
                .insert(m.id.as_str());
        }
    }
    let mut groups: ObservationGroups = BTreeMap::new();
    let mut staged_groups: ObservationGroups = BTreeMap::new();
    let mut unattributed: u64 = 0;
    let mut unattributed_task_counts: BTreeMap<(&str, &str), u64> = BTreeMap::new();
    let mut pending_counts: BTreeMap<(&str, &str, Option<&str>), u64> = BTreeMap::new();
    let mut families: BTreeMap<(String, String), FamilyBases> = BTreeMap::new();
    let mut note_family = |family: String, index: usize, ev: &pc_ir::Evidence| {
        let entry = families
            .entry((family, ev.metric.clone()))
            .or_insert_with(|| (ev.basis.clone(), None, BTreeMap::new()));
        if entry.0 != ev.basis && entry.1.is_none() {
            entry.1 = Some(index);
        }
        entry
            .2
            .entry(basis_name(&ev.basis))
            .or_default()
            .insert(ev.subject.clone());
    };
    for (index, ev) in doc.evidence.iter().enumerate() {
        if !KNOWN_METRICS.contains(&ev.metric.as_str()) {
            continue;
        }
        let time = chrono::DateTime::parse_from_rfc3339(&ev.time).ok();
        let record = (time, ev.value, ev.source.clone(), ev.basis.clone());
        // Live targets measure the accepted plan; staged targets stay
        // attributed to their proposal; unknown subjects stay pending.
        enum Placing {
            Live(Vec<String>),
            Staged(Vec<String>),
            Pending,
            Unattributed,
        }
        let placing = match ev.subject.split_once(':') {
            Some(("capability", id)) if caps.contains_key(id) => {
                if live_caps.contains(id) {
                    Placing::Live(vec![id.to_string()])
                } else {
                    Placing::Staged(vec![id.to_string()])
                }
            }
            Some(("method", id)) => match method_cap.get(id) {
                Some(cap) if live_caps.contains(cap) => Placing::Live(vec![(*cap).to_string()]),
                Some(cap) => Placing::Staged(vec![(*cap).to_string()]),
                None if doc.method.iter().any(|m| m.id == id) => Placing::Unattributed,
                None => Placing::Pending,
            },
            Some(("task", id)) => match task_caps.get(id) {
                Some(set) if set.len() == 1 => {
                    Placing::Live(set.iter().map(ToString::to_string).collect())
                }
                Some(_) => Placing::Unattributed,
                None if doc.task.iter().any(|t| t.id == id) => Placing::Unattributed,
                None => Placing::Pending,
            },
            Some(("capability", _)) => Placing::Pending,
            _ => Placing::Unattributed,
        };
        let basis_key = ev.basis.clone().unwrap_or_default();
        match placing {
            Placing::Live(targets) => {
                for cap in targets {
                    note_family(format!("capability:{cap}"), index, ev);
                    groups
                        .entry((cap, ev.metric.clone(), basis_key.clone()))
                        .or_default()
                        .push(record.clone());
                }
            }
            Placing::Staged(targets) => {
                for cap in targets {
                    note_family(format!("capability:{cap}"), index, ev);
                    staged_groups
                        .entry((cap, ev.metric.clone(), basis_key.clone()))
                        .or_default()
                        .push(record.clone());
                }
            }
            Placing::Pending => {
                note_family(ev.subject.clone(), index, ev);
                *pending_counts
                    .entry((ev.subject.as_str(), ev.metric.as_str(), ev.basis.as_deref()))
                    .or_default() += 1;
            }
            Placing::Unattributed => {
                unattributed += 1;
                if let Some(("task", id)) = ev.subject.split_once(':') {
                    *unattributed_task_counts
                        .entry((id, ev.metric.as_str()))
                        .or_default() += 1;
                }
            }
        }
    }
    let mut keys: BTreeSet<(String, String, String)> = groups.keys().cloned().collect();
    for c in &doc.capability {
        for e in &c.estimates {
            if KNOWN_METRICS.contains(&e.metric.as_str()) {
                keys.insert((c.id.clone(), e.metric.clone(), String::new()));
            }
        }
    }
    let mut profiles = Vec::with_capacity(keys.len());
    for (capability, metric, basis_key) in keys {
        let basis = if basis_key.is_empty() {
            None
        } else {
            Some(basis_key.clone())
        };
        let measured_bases = groups
            .keys()
            .filter(|(cap, met, _)| cap == &capability && met == &metric)
            .count();
        // A declared-only row beside measured bases repeats the guesses the
        // measured rows already carry: skip it so each family shows one row
        // per measured basis. (A declared-only family keeps its single row,
        // exactly as before.)
        let has_measured_group =
            groups.contains_key(&(capability.clone(), metric.clone(), basis_key.clone()));
        if !has_measured_group && measured_bases > 0 {
            continue;
        }
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
            .get(&(capability.clone(), metric.clone(), basis_key.clone()))
            .map(|records| {
                let mut values: Vec<f64> = records.iter().map(|(_, v, _, _)| *v).collect();
                values.sort_by(f64::total_cmp);
                let likely = median(&values);
                let (low, high) = (values[0], values[values.len() - 1]);
                let newest = records.iter().filter_map(|(t, _, _, _)| *t).max();
                let stale = match newest {
                    Some(max) => {
                        let cutoff = max - chrono::Duration::days(STALE_AFTER_DAYS);
                        records
                            .iter()
                            .filter(|(t, _, _, _)| t.is_some_and(|t| t < cutoff))
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
        let sources: Vec<String> = groups
            .get(&(capability.clone(), metric.clone(), basis_key.clone()))
            .map(|records| {
                records
                    .iter()
                    .map(|(_, _, source, _)| source.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        profiles.push(CapabilityProfile {
            capability,
            metric,
            basis,
            declared,
            measured,
            effective,
            contradicts_declared,
            sources,
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
    let staged = staged_groups
        .into_iter()
        .map(
            |((capability, metric, basis_key), records): (
                (String, String, String),
                Vec<Observation>,
            )| {
                StagedEvidence {
                    capability: capability.clone(),
                    metric,
                    basis: if basis_key.is_empty() {
                        None
                    } else {
                        Some(basis_key)
                    },
                    observations: records.len() as u64,
                    methods: names(proposal_methods.get(capability.as_str())),
                }
            },
        )
        .collect();
    let pending = pending_counts
        .into_iter()
        .map(|((subject, metric, basis), observations)| PendingEvidence {
            subject: subject.to_string(),
            metric: metric.to_string(),
            basis: basis.map(str::to_string),
            observations,
        })
        .collect();
    let basis_conflicts = families
        .into_iter()
        .filter_map(|((family, metric), (_, conflict, bases))| {
            conflict.map(|record| BasisConflict {
                family,
                metric,
                bases: bases
                    .into_iter()
                    .map(|(basis, subjects)| (basis, subjects.into_iter().collect()))
                    .collect(),
                record,
            })
        })
        .collect();
    EstimateReport {
        profiles,
        unattributed,
        unattributed_tasks,
        staged,
        pending,
        basis_conflicts,
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
                tasks_axi_sections: Vec::new(),
            },
            objective: pc_ir::Objective {
                root_task: "root".into(),
                weights: Default::default(),
                budget_usd: None,
                deadline: None,
                quality_floor: None,
                platforms: Vec::new(),
                licenses: Vec::new(),
                acceptance: vec![],
            },
            artifact: vec![],
            resource: vec![],
            task: vec![pc_ir::Task {
                id: "root".into(),
                title: "t".into(),
                state: pc_ir::TaskState::Resolved,
                tasks_axi: None,
                beads_jsonl: None,
                consumes: vec![],
                produces: vec![],
            }],
            // The capability is live in the accepted plan, so capability
            // evidence folds into its profile instead of staging.
            method: vec![pc_ir::Method {
                id: "m".into(),
                task: "root".into(),
                kind: pc_ir::MethodKind::Invoke,
                subtasks: vec![],
                order_edges: vec![],
                capability: Some(cap.into()),
                status: pc_ir::MethodStatus::Accepted,
                provenance: "test".into(),
                assumptions: vec![],
            }],
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
                provider: None,
                source: None,
                platform_requirements: vec![],
                licensing_constraints: vec![],
                resources: vec![],
            }],
            evidence: vec![],
            investigation: vec![],
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
            basis: None,
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
