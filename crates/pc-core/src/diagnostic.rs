//! Compiler diagnostics: file-anchored errors with stable codes.
//!
//! Every P1/P2 failure is a [`Diagnostic`] with a code in the `E000`-`E015`
//! range (see `docs/passes.md`). Codes are stable: negative fixtures assert
//! them and the CLI surfaces them in both human and `--json` output.

use serde::{Deserialize, Serialize};

/// A single compiler error, always anchored to a file and a best-effort line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable code, e.g. `"E008"`. See `docs/passes.md`.
    pub code: String,
    /// Human-readable explanation naming the subject and the fix direction.
    pub message: String,
    /// Display path of the plan file the diagnostic came from.
    pub file: String,
    /// 1-based line number, when the source text lets us locate the subject.
    pub line: Option<u32>,
    /// The entity the diagnostic is about, e.g. `method:voz-mfa`.
    pub subject: String,
}

impl Diagnostic {
    /// Build an error diagnostic.
    #[must_use]
    pub fn error(
        code: &str,
        message: String,
        file: &str,
        line: Option<u32>,
        subject: String,
    ) -> Self {
        Self {
            code: code.to_string(),
            message,
            file: file.to_string(),
            line,
            subject,
        }
    }
}

/// Render `error[E008]: message` plus an optional `  --> file:line` footer.
#[must_use]
pub fn render_human(diag: &Diagnostic) -> String {
    let mut out = format!("error[{}]: {}", diag.code, diag.message);
    match diag.line {
        Some(line) => out.push_str(&format!("\n  --> {}:{line}", diag.file)),
        None => out.push_str(&format!("\n  --> {}", diag.file)),
    }
    out
}
