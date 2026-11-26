//! Payloads and rendering for `scribe search`.

use super::common::console::Sink;
use super::common::record::{
    AuthorPayload, Envelope, Timestamp, WarningPayload, date, envelope, strings,
};
use super::common::table::{
    Cell, Result, cut, left, list_timestamp, one_line, right, shown_cite, table,
};
use cw_cli::{Column, Mode, Role};
use scribe_core::ShortIds;
use serde::Serialize;
use std::io::Write;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The widest title a hit's table row shows, in display columns.
const TITLE_CELL_COLUMNS: usize = 40;

/// One `scribe search` hit (docs/design/scribe/specs/output-contract.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct HitPayload {
    /// `entry`, `session` or `message`.
    pub(crate) kind: &'static str,
    /// What `ask` would cite: an entry id, `session:<id>` or `session:<id>#<n>`
    /// (a message cites its session).
    pub(crate) cite: String,
    /// The entry's or the session's id.
    pub(crate) id: String,
    /// The segment's number, or `null` for an entry or a session header.
    pub(crate) segment: Option<u32>,
    /// A stored message's provider id; `null` for any other kind.
    pub(crate) message: Option<String>,
    /// The BM25 score times the unit's weight, to three decimals.
    pub(crate) score: serde_json::Number,
    pub(crate) title: String,
    /// Up to 240 characters around the first matching word, on one line.
    pub(crate) snippet: String,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    /// An entry's author; `null` for a session.
    pub(crate) author: Option<AuthorPayload>,
    /// Whether the session is active; `null` for an entry.
    pub(crate) active: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    /// The entry's status (`active`, `superseded`, `retired`) or the
    /// session's (`open`, `final`); added beside the designed fields.
    pub(crate) status: &'static str,
    /// A finding entry's evidence pointer; `null` otherwise.
    pub(crate) evidence: Option<String>,
    /// The date a finding's result was true, `YYYY-MM-DD`; `null` otherwise.
    pub(crate) as_of: Option<String>,
}

impl From<&scribe_core::search::Hit> for HitPayload {
    fn from(hit: &scribe_core::search::Hit) -> Self {
        Self {
            kind: hit.kind.as_str(),
            cite: hit.cite.clone(),
            id: hit.id.clone(),
            segment: hit.segment,
            message: hit.message.clone(),
            score: serde_json::Number::from_f64(hit.score)
                .unwrap_or_else(|| serde_json::Number::from(0)),
            title: hit.title.clone(),
            snippet: hit.snippet.clone(),
            project: hit.project.to_string(),
            topics: strings(&hit.topics),
            author: hit.author.as_ref().map(AuthorPayload::from),
            active: hit.active,
            updated: hit.updated,
            status: hit.status,
            evidence: hit.evidence.clone(),
            as_of: hit.as_of.map(date),
        }
    }
}

/// Render a `scribe search` hit list.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[HitPayload],
    total: usize,
    warnings: &[WarningPayload],
    message_matches: Option<usize>,
) -> Result {
    let json = Envelope {
        message_matches,
        ..envelope("hits", items, total, warnings)
    };
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        let rows = items.iter().map(|h| hit_row(h, mode, shorts)).collect();
        table(mode, hit_columns(), rows)
    })
}

fn hit_columns() -> Vec<Column> {
    vec![
        left("CITE"),
        left("KIND"),
        right("SCORE"),
        left("UPDATED"),
        left("TITLE"),
        cut("SNIPPET"),
    ]
}

/// A hit's row, the same in the table and the plain line, except that the
/// table cuts the title to [`TITLE_CELL_COLUMNS`] so the snippet keeps room.
/// A hidden record's cite is muted in the table.
fn hit_row(h: &HitPayload, mode: Mode, shorts: &ShortIds) -> Vec<Cell> {
    let title = one_line(&h.title);
    let hidden = h.active == Some(false) || !matches!(h.status, "active" | "open" | "final");
    vec![
        Cell {
            text: if mode == Mode::Plain {
                shown_cite(&h.cite, shorts)
            } else {
                h.cite.clone()
            },
            role: if hidden { Role::Muted } else { Role::Neutral },
        },
        Cell::plain(h.kind),
        Cell::plain(format!("{:.3}", h.score.as_f64().unwrap_or_default())),
        Cell::plain(list_timestamp(h.updated, mode)),
        Cell::plain(if mode == Mode::Table {
            cap_title(&title)
        } else {
            title
        }),
        Cell::plain(one_line(&h.snippet)),
    ]
}

/// A hit's title as its table row shows it: cut to [`TITLE_CELL_COLUMNS`]
/// display columns, ending in one `…` when anything was cut. The cap is
/// scribe's own so the snippet keeps room; fitting a terminal is the kit's.
fn cap_title(title: &str) -> String {
    if title.width() <= TITLE_CELL_COLUMNS {
        return title.to_owned();
    }
    let keep = TITLE_CELL_COLUMNS - 1;
    let mut used = 0;
    let mut cut = String::new();
    for c in title.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > keep {
            break;
        }
        used += w;
        cut.push(c);
    }
    cut.push('…');
    cut
}
