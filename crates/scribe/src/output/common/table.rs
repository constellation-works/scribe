//! Shared table building and cell formatting helpers for list and report views.

use cw_cli::{Cell as KitCell, Column, Mode, Role, Table};
use scribe_core::ShortIds;
use std::borrow::Cow;
use time::format_description::well_known::{Iso8601, Rfc3339};
use time::{OffsetDateTime, UtcOffset};

/// The kit's result of a write; the binary's error wraps its failure.
pub(crate) type Result = cw_cli::Result<()>;

/// One cell of a row as scribe builds it: its text, which the plain line and
/// the detail view read back, and what its color means.
#[derive(Clone, Debug)]
pub(crate) struct Cell {
    /// The text; an absent value is `-`, never empty (STD-01 R14), except in a
    /// continuation row.
    pub(crate) text: String,
    /// Semantic color role.
    pub(crate) role: Role,
}

impl Cell {
    /// A cell with no emphasis.
    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: Role::Neutral,
        }
    }
}

pub(crate) fn left(header: &'static str) -> Column {
    Column::new(header)
}

pub(crate) fn right(header: &'static str) -> Column {
    Column::new(header).right()
}

pub(crate) fn cut(header: &'static str) -> Column {
    Column::new(header).truncatable()
}

/// The kit table for `rows`. In a table an empty cell is an absent value and
/// shows as `-` (STD-01 R14); a row whose first cell is empty continues the
/// row above it (a proposal's quote line), so its blanks stay blank.
pub(crate) fn table(mode: Mode, columns: Vec<Column>, rows: Vec<Vec<Cell>>) -> Table {
    let mut table = Table::new(columns);
    for row in rows {
        let continuation = row.first().is_some_and(|cell| cell.text.is_empty());
        table.push(row.into_iter().map(|cell| {
            let kit = if cell.text.is_empty() && mode == Mode::Table && !continuation {
                KitCell::absent()
            } else {
                KitCell::new(cell.text)
            };
            kit.role(cell.role)
        }));
    }
    table
}

/// Display names for non-JSON output; JSON retains provider ids.
pub(crate) fn table_shorts(mode: Mode, shorts: &ShortIds) -> Option<&ShortIds> {
    (mode != Mode::Json).then_some(shorts)
}

/// The ids a list renders for `mode`: prefix-only session ids in a table, the
/// display ids on other non-JSON output, and none in JSON (provider ids).
pub(crate) fn list_shorts(mode: Mode, shorts: &ShortIds) -> Option<Cow<'_, ShortIds>> {
    if mode == Mode::Table {
        Some(Cow::Owned(shorts.prefixes_only()))
    } else {
        table_shorts(mode, shorts).map(Cow::Borrowed)
    }
}

/// A session id in its display form when `shorts` is set.
pub(crate) fn shown_id(id: &str, shorts: Option<&ShortIds>) -> String {
    shorts.map_or_else(|| id.to_owned(), |s| s.display(id).to_owned())
}

/// Render a session citation without changing its segment suffix.
pub(crate) fn shown_cite(cite: &str, shorts: &ShortIds) -> String {
    let Some(session) = cite.strip_prefix("session:") else {
        return cite.to_owned();
    };
    let (id, suffix) = session
        .split_once('#')
        .map_or((session, ""), |(id, _)| (id, &session[id.len()..]));
    format!("session:{}{suffix}", shorts.display(id))
}

/// One tab-separated line, no header, no color (STD-01 R9). Free text is
/// flattened to one line by [`one_line`]; `--json` and `show` carry it
/// verbatim.
pub(crate) fn plain_line(cells: &[Cell]) -> String {
    let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
    texts.join("\t")
}

/// Free text on one line: every run of whitespace, tabs and line breaks
/// included, becomes one space, so a record never spans fields or lines.
/// Empty text is `-` (STD-01 R14).
pub(crate) fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        String::from("-")
    } else {
        flat
    }
}

pub(crate) fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

pub(crate) fn joined(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(",")
    }
}

pub(crate) fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

pub(crate) fn due_role(due: bool) -> Role {
    if due { Role::Warn } else { Role::Neutral }
}

pub(crate) fn timestamp(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| String::from("-"))
}

/// A human table's timestamp in table mode (on a terminal, or with
/// `--format table` on a pipe): its UTC date, `YYYY-MM-DD`. Sorting, plain
/// output and JSON use the full instant ([`timestamp`]).
/// A record's offset can put its UTC date outside the supported range, which
/// has no `YYYY-MM-DD`; the table shows `-` there rather than panicking.
pub(crate) fn table_date(at: OffsetDateTime) -> String {
    at.checked_to_offset(UtcOffset::UTC)
        .and_then(|utc| utc.date().format(&Iso8601::DATE).ok())
        .unwrap_or_else(|| String::from("-"))
}

pub(crate) fn list_timestamp(at: OffsetDateTime, mode: Mode) -> String {
    if mode == Mode::Table {
        table_date(at)
    } else {
        timestamp(at)
    }
}

pub(crate) fn optional_list_timestamp(at: Option<OffsetDateTime>, mode: Mode) -> String {
    at.map(|at| list_timestamp(at, mode))
        .unwrap_or_else(|| String::from("-"))
}

pub(crate) fn table_date_string(value: &str) -> String {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(table_date)
        .unwrap_or_else(|_| String::from("-"))
}
