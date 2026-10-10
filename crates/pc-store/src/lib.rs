//! Plan loading.
//!
//! State is text files: a `plan.toml` (or `plan.json`) plus an optional
//! sibling `evidence.jsonl`. This crate owns the filesystem layout; passes
//! P1-P2 in `pc-core` own validation. There is no database in v0.

use pc_ir::{Evidence, PlanDoc};
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
