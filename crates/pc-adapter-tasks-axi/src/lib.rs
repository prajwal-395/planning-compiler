//! tasks-axi `backlog.md` import/export adapter.
//!
//! This crate owns the Firstmate integration point: it converts a tasks-axi
//! `backlog.md` file into Work IR ([`import_plan`]) and converts a plan back
//! into canonical `backlog.md` text ([`export_backlog`]), so a backlog
//! round-trips byte-identical through the IR.
//!
//! ## Mapping
//!
//! tasks-axi items map to IR tasks; `blocked-by:` edges map to `order_edges`
//! inside one accepted decompose method on an import root task
//! ([`ROOT_TASK_ID`]). One method holds every edge because P2 (E013)
//! requires order endpoints to be subtasks of the same method, which a
//! per-section method cannot guarantee for cross-section blockers.
//!
//! Backlog fields with no IR counterpart - section, repo, kind, priority,
//! holds, date gates and body - ride along as [`pc_ir::TasksAxiMeta`]
//! passthrough, whose values no pass reads. They are carried through
//! untouched, never refused and never dropped (captain's 2026-10-12 ruling
//! on pc-tasks-axi). `queued` and `in_flight` items both become `open`
//! tasks; the original section is what restores them on export. `done`
//! items become `done` tasks, which the compiler reads and never sets.
//!
//! ## Canonical form
//!
//! The parser accepts what tasks-axi 0.2.6 reads and the renderer emits what
//! its `render` writes: a `# Backlog` header, `## <section>` sections in
//! file order, `- [ ] `/`- [x] ` task lines as
//! `<id> - <title>` plus `blocked-by: <id>` tokens plus trailing
//! `(key: value)` parentheticals in the order repo, kind, priority, date,
//! hold, hold-kind, hold-until, then two-space-indented body lines. Closed
//! dates render as `(merged <date>)` when the title carries a PR link,
//! `(reported <date>)` when it carries a report path, and `(done <date>)`
//! otherwise; the parser accepts all three spellings and export relabels
//! the date from the title. Anything outside that shape - free-form lines,
//! an unknown header, a dangling blocker, an order cycle - is refused
//! loudly with the offending line or id, never silently dropped.
//!
//! Out of scope: Beads import, changes to tasks-axi itself, and
//! firstmate-side code.

use pc_ir::{
    AcceptanceCriterion, AcceptanceKind, IR_VERSION, Method, MethodKind, MethodStatus, Objective,
    OrderEdge, PlanDoc, PlanMeta, Task, TaskState, TasksAxiMeta,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Task id of the import root created by [`import_plan`].
pub const ROOT_TASK_ID: &str = "backlog-root";
/// Method id of the single decompose method created by [`import_plan`].
pub const ROOT_METHOD_ID: &str = "backlog-decompose";

/// One parsed backlog item: the task line plus its body.
///
/// `blocked_by` preserves line order; every other field without an IR
/// counterpart is passthrough that [`import_plan`] copies verbatim into
/// [`pc_ir::TasksAxiMeta`] and [`export_backlog`] renders back untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklogItem {
    /// Stable item id.
    pub id: String,
    /// Human title, verbatim (may contain links and parentheses).
    pub title: String,
    /// Checkbox state: `true` for `- [x] `, `false` for `- [ ] `.
    pub done: bool,
    /// `blocked-by:` targets in line order.
    pub blocked_by: Vec<String>,
    /// Section the item lived in, e.g. `"Queued"`.
    pub section: String,
    /// `(repo: ...)` value, when present.
    pub repo: Option<String>,
    /// `(kind: ...)` value, when present.
    pub kind: Option<String>,
    /// `(priority: ...)` value, when present.
    pub priority: Option<i64>,
    /// `(since ...)` date, when present.
    pub created: Option<String>,
    /// Closed date from `(done ...)`/`(merged ...)`/`(reported ...)`.
    pub closed: Option<String>,
    /// `(hold: ...)` reason, when present.
    pub hold_reason: Option<String>,
    /// `(hold-kind: ...)` value, when present.
    pub hold_kind: Option<String>,
    /// `(hold-until: ...)` date gate, when present.
    pub hold_until: Option<String>,
    /// Body text with `\n` line breaks, without trailing newline.
    pub body: Option<String>,
}

/// A parsed backlog file.
///
/// `sections` holds section names in first-seen order; `items` holds items
/// in file order. Both orders are load-bearing: the renderer reproduces
/// them, which is what makes the round trip byte-identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlog {
    /// Section names in first-seen order.
    pub sections: Vec<String>,
    /// Items in file order.
    pub items: Vec<BacklogItem>,
}

/// Why [`parse_backlog`] refused its input.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    /// No usable header line exists.
    #[error("backlog is empty: expected a `# Backlog` header")]
    MissingHeader,
    /// The first non-blank line is not the header.
    #[error("backlog line {line}: expected `# Backlog` header, found `{found}`")]
    UnexpectedHeader {
        /// 1-based offending line.
        line: usize,
        /// Line content found.
        found: String,
    },
    /// A task line appears before any `## ` section.
    #[error("backlog line {line}: task appears before any `## ` section")]
    TaskBeforeSection {
        /// 1-based offending line.
        line: usize,
    },
    /// A task line is malformed; `reason` names the problem.
    #[error("backlog line {line}: {reason}")]
    BadLine {
        /// 1-based offending line.
        line: usize,
        /// What is wrong with the line.
        reason: String,
    },
    /// A line is neither blank, a section, a task, nor a body line.
    #[error(
        "backlog line {line}: unexpected line `{text}` (free-form lines are refused, never dropped)"
    )]
    UnexpectedLine {
        /// 1-based offending line.
        line: usize,
        /// Line content found.
        text: String,
    },
    /// Two items share one id.
    #[error("duplicate task id `{id}` (first at line {first}, again at line {second})")]
    DuplicateId {
        /// Repeated id.
        id: String,
        /// 1-based first occurrence.
        first: usize,
        /// 1-based second occurrence.
        second: usize,
    },
}

/// Parenthetical keys written as `(key: value)`.
const COLON_KEYS: &[&str] = &[
    "repo",
    "kind",
    "priority",
    "hold",
    "hold-kind",
    "hold-until",
];

/// Parenthetical keys written as `(key value)`: the closed-date labels.
const SPACE_KEYS: &[&str] = &["since", "merged", "done", "reported"];

/// Canonical trailing order, low to high: repo, kind, priority, date,
/// hold, hold-kind, hold-until. Stripping runs back to front, so stripped
/// ranks must not rise; a rise means the tail is title text, not metadata.
fn paren_rank(key: &str) -> u8 {
    match key {
        "repo" => 0,
        "kind" => 1,
        "priority" => 2,
        "since" | "merged" | "done" | "reported" => 3,
        "hold" => 4,
        "hold-kind" => 5,
        "hold-until" => 6,
        _ => 7,
    }
}

/// Split text into lines, tolerating `\r\n`.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// True for blank lines (whitespace only).
fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// A task id is usable when it is non-empty and holds no whitespace.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(char::is_whitespace)
}

/// A date gate or stamp is usable in `YYYY-MM-DD` shape.
fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// Strip one trailing `(key: value)` or `(key value)` parenthetical with a
/// known key.
///
/// Returns the key, value and remaining line, or `None` when the line does
/// not end with a well-formed known parenthetical (in which case the tail
/// belongs to the title, e.g. a title ending in `(see notes)`). Values are
/// not validated here; the caller absorbs invalid ones back into the title.
fn strip_known_paren(line: &str) -> Option<(&str, &str, &str)> {
    let trimmed = line.trim_end();
    if !trimmed.ends_with(')') {
        return None;
    }
    let open = trimmed.rfind('(')?;
    let inner = &trimmed[open + 1..trimmed.len() - 1];
    if let Some((key, value)) = inner.split_once(": ") {
        if !COLON_KEYS.contains(&key) {
            return None;
        }
        if value.is_empty() || value.contains('(') || value.contains(')') {
            return None;
        }
        return Some((key, value, trimmed[..open].trim_end()));
    }
    let (key, value) = inner.split_once(' ')?;
    if !SPACE_KEYS.contains(&key) {
        return None;
    }
    if value.is_empty() || value.contains(' ') || value.contains('(') || value.contains(')') {
        return None;
    }
    Some((key, value, trimmed[..open].trim_end()))
}

/// Split the post-id remainder into title plus `blocked-by:` targets.
///
/// The title is the text before the first `blocked-by:` token; each token
/// contributes one distinct blocker id, and only whitespace may separate
/// entries.
fn split_blocked_bys(line: &str, lineno: usize) -> Result<(String, Vec<String>), ParseError> {
    let bad = |reason: String| ParseError::BadLine {
        line: lineno,
        reason,
    };
    let mut title = String::new();
    let mut titled = false;
    let mut blockers = Vec::new();
    let mut rest = line;
    loop {
        match rest.find("blocked-by:") {
            None => {
                if !titled {
                    title = rest.trim().to_string();
                } else if !rest.trim().is_empty() {
                    return Err(bad(format!(
                        "unexpected text `{}` after blocked-by entries",
                        rest.trim()
                    )));
                }
                break;
            }
            Some(pos) => {
                let before = &rest[..pos];
                if !titled {
                    title = before.trim().to_string();
                    titled = true;
                } else if !before.trim().is_empty() {
                    return Err(bad(format!(
                        "unexpected text `{}` between blocked-by entries",
                        before.trim()
                    )));
                }
                let after = rest[pos + "blocked-by:".len()..].trim_start();
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                let id = &after[..end];
                if !valid_id(id) || id.contains('(') || id.contains(')') {
                    return Err(bad("expected a task id after `blocked-by:`".to_string()));
                }
                if blockers.iter().any(|known| known == id) {
                    return Err(bad(format!("duplicate `blocked-by: {id}`")));
                }
                blockers.push(id.to_string());
                rest = &after[end..];
            }
        }
    }
    if title.is_empty() {
        return Err(bad("task title is empty".to_string()));
    }
    Ok((title, blockers))
}

/// Parse one `- [ ] `/`- [x] ` task line (without its body).
fn parse_task_line(line: &str, lineno: usize) -> Result<BacklogItem, ParseError> {
    let bad = |reason: String| ParseError::BadLine {
        line: lineno,
        reason,
    };
    let (done, rest) = if let Some(tail) = line.strip_prefix("- [ ] ") {
        (false, tail)
    } else if let Some(tail) = line.strip_prefix("- [x] ") {
        (true, tail)
    } else if let Some(tail) = line.strip_prefix("- [X] ") {
        (true, tail)
    } else {
        return Err(bad("expected `- [ ] ` or `- [x] `".to_string()));
    };
    let (id, mut rest) = rest
        .split_once(" - ")
        .ok_or_else(|| bad("expected `<id> - <title>`".to_string()))?;
    if !valid_id(id) {
        return Err(bad(format!("bad task id `{id}`")));
    }
    let mut repo = None;
    let mut kind = None;
    let mut priority = None;
    let mut since = None;
    let mut closed = None;
    let mut hold = None;
    let mut hold_kind = None;
    let mut hold_until = None;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    // Strip trailing metadata back to front. Anything questionable - out of
    // canonical order, a repeated key, a non-canonical value, or a date that
    // contradicts the checkbox - stops the strip and stays in the title, so
    // the round trip still reproduces the line byte-identical instead of
    // misreading the tail as metadata. Left after a `blocked-by:` entry it
    // cannot be title text, so the line is refused loudly instead.
    let mut last_rank = u8::MAX;
    loop {
        let previous = rest;
        let Some((key, value, remaining)) = strip_known_paren(rest) else {
            break;
        };
        let mut accept = paren_rank(key) <= last_rank && seen.insert(key);
        if accept {
            accept = match key {
                "priority" => value
                    .parse::<i64>()
                    .is_ok_and(|parsed| parsed.to_string() == value),
                "since" | "merged" | "done" | "reported" => valid_date(value),
                _ => true,
            };
        }
        if accept && !done && matches!(key, "merged" | "done" | "reported") {
            accept = false;
        }
        if accept && done && key == "since" {
            accept = false;
        }
        if !accept {
            rest = previous;
            break;
        }
        match key {
            "repo" => repo = Some(value.to_string()),
            "kind" => kind = Some(value.to_string()),
            "priority" => priority = Some(value.parse::<i64>().unwrap_or_default()),
            "since" => since = Some(value.to_string()),
            "merged" | "done" | "reported" => closed = Some(value.to_string()),
            "hold" => hold = Some(value.to_string()),
            "hold-kind" => hold_kind = Some(value.to_string()),
            "hold-until" => hold_until = Some(value.to_string()),
            _ => {}
        }
        rest = remaining;
        last_rank = paren_rank(key);
    }
    if hold.is_none() && (hold_kind.is_some() || hold_until.is_some()) {
        return Err(bad(
            "`(hold-kind: ...)`/`(hold-until: ...)` without a `(hold: ...)` reason".to_string(),
        ));
    }
    let (title, blocked_by) = split_blocked_bys(rest, lineno)?;
    Ok(BacklogItem {
        id: id.to_string(),
        title,
        done,
        blocked_by,
        section: String::new(),
        repo,
        kind,
        priority,
        created: since,
        closed,
        hold_reason: hold,
        hold_kind,
        hold_until,
        body: None,
    })
}

/// Parse canonical tasks-axi `backlog.md` text.
///
/// Refuses anything outside the canonical shape loudly with the offending
/// line number; see the module docs for what counts as canonical.
pub fn parse_backlog(text: &str) -> Result<Backlog, ParseError> {
    let raw = split_lines(text);
    let mut idx = 0;
    while idx < raw.len() && is_blank(raw[idx]) {
        idx += 1;
    }
    let header = raw.get(idx).ok_or(ParseError::MissingHeader)?;
    if header.trim() != "# Backlog" {
        return Err(ParseError::UnexpectedHeader {
            line: idx + 1,
            found: header.to_string(),
        });
    }
    idx += 1;
    let mut backlog = Backlog {
        sections: Vec::new(),
        items: Vec::new(),
    };
    let mut id_lines: BTreeMap<String, usize> = BTreeMap::new();
    let mut current: Option<String> = None;
    while idx < raw.len() {
        let line = raw[idx];
        let lineno = idx + 1;
        if is_blank(line) {
            idx += 1;
            continue;
        }
        if let Some(name) = line.strip_prefix("## ") {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(ParseError::BadLine {
                    line: lineno,
                    reason: "section name is empty".to_string(),
                });
            }
            if !backlog.sections.contains(&name) {
                backlog.sections.push(name.clone());
            }
            current = Some(name);
            idx += 1;
            continue;
        }
        if line.starts_with("- [ ] ") || line.starts_with("- [x] ") || line.starts_with("- [X] ") {
            let section = current
                .clone()
                .ok_or(ParseError::TaskBeforeSection { line: lineno })?;
            let mut item = parse_task_line(line, lineno)?;
            if let Some(first) = id_lines.insert(item.id.clone(), lineno) {
                return Err(ParseError::DuplicateId {
                    id: item.id.clone(),
                    first,
                    second: lineno,
                });
            }
            item.section = section;
            let mut body_lines: Vec<String> = Vec::new();
            let mut next = idx + 1;
            while next < raw.len() {
                let body_line = raw[next];
                if is_blank(body_line) {
                    body_lines.push(String::new());
                } else if let Some(content) = body_line.strip_prefix("  ") {
                    body_lines.push(content.to_string());
                } else {
                    break;
                }
                next += 1;
            }
            while body_lines.last().is_some_and(String::is_empty) {
                body_lines.pop();
            }
            if !body_lines.is_empty() {
                item.body = Some(body_lines.join("\n"));
            }
            backlog.items.push(item);
            idx = next;
            continue;
        }
        return Err(ParseError::UnexpectedLine {
            line: lineno,
            text: line.to_string(),
        });
    }
    Ok(backlog)
}

/// True when the title carries a pull-request link.
///
/// Matches the canonical PR shapes tasks-axi itself writes: an `https://`
/// URL of `github.com/<owner>/<repo>/pull/<n>` or
/// `<host>/<owner>/<repo>/pulls/<n>`, ignoring trailing punctuation.
fn is_pr_link(token: &str) -> bool {
    let token = token
        .trim_start_matches('(')
        .trim_end_matches([',', '.', ';', ':', '!', '?', '"', '\'', ')']);
    let Some(rest) = token.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() != 4
        || segments[0].is_empty()
        || segments[1].is_empty()
        || segments[3].is_empty()
        || !segments[3].bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    if host == "github.com" {
        segments[2] == "pull"
    } else {
        segments[2] == "pulls"
    }
}

/// True when the title carries a report path (`data/.../report.md`).
fn is_report_link(token: &str) -> bool {
    let token = token
        .trim_start_matches('(')
        .trim_end_matches([',', '.', ';', ':', '!', '?', '"', '\'', ')']);
    let Some(inner) = token.strip_prefix("data/") else {
        return false;
    };
    match inner.rsplit_once('/') {
        Some((middle, file)) => !middle.is_empty() && file == "report.md",
        None => false,
    }
}

/// Closed-date label for a done task: PR links merge, report paths report.
fn closed_label(title: &str) -> &'static str {
    let mut report = false;
    for token in title.split_whitespace() {
        if is_pr_link(token) {
            return "merged";
        }
        if is_report_link(token) {
            report = true;
        }
    }
    if report { "reported" } else { "done" }
}

/// Render one task line (without its body) in canonical form.
fn render_task_line(item: &BacklogItem) -> String {
    let mut line = String::from(if item.done { "- [x] " } else { "- [ ] " });
    line.push_str(&item.id);
    line.push_str(" - ");
    line.push_str(&item.title);
    for blocker in &item.blocked_by {
        line.push_str(" blocked-by: ");
        line.push_str(blocker);
    }
    if let Some(repo) = &item.repo {
        line.push_str(" (repo: ");
        line.push_str(repo);
        line.push(')');
    }
    if let Some(kind) = &item.kind {
        line.push_str(" (kind: ");
        line.push_str(kind);
        line.push(')');
    }
    if let Some(priority) = item.priority {
        line.push_str(&format!(" (priority: {priority})"));
    }
    if item.done {
        if let Some(closed) = &item.closed {
            line.push_str(&format!(" ({} {closed})", closed_label(&item.title)));
        }
    } else if let Some(created) = &item.created {
        line.push_str(&format!(" (since {created})"));
    }
    if let Some(hold) = &item.hold_reason {
        line.push_str(" (hold: ");
        line.push_str(hold);
        line.push(')');
    }
    if let Some(hold_kind) = &item.hold_kind {
        line.push_str(" (hold-kind: ");
        line.push_str(hold_kind);
        line.push(')');
    }
    if let Some(hold_until) = &item.hold_until {
        line.push_str(" (hold-until: ");
        line.push_str(hold_until);
        line.push(')');
    }
    line
}

/// Render a parsed backlog in canonical form.
///
/// Sections render in first-seen order with their items in file order; an
/// item whose section names nothing in [`Backlog::sections`] still renders
/// under its own header, so the renderer never drops work.
pub fn render_backlog(backlog: &Backlog) -> String {
    let mut out = String::from("# Backlog\n\n");
    let mut emitted: BTreeSet<&str> = BTreeSet::new();
    for section in &backlog.sections {
        emitted.insert(section.as_str());
        out.push_str("## ");
        out.push_str(section);
        out.push('\n');
        render_section_items(&mut out, backlog, section);
    }
    for item in &backlog.items {
        if emitted.contains(item.section.as_str()) {
            continue;
        }
        emitted.insert(item.section.as_str());
        out.push_str("## ");
        out.push_str(&item.section);
        out.push('\n');
        render_section_items(&mut out, backlog, &item.section);
    }
    out
}

/// Render every item of one section with its body.
fn render_section_items(out: &mut String, backlog: &Backlog, section: &str) {
    for item in backlog.items.iter().filter(|item| item.section == section) {
        out.push_str(&render_task_line(item));
        out.push('\n');
        if let Some(body) = &item.body {
            for body_line in body.split('\n') {
                if body_line.is_empty() {
                    out.push('\n');
                } else {
                    out.push_str("  ");
                    out.push_str(body_line);
                    out.push('\n');
                }
            }
        }
    }
}

/// Why [`import_plan`] refused a parsed backlog.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImportError {
    /// The backlog holds no items.
    #[error("backlog has no tasks: nothing to import")]
    EmptyBacklog,
    /// Two items share one id (defensive: the parser already refuses this).
    #[error("duplicate task id `{id}`")]
    DuplicateId {
        /// Repeated id.
        id: String,
    },
    /// A `blocked-by:` target names nothing in the subset.
    #[error("task `{dependent}` is blocked by unknown task `{blocker}`: import a closed subset")]
    UnknownBlocker {
        /// Dependent item id.
        dependent: String,
        /// Missing blocker id.
        blocker: String,
    },
    /// An item id collides with the import root task.
    #[error("task id `{id}` collides with the import root task: rename it")]
    RootCollision {
        /// Colliding id.
        id: String,
    },
    /// The `blocked-by:` edges cycle, which P2 (E007) would refuse.
    #[error("blocked-by edges form a cycle: {path}")]
    OrderCycle {
        /// Cycle path joined with ` -> `.
        path: String,
    },
}

/// Find one order cycle over `from -> to` edges, if any.
fn find_cycle<'a>(
    adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
    ids: &[&'a str],
) -> Option<Vec<String>> {
    // 1 means on the current path, 2 means fully explored.
    let mut state: BTreeMap<&str, u8> = BTreeMap::new();
    for &start in ids {
        if state.get(start).is_some_and(|&flag| flag == 2) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(start, 0)];
        let mut path: Vec<&str> = vec![start];
        state.insert(start, 1);
        while let Some((node, child)) = stack.pop() {
            let children: &[&str] = adjacency.get(node).map(Vec::as_slice).unwrap_or(&[]);
            if child < children.len() {
                stack.push((node, child + 1));
                let next = children[child];
                match state.get(next) {
                    Some(1) => {
                        let from = path.iter().position(|&name| name == next).unwrap_or(0);
                        let mut cycle: Vec<String> = path[from..]
                            .iter()
                            .map(|name| (*name).to_string())
                            .collect();
                        cycle.push(next.to_string());
                        return Some(cycle);
                    }
                    Some(2) => {}
                    _ => {
                        state.insert(next, 1);
                        path.push(next);
                        stack.push((next, 0));
                    }
                }
            } else {
                state.insert(node, 2);
                path.pop();
            }
        }
    }
    None
}

/// Convert a parsed backlog into a plan document.
///
/// Items become tasks (open, or done from the checkbox) carrying their
/// unmappable fields as [`pc_ir::TasksAxiMeta`]; every `blocked-by:` edge
/// becomes an order edge in one accepted decompose method on
/// [`ROOT_TASK_ID`]. The result passes `pc check` or the import is refused
/// instead of writing a dirty plan.
pub fn import_plan(backlog: &Backlog) -> Result<PlanDoc, ImportError> {
    if backlog.items.is_empty() {
        return Err(ImportError::EmptyBacklog);
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for item in &backlog.items {
        if item.id == ROOT_TASK_ID {
            return Err(ImportError::RootCollision {
                id: item.id.clone(),
            });
        }
        if !seen.insert(item.id.as_str()) {
            return Err(ImportError::DuplicateId {
                id: item.id.clone(),
            });
        }
    }
    for item in &backlog.items {
        for blocker in &item.blocked_by {
            if !seen.contains(blocker.as_str()) {
                return Err(ImportError::UnknownBlocker {
                    dependent: item.id.clone(),
                    blocker: blocker.clone(),
                });
            }
        }
    }
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for item in &backlog.items {
        for blocker in &item.blocked_by {
            adjacency
                .entry(blocker.as_str())
                .or_default()
                .push(item.id.as_str());
        }
    }
    let ids: Vec<&str> = backlog.items.iter().map(|item| item.id.as_str()).collect();
    if let Some(cycle) = find_cycle(&adjacency, &ids) {
        return Err(ImportError::OrderCycle {
            path: cycle.join(" -> "),
        });
    }
    let mut tasks = Vec::with_capacity(backlog.items.len() + 1);
    tasks.push(Task {
        id: ROOT_TASK_ID.to_string(),
        title: "Backlog".to_string(),
        state: TaskState::Decomposed,
        consumes: Vec::new(),
        produces: Vec::new(),
        tasks_axi: None,
        beads_jsonl: None,
    });
    for item in &backlog.items {
        tasks.push(Task {
            id: item.id.clone(),
            title: item.title.clone(),
            state: if item.done {
                TaskState::Done
            } else {
                TaskState::Open
            },
            consumes: Vec::new(),
            produces: Vec::new(),
            tasks_axi: Some(TasksAxiMeta {
                section: item.section.clone(),
                repo: item.repo.clone(),
                kind: item.kind.clone(),
                priority: item.priority,
                created: item.created.clone(),
                closed: item.closed.clone(),
                hold_reason: item.hold_reason.clone(),
                hold_kind: item.hold_kind.clone(),
                hold_until: item.hold_until.clone(),
                body: item.body.clone(),
            }),
            beads_jsonl: None,
        });
    }
    let mut order_edges = Vec::new();
    for item in &backlog.items {
        for blocker in &item.blocked_by {
            order_edges.push(OrderEdge {
                from: blocker.clone(),
                to: item.id.clone(),
            });
        }
    }
    let doc = PlanDoc {
        ir_version: IR_VERSION,
        plan: PlanMeta {
            id: "backlog".to_string(),
            title: "Backlog".to_string(),
            description: "Imported from tasks-axi backlog.md; sections, holds, kinds and date gates ride along as tasks_axi passthrough.".to_string(),
            external_inputs: Vec::new(),
            tasks_axi_sections: backlog.sections.clone(),
        },
        objective: Objective {
            root_task: ROOT_TASK_ID.to_string(),
            weights: BTreeMap::from([("duration_minutes".to_string(), 1.0)]),
            budget_usd: None,
            deadline: None,
            quality_floor: None,
            acceptance: vec![AcceptanceCriterion {
                kind: AcceptanceKind::Manual,
                description: "The IR regenerates backlog.md byte-identical.".to_string(),
                command: None,
                metric: None,
                op: None,
                value: None,
            }],
        },
        artifact: Vec::new(),
        resource: Vec::new(),
        task: tasks,
        method: vec![Method {
            id: ROOT_METHOD_ID.to_string(),
            task: ROOT_TASK_ID.to_string(),
            kind: MethodKind::Decompose,
            subtasks: ids.iter().map(|id| (*id).to_string()).collect(),
            order_edges,
            capability: None,
            status: MethodStatus::Accepted,
            provenance: "agent:pc-adapter-tasks-axi".to_string(),
            assumptions: vec![
                "Blocked-by edges map to order edges; holds, kinds, repos, priorities, date gates and bodies ride along as tasks_axi passthrough the compiler ignores.".to_string(),
            ],
        }],
        capability: Vec::new(),
        evidence: Vec::new(),
    };
    Ok(doc)
}

/// Why [`export_backlog`] refused a plan.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExportError {
    /// The objective root names nothing in the plan.
    #[error("objective root task `{id}` names nothing in the plan")]
    MissingRoot {
        /// Missing root id.
        id: String,
    },
    /// A dropped task has no backlog counterpart.
    #[error(
        "task `{id}` is dropped: tasks-axi has no counterpart, un-drop it or remove it before export"
    )]
    DroppedTask {
        /// Dropped task id.
        id: String,
    },
}

/// Convert a plan document back into a backlog.
///
/// The objective root is structural and skipped; sections render in the
/// imported file order (so empty sections survive) with new item sections
/// appended in first-seen order. Every other task renders under its
/// passthrough section (`Queued` for tasks without passthrough).
/// `blocked-by:` lists rebuild from the non-rejected decompose methods in
/// plan order, so edits to the IR edges show up in the export. Dropped
/// tasks are refused loudly: tasks-axi has nowhere to put them.
pub fn export_backlog(doc: &PlanDoc) -> Result<Backlog, ExportError> {
    if !doc
        .task
        .iter()
        .any(|task| task.id == doc.objective.root_task)
    {
        return Err(ExportError::MissingRoot {
            id: doc.objective.root_task.clone(),
        });
    }
    let mut blockers: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for method in &doc.method {
        if method.status == MethodStatus::Rejected || method.kind != MethodKind::Decompose {
            continue;
        }
        for edge in &method.order_edges {
            let entry = blockers.entry(edge.to.as_str()).or_default();
            if !entry.iter().any(|known| known == &edge.from) {
                entry.push(edge.from.clone());
            }
        }
    }
    let mut backlog = Backlog {
        sections: Vec::new(),
        items: Vec::new(),
    };
    // File-order sections first, so empty sections survive the round trip;
    // item sections not listed there (new work) append in first-seen order.
    for section in &doc.plan.tasks_axi_sections {
        if !backlog.sections.contains(section) {
            backlog.sections.push(section.clone());
        }
    }
    for task in &doc.task {
        if task.id == doc.objective.root_task {
            continue;
        }
        if task.state == TaskState::Dropped {
            return Err(ExportError::DroppedTask {
                id: task.id.clone(),
            });
        }
        let meta = task.tasks_axi.clone().unwrap_or_default();
        let section = if meta.section.is_empty() {
            "Queued".to_string()
        } else {
            meta.section.clone()
        };
        if !backlog.sections.contains(&section) {
            backlog.sections.push(section.clone());
        }
        backlog.items.push(BacklogItem {
            id: task.id.clone(),
            title: task.title.clone(),
            done: task.state == TaskState::Done,
            blocked_by: blockers.get(task.id.as_str()).cloned().unwrap_or_default(),
            section,
            repo: meta.repo,
            kind: meta.kind,
            priority: meta.priority,
            created: meta.created,
            closed: meta.closed,
            hold_reason: meta.hold_reason,
            hold_kind: meta.hold_kind,
            hold_until: meta.hold_until,
            body: meta.body,
        });
    }
    Ok(backlog)
}
