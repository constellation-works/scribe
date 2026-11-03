//! Payload → bytes, one function per shape and mode. Every rendering reads
//! the same payload, so no mode can show a value the others lack.

use super::color::{self, Role};
use super::payload::{
    EntryPayload, HookJson, HookReply, HookSpecificJson, PassPayload, PassReport, ProposalPayload,
    Record, Records, SessionPayload, TagPayload, WarningPayload,
};
use super::sink::{OutputMode, Sink};
use super::table::{self, Align, Cell, Column};
use serde::ser::{Serialize, SerializeMap, Serializer};
use std::io::{self, Write};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const fn col(header: &'static str, align: Align, truncate: bool) -> Column {
    Column {
        header,
        align,
        truncate,
    }
}

const ENTRY_COLUMNS: [Column; 6] = [
    col("ID", Align::Left, false),
    col("STATUS", Align::Left, false),
    col("PROJECT", Align::Left, false),
    col("TOPICS", Align::Left, false),
    col("CREATED", Align::Left, false),
    col("CONTENT", Align::Left, true),
];

const SESSION_COLUMNS: [Column; 7] = [
    col("ID", Align::Left, false),
    col("STATUS", Align::Left, false),
    col("DUE", Align::Left, false),
    col("PROJECT", Align::Left, false),
    col("TOPICS", Align::Left, false),
    col("UPDATED", Align::Left, false),
    col("TITLE", Align::Left, true),
];

const PROPOSAL_COLUMNS: [Column; 7] = [
    col("ID", Align::Left, false),
    col("KIND", Align::Left, false),
    col("STATE", Align::Left, false),
    col("OLDER", Align::Left, false),
    col("NEWER", Align::Left, false),
    col("OPENED", Align::Left, false),
    col("REASON", Align::Left, true),
];

const PASS_COLUMNS: [Column; 5] = [
    col("SESSION", Align::Left, false),
    col("RESULT", Align::Left, false),
    col("CODE", Align::Left, false),
    col("PASSES", Align::Right, false),
    col("TERMS DROPPED", Align::Right, false),
];

const TAG_COLUMNS: [Column; 5] = [
    col("NAME", Align::Left, false),
    col("ENTRIES", Align::Right, false),
    col("STALE DAYS", Align::Right, false),
    col("ALIASES", Align::Left, false),
    col("DESCRIPTION", Align::Left, true),
];

/// The `--json` form of a list: always `{"<records>": [...], "total",
/// "truncated", "warnings"}`, whatever the flags, so its shape never depends
/// on them (STD-01 R34, R10).
struct Envelope<'a, T> {
    key: &'static str,
    items: &'a [T],
    total: usize,
    warnings: &'a [WarningPayload],
}

impl<T: Serialize> Serialize for Envelope<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry(self.key, self.items)?;
        map.serialize_entry("total", &self.total)?;
        map.serialize_entry("truncated", &(self.items.len() < self.total))?;
        map.serialize_entry("warnings", self.warnings)?;
        map.end()
    }
}

/// A list: table, tab-separated lines, or the JSON envelope. `total` is how
/// many matched.
pub(crate) fn list(
    out: &mut dyn Write,
    sink: &Sink,
    records: &Records,
    total: usize,
    warnings: &[WarningPayload],
) -> io::Result<()> {
    if sink.mode == OutputMode::Json {
        return match records {
            Records::Entries(items) => {
                write_json(out, &envelope("entries", items, total, warnings))
            }
            Records::Sessions(items) => {
                write_json(out, &envelope("sessions", items, total, warnings))
            }
            Records::Proposals(items) => {
                write_json(out, &envelope("proposals", items, total, warnings))
            }
            Records::Tags(items) => write_json(out, &envelope("tags", items, total, warnings)),
        };
    }
    // An empty list prints nothing, not a lone header (STD-01 R16).
    if records.len() == 0 {
        return Ok(());
    }
    let (columns, rows): (&[Column], Vec<Vec<Cell>>) = match records {
        Records::Entries(items) => (&ENTRY_COLUMNS, items.iter().map(entry_row).collect()),
        Records::Sessions(items) => (&SESSION_COLUMNS, items.iter().map(session_row).collect()),
        Records::Proposals(items) => (&PROPOSAL_COLUMNS, items.iter().map(proposal_row).collect()),
        Records::Tags(items) => (&TAG_COLUMNS, items.iter().map(tag_row).collect()),
    };
    match sink.mode {
        OutputMode::Table => {
            out.write_all(table::render(columns, &rows, sink.width, sink.color).as_bytes())
        }
        OutputMode::Plain | OutputMode::Json => rows
            .iter()
            .try_for_each(|row| writeln!(out, "{}", plain_line(row))),
    }
}

fn envelope<'a, T>(
    key: &'static str,
    items: &'a [T],
    total: usize,
    warnings: &'a [WarningPayload],
) -> Envelope<'a, T> {
    Envelope {
        key,
        items,
        total,
        warnings,
    }
}

/// One record: a field-per-line detail view, one tab-separated line (the
/// same line its list prints), or a JSON object. The detail view never
/// truncates: it is where a truncated table cell is read in full (STD-01
/// R15).
pub(crate) fn show(out: &mut dyn Write, sink: &Sink, record: &Record) -> io::Result<()> {
    match (sink.mode, record) {
        (OutputMode::Json, Record::Entry(entry)) => write_json(out, entry),
        (OutputMode::Json, Record::Session(session)) => write_json(out, session),
        (OutputMode::Plain, Record::Entry(entry)) => {
            writeln!(out, "{}", plain_line(&entry_row(entry)))
        }
        (OutputMode::Plain, Record::Session(session)) => {
            writeln!(out, "{}", plain_line(&session_row(session)))
        }
        (OutputMode::Table, Record::Entry(entry)) => entry_detail(out, sink, entry),
        (OutputMode::Table, Record::Session(session)) => session_detail(out, sink, session),
    }
}

/// A hook's reply: one line of Claude Code hook JSON, or nothing when the
/// hook has nothing to say. The mode is ignored: Claude Code reads this.
pub(crate) fn hook(out: &mut dyn Write, reply: &HookReply) -> io::Result<()> {
    if let Some(reason) = &reply.block {
        let json = HookJson {
            decision: Some("block"),
            reason: Some(reason),
            system_message: None,
            hook_specific_output: None,
        };
        serde_json::to_writer(&mut *out, &json).map_err(io::Error::from)?;
        return writeln!(out);
    }
    if reply.message.is_none() && reply.context.is_none() {
        return Ok(());
    }
    let json = HookJson {
        decision: None,
        reason: None,
        system_message: reply.message.as_deref(),
        hook_specific_output: reply.context.as_deref().map(|context| HookSpecificJson {
            hook_event_name: reply.event,
            additional_context: context,
        }),
    };
    serde_json::to_writer(&mut *out, &json).map_err(io::Error::from)?;
    writeln!(out)
}

fn entry_detail(out: &mut dyn Write, sink: &Sink, e: &EntryPayload) -> io::Result<()> {
    let from = match (
        e.content_from.role,
        &e.content_from.message,
        e.content_from.at,
    ) {
        (Some(role), Some(message), Some(at)) => {
            format!("{role} message {message} at {}", timestamp(at))
        }
        _ => String::from("the #scribe line"),
    };
    let cite = format!(
        "session {}, line {}, message {}, at {}",
        e.cite.session,
        e.cite.line,
        e.cite.message.as_deref().unwrap_or("-"),
        timestamp(e.cite.at)
    );
    let status = color::paint(e.status, color::role_for_status(e.status), sink.color);
    field(out, "id", &e.id)?;
    field(out, "tag", &e.tag)?;
    field(out, "status", &status)?;
    field(out, "project", &e.project)?;
    field(out, "topics", &joined(&e.topics))?;
    field(out, "related", &joined(&e.related))?;
    field(out, "provider", &e.provider)?;
    field(out, "host", &e.host)?;
    field(out, "created", &timestamp(e.created))?;
    field(out, "updated", &timestamp(e.updated))?;
    field(out, "review", e.review_after.as_deref().unwrap_or("-"))?;
    field(out, "superseded", e.superseded_by.as_deref().unwrap_or("-"))?;
    field(out, "from", &from)?;
    field(out, "cite", &cite)?;
    field(out, "cwd", &e.cite.cwd)?;
    field(out, "said", or_dash(&e.said))?;
    field(out, "content", or_dash(&e.content))
}

fn session_detail(out: &mut dyn Write, sink: &Sink, s: &SessionPayload) -> io::Result<()> {
    let due = color::paint(yes_no(s.due), due_role(s.due), sink.color);
    field(out, "id", &s.id)?;
    field(out, "title", or_dash(&s.title))?;
    field(out, "status", s.status)?;
    field(out, "on", yes_no(s.on))?;
    field(out, "due", &due)?;
    field(out, "project", &s.project)?;
    field(out, "topics", &joined(&s.topics))?;
    field(out, "related", &joined(&s.related))?;
    field(out, "provider", &s.provider)?;
    field(out, "host", &s.host)?;
    field(out, "cwd", &s.cwd)?;
    field(out, "started", &timestamp(s.started))?;
    field(out, "on since", &timestamp(s.on_since))?;
    field(out, "updated", &timestamp(s.updated))?;
    field(out, "passes", &s.passes.to_string())?;
    field(out, "next open", &s.next_open.to_string())?;
    field(out, "through", s.scribed_through.as_deref().unwrap_or("-"))?;
    // One line per term: the phrase, then Daniel's defining words.
    let terms: Vec<String> = s
        .terms
        .iter()
        .map(|t| format!("{}: \"{}\" (message {})", t.term, t.said, t.message))
        .collect();
    field(out, "terms", &joined_lines(&terms))?;
    for (name, text) in [
        ("Summary", &s.summary),
        ("Goals", &s.goals),
        ("Outcome", &s.outcome),
        ("Next", &s.next),
    ] {
        writeln!(out)?;
        writeln!(out, "## {name}")?;
        writeln!(out, "{}", or_dash(text))?;
    }
    Ok(())
}

/// The width of the detail view's label column.
const LABEL_WIDTH: usize = 12;

/// `label  value`, with continuation lines indented under the value.
fn field(out: &mut dyn Write, label: &str, value: &str) -> io::Result<()> {
    let indent = format!("\n{}", " ".repeat(LABEL_WIDTH));
    writeln!(out, "{label:<LABEL_WIDTH$}{}", value.replace('\n', &indent))
}

fn entry_row(e: &EntryPayload) -> Vec<Cell> {
    vec![
        Cell::plain(e.id.clone()),
        Cell {
            text: e.status.to_owned(),
            role: color::role_for_status(e.status),
        },
        Cell::plain(e.project.clone()),
        Cell::plain(joined(&e.topics)),
        Cell::plain(timestamp(e.created)),
        Cell::plain(one_line(&e.content)),
    ]
}

/// A session pass's report: a table or tab-separated lines, one per
/// session passed, or `{"passes": [...], "remaining": n}`. Nothing passed
/// prints nothing but the JSON object.
pub(crate) fn pass(out: &mut dyn Write, sink: &Sink, report: &PassReport) -> io::Result<()> {
    if sink.mode == OutputMode::Json {
        #[derive(serde::Serialize)]
        struct Json<'a> {
            passes: &'a [PassPayload],
            remaining: usize,
        }
        return write_json(
            out,
            &Json {
                passes: &report.passes,
                remaining: report.remaining,
            },
        );
    }
    if report.passes.is_empty() {
        return Ok(());
    }
    let rows: Vec<Vec<Cell>> = report.passes.iter().map(pass_row).collect();
    match sink.mode {
        OutputMode::Table => {
            out.write_all(table::render(&PASS_COLUMNS, &rows, sink.width, sink.color).as_bytes())
        }
        OutputMode::Plain | OutputMode::Json => rows
            .iter()
            .try_for_each(|row| writeln!(out, "{}", plain_line(row))),
    }
}

fn pass_row(p: &PassPayload) -> Vec<Cell> {
    vec![
        Cell::plain(p.session.clone()),
        Cell {
            text: p.result.to_owned(),
            role: if p.result == "failed" {
                Role::Warn
            } else {
                Role::Neutral
            },
        },
        Cell::plain(p.error_code.unwrap_or("-")),
        Cell::plain(
            p.passes
                .map_or_else(|| String::from("-"), |n| n.to_string()),
        ),
        Cell::plain(p.dropped_terms.to_string()),
    ]
}

fn session_row(s: &SessionPayload) -> Vec<Cell> {
    vec![
        Cell::plain(s.id.clone()),
        Cell::plain(s.status),
        Cell {
            text: yes_no(s.due).to_owned(),
            role: due_role(s.due),
        },
        Cell::plain(s.project.clone()),
        Cell::plain(joined(&s.topics)),
        Cell::plain(timestamp(s.updated)),
        Cell::plain(one_line(&s.title)),
    ]
}

fn proposal_row(p: &ProposalPayload) -> Vec<Cell> {
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
        Cell::plain(timestamp(p.opened_at)),
        Cell::plain(one_line(&p.reason)),
    ]
}

fn tag_row(t: &TagPayload) -> Vec<Cell> {
    vec![
        Cell {
            text: t.name.clone(),
            role: if t.configured {
                Role::Neutral
            } else {
                Role::Muted
            },
        },
        Cell::plain(t.entries.to_string()),
        Cell::plain(
            t.stale_after_days
                .map_or_else(|| String::from("-"), |d| d.to_string()),
        ),
        Cell::plain(joined(&t.aliases)),
        Cell::plain(one_line(t.description.as_deref().unwrap_or("-"))),
    ]
}

/// One tab-separated line, no header, no color (STD-01 R9). Free text is
/// flattened to one line by [`one_line`]; `--json` and `show` carry it
/// verbatim.
fn plain_line(cells: &[Cell]) -> String {
    let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
    texts.join("\t")
}

/// Free text on one line: every run of whitespace, tabs and line breaks
/// included, becomes one space, so a record never spans fields or lines.
/// Empty text is `-` (STD-01 R14).
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        String::from("-")
    } else {
        flat
    }
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

fn joined(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(",")
    }
}

fn joined_lines(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join("\n")
    }
}

fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

fn due_role(due: bool) -> Role {
    if due { Role::Warn } else { Role::Neutral }
}

fn timestamp(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_else(|_| String::from("-"))
}

fn write_json<T: Serialize + ?Sized>(out: &mut dyn Write, value: &T) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, value).map_err(io::Error::from)?;
    writeln!(out)
}
