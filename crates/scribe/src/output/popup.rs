//! The popup's text: its list rows and its record previews (`scribe tmux`'s
//! hidden `rows` and `preview` modes, docs/design/scribe/2_design.md §1.2).
//!
//! A row is `kind<TAB>id<TAB>display`. fzf shows only the display field and
//! hands the first two to its preview, copy and Enter commands, so the id
//! rides on the row without being shown. Display columns are padded to the
//! widest value (capped per column) so they stay aligned with color off;
//! color only wraps the text, never the padding.
//!
//! fzf searches only the fields it shows, so what must be searchable but not
//! visible (topics) follows the last column after [`HIDE_PAD`] spaces and is
//! cut off by the popup's `--no-hscroll`.

use super::color::{self, Role};
use super::payload::{
    EntryPayload, PopupRows, ProposalPayload, Record, Records, SessionPayload, TranscriptPayload,
};
use super::render::{entry_metadata, session_log};
use std::io::{self, Write};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Spaces between the last visible column and the searchable-only text; wider
/// than any terminal, so the text never shows.
const HIDE_PAD: usize = 300;

/// Column gap.
const GAP: &str = "  ";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Plain,
    Ok,
    Muted,
}

struct Cell {
    text: String,
    cap: usize,
    style: Style,
}

impl Cell {
    fn new(text: &str, cap: usize, style: Style) -> Self {
        Self {
            text: flat(text),
            cap,
            style,
        }
    }
}

struct Row {
    id: String,
    cells: Vec<Cell>,
    /// Searchable, not displayed.
    search: String,
}

/// Whitespace runs become one space; nothing may span a field or a line.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first non-empty line, flattened.
fn first_line(text: &str) -> String {
    text.lines()
        .map(flat)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
}

fn cut(text: &str, cap: usize) -> String {
    if text.chars().count() <= cap {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(cap.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// `MM-DD`, or `YYYY-MM-DD` when the year is not `year`.
fn short_date(at: OffsetDateTime, year: i32) -> String {
    let (y, m, d) = (at.year(), u8::from(at.month()), at.day());
    if y == year {
        format!("{m:02}-{d:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

fn long_time(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| String::from("-"))
}

/// Write one list view: its rows, or its notice row.
pub(crate) fn rows(out: &mut dyn Write, popup: &PopupRows, color: bool) -> io::Result<()> {
    if let Some(notice) = &popup.notice {
        return writeln!(out, "{}\t-\t{}", popup.kind, flat(notice));
    }
    let rows = match &popup.records {
        Some(records) => build(records, popup.year),
        None => Vec::new(),
    };
    let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.cells.get(c))
                .map(|cell| cell.text.chars().count().min(cell.cap))
                .max()
                .unwrap_or(0)
        })
        .collect();
    for row in &rows {
        let mut line = String::new();
        for (c, cell) in row.cells.iter().enumerate() {
            if c > 0 {
                line.push_str(GAP);
            }
            let text = cut(&cell.text, cell.cap);
            let pad = widths[c].saturating_sub(text.chars().count());
            let role = match cell.style {
                Style::Plain => Role::Neutral,
                Style::Ok => Role::Ok,
                Style::Muted => Role::Muted,
            };
            line.push_str(&color::paint(&text, role, color));
            if c + 1 < row.cells.len() {
                line.push_str(&" ".repeat(pad));
            }
        }
        if !row.search.is_empty() {
            line.push_str(&" ".repeat(HIDE_PAD));
            line.push_str(&flat(&row.search));
        }
        writeln!(out, "{}\t{}\t{}", popup.kind, row.id, line)?;
    }
    Ok(())
}

fn build(records: &Records, year: i32) -> Vec<Row> {
    match records {
        Records::Sessions { items, .. } => sessions(items, year),
        Records::Entries(items) => entries(items, year),
        Records::Proposals(items) => proposals(items, year),
        Records::Transcripts(items) => transcripts(items, year),
        Records::SessionTree { .. } | Records::Tags(_) | Records::Hits(_) => Vec::new(),
    }
}

/// Newest first; a stable sort keeps the list's order among equals.
fn sessions(items: &[SessionPayload], year: i32) -> Vec<Row> {
    let mut items: Vec<&SessionPayload> = items.iter().collect();
    items.sort_by_key(|s| std::cmp::Reverse(s.started));
    items
        .into_iter()
        .map(|s| {
            let dim = !s.active;
            let style = |on: Style| if dim { Style::Muted } else { on };
            let title = if s.title.trim().is_empty() {
                "(untitled)"
            } else {
                &s.title
            };
            Row {
                id: s.id.clone(),
                cells: vec![
                    Cell::new(&short_date(s.started, year), 10, style(Style::Plain)),
                    Cell::new(
                        s.status,
                        5,
                        style(if s.status == "open" {
                            Style::Ok
                        } else {
                            Style::Muted
                        }),
                    ),
                    Cell::new(title, 70, style(Style::Plain)),
                    Cell::new(&s.project, 40, Style::Muted),
                ],
                search: s.topics.join(" "),
            }
        })
        .collect()
}

fn entries(items: &[EntryPayload], year: i32) -> Vec<Row> {
    let mut items: Vec<&EntryPayload> = items.iter().collect();
    items.sort_by_key(|e| std::cmp::Reverse(e.created));
    items
        .into_iter()
        .map(|e| {
            let dim = e.status != "active";
            let style = |on: Style| if dim { Style::Muted } else { on };
            Row {
                id: e.id.clone(),
                cells: vec![
                    Cell::new(&short_date(e.created, year), 10, style(Style::Plain)),
                    Cell::new(&e.tag, 20, style(Style::Ok)),
                    Cell::new(&first_line(&e.content), 120, style(Style::Plain)),
                ],
                search: format!("{} {}", e.project, e.topics.join(" ")),
            }
        })
        .collect()
}

/// What a proposal asks for, in words.
fn proposed(p: &ProposalPayload) -> String {
    let what = match (p.kind, &p.newer) {
        ("candidate", _) => format!("record {} after approval", p.older),
        ("supersedes", Some(newer)) => format!("{newer} supersedes {}", p.older),
        ("supersedes", None) => format!("supersede {}", p.older),
        _ => format!("retire {} as stale", p.older),
    };
    let said = said(p);
    let reason = first_line(&said);
    if reason.is_empty() {
        what
    } else {
        format!("{what}: {reason}")
    }
}

/// A proposal's words: scribe's context for a candidate carrying one (an
/// older claim's statement, labelled; 2_design.md §15.3), else its reason,
/// which for a pass candidate is the user's raw quote.
fn said(p: &ProposalPayload) -> String {
    p.candidate
        .as_ref()
        .and_then(super::payload::claim_context)
        .unwrap_or_else(|| p.reason.clone())
}

fn proposals(items: &[ProposalPayload], year: i32) -> Vec<Row> {
    let mut items: Vec<&ProposalPayload> = items.iter().collect();
    items.sort_by_key(|p| std::cmp::Reverse(p.opened_at));
    items
        .into_iter()
        .map(|p| Row {
            id: p.id.clone(),
            cells: vec![
                Cell::new(&short_date(p.opened_at, year), 10, Style::Plain),
                Cell::new(
                    p.state,
                    8,
                    if p.state == "open" {
                        Style::Ok
                    } else {
                        Style::Muted
                    },
                ),
                Cell::new(&proposed(p), 120, Style::Plain),
            ],
            search: String::new(),
        })
        .collect()
}

fn transcripts(items: &[TranscriptPayload], year: i32) -> Vec<Row> {
    items
        .iter()
        .map(|t| Row {
            id: t.session.clone(),
            cells: vec![
                Cell::new(
                    &t.started
                        .map_or_else(|| "-".to_owned(), |at| short_date(at, year)),
                    10,
                    Style::Plain,
                ),
                Cell::new(&t.project, 30, Style::Muted),
                Cell::new(&first_line(&t.first_prompt), 120, Style::Plain),
            ],
            search: t.cwd.clone(),
        })
        .collect()
}

const LABEL: usize = 11;

fn detail(out: &mut dyn Write, label: &str, value: &str) -> io::Result<()> {
    let value = if value.is_empty() { "-" } else { value };
    let indent = format!("\n{}", " ".repeat(LABEL));
    writeln!(out, "{label:<LABEL$}{}", value.replace('\n', &indent))
}

/// A titled section, skipped when it has no text.
fn section(out: &mut dyn Write, heading: &str, text: &str) -> io::Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    writeln!(out, "\n## {heading}\n{}", text.trim_end())
}

/// One record's preview: what it says first, then the metadata.
pub(crate) fn preview(out: &mut dyn Write, record: &Record) -> io::Result<()> {
    match record {
        Record::Session(s) => session_preview(out, s),
        Record::Entry(e) => entry_preview(out, e),
        Record::Proposal(p) => proposal_preview(out, p),
        Record::Transcript(_)
        | Record::SessionHandoffs(_)
        | Record::Tag(_)
        | Record::TagRules(_)
        | Record::Memory(_)
        | Record::MemoryBlock(_)
        | Record::MemoryRevision(_)
        | Record::MemoryRefresh(_)
        | Record::StoreMigrate(_)
        | Record::Export(_)
        | Record::MemoryRules(_)
        | Record::Brief(_) => Ok(()),
    }
}

fn session_preview(out: &mut dyn Write, s: &SessionPayload) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        if s.title.is_empty() {
            "(untitled)"
        } else {
            &s.title
        }
    )?;
    if !s.summary.trim().is_empty() {
        writeln!(out, "\n{}", s.summary.trim_end())?;
    }
    section(out, "Goals", &s.goals)?;
    section(out, "Next", &s.next)?;
    writeln!(out, "\n## Details")?;
    detail(out, "id", &s.id)?;
    detail(out, "status", s.status)?;
    detail(out, "active", if s.active { "yes" } else { "no" })?;
    detail(out, "project", &s.project)?;
    detail(out, "topics", &s.topics.join(", "))?;
    detail(out, "related", &s.related.join(", "))?;
    for link in &s.links {
        detail(
            out,
            "link",
            &format!(
                "{} {} ({})",
                link.kind,
                link.session,
                link.title.as_deref().unwrap_or("untitled")
            ),
        )?;
    }
    detail(out, "host", &s.host)?;
    detail(out, "cwd", &s.cwd)?;
    detail(out, "provider", &s.provider)?;
    detail(out, "started", &long_time(s.started))?;
    detail(out, "updated", &long_time(s.updated))?;
    detail(out, "passes", &s.passes.to_string())?;
    session_log(out, s)
}

fn entry_preview(out: &mut dyn Write, e: &EntryPayload) -> io::Result<()> {
    writeln!(out, "{} · {}", e.tag, e.status)?;
    writeln!(out, "\n{}", e.content.trim_end())?;
    writeln!(out, "\n## Details")?;
    entry_metadata(out, e, false)
}

fn proposal_preview(out: &mut dyn Write, p: &ProposalPayload) -> io::Result<()> {
    writeln!(out, "{}", proposed(p))?;
    let stated = p.candidate.as_ref().is_some_and(|c| c.stated().is_some());
    section(out, if stated { "Context" } else { "Reason" }, &said(p))?;
    writeln!(out, "\n## Details")?;
    detail(out, "id", &p.id)?;
    detail(out, "kind", p.kind)?;
    detail(out, "state", p.state)?;
    detail(out, "older", &p.older)?;
    if let Some(candidate) = &p.candidate {
        detail(out, "tag", candidate.tag.as_str())?;
        if candidate.stated().is_some() {
            detail(out, "subject", candidate.subject.as_deref().unwrap_or(""))?;
        }
        detail(out, "quote", &candidate.quote)?;
        detail(out, "session", candidate.cite.session.as_str())?;
        detail(
            out,
            "message",
            candidate.cite.message.as_deref().unwrap_or("-"),
        )?;
    }
    detail(out, "newer", p.newer.as_deref().unwrap_or(""))?;
    detail(out, "opened", &long_time(p.opened_at))
}
