//! Structured capability discovery (PC-13): candidate adapters outside the
//! deterministic core.
//!
//! The compiler core stays model-free: agents (human or software) find
//! possibilities, the compiler validates them. This crate is the middle
//! layer: deterministic adapters scan external sources and turn what they
//! find into [`pc_ir::Patch`] proposals for the existing step-7 loop (`pc
//! propose` stages them, `pc accept`/`pc reject` decide them). Nothing here
//! edits a plan, accepts a method, calls a model or fabricates measurements.
//!
//! ## Guarantees
//!
//! - Every emitted method has `status = "proposed"` with an
//!   `agent:pc-discover-*` provenance: a candidate never becomes accepted
//!   because an adapter found it.
//! - Every emitted capability carries `source` (where it was found) and
//!   every emitted method carries an assumption naming that source, so the
//!   proposal stays traceable through `pc explain method:`.
//! - Capabilities already modeled in the plan are reused, never
//!   duplicated: the patch adds only a method for the target task, and a
//!   candidate is skipped when that task already invokes the capability.
//! - Candidates that fail the plan's platform/licence constraints (E016)
//!   are excluded with a reason: the merged plan would fail `check`, so
//!   `pc propose` would refuse them.
//! - Candidates that fail typed contract matching are still proposed: the
//!   frontier ranks them as blocked alternatives and `pc check` reports the
//!   same contract note, so the incompatibility stays visible instead of
//!   silently dropped.
//! - Adapters emit no estimates and no evidence: dependency versions and
//!   file names are existence facts, not performance measurements.
//!   Measurement evidence arrives through the normal `pc record` path (for
//!   example benchmark ingestion, a follow-up source) and P5 stages it with
//!   the proposal until the method is accepted.
//!
//! ## Sources
//!
//! Deterministic adapters implemented here: dependency manifests
//! ([`discover_from_manifest`]) and local repositories
//! ([`discover_from_repo`]). Agent-driven sources (GitHub and documentation
//! research) and benchmark ingestion plug in through the same proposal
//! interface and are follow-up work; see `docs/discovery.md`.

use pc_ir::{
    Capability, CapabilityKind, ContractItem, Method, MethodKind, MethodStatus, Patch, PlanDoc,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use thiserror::Error;

/// Provenance stamped on methods proposed from dependency manifests.
pub const MANIFEST_PROVENANCE: &str = "agent:pc-discover-manifest";
/// Provenance stamped on methods proposed from local repositories.
pub const REPO_PROVENANCE: &str = "agent:pc-discover-repo";
/// Method-id prefix for discovery proposals. Agent patches must not claim
/// the P3 `match-` namespace, so discovery uses its own prefix.
pub const DISCOVER_ID_PREFIX: &str = "discover-";

/// Which deterministic adapter found a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A dependency manifest (`[dependencies]` entry).
    Manifest,
    /// A local repository (top-level file).
    LocalRepo,
}

impl SourceKind {
    /// Provenance stamped on methods proposed from this source.
    #[must_use]
    pub fn provenance(self) -> &'static str {
        match self {
            Self::Manifest => MANIFEST_PROVENANCE,
            Self::LocalRepo => REPO_PROVENANCE,
        }
    }
}

/// Where a candidate was found: the adapter plus a stable locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Adapter that found the candidate.
    pub kind: SourceKind,
    /// Stable locator, e.g. `manifest:Cargo.toml#voz` or `repo:tools#denoise`.
    pub locator: String,
    /// Human-readable finding, recorded as the proposed method's assumption.
    pub detail: String,
}

/// One discovered implementation alternative, not yet proposed.
///
/// The capability carries the adapter's `source` but an empty contract and
/// no estimates: existence is a fact, the typed contract is an operator- or
/// agent-asserted hypothesis attached later with [`with_contract`], and
/// performance numbers arrive as evidence through `pc record`, never as
/// fabricated estimates.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveryCandidate {
    /// Candidate capability (contract attached separately).
    pub capability: Capability,
    /// Where the candidate was found.
    pub source: Source,
}

/// Attach the asserted typed contract to a candidate.
///
/// Adapters cannot know typed contracts: a manifest entry or a file name
/// says an implementation exists, not what artifacts it consumes or
/// produces. The caller (operator or research agent) asserts the contract
/// as a hypothesis; [`check_compatibility`] evaluates it and the
/// deterministic compiler validates the proposal. Returns the candidate
/// with the contract set.
#[must_use]
pub fn with_contract(
    mut candidate: DiscoveryCandidate,
    consumes: Vec<ContractItem>,
    produces: Vec<ContractItem>,
) -> DiscoveryCandidate {
    candidate.capability.consumes = consumes;
    candidate.capability.produces = produces;
    candidate
}

/// Why discovery refused its input or cannot propose for a task.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoverError {
    /// The manifest text is not valid TOML.
    #[error("cannot parse manifest `{path}`: {message}")]
    ManifestInvalid {
        /// Manifest path, as passed in for diagnostics.
        path: String,
        /// TOML parser error text.
        message: String,
    },
    /// The repository directory cannot be listed.
    #[error("cannot scan repository `{path}`: {message}")]
    RepoUnreadable {
        /// Directory path, as passed in for diagnostics.
        path: String,
        /// OS error text.
        message: String,
    },
    /// No task with this id is in the plan.
    #[error("plan declares no task `{id}`")]
    UnknownTask {
        /// Missing task id.
        id: String,
    },
}

impl DiscoverError {
    /// Manifest-path constructor for [`DiscoverError::ManifestInvalid`].
    #[must_use]
    pub fn manifest_invalid(path: &str, message: String) -> Self {
        Self::ManifestInvalid {
            path: path.to_string(),
            message,
        }
    }

    /// Directory-path constructor for [`DiscoverError::RepoUnreadable`].
    #[must_use]
    pub fn repo_unreadable(path: &str, message: String) -> Self {
        Self::RepoUnreadable {
            path: path.to_string(),
            message,
        }
    }
}

/// Sanitize one id or version component: keep alphanumerics plus `.`, `-`
/// and `_`, map every other run to a single `-`, trim edge separators.
/// Empty results become `"0"`, so a `*` dependency version still yields a
/// stable id. `^0.4` and `0.4` sanitize identically, so equivalent version
/// requirements model as one capability instead of two.
#[must_use]
pub fn sanitize_component(raw: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
            if ch == '-' {
                if !out.is_empty() && !dash {
                    out.push('-');
                }
                dash = true;
            } else {
                out.push(ch);
                dash = false;
            }
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches(['-', '.']).to_string();
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed
    }
}

/// Capability id for a discovered implementation: `{name}@{version}`.
/// Mirrors the `whisperx@3` convention already in the IR.
#[must_use]
pub fn capability_id(name: &str, version: &str) -> String {
    format!(
        "{}@{}",
        sanitize_component(name),
        sanitize_component(version)
    )
}

/// Scan a dependency manifest for candidate capabilities.
///
/// Parses the TOML text and maps every `[dependencies]` entry (sorted by
/// name, so file order never changes the output) to one candidate with
/// `kind = "tool"` and `source = "manifest:<path>#<name>"`. A string entry
/// uses its value as the version; a table entry uses its `version` key when
/// present, else `"*"`. Anything else (workspace inheritance without a
/// version string, malformed entries) also yields `"*"`, which sanitizes to
/// version `"0"`: the dependency is still reported, never silently dropped.
pub fn discover_from_manifest(
    manifest_toml: &str,
    manifest_path: &str,
) -> Result<Vec<DiscoveryCandidate>, DiscoverError> {
    let value: toml::Value = manifest_toml.parse().map_err(|e: toml::de::Error| {
        DiscoverError::manifest_invalid(manifest_path, e.to_string())
    })?;
    let Some(deps) = value.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<&str> = deps.keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut candidates = Vec::with_capacity(names.len());
    for name in names {
        let raw_version = match &deps[name] {
            toml::Value::String(version) => version.clone(),
            toml::Value::Table(table) => table
                .get("version")
                .and_then(toml::Value::as_str)
                .unwrap_or("*")
                .to_string(),
            _ => "*".to_string(),
        };
        let version = sanitize_component(&raw_version);
        candidates.push(DiscoveryCandidate {
            capability: Capability {
                id: capability_id(name, &version),
                kind: CapabilityKind::Tool,
                version,
                provider: None,
                source: Some(format!("manifest:{manifest_path}#{name}")),
                platform_requirements: Vec::new(),
                licensing_constraints: Vec::new(),
                consumes: Vec::new(),
                produces: Vec::new(),
                estimates: Vec::new(),
                resources: Vec::new(),
            },
            source: Source {
                kind: SourceKind::Manifest,
                locator: format!("manifest:{manifest_path}#{name}"),
                detail: format!("dependency `{name}` version `{raw_version}` in {manifest_path}"),
            },
        });
    }
    Ok(candidates)
}

/// Scan a local repository directory for candidate implementations.
///
/// Lists the top-level entries (sorted by file name, so directory order
/// never changes the output) and maps every non-hidden regular file to one
/// `tool` capability versioned `"0"` with
/// `source = "repo:<dir>#<file>"`. Hidden entries and subdirectories are
/// skipped: dotfiles are configuration, not implementations, and recursion
/// would vendor whole trees into the proposal. The capability name is the
/// file stem (extension stripped), sanitized; entries sanitizing to nothing
/// are skipped with the rest.
pub fn discover_from_repo(dir: &Path) -> Result<Vec<DiscoveryCandidate>, DiscoverError> {
    let display = dir.display().to_string();
    let entries = std::fs::read_dir(dir)
        .map_err(|e| DiscoverError::repo_unreadable(&display, e.to_string()))?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| DiscoverError::repo_unreadable(&display, e.to_string()))?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        if !entry.path().is_file() {
            continue;
        }
        names.push(file_name);
    }
    names.sort();
    let mut candidates = Vec::with_capacity(names.len());
    for file_name in names {
        let stem = Path::new(&file_name)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !stem
            .chars()
            .any(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            continue;
        }
        let name = sanitize_component(&stem);
        candidates.push(DiscoveryCandidate {
            capability: Capability {
                id: capability_id(&name, "0"),
                kind: CapabilityKind::Tool,
                version: "0".to_string(),
                provider: None,
                source: Some(format!("repo:{display}#{file_name}")),
                platform_requirements: Vec::new(),
                licensing_constraints: Vec::new(),
                consumes: Vec::new(),
                produces: Vec::new(),
                estimates: Vec::new(),
                resources: Vec::new(),
            },
            source: Source {
                kind: SourceKind::LocalRepo,
                locator: format!("repo:{display}#{file_name}"),
                detail: format!("file `{file_name}` in {display}"),
            },
        });
    }
    Ok(candidates)
}

/// Compatibility of one candidate with one task's contract.
///
/// Builds the invoke method discovery would propose and evaluates it with
/// the same typed-subset matching the compiler uses
/// ([`pc_core::contract_note_for`]): `compatible` is true exactly when the
/// frontier would rank the proposal as acceptable rather than blocked. The
/// reason names the coverage (`covers the task contract`) or the dropped
/// fields and their consumers, in the frontier's own words. A capability
/// the plan already models is evaluated with its modeled record, since the
/// proposal reuses it; an asserted contract that diverges from that record
/// is incompatible, with a reason naming both contracts.
pub fn check_compatibility(
    doc: &PlanDoc,
    candidate: &DiscoveryCandidate,
    task_id: &str,
) -> Result<Compatibility, DiscoverError> {
    if !doc.task.iter().any(|task| task.id == task_id) {
        return Err(DiscoverError::UnknownTask {
            id: task_id.to_string(),
        });
    }
    let modeled = doc
        .capability
        .iter()
        .find(|cap| cap.id == candidate.capability.id);
    if let Some(reason) = modeled.and_then(|cap| asserted_divergence(cap, candidate)) {
        return Ok(Compatibility {
            compatible: false,
            reason,
        });
    }
    let capability = modeled.unwrap_or(&candidate.capability);
    let conflicts = pc_core::passes::constraint_conflicts(&doc.objective, capability);
    if !conflicts.is_empty() {
        return Ok(Compatibility {
            compatible: false,
            reason: format!(
                "supports only {}, none of which the objective declares (E016)",
                conflicts.join("; "),
            ),
        });
    }
    let mut probe = doc.clone();
    if modeled.is_none() {
        probe.capability.push(candidate.capability.clone());
    }
    let method = Method {
        id: format!("{DISCOVER_ID_PREFIX}probe"),
        task: task_id.to_string(),
        kind: MethodKind::Invoke,
        subtasks: Vec::new(),
        order_edges: Vec::new(),
        capability: Some(candidate.capability.id.clone()),
        status: MethodStatus::Proposed,
        provenance: candidate.source.kind.provenance().to_string(),
        assumptions: Vec::new(),
    };
    match pc_core::contract_note_for(&probe, &method) {
        None => Ok(Compatibility {
            compatible: true,
            reason: "covers the task contract".to_string(),
        }),
        Some(note) => Ok(Compatibility {
            compatible: false,
            reason: note.frontier_reason(),
        }),
    }
}

/// Why a candidate's asserted contract cannot apply to the modeled
/// capability it names, or `None` when nothing was asserted or the
/// assertion matches the modeled record (ignoring item and field order).
fn asserted_divergence(modeled: &Capability, candidate: &DiscoveryCandidate) -> Option<String> {
    let asserted = &candidate.capability;
    if asserted.consumes.is_empty() && asserted.produces.is_empty() {
        return None;
    }
    let contract = |cap: &Capability| {
        format!(
            "consumes [{}], produces [{}]",
            contract_text(&cap.consumes),
            contract_text(&cap.produces),
        )
    };
    let (asserted, modeled_contract) = (contract(asserted), contract(modeled));
    (asserted != modeled_contract).then(|| {
        format!(
            "asserted contract ({asserted}) diverges from modeled capability `{}` ({modeled_contract}); the modeled record governs",
            modeled.id,
        )
    })
}

/// One contract list as `artifact[field,field]`, items and fields sorted.
fn contract_text(items: &[ContractItem]) -> String {
    let mut rendered: Vec<String> = items
        .iter()
        .map(|item| {
            let mut fields = item.fields.clone();
            fields.sort();
            format!("{}[{}]", item.artifact, fields.join(","))
        })
        .collect();
    rendered.sort();
    rendered.join(", ")
}

/// Whether a candidate can be proposed for a task, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compatibility {
    /// True when the proposal would rank as acceptable rather than blocked.
    pub compatible: bool,
    /// Coverage summary, or the dropped fields and their consumers.
    pub reason: String,
}

/// The task worth investigating first, with the frontier's own reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bottleneck {
    /// Task id to investigate.
    pub task: String,
    /// Why it is the bottleneck (frontier wording).
    pub reason: String,
}

/// Identify the bottleneck task to investigate.
///
/// Prefers critical-path cost resting on declared guesses
/// (`unmeasured-critical` entries, ranked before execution work carries
/// evidence), then the top-ranked execution entry, then the top
/// structurally ordered unestimated entry. `None` means the frontier is
/// fully planned: every reachable task has an accepted method and no
/// critical cost rests on guesses, so there is nothing to discover for.
#[must_use]
pub fn bottleneck_task(doc: &PlanDoc, frontier: &pc_core::FrontierReport) -> Option<Bottleneck> {
    let task_of = |subject: &str| -> Option<String> {
        if let Some(id) = subject.strip_prefix("task:") {
            return Some(id.to_string());
        }
        if let Some(id) = subject.strip_prefix("method:") {
            return doc
                .method
                .iter()
                .find(|method| method.id == id)
                .map(|method| method.task.clone());
        }
        None
    };
    let critical = frontier
        .items
        .iter()
        .chain(&frontier.unestimated)
        .find(|item| item.kind == pc_core::frontier::KIND_UNMEASURED_CRITICAL)
        .and_then(|item| task_of(&item.subject).map(|task| (task, item.reason.clone())));
    if let Some((task, reason)) = critical {
        return Some(Bottleneck { task, reason });
    }
    frontier
        .items
        .first()
        .or_else(|| frontier.unestimated.first())
        .and_then(|item| {
            task_of(&item.subject).map(|task| Bottleneck {
                task,
                reason: item.reason.clone(),
            })
        })
}

/// What happened to one candidate while building the proposal patch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalOutcome {
    /// Candidate capability id.
    pub capability: String,
    /// Proposed method id, when the candidate entered the patch.
    pub method: Option<String>,
    /// Contract evaluation against the target task.
    pub compatibility: Compatibility,
    /// Source locator of the candidate.
    pub source: String,
    /// Why the candidate was left out, when it was.
    pub skipped: Option<String>,
}

/// Turn candidates into a step-7 proposal patch for one task.
///
/// Every surviving candidate contributes one `proposed` invoke method
/// (`discover-<task>-<capability>`, never the P3 `match-` namespace) with
/// the adapter provenance and a source assumption, plus its capability
/// unless the plan already models it (then the method reuses the modeled
/// record and its assumption says so). Skipped candidates are reported,
/// never silently dropped: capabilities the task already invokes, asserted
/// contracts diverging from the modeled record, E016-violating capabilities (the
/// merged plan would fail `check`), and anything
/// [`pc_core::validate_patch`] refuses (colliding method ids, undeclared
/// artifacts or fields), each with the validator's own reason, so one bad
/// candidate never refuses the whole patch. Contract-mismatched candidates are
/// kept: the frontier ranks them as blocked alternatives carrying the same
/// reason. Output is sorted by capability id, so the same inputs always
/// give a byte-identical patch.
pub fn candidates_to_patch(
    doc: &PlanDoc,
    task_id: &str,
    candidates: Vec<DiscoveryCandidate>,
) -> Result<(Patch, Vec<ProposalOutcome>), DiscoverError> {
    if !doc.task.iter().any(|task| task.id == task_id) {
        return Err(DiscoverError::UnknownTask {
            id: task_id.to_string(),
        });
    }
    let mut ordered: Vec<DiscoveryCandidate> = candidates;
    ordered.sort_by(|a, b| a.capability.id.cmp(&b.capability.id));
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut patch = Patch::default();
    let mut outcomes = Vec::with_capacity(ordered.len());
    for mut candidate in ordered {
        let id = candidate.capability.id.clone();
        let source = candidate.source.locator.clone();
        let skip = |compatibility: Compatibility, reason: String| ProposalOutcome {
            capability: id.clone(),
            method: None,
            compatibility,
            source: source.clone(),
            skipped: Some(reason),
        };
        if !seen.insert(id.clone()) {
            outcomes.push(skip(
                check_compatibility(doc, &candidate, task_id)?,
                "duplicate of an earlier candidate with the same capability id".to_string(),
            ));
            continue;
        }
        if let Some(existing) = doc.method.iter().find(|method| {
            method.task == task_id
                && method.kind == MethodKind::Invoke
                && method.capability.as_deref() == Some(id.as_str())
        }) {
            outcomes.push(skip(
                check_compatibility(doc, &candidate, task_id)?,
                format!(
                    "task `{task_id}` already has method `{}` invoking it",
                    existing.id
                ),
            ));
            continue;
        }
        let modeled = doc.capability.iter().find(|cap| cap.id == id);
        if let Some(reason) = modeled.and_then(|cap| asserted_divergence(cap, &candidate)) {
            outcomes.push(skip(
                Compatibility {
                    compatible: false,
                    reason: reason.clone(),
                },
                reason,
            ));
            continue;
        }
        let admissible = pc_core::passes::constraint_conflicts(
            &doc.objective,
            modeled.unwrap_or(&candidate.capability),
        )
        .is_empty();
        candidate
            .capability
            .source
            .get_or_insert_with(|| candidate.source.locator.clone());
        let method_id = format!("{DISCOVER_ID_PREFIX}{task_id}-{id}");
        let proposal = Patch {
            add_task: Vec::new(),
            add_method: vec![Method {
                id: method_id.clone(),
                task: task_id.to_string(),
                kind: MethodKind::Invoke,
                subtasks: Vec::new(),
                order_edges: Vec::new(),
                capability: Some(id.clone()),
                status: MethodStatus::Proposed,
                provenance: candidate.source.kind.provenance().to_string(),
                assumptions: vec![if modeled.is_some() {
                    format!(
                        "discovered from {}; reuses modeled capability `{id}`, whose record governs the contract",
                        candidate.source.detail,
                    )
                } else {
                    format!(
                        "discovered from {}; contract asserted by the operator, pending compiler validation",
                        candidate.source.detail,
                    )
                }],
            }],
            add_capability: if modeled.is_some() {
                Vec::new()
            } else {
                vec![candidate.capability.clone()]
            },
        };
        if let Err(error) = pc_core::validate_patch(doc, &proposal) {
            outcomes.push(skip(
                Compatibility {
                    compatible: false,
                    reason: error.to_string(),
                },
                error.to_string(),
            ));
            continue;
        }
        let compatibility = check_compatibility(doc, &candidate, task_id)?;
        if !admissible {
            outcomes.push(skip(
                compatibility,
                "platform/licence constraints (E016) would fail `pc check`".to_string(),
            ));
            continue;
        }
        patch.add_capability.extend(proposal.add_capability);
        patch.add_method.extend(proposal.add_method);
        outcomes.push(ProposalOutcome {
            capability: id,
            method: Some(method_id),
            compatibility,
            source,
            skipped: None,
        });
    }
    Ok((patch, outcomes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = r#"
ir_version = 0

[plan]
id = "p"
title = "t"

[objective]
root_task = "job"
weights = { duration_minutes = 1.0 }

[[artifact]]
name = "transcript"
fields = ["words", "timings", "segment_confidence"]

[[task]]
id = "job"
title = "Job"
state = "resolved"
produces = [{ artifact = "transcript", fields = ["words", "timings", "segment_confidence"] }]

[[method]]
id = "via-whisperx"
task = "job"
kind = "invoke"
capability = "whisperx@3"
status = "accepted"
provenance = "human:test"

[[capability]]
id = "whisperx@3"
kind = "tool"
version = "3"
produces = [{ artifact = "transcript", fields = ["words", "timings", "segment_confidence"] }]
estimates = [{ metric = "duration_minutes", low = 8.0, likely = 12.0, high = 20.0, provenance = "declared" }]
"#;

    fn plan() -> PlanDoc {
        pc_ir::from_toml(PLAN).unwrap()
    }

    fn checked(doc: &mut PlanDoc) -> pc_core::passes::Report {
        let text = pc_ir::to_toml(doc).unwrap();
        let owned = text.clone();
        pc_core::passes::check("plan.toml", &owned, doc, None)
    }

    fn compiled(doc: &PlanDoc) -> pc_core::CompiledPlan {
        pc_core::compile(doc).unwrap()
    }

    fn manifest_candidate() -> DiscoveryCandidate {
        with_contract(
            discover_from_manifest("[dependencies]\nvoz = \"0.4\"\n", "Cargo.toml")
                .unwrap()
                .pop()
                .unwrap(),
            Vec::new(),
            vec![ContractItem {
                artifact: "transcript".to_string(),
                fields: vec![
                    "words".to_string(),
                    "timings".to_string(),
                    "segment_confidence".to_string(),
                ],
            }],
        )
    }

    /// Manifest entries must sort by name: file order is an authoring
    /// accident, and an unstable order would emit byte-different patches
    /// from the same dependencies.
    #[test]
    fn manifest_output_ignores_file_order() {
        let first = discover_from_manifest(
            "[dependencies]\nzebra = \"1.0\"\nvoz = \"0.4\"\n",
            "Cargo.toml",
        )
        .unwrap();
        let second = discover_from_manifest(
            "[dependencies]\nvoz = \"0.4\"\nzebra = \"1.0\"\n",
            "Cargo.toml",
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].capability.id, "voz@0.4");
        assert_eq!(first[1].capability.id, "zebra@1.0");
    }

    /// `^0.4` and `0.4` are the same requirement: without sanitization they
    /// would model as two capabilities and propose twice.
    #[test]
    fn manifest_versions_sanitize_to_one_capability() {
        assert_eq!(sanitize_component("^0.4"), "0.4");
        assert_eq!(sanitize_component(">=1.0, <2.0"), "1.0-2.0");
        assert_eq!(sanitize_component("*"), "0");
        assert_eq!(capability_id("voz", "^0.4"), "voz@0.4");
    }

    /// Table-form dependencies use their version key; workspace-inherited
    /// ones without a version still report as `0` instead of vanishing.
    #[test]
    fn manifest_table_entries_keep_their_version() {
        let found = discover_from_manifest(
            "[dependencies]\nvoz = { version = \"0.4\", features = [\"x\"] }\nlocal = { path = \"../local\" }\n",
            "Cargo.toml",
        )
        .unwrap();
        assert_eq!(found[0].capability.id, "local@0");
        assert_eq!(found[1].capability.id, "voz@0.4");
    }

    /// Dotfiles are configuration, not implementations: scanning them would
    /// propose a capability per dotfile on every repository.
    #[test]
    fn repo_scan_ignores_hidden_entries_and_sorts() {
        let dir = std::env::temp_dir().join(format!(
            "pc-discover-repo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".hidden"), "x").unwrap();
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("zebra.sh"), "x").unwrap();
        std::fs::write(dir.join("denoise"), "x").unwrap();
        let found = discover_from_repo(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].capability.id, "denoise@0");
        assert_eq!(found[1].capability.id, "zebra@0");
        assert!(found[0].source.locator.contains("denoise"));
    }

    /// A capability the task already invokes must be skipped: proposing it
    /// again adds a redundant alternative, and re-adding the capability
    /// would refuse the whole patch in `pc propose` on collision.
    #[test]
    fn capabilities_the_task_invokes_are_skipped() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let mut dup = manifest_candidate();
        dup.capability.id = "whisperx@3".to_string();
        let (patch, outcomes) = candidates_to_patch(&doc, "job", vec![dup]).unwrap();
        assert!(patch.add_capability.is_empty());
        assert!(patch.add_method.is_empty());
        assert_eq!(
            outcomes[0].skipped.as_deref(),
            Some("task `job` already has method `via-whisperx` invoking it")
        );
    }

    /// A capability modeled for another task must still reach this one:
    /// skipping it would hide an existing alternative once the bottleneck
    /// moves. The patch reuses the modeled record instead of duplicating it,
    /// and compatibility reads the modeled contract.
    #[test]
    fn modeled_capability_is_reused_for_another_task() {
        let mut doc = plan();
        doc.task.push(pc_ir::Task {
            id: "archive".to_string(),
            title: "Archive".to_string(),
            state: pc_ir::TaskState::Open,
            consumes: Vec::new(),
            produces: vec![ContractItem {
                artifact: "transcript".to_string(),
                fields: vec!["words".to_string()],
            }],
            tasks_axi: None,
            beads_jsonl: None,
        });
        let mut candidate = manifest_candidate();
        candidate.capability.id = "whisperx@3".to_string();
        candidate.capability.produces = Vec::new();
        let (patch, outcomes) = candidates_to_patch(&doc, "archive", vec![candidate]).unwrap();
        assert!(patch.add_capability.is_empty());
        assert_eq!(patch.add_method.len(), 1);
        assert_eq!(
            patch.add_method[0].capability.as_deref(),
            Some("whisperx@3")
        );
        assert_eq!(outcomes[0].skipped, None);
        assert!(outcomes[0].compatibility.compatible, "{outcomes:?}");
        assert!(
            patch.add_method[0].assumptions[0].contains("reuses modeled capability `whisperx@3`"),
            "{:?}",
            patch.add_method[0].assumptions
        );
        assert!(pc_core::validate_patch(&doc, &patch).is_ok());
    }

    /// An asserted contract the modeled record contradicts must not be
    /// dropped silently: the candidate is skipped naming both contracts,
    /// while a matching assertion (in any order) is reused as usual.
    #[test]
    fn asserted_contract_diverging_from_the_modeled_record_is_skipped() {
        let mut doc = plan();
        doc.task.push(pc_ir::Task {
            id: "archive".to_string(),
            title: "Archive".to_string(),
            state: pc_ir::TaskState::Open,
            consumes: Vec::new(),
            produces: vec![ContractItem {
                artifact: "transcript".to_string(),
                fields: vec!["words".to_string()],
            }],
            tasks_axi: None,
            beads_jsonl: None,
        });
        let mut candidate = manifest_candidate();
        candidate.capability.id = "whisperx@3".to_string();
        candidate.capability.produces = vec![ContractItem {
            artifact: "transcript".to_string(),
            fields: vec!["words".to_string()],
        }];
        let (patch, outcomes) =
            candidates_to_patch(&doc, "archive", vec![candidate.clone()]).unwrap();
        assert!(patch.add_method.is_empty());
        assert!(patch.add_capability.is_empty());
        let expected = "asserted contract (consumes [], produces [transcript[words]]) diverges \
                        from modeled capability `whisperx@3` (consumes [], produces \
                        [transcript[segment_confidence,timings,words]]); the modeled record governs";
        assert_eq!(outcomes[0].skipped.as_deref(), Some(expected));
        assert!(!outcomes[0].compatibility.compatible);
        assert_eq!(
            check_compatibility(&doc, &candidate, "archive").unwrap(),
            Compatibility {
                compatible: false,
                reason: expected.to_string(),
            }
        );

        candidate.capability.produces[0].fields = vec![
            "timings".to_string(),
            "segment_confidence".to_string(),
            "words".to_string(),
        ];
        let (patch, outcomes) = candidates_to_patch(&doc, "archive", vec![candidate]).unwrap();
        assert_eq!(outcomes[0].skipped, None, "{outcomes:?}");
        assert_eq!(patch.add_method.len(), 1);
    }

    /// Stems with no id characters must be skipped: they would all
    /// sanitize to the meaningless `0@0` capability.
    #[test]
    fn repo_scan_skips_stems_without_id_characters() {
        let dir = std::env::temp_dir().join(format!(
            "pc-discover-repo-stems-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("!!!.sh"), "x").unwrap();
        std::fs::write(dir.join("-.py"), "x").unwrap();
        std::fs::write(dir.join("0.sh"), "x").unwrap();
        let found = discover_from_repo(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let ids: Vec<&str> = found.iter().map(|c| c.capability.id.as_str()).collect();
        assert_eq!(ids, vec!["0@0"]);
        assert!(found[0].source.locator.ends_with("#0.sh"));
    }

    /// Candidates the patch validator would refuse are skipped with its
    /// reason: one undeclared field must not refuse every other candidate.
    #[test]
    fn unknown_contract_fields_are_skipped_with_the_validator_reason() {
        let doc = plan();
        let mut candidate = manifest_candidate();
        candidate.capability.produces[0]
            .fields
            .push("ghost".to_string());
        let (patch, outcomes) = candidates_to_patch(&doc, "job", vec![candidate]).unwrap();
        assert!(patch.add_method.is_empty());
        assert!(!outcomes[0].compatibility.compatible);
        assert_eq!(
            outcomes[0].skipped.as_deref(),
            Some("capability:voz@0.4 uses unknown field `transcript.ghost`")
        );
    }

    /// Discovery ids must avoid the P3 `match-` namespace: a proposed
    /// `match-*` id would shadow a derived proposal for the same task.
    #[test]
    fn method_ids_avoid_the_matcher_namespace() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let (patch, _) = candidates_to_patch(&doc, "job", vec![manifest_candidate()]).unwrap();
        assert_eq!(patch.add_method.len(), 1);
        assert!(patch.add_method[0].id.starts_with(DISCOVER_ID_PREFIX));
        assert!(!patch.add_method[0].id.starts_with(pc_core::MATCH_ID_PREFIX));
    }

    /// The adapter must never accept: an `accepted` method in the patch
    /// would bypass the proposal workflow and move selection immediately.
    #[test]
    fn proposals_are_never_accepted() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let (patch, _) = candidates_to_patch(&doc, "job", vec![manifest_candidate()]).unwrap();
        assert!(patch.add_method.iter().all(|method| {
            method.status == MethodStatus::Proposed && !method.provenance.trim().is_empty()
        }));
        assert!(pc_core::validate_patch(&doc, &patch).is_ok());
    }

    /// E016-violating candidates cannot enter the patch: the merged plan
    /// would fail `check`, so `pc propose` would refuse every candidate.
    #[test]
    fn platform_violations_are_excluded_with_a_reason() {
        let mut doc = plan();
        doc.objective.platforms = vec!["linux".to_string()];
        assert!(checked(&mut doc).ok());
        let mut candidate = manifest_candidate();
        candidate.capability.platform_requirements = vec!["darwin".to_string()];
        let (patch, outcomes) = candidates_to_patch(&doc, "job", vec![candidate]).unwrap();
        assert!(patch.add_capability.is_empty());
        assert!(!outcomes[0].compatibility.compatible);
        assert!(outcomes[0].skipped.is_some());
    }

    /// Contract-mismatched candidates stay in the patch: the frontier ranks
    /// them as blocked alternatives, so silently dropping them would hide
    /// the incompatibility the operator must see.
    #[test]
    fn contract_mismatches_stay_visible_as_blocked() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let mut candidate = manifest_candidate();
        candidate.capability.produces = vec![ContractItem {
            artifact: "transcript".to_string(),
            fields: vec!["words".to_string()],
        }];
        let (patch, outcomes) = candidates_to_patch(&doc, "job", vec![candidate]).unwrap();
        assert_eq!(patch.add_method.len(), 1);
        assert!(!outcomes[0].compatibility.compatible);
        assert!(outcomes[0].compatibility.reason.contains("transcript."));
        assert_eq!(outcomes[0].skipped, None);
    }

    /// The compatibility verdict must agree with the compiler: a candidate
    /// reported compatible has no contract note when the same invoke is
    /// evaluated by the pass the frontier uses.
    #[test]
    fn compatibility_agrees_with_the_frontier_note() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let candidate = manifest_candidate();
        let verdict = check_compatibility(&doc, &candidate, "job").unwrap();
        assert!(verdict.compatible);
        let mut probe = doc.clone();
        probe.capability.push(candidate.capability.clone());
        let method = Method {
            id: "probe".to_string(),
            task: "job".to_string(),
            kind: MethodKind::Invoke,
            subtasks: Vec::new(),
            order_edges: Vec::new(),
            capability: Some(candidate.capability.id.clone()),
            status: MethodStatus::Proposed,
            provenance: MANIFEST_PROVENANCE.to_string(),
            assumptions: Vec::new(),
        };
        assert_eq!(pc_core::contract_note_for(&probe, &method), None);
    }

    /// Bottleneck ranking must prefer unmeasured critical-path cost over
    /// ranked proposals: investigating a side quest first wastes the
    /// discovery budget on work off the critical path.
    #[test]
    fn bottleneck_prefers_unmeasured_critical_cost() {
        let mut doc = plan();
        assert!(checked(&mut doc).ok());
        let report = pc_core::frontier(&doc, &compiled(&doc));
        let bottleneck = bottleneck_task(&doc, &report).unwrap();
        assert_eq!(bottleneck.task, "job");
    }

    /// A fully planned frontier has no bottleneck: emitting a task then
    /// would invent investigation work where none remains.
    #[test]
    fn fully_planned_frontier_has_no_bottleneck() {
        let mut doc = plan();
        doc.task.push(pc_ir::Task {
            id: "extra".to_string(),
            title: "Extra".to_string(),
            state: pc_ir::TaskState::Dropped,
            consumes: Vec::new(),
            produces: Vec::new(),
            tasks_axi: None,
            beads_jsonl: None,
        });
        doc.evidence.push(pc_ir::Evidence {
            subject: "capability:whisperx@3".to_string(),
            metric: "duration_minutes".to_string(),
            value: 12.0,
            unit: "minutes".to_string(),
            time: "2026-10-01T00:00:00Z".to_string(),
            source: "benchmark".to_string(),
            basis: None,
            artifact_hash: None,
        });
        assert!(checked(&mut doc).ok());
        let report = pc_core::frontier(&doc, &compiled(&doc));
        assert!(report.items.is_empty());
        assert!(report.unestimated.is_empty());
        assert!(bottleneck_task(&doc, &report).is_none());
    }
}
