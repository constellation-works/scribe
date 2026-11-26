//! Payloads and rendering for `scribe mark`.

use super::common::console::Sink;
use super::common::record::{WarningPayload, envelope};
use super::common::table::{
    Cell, Result, cut, left, list_timestamp, right, shown_id, table, table_shorts,
};
use cw_cli::{Column, Mode};
use scribe_core::ShortIds;
use std::io::Write;

/// Render a `mark list` or `mark retry` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[scribe_core::annotation::MarkRecord],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("marks", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        let rows = items
            .iter()
            .map(|m| mark_row(m, mode, table_shorts(mode, shorts)))
            .collect();
        table(mode, mark_columns(), rows)
    })
}

fn mark_columns() -> Vec<Column> {
    vec![
        left("SESSION"),
        cut("MESSAGE"),
        right("MARKER"),
        left("OUTCOME"),
        left("CODE"),
        right("ATTEMPTS"),
        left("AT"),
    ]
}

fn mark_row(
    m: &scribe_core::annotation::MarkRecord,
    mode: Mode,
    shorts: Option<&ShortIds>,
) -> Vec<Cell> {
    vec![
        Cell::plain(shown_id(m.session.as_str(), shorts)),
        Cell::plain(m.message.clone()),
        Cell::plain(m.marker.to_string()),
        Cell::plain(m.outcome.as_str().to_owned()),
        Cell::plain(m.code.clone().unwrap_or_else(|| String::from("-"))),
        Cell::plain(m.attempts.to_string()),
        Cell::plain(list_timestamp(m.at, mode)),
    ]
}
