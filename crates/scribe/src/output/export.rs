//! Payloads and rendering for `scribe export`.

use super::common::console::Sink;
use super::common::detail::{emit_detail, field, or_none};
use super::common::table::Result;
use cw_cli::Mode;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write;

/// `export`: the files written. Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ExportPayload {
    /// The directory written into.
    pub(crate) dir: String,
    /// `markdown` or `jsonl`.
    pub(crate) format: &'static str,
    /// Sessions written.
    pub(crate) sessions: usize,
    /// Entries written.
    pub(crate) entries: usize,
    /// Claims written.
    pub(crate) claims: usize,
    /// Audit lines written.
    pub(crate) audit_lines: usize,
    /// Every file written, relative to `dir`, in order.
    pub(crate) files: Vec<String>,
    /// Records that did not read, so were not written.
    pub(crate) skipped: Vec<String>,
}

/// Render an `export` result.
pub(crate) fn show<O: Write, E: Write>(sink: &mut Sink<O, E>, report: &ExportPayload) -> Result {
    emit_detail(sink, report, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => {
            for file in &report.files {
                writeln!(out, "{file}")?;
            }
            Ok(())
        }
        Mode::Table => {
            field(out, "dir", &report.dir)?;
            field(out, "format", report.format)?;
            field(out, "sessions", &report.sessions.to_string())?;
            field(out, "entries", &report.entries.to_string())?;
            field(out, "claims", &report.claims.to_string())?;
            field(out, "audit lines", &report.audit_lines.to_string())?;
            field(out, "files", &report.files.len().to_string())?;
            field(out, "skipped", &or_none(&report.skipped))
        }
    })
}
