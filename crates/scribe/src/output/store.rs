//! Payloads and rendering for `scribe store`.

use super::common::console::Sink;
use super::common::detail::{emit_detail, field};
use super::common::table::Result;
use cw_cli::Mode;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write;

/// `store migrate`: the archives imported into `scribe.db`. Field names
/// are a contract. Since phase 8 nothing moves from files, so the counts
/// of moved records (`sessions` through `pickups`, and `audit_torn`) are 0
/// and `moved_to` is `null`; they stay for the contract (STD-01 R10).
/// `audit_lines` still counts the spooled lines the write drained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MigratePayload {
    /// Session records imported.
    pub(crate) sessions: usize,
    /// Switches imported.
    pub(crate) switches: usize,
    /// Codex cursors imported.
    pub(crate) cursors: usize,
    /// Where the files went, under the data directory; `null` when there
    /// were none (always since phase 8).
    pub(crate) moved_to: Option<String>,
    /// Entries imported.
    pub(crate) entries: usize,
    /// Claims imported.
    pub(crate) claims: usize,
    /// Agent tags imported.
    pub(crate) tags: usize,
    /// Annotation marks imported.
    pub(crate) marks: usize,
    /// Pickups imported.
    pub(crate) pickups: usize,
    /// Spooled audit lines drained into the table.
    pub(crate) audit_lines: usize,
    /// Audit lines that did not parse (always 0 since phase 8).
    pub(crate) audit_torn: usize,
    /// `pre-sqlite-*` and `pre-issue-*` directories imported into the
    /// archive tables, then removed.
    pub(crate) archives: usize,
    /// Archived records imported: one per file, one per audit line.
    pub(crate) archived: usize,
}

/// Render a `store migrate` result.
pub(crate) fn show<O: Write, E: Write>(sink: &mut Sink<O, E>, report: &MigratePayload) -> Result {
    emit_detail(sink, report, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            report.sessions,
            report.switches,
            report.cursors,
            report.moved_to.as_deref().unwrap_or("-"),
            report.entries,
            report.claims,
            report.tags,
            report.marks,
            report.pickups,
            report.audit_lines,
            report.archives,
            report.archived
        ),
        Mode::Table => {
            field(out, "sessions", &report.sessions.to_string())?;
            field(out, "switches", &report.switches.to_string())?;
            field(out, "cursors", &report.cursors.to_string())?;
            field(out, "entries", &report.entries.to_string())?;
            field(out, "claims", &report.claims.to_string())?;
            field(out, "tags", &report.tags.to_string())?;
            field(out, "marks", &report.marks.to_string())?;
            field(out, "pickups", &report.pickups.to_string())?;
            field(out, "audit lines", &report.audit_lines.to_string())?;
            if report.audit_torn > 0 {
                field(out, "audit torn", &report.audit_torn.to_string())?;
            }
            field(out, "moved to", report.moved_to.as_deref().unwrap_or("-"))?;
            field(out, "archives", &report.archives.to_string())?;
            field(out, "archived", &report.archived.to_string())
        }
    })
}
