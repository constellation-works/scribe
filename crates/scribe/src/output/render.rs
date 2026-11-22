//! Payload → bytes, one function per shape and mode. Every rendering reads
//! the same payload, so no mode can show a value the others lack.

use super::color::{self, Role};
use super::payload::{
    AskPayload, EntryPayload, FoundPayload, HitPayload, HookJson, HookReply, HookSpecificJson,
    OriginPayload, PassPayload, PassReport, PluginReply, ProposalPayload, Record, Records,
    ScanReport, SessionPayload, TagPayload, TagShowPayload, TaskPayload, TranscriptPayload,
    TreeRow, WarningPayload,
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

const ENTRY_COLUMNS: [Column; 8] = [
    col("ID", Align::Left, false),
    col("STATUS", Align::Left, false),
    col("PROJECT", Align::Left, false),
    col("TOPICS", Align::Left, false),
    col("CREATED", Align::Left, false),
    col("CONTENT", Align::Left, true),
    col("AUTHOR", Align::Left, false),
    col("AGE", Align::Right, false),
];

const MEMORY_COLUMNS: [Column; 7] = [
    col("ID", Align::Left, false),
    col("SECTION", Align::Left, false),
    col("STATUS", Align::Left, false),
    col("PAGE", Align::Left, false),
    col("SEEN", Align::Right, false),
    col("LATEST", Align::Left, false),
    col("STATEMENT", Align::Left, true),
];

const SESSION_TREE_COLUMNS: [Column; 8] = [
    col("ID", Align::Left, false),
    col("PARENT", Align::Left, false),
    col("STATUS", Align::Left, false),
    col("DUE", Align::Left, false),
    col("PROJECT", Align::Left, false),
    col("TOPICS", Align::Left, false),
    col("UPDATED", Align::Left, false),
    col("TITLE", Align::Left, true),
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

const PROPOSAL_COLUMNS: [Column; 9] = [
    col("ID", Align::Left, false),
    col("KIND", Align::Left, false),
    col("STATE", Align::Left, false),
    col("OLDER", Align::Left, false),
    col("NEWER", Align::Left, false),
    col("OPENED", Align::Left, false),
    col("REASON", Align::Left, true),
    col("ADMITTED", Align::Left, true),
    col("CLOSED_REASON", Align::Left, false),
];

const PASS_COLUMNS: [Column; 5] = [
    col("SESSION", Align::Left, false),
    col("RESULT", Align::Left, false),
    col("CODE", Align::Left, false),
    col("PASSES", Align::Right, false),
    col("TERMS DROPPED", Align::Right, false),
];

const SCAN_COLUMNS: [Column; 5] = [
    col("ACTION", Align::Left, false),
    col("ID", Align::Left, false),
    col("WHAT", Align::Left, false),
    col("OLDER", Align::Left, false),
    col("NEWER", Align::Left, false),
];

const MEMORY_RULE_COLUMNS: [Column; 4] = [
    col("RULE", Align::Left, false),
    col("VALUE", Align::Left, false),
    col("ORIGIN", Align::Left, false),
    col("MEANING", Align::Left, true),
];

const TAG_COLUMNS: [Column; 10] = [
    col("NAME", Align::Left, false),
    col("ENTRIES", Align::Right, false),
    col("STALE DAYS", Align::Right, false),
    col("ALIASES", Align::Left, false),
    col("DESCRIPTION", Align::Left, true),
    col("SOURCE", Align::Left, false),
    col("ACTION", Align::Left, false),
    col("RETIRED", Align::Right, false),
    col("REJECTED", Align::Right, false),
    col("ROUTE", Align::Left, false),
];

const TRANSCRIPT_COLUMNS: [Column; 7] = [
    col("SESSION", Align::Left, false),
    col("PROJECT", Align::Left, false),
    col("USER_TURNS", Align::Right, false),
    col("BYTES", Align::Right, false),
    col("STARTED", Align::Left, false),
    col("ENDED", Align::Left, false),
    col("FIRST_PROMPT", Align::Left, true),
];

const HIT_COLUMNS: [Column; 6] = [
    col("CITE", Align::Left, false),
    col("KIND", Align::Left, false),
    col("SCORE", Align::Right, false),
    col("UPDATED", Align::Left, false),
    col("TITLE", Align::Left, false),
    col("SNIPPET", Align::Left, true),
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
            Records::Sessions { items, .. } => {
                write_json(out, &envelope("sessions", items, total, warnings))
            }
            Records::SessionTree { rows, .. } => {
                write_json(out, &envelope("sessions", rows, total, warnings))
            }
            Records::Proposals(items) => {
                write_json(out, &envelope("proposals", items, total, warnings))
            }
            Records::Tags(items) => write_json(out, &envelope("tags", items, total, warnings)),
            Records::Hits(items) => write_json(out, &envelope("hits", items, total, warnings)),
            Records::Transcripts(items) => {
                write_json(out, &envelope("transcripts", items, total, warnings))
            }
            Records::Memory(items) => write_json(out, &envelope("items", items, total, warnings)),
        };
    }
    // An empty list prints nothing, not a lone header (STD-01 R16).
    if records.len() == 0 {
        return Ok(());
    }
    let (columns, rows): (Vec<Column>, Vec<Vec<Cell>>) = match records {
        Records::Entries(items) => (
            ENTRY_COLUMNS.to_vec(),
            items.iter().map(entry_row).collect(),
        ),
        Records::Sessions {
            items,
            active_column,
        } => (
            with_active(&SESSION_COLUMNS, *active_column, 2),
            items
                .iter()
                .map(|s| session_row(s, *active_column))
                .collect(),
        ),
        Records::SessionTree {
            rows,
            active_column,
        } => (
            with_active(&SESSION_TREE_COLUMNS, *active_column, 3),
            rows.iter()
                .map(|row| tree_row(row, sink.mode == OutputMode::Table, *active_column))
                .collect(),
        ),
        Records::Proposals(items) => (
            PROPOSAL_COLUMNS.to_vec(),
            items
                .iter()
                .flat_map(|p| proposal_rows(p, sink.mode))
                .collect(),
        ),
        Records::Tags(items) => (TAG_COLUMNS.to_vec(), items.iter().map(tag_row).collect()),
        Records::Hits(items) => (
            HIT_COLUMNS.to_vec(),
            items.iter().map(|h| hit_row(h, sink.mode)).collect(),
        ),
        Records::Transcripts(items) => (
            TRANSCRIPT_COLUMNS.to_vec(),
            items.iter().map(transcript_row).collect(),
        ),
        Records::Memory(items) => (
            MEMORY_COLUMNS.to_vec(),
            items.iter().map(memory_row).collect(),
        ),
    };
    match sink.mode {
        OutputMode::Table => {
            out.write_all(table::render(&columns, &rows, sink.width, sink.color).as_bytes())
        }
        OutputMode::Plain | OutputMode::Json => rows
            .iter()
            .try_for_each(|row| writeln!(out, "{}", plain_line(row))),
    }
}

/// `columns` with an `ACTIVE` column inserted at `at` when `active`.
fn with_active(columns: &[Column], active: bool, at: usize) -> Vec<Column> {
    let mut out = columns.to_vec();
    if active {
        out.insert(at, col("ACTIVE", Align::Left, false));
    }
    out
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
        (OutputMode::Json, Record::Tag(tag)) => write_json(out, tag),
        (OutputMode::Json, Record::TagRules(tag)) => write_json(out, tag),
        (OutputMode::Plain, Record::TagRules(tag)) => tag_rules(out, tag, false),
        (OutputMode::Table, Record::TagRules(tag)) => tag_rules(out, tag, true),
        (OutputMode::Json, Record::Proposal(proposal)) => write_json(out, proposal),
        (OutputMode::Plain, Record::Tag(tag)) => writeln!(
            out,
            "{}\t{}\t{}\t{}",
            tag.name,
            one_line(&tag.description),
            tag.author,
            tag.aliases
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        (OutputMode::Plain, Record::Proposal(p)) => {
            writeln!(out, "{}", plain_line(&proposal_row(p, OutputMode::Plain)))
        }
        (OutputMode::Table, Record::Tag(tag)) => {
            field(out, "name", tag.name.as_str())?;
            field(out, "description", &tag.description)?;
            field(
                out,
                "aliases",
                &tag.aliases
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            )?;
            field(out, "author", &tag.author.to_string())?;
            field(
                out,
                "updated_by",
                &tag.updated_by
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), ToString::to_string),
            )?;
            field(
                out,
                "stale_after_days",
                &tag.stale_after_days
                    .map_or_else(|| "-".to_owned(), |d| d.to_string()),
            )?;
            field(out, "created", &timestamp(tag.created))?;
            field(out, "updated", &timestamp(tag.updated))
        }
        (OutputMode::Table, Record::Proposal(p)) => {
            field(out, "id", &p.id)?;
            field(out, "kind", p.kind)?;
            field(out, "state", p.state)?;
            field(out, "older", &p.older)?;
            if let Some(candidate) = &p.candidate {
                if let Some(statement) = &candidate.statement {
                    field(out, "statement", statement)?;
                    field(out, "subject", candidate.subject.as_deref().unwrap_or("-"))?;
                }
                field(out, "quote", &candidate.quote)?;
                field(out, "tag", candidate.tag.as_str())?;
                field(out, "session", candidate.cite.session.as_str())?;
                field(
                    out,
                    "message",
                    candidate.cite.message.as_deref().unwrap_or("-"),
                )?;
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
            if let Some(line) = settled_line(p) {
                field(out, "instruction", &line)?;
            }
            field(out, "opened", &timestamp(p.opened_at))?;
            field(
                out,
                "settled",
                &p.settled_at.map_or_else(|| "-".to_owned(), timestamp),
            )
        }
        (OutputMode::Json, Record::Entry(entry)) => write_json(out, entry),
        (OutputMode::Json, Record::Session(session)) => write_json(out, session),
        (OutputMode::Plain, Record::Entry(entry)) => {
            writeln!(out, "{}", plain_line(&entry_row(entry)))?;
            // Scribe's statement is the row's content; Daniel's own words
            // follow on their line, with whose statement it is.
            if let (Some(by), Some(quote)) = (&entry.stated_by, &entry.quote) {
                writeln!(out, "stated_by\t{}\t{}", author(by), one_line(quote))?;
            }
            // An approval an agent made on his instruction (2_design.md §12.3).
            if let Some(line) = approved_line(entry) {
                writeln!(out, "approved\t{line}")?;
            }
            // An idea the pass captured: where it stands, then the parking
            // words or the task (`-` for neither).
            if let Some(outcome) = &entry.outcome {
                writeln!(
                    out,
                    "outcome\t{}\t{}",
                    outcome.state,
                    one_line(
                        outcome
                            .reason
                            .as_deref()
                            .or(outcome.task.as_deref())
                            .unwrap_or("-")
                    )
                )?;
            }
            if let Some(a) = &entry.admitted {
                writeln!(
                    out,
                    "admitted\t{}\t{}\t{}",
                    one_line(&a.trigger),
                    one_line(&a.matched),
                    one_line(a.criteria.as_deref().unwrap_or("-"))
                )?;
            }
            Ok(())
        }
        (OutputMode::Plain, Record::Session(session)) => {
            writeln!(out, "{}", plain_line(&session_row(session, false)))?;
            for segment in session.log.iter().flatten() {
                writeln!(
                    out,
                    "segment\t{}\t{}\t{}\t{}\t{}",
                    segment.n,
                    segment.state,
                    timestamp(segment.from),
                    timestamp(segment.to),
                    one_line(&segment.heading)
                )?;
            }
            Ok(())
        }
        (OutputMode::Table, Record::Entry(entry)) => entry_detail(out, sink, entry),
        (OutputMode::Table, Record::Session(session)) => session_detail(out, sink, session),
        (OutputMode::Json, Record::Transcript(transcript)) => write_json(out, transcript),
        (OutputMode::Plain, Record::Transcript(transcript)) => {
            writeln!(out, "{}", plain_line(&transcript_row(transcript)))?;
            if let Some(turns) = &transcript.user_first_turns {
                for (n, turn) in turns.iter().enumerate() {
                    let index = n.saturating_add(1);
                    writeln!(out, "turn\t{index}\t{}", one_line(turn))?;
                }
            }
            Ok(())
        }
        (OutputMode::Table, Record::Transcript(transcript)) => transcript_detail(out, transcript),
        (OutputMode::Json, Record::Memory(item)) => write_json(out, item),
        (OutputMode::Plain, Record::Memory(item)) => {
            writeln!(out, "{}", plain_line(&memory_row(item)))?;
            for o in &item.occurrences {
                writeln!(
                    out,
                    "occurrence\t{}\t{}\t{}\t{}\t{}",
                    o.n,
                    timestamp(o.date),
                    o.session,
                    o.message,
                    one_line(&o.quote)
                )?;
            }
            Ok(())
        }
        (OutputMode::Table, Record::Memory(item)) => memory_detail(out, item),
        (OutputMode::Json, Record::MemoryRules(rules)) => write_json(out, rules),
        (OutputMode::Plain | OutputMode::Table, Record::MemoryRules(rules)) => {
            memory_rules(out, sink, rules)
        }
        (OutputMode::Json, Record::MemoryPage(page)) => write_json(out, page),
        (OutputMode::Plain | OutputMode::Table, Record::MemoryPage(page)) => match &page.page {
            Some(text) => writeln!(out, "{text}"),
            None => Ok(()),
        },
        (OutputMode::Json, Record::Brief(brief)) => write_json(out, brief),
        (OutputMode::Plain | OutputMode::Table, Record::Brief(brief)) => {
            write!(out, "{}", brief.text)
        }
    }
}

fn memory_row(m: &super::payload::MemoryPayload) -> Vec<Cell> {
    vec![
        Cell::plain(m.id.clone()),
        Cell::plain(m.section.to_owned()),
        Cell {
            text: m.status.to_owned(),
            role: color::role_for_status(m.status),
        },
        Cell::plain(m.off_page.unwrap_or("on").to_owned()),
        Cell::plain(m.occurrence_count.to_string()),
        Cell::plain(timestamp(m.latest)),
        Cell::plain(one_line(&m.statement)),
    ]
}

fn memory_detail(out: &mut dyn Write, m: &super::payload::MemoryPayload) -> io::Result<()> {
    let or_dash = |v: Option<&str>| v.unwrap_or("-").to_owned();
    field(out, "id", &m.id)?;
    field(out, "section", m.section)?;
    field(out, "status", m.status)?;
    field(out, "statement", &m.statement)?;
    field(out, "page", m.off_page.unwrap_or("on"))?;
    field(out, "project", &or_dash(m.project.as_deref()))?;
    field(out, "event_date", &or_dash(m.event_date.as_deref()))?;
    field(
        out,
        "sensitive",
        &m.sensitive_kind
            .map_or_else(|| m.sensitive.to_string(), |k| format!("true ({k})")),
    )?;
    if m.section == "now" {
        field(
            out,
            "acknowledged",
            &m.acknowledged.as_ref().map_or_else(
                || String::from("-"),
                |a| format!("{} in {} by {}", timestamp(a.at), a.session, a.author),
            ),
        )?;
        field(out, "not_yet_raised", &m.not_yet_raised.to_string())?;
    }
    if let Some(reason) = &m.retirement_reason {
        field(out, "retirement_reason", reason)?;
    }
    field(out, "author", &m.author.to_string())?;
    field(out, "updated_by", &m.updated_by.to_string())?;
    field(out, "created", &timestamp(m.created))?;
    field(out, "updated", &timestamp(m.updated))?;
    for o in &m.occurrences {
        field(
            out,
            &format!("occurrence {}", o.n),
            &format!(
                "{} {} {}\n{}",
                timestamp(o.date),
                o.session,
                o.message,
                if o.quote.is_empty() {
                    "(forgotten)"
                } else {
                    &o.quote
                }
            ),
        )?;
    }
    Ok(())
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

/// An answer: the JSON payload, one tab-separated line per part, or a
/// detail view with each record a lookup returned shown in full.
pub(crate) fn ask(out: &mut dyn Write, sink: &Sink, a: &AskPayload) -> io::Result<()> {
    match sink.mode {
        OutputMode::Json => write_json(out, a),
        OutputMode::Plain => {
            writeln!(out, "{}\t{}", a.status, one_line(&a.answer))?;
            for c in &a.claims {
                writeln!(
                    out,
                    "claim\t{}\t{}\t{}",
                    c.cite,
                    one_line(&c.quote),
                    one_line(&c.text)
                )?;
            }
            for c in &a.conflicts {
                writeln!(
                    out,
                    "conflict\t{}\t{}",
                    c.cites.join(","),
                    one_line(&c.note)
                )?;
            }
            for r in &a.records {
                writeln!(out, "record\t{}\t{}", r.kind, r.cite)?;
            }
            Ok(())
        }
        OutputMode::Table => {
            field(out, "status", a.status)?;
            field(out, "answer", or_dash(&a.answer))?;
            field(out, "lookup", a.lookup.unwrap_or("-"))?;
            field(
                out,
                "evidence",
                &format!(
                    "{} considered, {} quotes dropped, {} hidden matches",
                    a.evidence_considered, a.dropped_quotes, a.hidden_matches
                ),
            )?;
            let claims: Vec<String> = a
                .claims
                .iter()
                .map(|c| format!("{} \"{}\" [{}]", c.text, c.quote, c.cite))
                .collect();
            field(out, "claims", &joined_lines(&claims))?;
            let conflicts: Vec<String> = a
                .conflicts
                .iter()
                .map(|c| format!("{}: {}", c.cites.join(", "), c.note))
                .collect();
            field(out, "conflicts", &joined_lines(&conflicts))?;
            for r in &a.records {
                writeln!(out)?;
                found_detail(out, sink, r)?;
            }
            Ok(())
        }
    }
}

fn found_detail(out: &mut dyn Write, sink: &Sink, r: &FoundPayload) -> io::Result<()> {
    if let Some(entry) = &r.entry {
        entry_detail(out, sink, entry)?;
    }
    if let Some(session) = &r.session {
        session_detail(out, sink, session)?;
    }
    if let Some(task) = &r.task {
        task_detail(out, task)?;
    }
    if !r.terms.is_empty() {
        let terms: Vec<String> = r
            .terms
            .iter()
            .map(|t| format!("{}: \"{}\" (message {})", t.term, t.said, t.message))
            .collect();
        writeln!(out)?;
        field(out, "matched", &joined_lines(&terms))?;
    }
    Ok(())
}

fn task_detail(out: &mut dyn Write, t: &TaskPayload) -> io::Result<()> {
    field(out, "task", &t.id)?;
    field(out, "title", or_dash(&t.title))?;
    field(out, "status", or_dash(&t.status))?;
    field(out, "tags", &joined(&t.tags))?;
    field(
        out,
        "updated",
        &t.updated.map_or_else(|| String::from("-"), timestamp),
    )?;
    writeln!(out)?;
    writeln!(out, "## Execution summary")?;
    writeln!(out, "{}", or_dash(&t.execution_summary))
}

/// A plugin call's envelope: one JSON line, whatever the mode, since the
/// Orbit host reads it.
pub(crate) fn plugin(out: &mut dyn Write, reply: &PluginReply) -> io::Result<()> {
    #[derive(serde::Serialize)]
    struct Ok<'a, T: serde::Serialize> {
        ok: bool,
        output: &'a T,
    }
    #[derive(serde::Serialize)]
    struct Failed<'a> {
        ok: bool,
        error: &'a super::payload::PluginError,
    }
    match reply {
        PluginReply::Ask(payload) => serde_json::to_writer(
            &mut *out,
            &Ok {
                ok: true,
                output: payload.as_ref(),
            },
        ),
        PluginReply::Pass(report) => serde_json::to_writer(
            &mut *out,
            &Ok {
                ok: true,
                output: report,
            },
        ),
        PluginReply::Scan(report) => serde_json::to_writer(
            &mut *out,
            &Ok {
                ok: true,
                output: report.as_ref(),
            },
        ),
        PluginReply::Error(error) => serde_json::to_writer(&mut *out, &Failed { ok: false, error }),
    }
    .map_err(io::Error::from)?;
    writeln!(out)
}

fn entry_detail(out: &mut dyn Write, sink: &Sink, e: &EntryPayload) -> io::Result<()> {
    entry_metadata(out, e, sink.color)?;
    field(out, "content", or_dash(&e.content))
}

pub(super) fn entry_metadata(out: &mut dyn Write, e: &EntryPayload, color: bool) -> io::Result<()> {
    let from = match (
        e.content_from.role,
        &e.content_from.message,
        e.content_from.at,
    ) {
        (Some(role), Some(message), Some(at)) => {
            format!("{role} message {message} at {}", timestamp(at))
        }
        _ if e.content_from.kind == "agent" => format!("agent note ({})", e.author.name),
        _ => String::from("the #scribe line"),
    };
    let cite = e
        .cite
        .as_ref()
        .map(|c| {
            format!(
                "session {}, line {}, message {}, at {}",
                c.session,
                c.line,
                c.message.as_deref().unwrap_or("-"),
                timestamp(c.at)
            )
        })
        .unwrap_or_else(|| format!("agent note ({})", e.author.name));
    let status = color::paint(e.status, color::role_for_status(e.status), color);
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
    if let Some(line) = approved_line(e) {
        field(out, "approved", &line)?;
    }
    field(out, "cwd", e.cite.as_ref().map_or("-", |c| c.cwd.as_str()))?;
    field(out, "said", or_dash(&e.said))?;
    if let Some(reason) = &e.retirement_reason {
        field(out, "retirement_reason", reason)?;
    }
    field(out, "author", &author(&e.author))?;
    if let Some(by) = &e.stated_by {
        // Output carries no config, so it names the user generically.
        let user = scribe_core::voice::UserName::default();
        let whose = if e.outcome.is_some() {
            let speaker = if e.content_from.role == Some("assistant") {
                scribe_core::Speaker::Assistant
            } else {
                scribe_core::Speaker::User
            };
            scribe_core::ask::captured_whose(&user, speaker)
        } else {
            scribe_core::ask::stated_whose(&user)
        };
        field(
            out,
            "stated_by",
            &format!("{} (content is {whose})", author(by)),
        )?;
    }
    if let Some(quote) = &e.quote {
        field(out, "quote", quote)?;
    }
    if let Some(outcome) = &e.outcome {
        field(
            out,
            "outcome",
            &scribe_core::ask::outcome_text(outcome, &scribe_core::voice::UserName::default()),
        )?;
    }
    if let Some(a) = &e.admitted {
        field(out, "admitted", &admitted(a))?;
    }
    if let Some(action) = &e.action {
        let cancel = match (action.cancel_window_ends, action.past_cancel_window) {
            (None, _) if e.author.kind == "scribe" => {
                String::from("none (written by scribe, not an agent)")
            }
            (None, _) => String::from("none (the user's own line)"),
            (Some(ends), true) => format!("ended {}", timestamp(ends)),
            (Some(ends), false) => format!("open until {}", timestamp(ends)),
        };
        field(out, "age", &age(action.age_seconds))?;
        field(out, "cancel", &cancel)?;
        let pickups: Vec<String> = action
            .pickups
            .iter()
            .map(|p| {
                format!(
                    "{} by {}:{}{}{}",
                    timestamp(p.at),
                    p.author.kind,
                    p.author.name,
                    p.result
                        .as_ref()
                        .map_or_else(String::new, |r| format!(", result {r}")),
                    if p.rejected { ", rejected" } else { "" }
                )
            })
            .collect();
        field(out, "pickups", &joined_lines(&pickups))?;
    }
    match (&e.window, e.window_turns) {
        (Some(window), _) => {
            let mut lines = Vec::new();
            if window.truncated {
                lines.push(format!("({} older turns cut)", window.omitted));
            }
            for turn in &window.turns {
                lines.push(format!(
                    "{} {}: {}{}",
                    timestamp(turn.at),
                    turn.role,
                    one_line(&turn.text),
                    if turn.cut { " [… cut]" } else { "" }
                ));
            }
            field(out, "window", &joined_lines(&lines))?;
        }
        (None, Some(turns)) => {
            field(
                out,
                "window",
                &format!("{turns} turns (show with --window)"),
            )?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// `tag show`: the tag's settings, then each capture field with its value
/// and origin. Plain is `<field>\t<value>\t<origin>` lines (origin `-` for
/// a setting); the table aligns the same lines, labels first.
fn tag_rules(out: &mut dyn Write, t: &TagShowPayload, table: bool) -> io::Result<()> {
    let origin = origin_text;
    let c = &t.capture;
    let action = t.action.as_ref().map_or_else(
        || String::from("-"),
        |a| {
            if a.require_quote {
                format!("{},quote", a.mode)
            } else {
                a.mode.to_owned()
            }
        },
    );
    let rows: Vec<(&str, String, String)> = vec![
        ("tag", t.tag.clone(), String::from("-")),
        ("source", t.source.to_owned(), String::from("-")),
        ("aliases", joined(&t.aliases), String::from("-")),
        (
            "description",
            one_line(t.description.as_deref().unwrap_or("-")),
            String::from("-"),
        ),
        (
            "stale_after_days",
            t.stale_after_days
                .map_or_else(|| String::from("-"), |d| d.to_string()),
            String::from("-"),
        ),
        ("action", action, String::from("-")),
        (
            "from",
            c.from.unwrap_or("-").to_owned(),
            origin(&c.origin.from),
        ),
        (
            "route",
            c.route.unwrap_or("-").to_owned(),
            origin(&c.origin.route),
        ),
        (
            "triggers",
            c.triggers.as_ref().map_or_else(
                || String::from("-"),
                |t| {
                    if t.is_empty() {
                        String::from("(none: the pass captures nothing)")
                    } else {
                        t.iter()
                            .map(|x| format!("{x:?}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                },
            ),
            origin(&c.origin.triggers),
        ),
        (
            "criteria",
            one_line(c.criteria.as_deref().unwrap_or("-")),
            origin(&c.origin.criteria),
        ),
        (
            "not",
            one_line(c.not.as_deref().unwrap_or("-")),
            origin(&c.origin.not),
        ),
        (
            "max_open",
            c.max_open
                .map_or_else(|| String::from("-"), |n| n.to_string()),
            origin(&c.origin.max_open),
        ),
    ];
    for (label, value, from) in rows {
        if table {
            let label = if label == "stale_after_days" {
                "stale days"
            } else {
                label
            };
            if from == "-" {
                field(out, label, &value)?;
            } else {
                field(out, label, &format!("{value}  ({from})"))?;
            }
        } else {
            writeln!(out, "{label}\t{value}\t{from}")?;
        }
    }
    Ok(())
}

/// `memory rules`: plain is `<rule>\t<value>\t<origin>` lines, like `tag
/// show`; the table adds each rule's meaning, cut to fit (`--json` keeps
/// it whole).
fn memory_rules(
    out: &mut dyn Write,
    sink: &Sink,
    rules: &super::payload::MemoryRulesPayload,
) -> io::Result<()> {
    let rows: Vec<Vec<Cell>> = rules
        .rules
        .iter()
        .map(|r| {
            let value = r.value.as_array().map_or_else(
                || r.value.to_string(),
                |list| {
                    list.iter()
                        .map(|v| format!("{:?}", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );
            vec![
                Cell::plain(r.rule),
                Cell::plain(value),
                Cell::plain(origin_text(&r.origin)),
                Cell::plain(r.meaning),
            ]
        })
        .collect();
    if sink.mode == OutputMode::Table {
        return out.write_all(
            table::render(&MEMORY_RULE_COLUMNS, &rows, sink.width, sink.color).as_bytes(),
        );
    }
    rows.iter()
        .try_for_each(|row| writeln!(out, "{}", plain_line(&row[..3])))
}

fn origin_text(o: &OriginPayload) -> String {
    match (&o.file, o.line) {
        (Some(file), Some(line)) => format!("{} {file}:{line}", o.source),
        (Some(file), None) => format!("{} {file}", o.source),
        (None, _) => o.source.to_owned(),
    }
}

fn session_detail(out: &mut dyn Write, sink: &Sink, s: &SessionPayload) -> io::Result<()> {
    let due = color::paint(yes_no(s.due), due_role(s.due), sink.color);
    field(out, "id", &s.id)?;
    field(out, "title", or_dash(&s.title))?;
    field(out, "status", s.status)?;
    field(out, "active", yes_no(s.active))?;
    field(out, "keep active", yes_no(s.keep_active))?;
    let change = s.active_change.as_ref().map_or_else(
        || String::from("-"),
        |c| {
            format!(
                "{} {} by {}:{} ({}){}",
                timestamp(c.at),
                c.route,
                c.by.kind,
                c.by.name,
                one_line(&c.reason),
                c.cite
                    .as_ref()
                    .map_or_else(String::new, |cite| format!(", cite {cite}"))
            )
        },
    );
    field(out, "change", &change)?;
    let cataloged = s.cataloged_by.as_ref().map_or_else(
        || String::from("-"),
        |c| format!("{} by {}:{}", timestamp(c.at), c.by.kind, c.by.name),
    );
    field(out, "cataloged", &cataloged)?;
    field(out, "on", yes_no(s.on))?;
    field(out, "due", &due)?;
    field(out, "project", &s.project)?;
    field(out, "topics", &joined(&s.topics))?;
    field(out, "related", &joined(&s.related))?;
    let links: Vec<String> = s
        .links
        .iter()
        .map(|l| match &l.title {
            Some(title) => format!("{} {} ({})", l.kind, l.session, one_line(title)),
            None => format!("{} {}", l.kind, l.session),
        })
        .collect();
    field(out, "links", &joined_lines(&links))?;
    let children: Vec<String> = s
        .children
        .iter()
        .map(|c| format!("{} {} ({})", c.session, c.kind, one_line(&c.title)))
        .collect();
    field(out, "children", &joined_lines(&children))?;
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
        ("Next", &s.next),
    ] {
        writeln!(out)?;
        writeln!(out, "## {name}")?;
        writeln!(out, "{}", or_dash(text))?;
    }
    session_log(out, s)
}

pub(super) fn session_log(out: &mut dyn Write, s: &SessionPayload) -> io::Result<()> {
    let log = s.log.as_deref().unwrap_or_default();
    writeln!(out)?;
    writeln!(out, "## Log")?;
    if log.is_empty() {
        writeln!(out, "-")?;
        return Ok(());
    }
    // The table of contents: one line per segment.
    for segment in log {
        writeln!(
            out,
            "{:>3}  {:<6}  {}  {}  {}",
            segment.n,
            segment.state,
            timestamp(segment.from),
            timestamp(segment.to),
            one_line(&segment.heading)
        )?;
    }
    if s.log_text {
        for segment in log {
            writeln!(out)?;
            writeln!(out, "### {}. {}", segment.n, segment.heading)?;
            writeln!(out, "{}", or_dash(&segment.text))?;
        }
    }
    Ok(())
}

/// The width of the detail view's label column.
const LABEL_WIDTH: usize = 12;

/// `label  value`, with continuation lines indented under the value. A label
/// as long as the column or longer is followed by one space, never run into
/// its value.
fn field(out: &mut dyn Write, label: &str, value: &str) -> io::Result<()> {
    let width = LABEL_WIDTH.max(label.chars().count() + 1);
    let indent = format!("\n{}", " ".repeat(width));
    writeln!(out, "{label:<width$}{}", value.replace('\n', &indent))
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
        Cell::plain(author(&e.author)),
        Cell::plain(
            e.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| age(a.age_seconds)),
        ),
    ]
}

/// `user`, or `<kind>:<name>` for an agent's or scribe's entry.
fn author(a: &super::payload::AuthorPayload) -> String {
    if a.kind == "user" {
        a.name.clone()
    } else {
        format!("{}:{}", a.kind, a.name)
    }
}

/// An age in its largest whole unit: `45s`, `12m`, `3h`, `9d`.
fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3_600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3_600)
    } else {
        format!("{}d", s / 86_400)
    }
}

/// A session pass's report: a table or tab-separated lines, one per
/// session passed, or `{"passes": [...], "remaining": n}`. Nothing passed
/// prints nothing but the JSON object.
pub(crate) fn pass(out: &mut dyn Write, sink: &Sink, report: &PassReport) -> io::Result<()> {
    if sink.mode == OutputMode::Json {
        return write_json(out, report);
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

/// A consistency scan's report: a table or tab-separated lines, one per
/// approval re-applied, proposal opened, retired-tag candidate closed
/// (`closed`, the proposal id, `closed_reason` or kind, older, newer),
/// covered candidate, session deactivated or deactivation
/// failed, or the JSON object. A scan that changed nothing prints nothing
/// but the JSON object.
pub(crate) fn scan(out: &mut dyn Write, sink: &Sink, report: &ScanReport) -> io::Result<()> {
    if sink.mode == OutputMode::Json {
        return write_json(out, report);
    }
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for r in &report.reapplied {
        rows.push(vec![
            Cell::plain("reapplied"),
            Cell::plain(r.proposal.clone()),
            Cell::plain(r.status),
            Cell::plain(r.entry.clone()),
            Cell::plain("-"),
        ]);
    }
    for p in &report.opened {
        rows.push(vec![
            Cell {
                text: String::from("opened"),
                role: Role::Warn,
            },
            Cell::plain(p.id.clone()),
            Cell::plain(p.kind),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for p in &report.closed {
        rows.push(vec![
            Cell::plain("closed"),
            Cell::plain(p.id.clone()),
            Cell::plain(p.closed_reason.unwrap_or(p.kind)),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for item in &report.covered {
        rows.push(vec![
            Cell::plain("covered"),
            Cell::plain(item.proposal.clone()),
            Cell::plain(item.by.clone()),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for f in &report.failed_batches {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(format!("batch-{}", f.batch)),
            Cell::plain(f.error_code),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for d in &report.deactivated {
        rows.push(vec![
            Cell {
                text: String::from("deactivated"),
                role: Role::Warn,
            },
            Cell::plain(d.session.clone()),
            Cell::plain(d.route),
            Cell::plain(d.cite.clone().unwrap_or_else(|| String::from("-"))),
            Cell::plain("-"),
        ]);
    }
    for f in &report.failed_deactivations {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(f.session.clone()),
            Cell::plain(f.route),
            Cell::plain(f.error_code),
            Cell::plain("-"),
        ]);
    }
    let memory = &report.memory;
    for m in &memory.merged {
        rows.push(vec![
            Cell::plain("merged"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.into.clone()),
            Cell::plain(m.item.clone()),
        ]);
    }
    for m in &memory.superseded {
        rows.push(vec![
            Cell::plain("superseded"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.by.clone()),
        ]);
    }
    for m in &memory.promoted {
        rows.push(vec![
            Cell::plain("promoted"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.rule),
            Cell::plain(m.item.clone()),
            Cell::plain(m.to.clone()),
        ]);
    }
    for id in &memory.completed {
        rows.push(vec![
            Cell::plain("completed"),
            Cell::plain(id.clone()),
            Cell::plain("memory"),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for f in &memory.failed_batches {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(format!("memory-batch-{}", f.batch)),
            Cell::plain(f.error_code),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for f in &report.failed_removal_batches {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(format!("removal-batch-{}", f.batch)),
            Cell::plain(f.error_code),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    if rows.is_empty() {
        return Ok(());
    }
    match sink.mode {
        OutputMode::Table => {
            out.write_all(table::render(&SCAN_COLUMNS, &rows, sink.width, sink.color).as_bytes())
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

/// A `session list --tree` row: the session's row with its parent after
/// the id; in a table the title is indented two spaces per level.
fn tree_row(row: &TreeRow, indent: bool, active: bool) -> Vec<Cell> {
    let mut cells = session_row(&row.session, active);
    cells.insert(
        1,
        Cell::plain(row.parent.clone().unwrap_or_else(|| String::from("-"))),
    );
    if indent && let Some(title) = cells.last_mut() {
        title.text = format!("{}{}", "  ".repeat(row.depth), title.text);
    }
    cells
}

/// A session's list row; `active` adds its `active` column after the
/// status (`session list --include-inactive`).
fn session_row(s: &SessionPayload, active: bool) -> Vec<Cell> {
    let mut cells = vec![
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
    ];
    if active {
        cells.insert(
            2,
            Cell {
                text: yes_no(s.active).to_owned(),
                role: if s.active { Role::Neutral } else { Role::Muted },
            },
        );
    }
    cells
}

/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`
/// for an entry an agent approved on the user's checked instruction
/// (2_design.md §12.3); `None` when they typed the approval.
fn approved_line(e: &EntryPayload) -> Option<String> {
    let by = e.approved_by.as_ref()?;
    let inst = e.instruction.as_ref()?;
    Some(scribe_core::ask::instructed_line(
        "approved",
        &by.name,
        scribe_core::ask::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &inst.session,
        inst.at,
    ))
}

/// How a candidate settled on the user's checked instruction reads:
/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`,
/// or `rejected by …`; `None` for one they settled by typing, or still open.
fn settled_line(p: &ProposalPayload) -> Option<String> {
    let inst = p.instruction.as_ref()?;
    let by = p.settled_author.as_ref()?;
    Some(scribe_core::ask::instructed_line(
        p.state,
        &by.name,
        scribe_core::ask::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &inst.session,
        inst.at,
    ))
}

/// A proposal's rows: one, and for a candidate carrying scribe's
/// statement, in the table, a second under it holding the user's quote in
/// the `REASON` column (2_design.md §11.1).
fn proposal_rows(p: &ProposalPayload, mode: OutputMode) -> Vec<Vec<Cell>> {
    let mut rows = vec![proposal_row(p, mode)];
    if mode == OutputMode::Table
        && let Some(quote) = stated_candidate_quote(p)
    {
        let mut under: Vec<Cell> = PROPOSAL_COLUMNS
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
    if mode == OutputMode::Table
        && let Some(line) = settled_line(p)
    {
        let mut under: Vec<Cell> = PROPOSAL_COLUMNS
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

/// Where `REASON` sits in [`PROPOSAL_COLUMNS`].
const PROPOSAL_REASON_COLUMN: usize = 6;

/// `The user's words: "<quote>"` (or the assistant's) for a candidate
/// carrying scribe's statement.
fn stated_candidate_quote(p: &ProposalPayload) -> Option<String> {
    let candidate = p.candidate.as_ref()?;
    candidate.statement.as_ref()?;
    Some(format!(
        "{} \"{}\"",
        scribe_core::ask::words_label(&scribe_core::voice::UserName::default(), candidate.role),
        one_line(&candidate.quote)
    ))
}

/// A proposal's line. A candidate carrying scribe's statement shows the
/// statement as its reason; the plain line adds the user's quote after it,
/// where the table puts it on the row under.
fn proposal_row(p: &ProposalPayload, mode: OutputMode) -> Vec<Cell> {
    let reason = match (
        p.candidate.as_ref().and_then(|c| c.statement.as_deref()),
        stated_candidate_quote(p),
    ) {
        (Some(statement), Some(quote)) if mode != OutputMode::Table => {
            format!("{} | {quote}", one_line(statement))
        }
        (Some(statement), _) => one_line(statement),
        (None, _) => one_line(&p.reason),
    };
    // The plain line carries the instruction after the reason, where the
    // table puts it on a row under (2_design.md §12.3).
    let reason = match settled_line(p) {
        Some(line) if mode != OutputMode::Table => format!("{reason} | {line}"),
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
        Cell::plain(timestamp(p.opened_at)),
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

/// What admitted a pass claim, on one line: the trigger as configured, the
/// turn's words it matched and the criteria in force.
fn admitted(a: &scribe_core::Admitted) -> String {
    format!(
        "trigger {:?} matched {:?}; criteria: {}",
        a.trigger,
        a.matched,
        a.criteria.as_deref().unwrap_or("-")
    )
}

/// A hit's row, the same in the table and the plain line, except that the
/// table cuts the title to [`TITLE_CELL_CHARS`] so the snippet keeps room.
/// A hidden record's cite is muted in the table.
fn hit_row(h: &HitPayload, mode: OutputMode) -> Vec<Cell> {
    let title = one_line(&h.title);
    let hidden = h.active == Some(false) || !matches!(h.status, "active" | "open" | "final");
    vec![
        Cell {
            text: h.cite.clone(),
            role: if hidden { Role::Muted } else { Role::Neutral },
        },
        Cell::plain(h.kind),
        Cell::plain(format!("{:.3}", h.score.as_f64().unwrap_or_default())),
        Cell::plain(timestamp(h.updated)),
        Cell::plain(if mode == OutputMode::Table {
            table::truncate(&title, TITLE_CELL_CHARS)
        } else {
            title
        }),
        Cell::plain(one_line(&h.snippet)),
    ]
}

/// The widest title a hit's table row shows.
const TITLE_CELL_CHARS: usize = 40;

fn transcript_row(t: &TranscriptPayload) -> Vec<Cell> {
    vec![
        Cell::plain(t.session.clone()),
        Cell::plain(t.project.clone()),
        Cell::plain(t.user_turns.to_string()),
        Cell::plain(t.bytes.to_string()),
        Cell::plain(
            t.started
                .map(timestamp)
                .unwrap_or_else(|| String::from("-")),
        ),
        Cell::plain(t.ended.map(timestamp).unwrap_or_else(|| String::from("-"))),
        Cell::plain(one_line(&t.first_prompt)),
    ]
}

fn transcript_detail(out: &mut dyn Write, t: &TranscriptPayload) -> io::Result<()> {
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
        Cell::plain(t.source.unwrap_or("-").to_owned()),
        Cell::plain(t.action.as_ref().map_or_else(
            || String::from("-"),
            |a| {
                if a.require_quote {
                    format!("{},quote", a.mode)
                } else {
                    a.mode.to_owned()
                }
            },
        )),
        Cell::plain(
            t.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| a.retired.to_string()),
        ),
        Cell::plain(
            t.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| a.rejected.to_string()),
        ),
        Cell::plain(t.route.unwrap_or("-").to_owned()),
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

/// Render scaffold artifacts and actionable next steps without claiming capture.
pub(super) fn init(
    out: &mut dyn Write,
    sink: &Sink,
    report: &scribe_core::InitReport,
) -> io::Result<()> {
    if sink.mode == OutputMode::Json {
        return write_json(out, report);
    }
    for file in &report.files {
        writeln!(out, "{}\t{}", file.status, file.path.display())?;
    }
    for step in &report.next {
        writeln!(out, "{step}")?;
    }
    Ok(())
}
