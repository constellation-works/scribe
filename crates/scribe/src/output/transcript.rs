//! Payloads and rendering for `scribe transcript`.

use super::common::console::Sink;
use super::common::detail::{Detail, emit_detail, field};
use super::common::record::{Timestamp, WarningPayload, envelope};
use super::common::table::{
    Cell, Result, cut, left, list_shorts, one_line, optional_list_timestamp, plain_line, right,
    shown_id, table, timestamp,
};
use cw_cli::{Column, Mode};
use scribe_core::ShortIds;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::io::Write;

/// One uncataloged transcript (`transcript list` / `transcript show`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TranscriptPayload {
    /// The transcript's id.
    pub(crate) session: String,
    /// The attributed project slug.
    pub(crate) project: String,
    /// Mentions of each configured project's paths. Every configured project
    /// is present.
    pub(crate) project_mentions: BTreeMap<String, u32>,
    /// The transcript's cwd, empty when it names none.
    pub(crate) cwd: String,
    /// The first kept message. `null` when the file has none.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) started: Option<Timestamp>,
    /// The last kept message.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) ended: Option<Timestamp>,
    /// The file's length.
    pub(crate) bytes: u64,
    /// The user's turns, by the pass's rule.
    pub(crate) user_turns: u32,
    /// Their first turn, cut to 300 characters. Empty when they have none.
    pub(crate) first_prompt: String,
    /// His first five turns, each cut to 300 characters. Present on
    /// `transcript show` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_first_turns: Option<Vec<String>>,
}

/// Render a `transcript list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[TranscriptPayload],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("transcripts", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        let list_ids = list_shorts(mode, shorts);
        let rows = items
            .iter()
            .map(|t| transcript_row(t, mode, list_ids.as_deref()))
            .collect();
        table(mode, transcript_columns(), rows)
    })
}

/// Render a single transcript (`transcript show`).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    transcript: &TranscriptPayload,
) -> Result {
    emit_detail(sink, transcript, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => {
            writeln!(
                out,
                "{}",
                plain_line(&transcript_row(transcript, Mode::Plain, Some(shorts)))
            )?;
            if let Some(turns) = &transcript.user_first_turns {
                for (n, turn) in turns.iter().enumerate() {
                    let index = n.saturating_add(1);
                    writeln!(out, "turn\t{index}\t{}", one_line(turn))?;
                }
            }
            Ok(())
        }
        Mode::Table => transcript_detail(out, transcript),
    })
}

fn transcript_columns() -> Vec<Column> {
    vec![
        left("SESSION"),
        left("PROJECT"),
        right("USER_TURNS"),
        right("BYTES"),
        left("STARTED"),
        left("ENDED"),
        cut("FIRST_PROMPT"),
    ]
}

fn transcript_row(t: &TranscriptPayload, mode: Mode, shorts: Option<&ShortIds>) -> Vec<Cell> {
    vec![
        Cell::plain(shown_id(&t.session, shorts)),
        Cell::plain(t.project.clone()),
        Cell::plain(t.user_turns.to_string()),
        Cell::plain(t.bytes.to_string()),
        Cell::plain(optional_list_timestamp(t.started, mode)),
        Cell::plain(optional_list_timestamp(t.ended, mode)),
        Cell::plain(one_line(&t.first_prompt)),
    ]
}

fn transcript_detail(out: &mut Detail, t: &TranscriptPayload) -> fmt::Result {
    field(out, "session", &t.session)?;
    field(out, "project", &t.project)?;
    let mentions = if t.project_mentions.is_empty() {
        String::from("-")
    } else {
        t.project_mentions
            .iter()
            .map(|(project, count)| format!("{project} {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    field(out, "project_mentions", &mentions)?;
    field(out, "cwd", &one_line(&t.cwd))?;
    field(
        out,
        "started",
        &t.started
            .map(timestamp)
            .unwrap_or_else(|| String::from("-")),
    )?;
    field(
        out,
        "ended",
        &t.ended.map(timestamp).unwrap_or_else(|| String::from("-")),
    )?;
    field(out, "bytes", &t.bytes.to_string())?;
    field(out, "user_turns", &t.user_turns.to_string())?;
    field(out, "first_prompt", &t.first_prompt)?;
    if let Some(turns) = &t.user_first_turns {
        for (n, turn) in turns.iter().enumerate() {
            field(out, &format!("turn {}", n.saturating_add(1)), turn)?;
        }
    }
    Ok(())
}
