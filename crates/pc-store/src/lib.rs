//! Plan loading.
//!
//! State is text files: a `plan.toml` (or `plan.json`) plus optional
//! sibling `evidence.jsonl`, `decisions.jsonl`, `outcomes.jsonl`, and
//! `metrics.jsonl` logs. This crate owns the filesystem layout; passes
//! P1-P5 in `pc-core` own validation; [`LoadedPlan::check`] runs them. There is no database in v0.

use pc_ir::{CompilerRunEvent, DecisionEvent, Evidence, OutcomeEvent, PlanDoc, canonical_unit};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Revision of the bytes this load read, as [`plan_revision`] reports it.
    /// Writes pass it back so they never overwrite a change made since.
    pub revision: String,
}

impl LoadedPlan {
    /// Run passes P1-P5 over a copy of the document.
    ///
    /// Returns the passed copy (carrying P3 derived proposals, P4 share
    /// merges and P1 unit canonicalization) plus its report. The copy is
    /// what compile, frontier and explain run over; it must never be
    /// written back to the plan file.
    pub fn check(&self) -> (PlanDoc, pc_core::passes::Report) {
        let mut doc = self.doc.clone();
        let evidence_log = self
            .evidence_file
            .as_deref()
            .map(|file| pc_core::passes::EvidenceLog {
                file,
                lines: &self.evidence_lines,
            });
        let report = pc_core::passes::check(&self.display_path, &self.text, &mut doc, evidence_log);
        (doc, report)
    }
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

/// Load and parse a plan file, folding in the `evidence.jsonl` beside the
/// real (symlink-resolved) plan file when there is one. Format follows the file extension:
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
    let log = if file.is_symlink() {
        evidence_log(&canonical_plan_file(&file)?)
    } else {
        evidence_log(&file)
    };
    let mut evidence_file = None;
    let mut evidence_lines = Vec::new();
    let mut evidence_text = None;
    if log.is_file() {
        let log_text = std::fs::read_to_string(&log).map_err(|e| LoadError::Unreadable {
            path: log.display().to_string(),
            message: e.to_string(),
        })?;
        for (line, record) in parse_evidence_jsonl(&log, &log_text)? {
            evidence_lines.push(line);
            doc.evidence.push(record);
        }
        evidence_file = Some(log.display().to_string());
        evidence_text = Some(log_text);
    }
    let revision = revision_of(text.as_bytes(), evidence_text.as_deref().map(str::as_bytes));
    Ok(LoadedPlan {
        display_path,
        text,
        doc,
        evidence_file,
        evidence_lines,
        revision,
    })
}

fn evidence_log(plan_file: &Path) -> PathBuf {
    plan_file
        .parent()
        .unwrap_or(Path::new("."))
        .join("evidence.jsonl")
}

fn serialize_plan(file: &Path, doc: &PlanDoc) -> Result<String, LoadError> {
    let is_toml = file.extension().is_some_and(|ext| ext == "toml");
    if is_toml {
        pc_ir::to_toml(doc).map_err(|e| e.to_string())
    } else {
        pc_ir::to_json_pretty(doc).map_err(|e| e.to_string())
    }
    .map_err(|message| LoadError::Unwritable {
        path: file.display().to_string(),
        message,
    })
}

fn replace_file(file: &Path, text: &str) -> Result<(), LoadError> {
    atomic_replace(file, text.as_bytes()).map_err(|e| LoadError::Unwritable {
        path: file.display().to_string(),
        message: e.to_string(),
    })
}

/// Stable token for the current raw plan file, used by guarded MCP writes.
pub fn plan_revision(file: &Path) -> Result<String, LoadError> {
    revision_of_file(&canonical_plan_file(file)?)
}

/// Resolve a plan path to the real file it names, following symlinks, so
/// writes land on (and sandbox checks see) the file that is actually read.
pub fn canonical_plan_file(path: &Path) -> Result<PathBuf, LoadError> {
    let file = resolve_plan_file(path)?;
    file.canonicalize().map_err(|e| LoadError::Unreadable {
        path: file.display().to_string(),
        message: e.to_string(),
    })
}

/// Write a plan document back to its file only if its current revision
/// matches `expected`, preserving the on-disk format: `.toml` serializes as
/// TOML, anything else as pretty JSON. Returns the new revision.
///
/// Used by the step-7 decision commands (`propose`, `accept`, `reject`) in
/// the CLI and over MCP, which edit the pristine plan and only write when
/// the merged copy passes `check`. Pass mutations (P3 proposals, P4 merges)
/// are never written.
pub fn write_plan_if_revision(
    file: &Path,
    expected: &str,
    doc: &PlanDoc,
) -> Result<String, LoadError> {
    let lock = PlanLock::acquire(file)?;
    let file = &lock.file;
    let actual = revision_of_file(file)?;
    if actual != expected {
        return Err(LoadError::Unwritable {
            path: file.display().to_string(),
            message: format!("revision conflict: expected {expected}, current {actual}"),
        });
    }
    replace_file(file, &serialize_plan(file, doc)?)?;
    revision_of_file(file)
}

fn revision_of_file(file: &Path) -> Result<String, LoadError> {
    let read = |path: &Path| {
        std::fs::read(path).map_err(|e| LoadError::Unreadable {
            path: path.display().to_string(),
            message: e.to_string(),
        })
    };
    let plan = read(file)?;
    let log = evidence_log(file);
    let evidence = if log.is_file() {
        Some(read(&log)?)
    } else {
        None
    };
    Ok(revision_of(&plan, evidence.as_deref()))
}

fn revision_of(plan: &[u8], evidence: Option<&[u8]>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(plan);
    if let Some(evidence) = evidence {
        hasher.update(b"\0evidence.jsonl\0");
        hasher.update(evidence);
    }
    hasher.finalize().to_hex().to_string()
}

fn atomic_replace(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = file.parent().unwrap_or(Path::new("."));
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let (temp, mut handle) = loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let temp = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), n));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    use std::io::Write as _;
    let result = std::fs::metadata(file)
        .map_or(Ok(()), |meta| handle.set_permissions(meta.permissions()))
        .and_then(|()| handle.write_all(bytes))
        .and_then(|()| handle.sync_all())
        .and_then(|()| std::fs::rename(&temp, file));
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

/// Exclusive write lock on a plan, held as an OS advisory lock on a
/// persistent hidden sibling of the real plan file (`.plan.toml.lock`). The
/// kernel releases it when the guard drops or the process dies, so a killed
/// writer never leaves the plan locked.
///
/// Hold it across load, validation and write so the write is based on what
/// was read.
pub struct PlanLock {
    file: PathBuf,
    _handle: std::fs::File,
}

impl PlanLock {
    /// Block until the write lock for the plan at `path` is held.
    pub fn acquire(path: &Path) -> Result<Self, LoadError> {
        let file = canonical_plan_file(path)?;
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let lock = file
            .parent()
            .unwrap_or(Path::new("."))
            .join(format!(".{name}.lock"));
        let handle = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock)
            .and_then(|handle| handle.lock().map(|()| handle))
            .map_err(|e| LoadError::Unwritable {
                path: lock.display().to_string(),
                message: format!("cannot acquire write lock: {e}"),
            })?;
        Ok(Self {
            file,
            _handle: handle,
        })
    }

    /// Append one record to the locked plan's `evidence.jsonl`, creating the
    /// log when it is missing. Returns the log, the record's 1-based line
    /// (counted as [`load`] counts) and the plan revision after the append.
    pub fn append_evidence(&self, record: &Evidence) -> Result<(PathBuf, u32, String), LoadError> {
        let (log, line) = append_evidence_line(&self.file, record)?;
        Ok((log, line, revision_of_file(&self.file)?))
    }
}

/// Parse one JSON [`Evidence`] per non-blank line, paired with its 1-based
/// line number.
fn parse_evidence_jsonl(path: &Path, text: &str) -> Result<Vec<(u32, Evidence)>, LoadError> {
    let display_path = path.display().to_string();
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

fn append_evidence_line(plan_file: &Path, record: &Evidence) -> Result<(PathBuf, u32), LoadError> {
    let log = evidence_log(plan_file);
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

/// Append one decision event beside the plan, refusing an event ID already in the log.
pub fn append_decision(
    plan_file: &Path,
    event: &DecisionEvent,
) -> Result<(PathBuf, u32), LoadError> {
    validate_event(&event.event, &event.objective)?;
    let mut ids = std::collections::BTreeSet::new();
    if !event.alternatives.iter().all(|a| ids.insert(a.id.as_str())) {
        return Err(event_error("alternative IDs must be unique"));
    }
    if !ids.contains(event.selected_alternative.as_str()) {
        return Err(event_error(
            "selected alternative is not listed in alternatives",
        ));
    }
    if event
        .baseline_alternative
        .as_deref()
        .is_some_and(|baseline| !ids.contains(baseline))
    {
        return Err(event_error(
            "baseline alternative is not listed in alternatives",
        ));
    }
    if event
        .compiler_recommendation
        .as_deref()
        .is_some_and(|recommendation| !ids.contains(recommendation))
    {
        return Err(event_error(
            "compiler recommendation is not listed in alternatives",
        ));
    }
    if event
        .initial_decision
        .as_deref()
        .is_some_and(|initial| !ids.contains(initial))
    {
        return Err(event_error(
            "initial decision is not listed in alternatives",
        ));
    }
    append_event(plan_file, "decisions.jsonl", &event.event.id, event)
}

/// Append one observed outcome event beside the plan, refusing an event ID already in the log.
pub fn append_outcome(plan_file: &Path, event: &OutcomeEvent) -> Result<(PathBuf, u32), LoadError> {
    validate_event(&event.event, &event.decision_id)?;
    if !read_decisions(plan_file)?
        .iter()
        .any(|decision| decision.event.id == event.decision_id)
    {
        return Err(event_error("outcome references an unknown decision ID"));
    }
    if event
        .confidence
        .is_some_and(|c| !c.is_finite() || !(0.0..=1.0).contains(&c))
    {
        return Err(event_error("outcome confidence must be in [0, 1]"));
    }
    append_event(plan_file, "outcomes.jsonl", &event.event.id, event)
}

/// Read decision records from the log adjacent to a plan.
pub fn read_decisions(plan_file: &Path) -> Result<Vec<DecisionEvent>, LoadError> {
    read_events(&event_log_path(plan_file, "decisions.jsonl"))
}

/// Read outcome records from the log adjacent to a plan.
pub fn read_outcomes(plan_file: &Path) -> Result<Vec<OutcomeEvent>, LoadError> {
    read_events(&event_log_path(plan_file, "outcomes.jsonl"))
}

/// Append one automatic compiler run measurement beside the decision/outcome logs.
///
/// Unlike decisions and outcomes, this log grows on every read command, so
/// the row is appended in one write without parsing earlier rows: append
/// cost stays constant and an unparsable earlier row never blocks recording.
pub fn append_compiler_run(
    plan_file: &Path,
    event: &CompilerRunEvent,
) -> Result<PathBuf, LoadError> {
    validate_event(&event.event, &event.operation)?;
    if !matches!(
        event.operation.as_str(),
        "check" | "compile" | "frontier" | "context"
    ) {
        return Err(event_error("unknown compiler run operation"));
    }
    let log = event_log_path(plan_file, "metrics.jsonl");
    let mut line = serde_json::to_string(event).map_err(|e| LoadError::Unparsable {
        path: log.display().to_string(),
        message: e.to_string(),
    })?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&log)
        .and_then(|mut f| {
            use std::io::{Read as _, Seek as _, Write as _};
            // A crash mid-write leaves a partial last line; start a new one
            // so this row is not glued onto it. Only the last byte is read,
            // keeping the append constant-cost.
            if f.seek(std::io::SeekFrom::End(0))? > 0 {
                let mut last = [0];
                f.seek(std::io::SeekFrom::End(-1))?;
                f.read_exact(&mut last)?;
                if last[0] != b'\n' {
                    line.insert(0, '\n');
                }
            }
            f.write_all(line.as_bytes())
        })
        .map_err(|e| LoadError::Unreadable {
            path: log.display().to_string(),
            message: e.to_string(),
        })?;
    Ok(log)
}

/// Critical path of the most recent recorded run of `plan_file` selected
/// under `objective` and `time_budget_ms`, skipping unparsable rows so one
/// bad row never hides earlier history. The log is
/// read backwards in chunks, so the cost follows the rows inspected rather
/// than the size of the ever-growing log.
pub fn previous_critical_path(
    plan_file: &Path,
    objective: &str,
    time_budget_ms: u64,
) -> Option<Vec<String>> {
    use std::io::{Read as _, Seek as _};
    let name = plan_file.file_name()?.to_string_lossy();
    const CHUNK: u64 = 64 * 1024;
    let mut file = std::fs::File::open(event_log_path(plan_file, "metrics.jsonl")).ok()?;
    let mut end = file.metadata().ok()?.len();
    let mut pending = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut chunk = vec![0; usize::try_from(end - start).ok()?];
        file.seek(std::io::SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.append(&mut pending);
        end = start;
        let complete = if start == 0 {
            0
        } else if let Some(newline) = chunk.iter().position(|&byte| byte == b'\n') {
            newline + 1
        } else {
            pending = chunk;
            continue;
        };
        let found = chunk[complete..]
            .rsplit(|&byte| byte == b'\n')
            .find_map(|line| {
                let run = serde_json::from_slice::<CompilerRunEvent>(line).ok()?;
                (run.plan_file == name
                    && run.selection_objective.as_deref() == Some(objective)
                    && run.selection_time_budget_ms == Some(time_budget_ms))
                .then_some(run.critical_path_tasks)?
            });
        if found.is_some() {
            return found;
        }
        chunk.truncate(complete);
        pending = chunk;
    }
    None
}

/// Read automatically captured compiler runs.
pub fn read_compiler_runs(plan_file: &Path) -> Result<Vec<CompilerRunEvent>, LoadError> {
    read_events(&event_log_path(plan_file, "metrics.jsonl"))
}

fn validate_event(event: &pc_ir::EventMetadata, reference: &str) -> Result<(), LoadError> {
    if event.id.trim().is_empty()
        || event.plan_revision.trim().is_empty()
        || event.source.trim().is_empty()
        || reference.trim().is_empty()
        || (event.tasks.is_empty() && event.capabilities.is_empty())
        || event.schema_version != 1
        || chrono::DateTime::parse_from_rfc3339(&event.timestamp).is_err()
    {
        return Err(event_error(
            "event requires an ID, RFC 3339 timestamp, plan revision, source, schema_version 1, and at least one task or capability reference",
        ));
    }
    Ok(())
}

fn event_error(message: &str) -> LoadError {
    LoadError::Unparsable {
        path: "decision/outcome event".to_string(),
        message: message.to_string(),
    }
}

fn event_log_path(plan_file: &Path, name: &str) -> PathBuf {
    plan_file.parent().unwrap_or(Path::new(".")).join(name)
}

fn append_event<T: serde::Serialize>(
    plan_file: &Path,
    name: &str,
    id: &str,
    event: &T,
) -> Result<(PathBuf, u32), LoadError> {
    let log = event_log_path(plan_file, name);
    let display = log.display().to_string();
    let existing = if log.is_file() {
        std::fs::read_to_string(&log).map_err(|e| LoadError::Unreadable {
            path: display.clone(),
            message: e.to_string(),
        })?
    } else {
        String::new()
    };
    for (index, line) in existing.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|e| LoadError::Unparsable {
                path: display.clone(),
                message: format!("line {}: {e}", index + 1),
            })?;
        if value.get("id").and_then(serde_json::Value::as_str) == Some(id) {
            return Err(LoadError::Unparsable {
                path: display,
                message: format!("duplicate event ID `{id}`"),
            });
        }
    }
    let line = u32::try_from(existing.lines().count() + 1).unwrap_or(u32::MAX);
    let mut bytes = if existing.is_empty() || existing.ends_with('\n') {
        String::new()
    } else {
        "\n".to_string()
    };
    bytes.push_str(
        &serde_json::to_string(event).map_err(|e| LoadError::Unparsable {
            path: display.clone(),
            message: e.to_string(),
        })?,
    );
    bytes.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(bytes.as_bytes())
        })
        .map_err(|e| LoadError::Unreadable {
            path: display.clone(),
            message: e.to_string(),
        })?;
    Ok((log, line))
}

fn read_events<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>, LoadError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| LoadError::Unreadable {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|e| LoadError::Unparsable {
                path: path.display().to_string(),
                message: format!("line {}: {e}", index + 1),
            })
        })
        .collect()
}
