//! Payloads and rendering for `scribe group`.

use super::common::console::Sink;
use super::common::detail::{emit_detail, field};
use super::common::record::{WarningPayload, envelope};
use super::common::table::{
    Cell, Result, cut, left, one_line, or_dash, plain_line, right, shown_cite, shown_id, table,
    table_date_string,
};
use cw_cli::{Column, Mode};
use scribe_core::ShortIds;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write;

/// The full field list: `group show` labels and the tab-separated line.
const GROUP_COLUMNS: [&str; 10] = [
    "ID", "TITLE", "KIND", "PROJECT", "OPENED", "CLOSED", "OPENER", "RELATED", "SEATS", "SUMMARY",
];

/// Result of backfilling the current holders of a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GroupBackfill {
    pub(crate) group: String,
    pub(crate) members: Vec<GroupBackfillMember>,
    pub(crate) stored: usize,
    pub(crate) skipped: usize,
}

/// One seat's backfill counts in [`GroupBackfill`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GroupBackfillMember {
    pub(crate) seat: String,
    pub(crate) session: String,
    pub(crate) stored: usize,
    pub(crate) skipped: usize,
    pub(crate) transcript_unavailable: bool,
}

/// Render a `group list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[scribe_core::group::Group],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("items", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        // The table holds short values only; the tab-separated form keeps
        // every field, and `group show` and `--json` carry them all.
        let (columns, rows) = if mode == Mode::Table {
            (
                group_list_columns(),
                items.iter().map(group_list_row).collect(),
            )
        } else {
            (
                GROUP_COLUMNS.iter().map(|label| left(label)).collect(),
                items.iter().map(|g| group_row(g, Some(shorts))).collect(),
            )
        };
        table(mode, columns, rows)
    })
}

/// Render a single group (`group show` and group write verbs).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    group: &scribe_core::group::Group,
) -> Result {
    emit_detail(sink, group, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(out, "{}", plain_line(&group_row(group, Some(shorts)))),
        Mode::Table => {
            for (label, cell) in GROUP_COLUMNS
                .iter()
                .take(GROUP_COLUMNS.len() - 1)
                .zip(group_row(group, Some(shorts)))
            {
                field(out, &label.to_ascii_lowercase(), &cell.text)?;
            }
            if let Some(summary) = &group.summary {
                field(out, "summary", summary)?;
            }
            Ok(())
        }
    })
}

/// Render bounded evidence for a group close (always JSON).
pub(crate) fn emit_evidence<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    value: &serde_json::Value,
) -> Result {
    sink.emit_text(value, |_| String::new())
}

/// Render the result of a group backfill.
pub(crate) fn emit_backfill<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &GroupBackfill,
) -> Result {
    sink.emit_text(report, |report| {
        let mut text = String::new();
        for member in &report.members {
            text.push_str(&format!(
                "{} {}: {} stored, {} skipped{}\n",
                member.seat,
                shown_id(&member.session, Some(shorts)),
                member.stored,
                member.skipped,
                if member.transcript_unavailable {
                    " (transcript unavailable)"
                } else {
                    ""
                }
            ));
        }
        text.push_str(&format!(
            "Total: {} stored, {} skipped\n",
            report.stored, report.skipped
        ));
        text
    })
}

/// The `group list` table: short values only (STD-01 R15). The opener,
/// related references, seat chains and summary are in `group show`.
fn group_list_columns() -> Vec<Column> {
    vec![
        left("ID"),
        cut("TITLE"),
        left("KIND"),
        left("PROJECT"),
        left("OPENED"),
        left("CLOSED"),
        right("SEATS"),
    ]
}

fn group_list_row(g: &scribe_core::group::Group) -> Vec<Cell> {
    [
        g.id.clone(),
        one_line(&g.title),
        one_line(&g.kind),
        g.project.clone(),
        table_date_string(&g.opened),
        g.closed
            .as_deref()
            .map(table_date_string)
            .unwrap_or_else(|| "-".into()),
        g.seats.len().to_string(),
    ]
    .into_iter()
    .map(Cell::plain)
    .collect()
}

/// A group's full field list; `shorts` prints each seat's sessions as short
/// ids, for a terminal.
fn group_row(g: &scribe_core::group::Group, shorts: Option<&ShortIds>) -> Vec<Cell> {
    let seats = g
        .seats
        .iter()
        .map(|s| {
            let chain = s
                .transcripts
                .iter()
                .map(|id| shown_id(id.as_str(), shorts))
                .collect::<Vec<_>>()
                .join(" -> ");
            format!(
                "{} ({}): {} [handoff: {}]",
                s.name,
                one_line(&s.role),
                chain,
                s.newest_handoff.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    [
        g.id.clone(),
        one_line(&g.title),
        one_line(&g.kind),
        g.project.clone(),
        g.opened.clone(),
        g.closed.clone().unwrap_or_else(|| "-".into()),
        shown_id(&g.opener, shorts),
        or_dash(
            &g.related
                .iter()
                .map(|r| {
                    let cite = r.to_string();
                    shorts.map_or(cite.clone(), |ids| shown_cite(&cite, ids))
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
        .to_owned(),
        or_dash(&seats).to_owned(),
        g.summary.as_deref().map_or_else(|| "-".into(), one_line),
    ]
    .into_iter()
    .map(Cell::plain)
    .collect()
}
