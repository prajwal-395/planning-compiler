//! Plan loading.
//!
//! State is text files: a `plan.toml` (or `plan.json`) plus an optional
//! sibling `evidence.jsonl`. This crate owns the filesystem layout; passes
//! P1-P2 in `pc-core` own validation. There is no database in v0.

use pc_ir::{Evidence, PlanDoc, canonical_unit};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// A plan read from disk, ready for [`pc_core::passes::check`].
pub struct LoadedPlan {
    /// Display path used in diagnostics.
    pub display_path: String,
    /// Raw plan-file text, used for best-effort line lookup.
    pub text: String,
    /// Parsed document, including any sibling `evidence.jsonl` records.
    pub doc: PlanDoc,
    /// Display path of the sibling `evidence.jsonl`, when one was loaded.
    pub evidence_file: Option<String>,
    /// 1-based `evidence.jsonl` line of each record appended to `doc.evidence`.
    pub evidence_lines: Vec<u32>,
}

/// Errors reading a plan from disk. The CLI renders these as `error[E000]`
/// diagnostics: an unreadable plan is a resolve failure.
#[derive(Debug, Error)]
pub enum LoadError {
    /// No plan file found at or under the given path.
    #[error("no plan found at `{0}` (looked for plan.toml and plan.json)")]
    NotFound(String),
    /// The file could not be read.
    #[error("cannot read `{path}`: {message}")]
    Unreadable {
        /// File that could not be read.
        path: String,
        /// OS error text.
        message: String,
    },
    /// The plan file could not be written.
    #[error("cannot write `{path}`: {message}")]
    Unwritable {
        /// File that could not be written.
        path: String,
        /// OS or serializer error text.
        message: String,
    },
    /// The file is not valid TOML or JSON for the IR.
    #[error("cannot parse `{path}`: {message}")]
    Unparsable {
        /// File that could not be parsed.
        path: String,
        /// Parser error text.
        message: String,
    },
}

/// Candidate plan files inside a directory, in lookup order.
fn candidates(dir: &Path) -> Vec<PathBuf> {
    ["plan.toml", "plan.json"]
        .iter()
        .map(|name| dir.join(name))
        .collect()
}

/// Resolve the user-facing path to one plan file.
///
/// A file path loads directly. A directory probes [`candidates`] in order.
/// Anything else is [`LoadError::NotFound`].
pub fn resolve_plan_file(path: &Path) -> Result<PathBuf, LoadError> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        for candidate in candidates(path) {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(LoadError::NotFound(path.display().to_string()))
}

/// Load and parse a plan file, folding in a sibling `evidence.jsonl` when
/// one sits next to the plan file. Format follows the file extension:
/// `.toml` parses as TOML, anything else as JSON.
pub fn load(path: &Path) -> Result<LoadedPlan, LoadError> {
    let file = resolve_plan_file(path)?;
    let display_path = file.display().to_string();
    let text = std::fs::read_to_string(&file).map_err(|e| LoadError::Unreadable {
        path: display_path.clone(),
        message: e.to_string(),
    })?;
    let is_toml = file.extension().is_some_and(|ext| ext == "toml");
    let mut doc: PlanDoc = if is_toml {
        pc_ir::from_toml(&text).map_err(|e| LoadError::Unparsable {
            path: display_path.clone(),
            message: e.to_string(),
        })?
    } else {
        pc_ir::from_json(&text).map_err(|e| LoadError::Unparsable {
            path: display_path.clone(),
            message: e.to_string(),
        })?
    };
    let mut evidence_file = None;
    let mut evidence_lines = Vec::new();
    if let Some(parent) = file.parent() {
        let path = parent.join("evidence.jsonl");
        if path.is_file() {
            for (line, record) in read_evidence_jsonl(&path)? {
                evidence_lines.push(line);
                doc.evidence.push(record);
            }
            evidence_file = Some(path.display().to_string());
        }
    }
    Ok(LoadedPlan {
        display_path,
        text,
        doc,
        evidence_file,
        evidence_lines,
    })
}

/// Write a plan document back to its file, preserving the on-disk format:
/// `.toml` serializes as TOML, anything else as pretty JSON.
///
/// Used by the step-7 decision commands (`propose`, `accept`, `reject`),
/// which edit the pristine plan and only write when the merged copy passes
/// `check`. Pass mutations (P3 proposals, P4 merges) are never written.
pub fn write_plan(file: &Path, doc: &PlanDoc) -> Result<(), LoadError> {
    let display = file.display().to_string();
    let is_toml = file.extension().is_some_and(|ext| ext == "toml");
    let text = if is_toml {
        pc_ir::to_toml(doc).map_err(|e| LoadError::Unwritable {
            path: display.clone(),
            message: e.to_string(),
        })?
    } else {
        pc_ir::to_json_pretty(doc).map_err(|e| LoadError::Unwritable {
            path: display.clone(),
            message: e.to_string(),
        })?
    };
    std::fs::write(file, text).map_err(|e| LoadError::Unwritable {
        path: display,
        message: e.to_string(),
    })
}

/// Read one JSON [`Evidence`] per non-blank line, paired with its 1-based
/// line number.
fn read_evidence_jsonl(path: &Path) -> Result<Vec<(u32, Evidence)>, LoadError> {
    let display_path = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Unreadable {
        path: display_path.clone(),
        message: e.to_string(),
    })?;
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Evidence = serde_json::from_str(line).map_err(|e| LoadError::Unparsable {
            path: display_path.clone(),
            message: format!("line {}: {e}", index + 1),
        })?;
        out.push((u32::try_from(index + 1).unwrap_or(u32::MAX), record));
    }
    Ok(out)
}

/// Why one evidence record was refused by [`parse_evidence_record`].
/// Messages mirror the P1/P2 codes `pc check` would report for the same
/// record, so `pc record` and `pc check` never disagree about validity.
#[derive(Debug, Error)]
pub enum EvidenceRecordError {
    /// The text is not one JSON evidence object.
    #[error("cannot parse evidence: {0}")]
    Unparsable(String),
    /// The subject is not `capability:`, `method:` or `task:` with an id
    /// (P2 reports E014).
    #[error("{0}")]
    BadSubject(String),
    /// The metric is not one v0 knows (P2 reports E010).
    #[error("evidence uses unknown metric `{0}`")]
    UnknownMetric(String),
    /// The unit is not the metric's canonical unit (P1 reports E014).
    #[error("evidence uses unit `{unit}` but metric `{metric}` takes `{expected}`")]
    BadUnit {
        /// Metric the record claims.
        metric: String,
        /// Unit the record carries.
        unit: String,
        /// Canonical unit the metric takes.
        expected: String,
    },
    /// The timestamp is not RFC 3339 (P2 reports E014).
    #[error("evidence has timestamp `{0}` which is not RFC 3339")]
    BadTime(String),
    /// The basis is not a typed basis (P2 reports E014).
    #[error("{0}")]
    BadBasis(String),
    /// The value is not finite or is negative (P2 reports E014).
    #[error("evidence must have a finite, non-negative value")]
    BadValue,
}

/// Parse and validate one JSON evidence object for `pc record`.
///
/// Accepts exactly what P1/P2 accept for one record: a well-formed subject,
/// a known metric, the metric's canonical unit (case-insensitive,
/// canonicalized in place), an RFC 3339 timestamp, a typed basis when one
/// is recorded, and a finite, non-negative value. Subject existence and
/// basis agreement with earlier records need the plan, so `pc record`
/// checks those separately: an unknown subject is staged as pending
/// evidence, not refused, and a basis conflict is refused.
pub fn parse_evidence_record(text: &str) -> Result<Evidence, EvidenceRecordError> {
    let mut record: Evidence =
        serde_json::from_str(text).map_err(|e| EvidenceRecordError::Unparsable(e.to_string()))?;
    if let Err(reason) = pc_ir::check_evidence_subject(&record.subject) {
        return Err(EvidenceRecordError::BadSubject(reason));
    }
    if !pc_ir::KNOWN_METRICS.contains(&record.metric.as_str()) {
        return Err(EvidenceRecordError::UnknownMetric(record.metric));
    }
    let canonical = canonical_unit(&record.metric).unwrap_or_default();
    if record.unit.eq_ignore_ascii_case(canonical) {
        record.unit = canonical.to_string();
    } else {
        return Err(EvidenceRecordError::BadUnit {
            metric: record.metric,
            unit: record.unit,
            expected: canonical.to_string(),
        });
    }
    if chrono::DateTime::parse_from_rfc3339(&record.time).is_err() {
        return Err(EvidenceRecordError::BadTime(record.time));
    }
    if let Err(reason) = pc_ir::check_evidence_basis(&record.basis) {
        return Err(EvidenceRecordError::BadBasis(reason));
    }
    if !record.value.is_finite() || record.value < 0.0 {
        return Err(EvidenceRecordError::BadValue);
    }
    Ok(record)
}

/// Append one record to the `evidence.jsonl` sibling of a plan file,
/// creating the log when it is missing.
///
/// Returns the log path and the record's 1-based line number, using the
/// same line counting as [`load`]: every line counts, including blanks.
pub fn append_evidence(plan_file: &Path, record: &Evidence) -> Result<(PathBuf, u32), LoadError> {
    let log = plan_file
        .parent()
        .unwrap_or(Path::new("."))
        .join("evidence.jsonl");
    let display = log.display().to_string();
    let existing = if log.is_file() {
        std::fs::read_to_string(&log).map_err(|e| LoadError::Unreadable {
            path: display.clone(),
            message: e.to_string(),
        })?
    } else {
        String::new()
    };
    let line = u32::try_from(existing.lines().count() + 1).unwrap_or(u32::MAX);
    let mut line_text = if existing.is_empty() || existing.ends_with('\n') {
        String::new()
    } else {
        "\n".to_string()
    };
    line_text.push_str(
        &serde_json::to_string(record).map_err(|e| LoadError::Unparsable {
            path: display.clone(),
            message: e.to_string(),
        })?,
    );
    line_text.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(line_text.as_bytes())
        })
        .map_err(|e| LoadError::Unreadable {
            path: display.clone(),
            message: e.to_string(),
        })?;
    Ok((log, line))
}
