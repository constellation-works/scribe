//! Rendering for a single record, with its untruncated detail fields.

use super::{
    GROUP_COLUMNS, admitted, age, approved_line, author, col, due_role, entry_row, group_row,
    joined, one_line, or_dash, plain_line, proposal_row, session_row, settled_line, shown_id,
    timestamp, transcript_row, write_json, yes_no,
};
use crate::output::color;
use crate::output::payload::{
    EntryPayload, HandoffPayload, OriginPayload, Record, SessionHandoffsPayload, SessionPayload,
    TagShowPayload, TranscriptPayload,
};
use crate::output::sink::{OutputMode, Sink};
use crate::output::table::{self, Align, Cell, Column};
use scribe_core::ShortIds;
use std::io::{self, Write};

const MEMORY_RULE_COLUMNS: [Column; 4] = [
    col("RULE", Align::Left, false),
    col("VALUE", Align::Left, false),
    col("ORIGIN", Align::Left, false),
    col("MEANING", Align::Left, true),
];

/// One record: a field-per-line detail view, one tab-separated line (the
/// same line its list prints), or a JSON object. The detail view never
/// truncates: it is where a truncated table cell is read in full (STD-01
/// R15).
pub(crate) fn show(
    out: &mut dyn Write,
    sink: &Sink,
    shorts: &ShortIds,
    record: &Record,
) -> io::Result<()> {
    match (sink.mode, record) {
        (OutputMode::Json, Record::Group(g)) => write_json(out, g),
        (OutputMode::Plain, Record::Group(g)) => {
            writeln!(out, "{}", plain_line(&group_row(g, Some(shorts))))
        }
        (OutputMode::Table, Record::Group(g)) => {
            for (label, cell) in GROUP_COLUMNS
                .iter()
                .take(GROUP_COLUMNS.len() - 1)
                .zip(group_row(g, Some(shorts)))
            {
                field(out, &label.to_ascii_lowercase(), &cell.text)?;
            }
            if let Some(summary) = &g.summary {
                field(out, "summary", summary)?;
            }
            Ok(())
        }
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
            writeln!(
                out,
                "{}",
                plain_line(&proposal_row(p, OutputMode::Plain, shorts))
            )
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
                if let Some(context) = crate::output::payload::claim_context(candidate) {
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
        (OutputMode::Json, Record::Entry(entry)) => write_json(out, entry),
        (OutputMode::Json, Record::Session(session)) => write_json(out, session),
        (OutputMode::Json, Record::SessionHandoffs(session)) => write_json(out, session),
        (OutputMode::Plain, Record::Entry(entry)) => {
            writeln!(out, "{}", plain_line(&entry_row(entry, OutputMode::Plain)))?;
            // Scribe's statement is the row's content; Daniel's own words
            // follow on their line, with whose statement it is.
            if let (Some(by), Some(quote)) = (&entry.stated_by, &entry.quote) {
                writeln!(out, "stated_by\t{}\t{}", author(by), one_line(quote))?;
            }
            // An approval an agent made on his instruction (2_design.md §12.3).
            if let Some(line) = approved_line(entry, Some(shorts)) {
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
            if entry.decision.is_some()
                && let Some(quote) = &entry.quote
            {
                writeln!(out, "quote\t{}", one_line(quote))?;
            }
            for (label, value) in entry_record_fields(entry) {
                writeln!(out, "{label}\t{}", one_line(&value))?;
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
            writeln!(
                out,
                "{}",
                plain_line(&session_row(
                    session,
                    false,
                    Some(shorts),
                    OutputMode::Plain
                ))
            )?;
            if let Some(at) = session.forgotten {
                writeln!(out, "forgotten\t{}", timestamp(at))?;
            }
            for handoff in session.handoffs.iter().flatten() {
                writeln!(
                    out,
                    "handoff\t{}\t{}\t{}\t{}",
                    handoff.id,
                    author(&handoff.author),
                    timestamp(handoff.created),
                    format_args!(
                        "{}{}",
                        handoff
                            .handoff_mode
                            .map_or(String::new(), |m| format!("mode: {} ", m.as_str())),
                        one_line(&handoff.content)
                    )
                )?;
            }
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
        (OutputMode::Plain, Record::SessionHandoffs(session)) => {
            let latest = session
                .handoffs
                .first()
                .map_or_else(|| String::from("-"), |h| timestamp(h.created));
            writeln!(
                out,
                "{}\thandoff-only\t{}\t{}\t-\t{}\tno session record",
                shown_id(&session.id, Some(shorts)),
                yes_no(session.due),
                session.project,
                latest
            )?;
            for handoff in &session.handoffs {
                writeln!(
                    out,
                    "handoff\t{}\t{}\t{}\t{}",
                    handoff.id,
                    author(&handoff.author),
                    timestamp(handoff.created),
                    format_args!(
                        "{}{}",
                        handoff
                            .handoff_mode
                            .map_or(String::new(), |m| format!("mode: {} ", m.as_str())),
                        one_line(&handoff.content)
                    )
                )?;
            }
            Ok(())
        }
        (OutputMode::Table, Record::Entry(entry)) => entry_detail(out, sink, entry),
        (OutputMode::Table, Record::Session(session)) => session_detail(out, sink, session),
        (OutputMode::Table, Record::SessionHandoffs(session)) => {
            session_handoffs_detail(out, sink, session)
        }
        (OutputMode::Json, Record::Transcript(transcript)) => write_json(out, transcript),
        (OutputMode::Plain, Record::Transcript(transcript)) => {
            writeln!(
                out,
                "{}",
                plain_line(&transcript_row(transcript, OutputMode::Plain, Some(shorts)))
            )?;
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
            writeln!(
                out,
                "{}",
                plain_line(&[
                    Cell::plain(item.id.clone()),
                    Cell::plain(item.kind.to_owned()),
                    Cell::plain(item.date.clone().unwrap_or_else(|| String::from("-"))),
                    Cell::plain(item.title.clone().unwrap_or_else(|| String::from("-"))),
                ])
            )?;
            for line in item.text.lines() {
                writeln!(out, "text\t{line}")?;
            }
            Ok(())
        }
        (OutputMode::Table, Record::Memory(item)) => memory_detail(out, item),
        (OutputMode::Json, Record::MemoryRules(rules)) => write_json(out, rules),
        (OutputMode::Plain | OutputMode::Table, Record::MemoryRules(rules)) => {
            memory_rules(out, sink, rules)
        }
        (OutputMode::Json, Record::MemoryBlock(block)) => write_json(out, block.as_ref()),
        (OutputMode::Plain | OutputMode::Table, Record::MemoryBlock(block)) => {
            write!(out, "{}", block.text)
        }
        (OutputMode::Json, Record::MemoryRevision(r)) => write_json(out, r.as_ref()),
        (OutputMode::Plain | OutputMode::Table, Record::MemoryRevision(r)) => {
            write!(out, "{}", r.text)?;
            if !r.text.ends_with('\n') {
                writeln!(out)?;
            }
            Ok(())
        }
        (OutputMode::Json, Record::MemoryRefresh(r)) => write_json(out, r.as_ref()),
        (OutputMode::Plain, Record::MemoryRefresh(r)) => writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            r.result,
            r.revision
                .map_or_else(|| String::from("-"), |n| n.to_string()),
            r.sessions,
            r.backlog,
            r.milestones.len(),
            r.removed.len()
        ),
        (OutputMode::Table, Record::MemoryRefresh(r)) => {
            field(out, "result", r.result)?;
            field(
                out,
                "revision",
                &r.revision
                    .map_or_else(|| String::from("-"), |n| n.to_string()),
            )?;
            field(out, "sessions", &r.sessions.to_string())?;
            field(out, "backlog", &r.backlog.to_string())?;
            field(out, "milestones", &or_none(&r.milestones))?;
            field(out, "removed", &or_none(&r.removed))
        }
        (OutputMode::Json, Record::MemoryPrepare(p)) => write_json(out, p.as_ref()),
        (OutputMode::Plain, Record::MemoryPrepare(p)) => writeln!(
            out,
            "{}\t{}\t{}\t{}",
            p.nothing_new,
            p.session_ids.len(),
            p.backlog,
            p.passed_stems.len()
        ),
        (OutputMode::Table, Record::MemoryPrepare(p)) => {
            field(
                out,
                "nothing_new",
                if p.nothing_new { "true" } else { "false" },
            )?;
            field(out, "sessions", &p.session_ids.len().to_string())?;
            field(out, "backlog", &p.backlog.to_string())?;
            field(out, "passed", &p.passed_stems.len().to_string())?;
            field(
                out,
                "corrections",
                yes_no(p.inputs.as_ref().is_some_and(|i| i.corrections.is_some())),
            )
        }
        (OutputMode::Json, Record::MemoryRefreshRetry(r)) => write_json(out, r.as_ref()),
        (OutputMode::Plain, Record::MemoryRefreshRetry(r)) => {
            writeln!(out, "{}\t{}\t{}", r.status, r.code, one_line(&r.correction))
        }
        (OutputMode::Table, Record::MemoryRefreshRetry(r)) => {
            field(out, "status", r.status)?;
            field(out, "code", r.code)?;
            field(out, "correction", &r.correction)
        }
        (OutputMode::Json, Record::StoreMigrate(r)) => write_json(out, r.as_ref()),
        (OutputMode::Plain, Record::StoreMigrate(r)) => writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            r.sessions,
            r.switches,
            r.cursors,
            r.moved_to.as_deref().unwrap_or("-"),
            r.entries,
            r.claims,
            r.tags,
            r.marks,
            r.pickups,
            r.audit_lines,
            r.archives,
            r.archived
        ),
        (OutputMode::Table, Record::StoreMigrate(r)) => {
            field(out, "sessions", &r.sessions.to_string())?;
            field(out, "switches", &r.switches.to_string())?;
            field(out, "cursors", &r.cursors.to_string())?;
            field(out, "entries", &r.entries.to_string())?;
            field(out, "claims", &r.claims.to_string())?;
            field(out, "tags", &r.tags.to_string())?;
            field(out, "marks", &r.marks.to_string())?;
            field(out, "pickups", &r.pickups.to_string())?;
            field(out, "audit lines", &r.audit_lines.to_string())?;
            if r.audit_torn > 0 {
                field(out, "audit torn", &r.audit_torn.to_string())?;
            }
            field(out, "moved to", r.moved_to.as_deref().unwrap_or("-"))?;
            field(out, "archives", &r.archives.to_string())?;
            field(out, "archived", &r.archived.to_string())
        }
        (OutputMode::Json, Record::Export(r)) => write_json(out, r.as_ref()),
        (OutputMode::Plain, Record::Export(r)) => {
            for file in &r.files {
                writeln!(out, "{file}")?;
            }
            Ok(())
        }
        (OutputMode::Table, Record::Export(r)) => {
            field(out, "dir", &r.dir)?;
            field(out, "format", r.format)?;
            field(out, "sessions", &r.sessions.to_string())?;
            field(out, "entries", &r.entries.to_string())?;
            field(out, "claims", &r.claims.to_string())?;
            field(out, "audit lines", &r.audit_lines.to_string())?;
            field(out, "files", &r.files.len().to_string())?;
            field(out, "skipped", &or_none(&r.skipped))
        }
        (OutputMode::Json, Record::Brief(brief)) => write_json(out, brief),
        (OutputMode::Plain | OutputMode::Table, Record::Brief(brief)) => {
            write!(out, "{}", brief.text)
        }
    }
}

fn or_none(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join(", ")
    }
}

fn memory_detail(out: &mut dyn Write, m: &crate::output::payload::MemoryPayload) -> io::Result<()> {
    let or_dash = |v: Option<&str>| v.unwrap_or("-").to_owned();
    field(out, "id", &m.id)?;
    field(out, "kind", m.kind)?;
    if m.kind == "profile" {
        field(out, "as_of", &or_dash(m.as_of.as_deref()))?;
        return writeln!(out, "\n{}", m.text.trim_end());
    }
    field(out, "date", &or_dash(m.date.as_deref()))?;
    if let Some(kind) = m.now_kind {
        field(out, "now_kind", kind)?;
    }
    field(out, "title", &or_dash(m.title.as_deref()))?;
    field(out, "text", &m.text)?;
    field(out, "sources", &or_none(&m.sources))?;
    if let Some(updated) = m.updated {
        field(out, "updated", &timestamp(updated))?;
    }
    if m.kind == "now" {
        field(
            out,
            "raised",
            &m.raised.as_ref().map_or_else(
                || String::from("-"),
                |r| format!("{} in {}", timestamp(r.at), r.session),
            ),
        )?;
    }
    Ok(())
}

fn entry_record_fields(entry: &EntryPayload) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    if let Some(state) = entry.state {
        fields.push(("state", state.to_owned()));
    }
    let (options, recommended) = if let Some(decision) = &entry.decision {
        fields.push(("decided_by", decision.decided_by.to_string()));
        if let Some(rationale) = &decision.rationale {
            fields.push(("rationale", rationale.clone()));
        }
        if let Some(reason) = &decision.reason_quote {
            fields.push(("reason_quote", reason.clone()));
        }
        if let Some(id) = &decision.supersedes {
            fields.push(("supersedes", id.to_string()));
        }
        if let Some(id) = &decision.settles {
            fields.push(("settles", id.to_string()));
        }
        (&decision.options, decision.recommended)
    } else if let Some(item) = &entry.item {
        fields.push(("asked_by", item.asked_by.to_string()));
        fields.push(("blocked", item.blocked.clone()));
        fields.push(("answer_in", item.answer_in.to_string()));
        if let Some(why) = &item.why {
            fields.push(("why", why.clone()));
        }
        (&item.options, item.recommended)
    } else {
        return fields;
    };
    fields.extend(
        options
            .iter()
            .enumerate()
            .map(|(i, option)| ("option", format!("{}. {option}", i + 1))),
    );
    if let Some(n) = recommended {
        fields.push(("recommended", n.to_string()));
    }
    if let Some(item) = &entry.item {
        if let Some(answer) = &item.answer {
            fields.push(("answer", answer.words.clone()));
            fields.push((
                "answered",
                format!(
                    "{} in session:{} (recorded by {})",
                    timestamp(answer.at),
                    answer.session,
                    answer.recorded_by
                ),
            ));
        }
        if let Some(closed) = &item.closed {
            fields.push((
                "closed",
                format!("{} by {}", timestamp(closed.at), closed.by),
            ));
            if let Some(decision) = &closed.decision {
                fields.push(("closed_decision", decision.to_string()));
            }
            fields.push(("closed_note", closed.note.clone()));
        }
    }
    fields
}

fn entry_detail(out: &mut dyn Write, sink: &Sink, e: &EntryPayload) -> io::Result<()> {
    entry_metadata(out, e, sink.color)?;
    field(out, "content", or_dash(&e.content))
}

fn entry_metadata(out: &mut dyn Write, e: &EntryPayload, color: bool) -> io::Result<()> {
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
            if c.session == "deleted" && c.line == 0 {
                return String::from("session:deleted");
            }
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
    if let Some(line) = approved_line(e, None) {
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
        let speaker = if e.content_from.role == Some("assistant") {
            scribe_core::Speaker::Assistant
        } else {
            scribe_core::Speaker::User
        };
        let whose = if e.outcome.is_some() {
            scribe_core::voice::captured_whose(&user, speaker)
        } else if by.kind == "agent" {
            scribe_core::voice::agent_stated_whose(&user, &by.name)
        } else {
            scribe_core::voice::stated_whose_for(&user, speaker)
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
    for (label, value) in entry_record_fields(e) {
        field(out, label, &value)?;
    }
    if let Some(outcome) = &e.outcome {
        field(
            out,
            "outcome",
            &scribe_core::voice::outcome_text(outcome, &scribe_core::voice::UserName::default()),
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
    rules: &crate::output::payload::MemoryRulesPayload,
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
    if let Some(short) = &s.short_id {
        field(out, "short id", short)?;
    }
    field(out, "title", or_dash(&s.title))?;
    field(out, "status", s.status)?;
    if let Some(at) = s.forgotten {
        field(out, "forgotten", &timestamp(at))?;
    }
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
    if let Some(handoffs) = s.handoffs.as_deref().filter(|h| !h.is_empty()) {
        session_handoffs(out, handoffs)?;
    }
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

fn session_handoffs_detail(
    out: &mut dyn Write,
    sink: &Sink,
    s: &SessionHandoffsPayload,
) -> io::Result<()> {
    let due = color::paint(yes_no(s.due), due_role(s.due), sink.color);
    field(out, "id", &s.id)?;
    field(
        out,
        "record",
        if s.on || s.due {
            "none (handoff only; no catalog record yet)"
        } else {
            "none (handoff only; session was not switched on)"
        },
    )?;
    field(out, "on", yes_no(s.on))?;
    field(out, "due", &due)?;
    field(out, "project", &s.project)?;
    session_handoffs(out, &s.handoffs)
}

fn session_handoffs(out: &mut dyn Write, handoffs: &[HandoffPayload]) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "## Handoffs")?;
    for h in handoffs {
        writeln!(out)?;
        writeln!(
            out,
            "### {} · {} · {}",
            h.id,
            author(&h.author),
            timestamp(h.created)
        )?;
        if let Some(mode) = h.handoff_mode {
            writeln!(out, "mode: {}", mode.as_str())?;
        }
        writeln!(out, "{}", h.content.trim_end())?;
    }
    Ok(())
}

fn session_log(out: &mut dyn Write, s: &SessionPayload) -> io::Result<()> {
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

fn joined_lines(items: &[String]) -> String {
    if items.is_empty() {
        String::from("-")
    } else {
        items.join("\n")
    }
}
