//! Payloads and rendering for `scribe proposal`.

use super::common::console::Sink;
use super::common::detail::{Detail, emit_detail, field};
use super::common::record::{
    AuthorPayload, CitePayload, InstructionPayload, Timestamp, WarningPayload, admitted, envelope,
};
use super::common::table::{
    Cell, Result, cut, left, list_timestamp, one_line, plain_line, shown_id, table, timestamp,
};
use cw_cli::{Column, Mode, Role};
use scribe_core::{Proposal, Quote, ShortIds};
use serde::Serialize;
use std::fmt::{self, Write as _};
use std::io::Write;

/// The label an older claim's statement carries where a context would be
/// (2_design.md §15.3).
const OLDER_CLAIM: &str = "older claim";

/// Where `REASON` sits in [`proposal_columns`].
const PROPOSAL_REASON_COLUMN: usize = 6;

/// A verbatim span a proposal rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct QuotePayload {
    pub(crate) entry: String,
    pub(crate) text: String,
}

impl From<&Quote> for QuotePayload {
    fn from(quote: &Quote) -> Self {
        Self {
            entry: quote.entry.to_string(),
            text: quote.text.clone(),
        }
    }
}

/// One proposal as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProposalPayload {
    pub(crate) candidate: Option<scribe_core::EntryCandidate>,
    pub(crate) id: String,
    /// `supersedes`, `stale` or `candidate`.
    pub(crate) kind: &'static str,
    pub(crate) older: String,
    pub(crate) newer: Option<String>,
    /// Model text.
    pub(crate) reason: String,
    pub(crate) quotes: Vec<QuotePayload>,
    /// `open`, `approved` or `rejected`.
    pub(crate) state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) opened_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) settled_at: Option<Timestamp>,
    pub(crate) settled_by: Option<CitePayload>,
    /// Who opened it.
    pub(crate) author: AuthorPayload,
    /// Who settled it; `null` while open.
    pub(crate) settled_author: Option<AuthorPayload>,
    pub(crate) updated_by: Option<AuthorPayload>,
    /// Why scribe closed it without the user (`tag_retired`, `instructed`); `null` otherwise.
    pub(crate) closed_reason: Option<&'static str>,
    /// The user's checked instruction that settled this candidate; `null` otherwise.
    pub(crate) instruction: Option<InstructionPayload>,
}

impl From<&Proposal> for ProposalPayload {
    fn from(p: &Proposal) -> Self {
        Self {
            candidate: p.candidate.clone(),
            id: p.id.to_string(),
            kind: p.kind.as_str(),
            older: p.older.to_string(),
            newer: p.newer.as_ref().map(ToString::to_string),
            reason: p.reason.clone(),
            quotes: p.quotes.iter().map(QuotePayload::from).collect(),
            state: p.state.as_str(),
            opened_at: p.opened_at,
            settled_at: p.settled_at,
            settled_by: p.settled_by.as_ref().map(CitePayload::from),
            author: AuthorPayload::from(&p.author),
            settled_author: p.settled_author.as_ref().map(AuthorPayload::from),
            updated_by: p.updated_by.as_ref().map(AuthorPayload::from),
            closed_reason: p.closed_reason.map(scribe_core::ClosedReason::as_str),
            instruction: p.instruction.as_ref().map(InstructionPayload::from),
        }
    }
}

/// Render a `proposal list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    items: &[ProposalPayload],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("proposals", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        let rows = items
            .iter()
            .flat_map(|p| proposal_rows(p, mode, shorts))
            .collect();
        table(mode, proposal_columns(), rows)
    })
}

/// Render a single proposal (`proposal show` and proposal write verbs).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    proposal: &ProposalPayload,
) -> Result {
    emit_detail(sink, proposal, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}",
            plain_line(&proposal_row(proposal, Mode::Plain, shorts))
        ),
        Mode::Table => proposal_detail(out, proposal),
    })
}

/// What a claim says, for the outputs (2_design.md §15.3):
/// its context paragraph, or an older claim's one-line statement labelled
/// [`OLDER_CLAIM`]; `None` for a claim opened before statements.
fn claim_context(candidate: &scribe_core::EntryCandidate) -> Option<String> {
    match (&candidate.context, &candidate.statement) {
        (Some(context), _) => Some(context.clone()),
        (None, Some(statement)) => Some(format!("{OLDER_CLAIM}: {statement}")),
        (None, None) => None,
    }
}

fn proposal_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("KIND"),
        left("STATE"),
        left("OLDER"),
        left("NEWER"),
        left("OPENED"),
        cut("REASON"),
        cut("ADMITTED"),
        left("CLOSED_REASON"),
    ]
}

/// A proposal's rows: one, and for a candidate carrying scribe's
/// context (or an older claim's statement), in the table, a second under it holding the user's quote in
/// the `REASON` column (2_design.md §11.1).
fn proposal_rows(p: &ProposalPayload, mode: Mode, shorts: &ShortIds) -> Vec<Vec<Cell>> {
    let mut rows = vec![proposal_row(p, mode, shorts)];
    if mode == Mode::Table
        && let Some(quote) = stated_candidate_quote(p)
    {
        let mut under: Vec<Cell> = proposal_columns()
            .iter()
            .map(|_| Cell::plain(String::new()))
            .collect();
        if let Some(cell) = under.get_mut(PROPOSAL_REASON_COLUMN) {
            *cell = Cell {
                text: quote,
                role: Role::Muted,
            };
        }
        rows.push(under);
    }
    if mode == Mode::Table
        && let Some(line) = settled_line(p, None)
    {
        let mut under: Vec<Cell> = proposal_columns()
            .iter()
            .map(|_| Cell::plain(String::new()))
            .collect();
        if let Some(cell) = under.get_mut(PROPOSAL_REASON_COLUMN) {
            *cell = Cell {
                text: line,
                role: Role::Muted,
            };
        }
        rows.push(under);
    }
    rows
}

/// `The user's words: "<quote>"` (or the assistant's) for a candidate
/// carrying scribe's context or an older claim's statement.
fn stated_candidate_quote(p: &ProposalPayload) -> Option<String> {
    let candidate = p.candidate.as_ref()?;
    candidate.stated()?;
    Some(format!(
        "{} \"{}\"",
        scribe_core::voice::words_label(&scribe_core::voice::UserName::default(), candidate.role),
        one_line(&candidate.quote)
    ))
}

/// A proposal's line. A candidate carrying scribe's context shows the
/// context (an older claim its labelled statement) as its reason; the plain
/// line adds the speaker's quote after it, where the table puts it on the
/// row under.
fn proposal_row(p: &ProposalPayload, mode: Mode, shorts: &ShortIds) -> Vec<Cell> {
    let reason = match (
        p.candidate.as_ref().and_then(claim_context),
        stated_candidate_quote(p),
    ) {
        (Some(context), Some(quote)) if mode != Mode::Table => {
            format!("{} | {quote}", one_line(&context))
        }
        (Some(context), _) => one_line(&context),
        (None, _) => one_line(&p.reason),
    };
    // The plain line carries the instruction after the reason, where the
    // table puts it on a row under (2_design.md §12.3).
    let reason = match settled_line(p, (mode == Mode::Plain).then_some(shorts)) {
        Some(line) if mode != Mode::Table => format!("{reason} | {line}"),
        _ => reason,
    };
    vec![
        Cell::plain(p.id.clone()),
        Cell::plain(p.kind),
        Cell {
            text: p.state.to_owned(),
            role: if p.state == "open" {
                Role::Warn
            } else {
                Role::Muted
            },
        },
        Cell::plain(p.older.clone()),
        Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        Cell::plain(list_timestamp(p.opened_at, mode)),
        Cell::plain(reason),
        Cell::plain(
            p.candidate
                .as_ref()
                .and_then(|c| c.admitted.as_ref())
                .map_or_else(|| String::from("-"), |a| one_line(&admitted(a))),
        ),
        Cell::plain(p.closed_reason.unwrap_or("-")),
    ]
}

/// How a candidate settled on the user's checked instruction reads:
/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`,
/// or `rejected by …`; `None` for one they settled by typing, or still open.
fn settled_line(p: &ProposalPayload, shorts: Option<&ShortIds>) -> Option<String> {
    let inst = p.instruction.as_ref()?;
    let by = p.settled_author.as_ref()?;
    Some(scribe_core::voice::instructed_line(
        p.state,
        &by.name,
        scribe_core::voice::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &shown_id(&inst.session, shorts),
        inst.at,
    ))
}

fn proposal_detail(out: &mut Detail, p: &ProposalPayload) -> fmt::Result {
    field(out, "id", &p.id)?;
    field(out, "kind", p.kind)?;
    field(out, "state", p.state)?;
    field(out, "older", &p.older)?;
    if let Some(candidate) = &p.candidate {
        if let Some(context) = claim_context(candidate) {
            field(out, "subject", candidate.subject.as_deref().unwrap_or("-"))?;
            field(out, "context", &context)?;
        }
        field(out, "quote", &candidate.quote)?;
        field(out, "tag", candidate.tag.as_str())?;
        field(out, "session", candidate.cite.session.as_str())?;
        field(
            out,
            "message",
            candidate.cite.message.as_deref().unwrap_or("-"),
        )?;
        // An open claim says how to settle it, so it is settled
        // from here without opening its session (2_design.md §15.3).
        if p.state == "open" {
            field(
                out,
                "settle",
                &format!("#scribe approve {0} or #scribe reject {0}", p.id),
            )?;
        }
    }
    field(out, "newer", p.newer.as_deref().unwrap_or("-"))?;
    field(out, "reason", &p.reason)?;
    field(
        out,
        "author",
        &format!("{}:{}", p.author.kind, p.author.name),
    )?;
    field(
        out,
        "updated_by",
        &p.updated_by
            .as_ref()
            .map_or_else(|| "-".to_owned(), |a| format!("{}:{}", a.kind, a.name)),
    )?;
    field(
        out,
        "settled_author",
        &p.settled_author
            .as_ref()
            .map_or_else(|| "-".to_owned(), |a| format!("{}:{}", a.kind, a.name)),
    )?;
    if let Some(reason) = p.closed_reason {
        field(out, "closed_reason", reason)?;
    }
    if let Some(line) = settled_line(p, None) {
        field(out, "instruction", &line)?;
    }
    field(out, "opened", &timestamp(p.opened_at))?;
    field(
        out,
        "settled",
        &p.settled_at.map_or_else(|| "-".to_owned(), timestamp),
    )
}
