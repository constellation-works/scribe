//! Payloads and rendering for `scribe entry`.

use super::common::color::role_for_status;
use super::common::console::Sink;
use super::common::detail::{Detail, emit_detail, field, field_role, joined_lines};
use super::common::record::{
    AuthorPayload, CitePayload, InstructionPayload, Timestamp, WarningPayload, admitted, author,
    date, envelope, strings,
};
use super::common::table::{
    Cell, Result, cut, joined, left, list_timestamp, one_line, or_dash, plain_line, right,
    shown_id, table, timestamp,
};
use cw_cli::{Column, Mode};
use scribe_core::{ContentFrom, Entry, ShortIds};
use serde::Serialize;
use std::fmt::{self, Write as _};
use std::io::Write;

/// Where an entry's content came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ContentFromPayload {
    /// `instruction`, `message` or `agent`.
    pub(crate) kind: &'static str,
    /// `user` or `assistant` for a message; `null` for an instruction.
    pub(crate) role: Option<&'static str>,
    /// The message's uuid; `null` for an instruction.
    pub(crate) message: Option<String>,
    /// When the message was sent; `null` for an instruction.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) at: Option<Timestamp>,
}

impl From<&ContentFrom> for ContentFromPayload {
    fn from(from: &ContentFrom) -> Self {
        match from {
            ContentFrom::Agent => Self {
                kind: "agent",
                role: None,
                message: None,
                at: None,
            },
            ContentFrom::Instruction => Self {
                kind: "instruction",
                role: None,
                message: None,
                at: None,
            },
            ContentFrom::Message { role, message, at } => Self {
                kind: "message",
                role: Some(role.as_str()),
                message: Some(message.clone()),
                at: Some(*at),
            },
        }
    }
}

/// One turn of a window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WindowTurnPayload {
    /// `user` or `assistant`.
    pub(crate) role: &'static str,
    /// The transcript message's uuid; `null` for the prompt the line was
    /// typed in.
    pub(crate) message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) text: String,
    /// Whether `text` was cut.
    pub(crate) cut: bool,
}

/// An action entry's transcript window (docs/design/scribe/2_design.md §4.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WindowPayload {
    pub(crate) turns: Vec<WindowTurnPayload>,
    /// Whether older turns were dropped to fit the cap.
    pub(crate) truncated: bool,
    /// How many older turns were dropped.
    pub(crate) omitted: u32,
}

impl From<&scribe_core::Window> for WindowPayload {
    fn from(window: &scribe_core::Window) -> Self {
        Self {
            turns: window
                .turns
                .iter()
                .map(|t| WindowTurnPayload {
                    role: t.role.as_str(),
                    message: t.message.clone(),
                    at: t.at,
                    text: t.text.clone(),
                    cut: t.cut,
                })
                .collect(),
            truncated: window.truncated,
            omitted: window.omitted,
        }
    }
}

/// A consumer's note on an action entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PickupPayload {
    pub(crate) entry: String,
    pub(crate) author: AuthorPayload,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    /// What the consumer made of it, such as `task:<id>`; `null` when it
    /// named nothing.
    pub(crate) result: Option<String>,
    pub(crate) rejected: bool,
}

impl From<&scribe_core::action::Pickup> for PickupPayload {
    fn from(p: &scribe_core::action::Pickup) -> Self {
        Self {
            entry: p.entry.to_string(),
            author: AuthorPayload::from(&p.author),
            at: p.at,
            result: p.result.as_ref().map(ToString::to_string),
            rejected: p.rejected,
        }
    }
}

/// An action entry's state for an intake, at the time of the command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ActionPayload {
    /// Seconds since it was created.
    pub(crate) age_seconds: i64,
    /// When an agent-written entry's cancel window ends; `null` for
    /// the user's, which have none.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) cancel_window_ends: Option<Timestamp>,
    /// Whether an intake may take it now.
    pub(crate) past_cancel_window: bool,
    /// Whether a consumer recorded a pickup.
    pub(crate) picked_up: bool,
    /// Whether a consumer recorded that the user rejected its result.
    pub(crate) rejected: bool,
    pub(crate) pickups: Vec<PickupPayload>,
}

impl From<&scribe_core::action::ActionState> for ActionPayload {
    fn from(state: &scribe_core::action::ActionState) -> Self {
        Self {
            age_seconds: state.age.whole_seconds(),
            cancel_window_ends: state.cancel_window_ends,
            past_cancel_window: state.past_cancel_window,
            picked_up: state.picked_up,
            rejected: state.rejected,
            pickups: state.pickups.iter().map(PickupPayload::from).collect(),
        }
    }
}

/// One entry as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct EntryPayload {
    pub(crate) id: String,
    pub(crate) tag: String,
    /// `active`, `superseded`, `retired` or `forgotten`.
    pub(crate) status: &'static str,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    /// The `#scribe` line as typed.
    pub(crate) said: String,
    /// What the entry records, verbatim.
    pub(crate) content: String,
    pub(crate) content_from: ContentFromPayload,
    pub(crate) cite: Option<CitePayload>,
    pub(crate) superseded_by: Option<String>,
    /// `YYYY-MM-DD`.
    pub(crate) review_after: Option<String>,
    /// Who wrote it; `daniel` for a typed `#scribe` line.
    pub(crate) author: AuthorPayload,
    /// Who last updated or retired it through a CLI verb.
    pub(crate) updated_by: Option<AuthorPayload>,
    pub(crate) retirement_reason: Option<String>,
    /// An agent entry's quote of the user's correction, or the user's verbatim
    /// words beside scribe's statement of them (`stated_by` set); `null`
    /// otherwise.
    pub(crate) quote: Option<String>,
    /// A finding's evidence pointer; `null` for other entries.
    pub(crate) evidence: Option<String>,
    /// The date a finding's result was true, `YYYY-MM-DD`; `null` otherwise.
    pub(crate) as_of: Option<String>,
    /// How many turns the entry's transcript window holds; `null` when it
    /// has none (only typed lines under an action tag do).
    pub(crate) window_turns: Option<usize>,
    /// The window itself, only with `--window`; otherwise `null`.
    pub(crate) window: Option<WindowPayload>,
    /// For an entry under an action tag, where it stands for an intake
    /// (`entry list`, `entry show`, `entry add`, `entry pickup`); `null`
    /// for any other entry and in other commands' output.
    pub(crate) action: Option<ActionPayload>,
    /// For an entry approved from a pass claim, what admitted the claim
    /// (`entry show` only); `null` for any other entry and in other
    /// commands' output.
    pub(crate) admitted: Option<scribe_core::Admitted>,
    /// The agent that approved this candidate on the user's instruction; `null` otherwise.
    pub(crate) approved_by: Option<AuthorPayload>,
    /// The user's checked instruction `approved_by` approved this candidate
    /// on; `null` otherwise. A later retirement or forgetting never
    /// replaces it.
    pub(crate) instruction: Option<InstructionPayload>,
    /// Whose words `content` is when it is a statement of a quoted turn:
    /// `{kind: scribe, name: session-pass}` or `{kind: scribe, name: annotation}`
    /// when scribe wrote it, or `{kind: agent, name: <author>}` when an agent
    /// wrote it on the user's spoken instruction (their words are then in
    /// `quote`); `null` otherwise.
    pub(crate) stated_by: Option<AuthorPayload>,
    /// Where an idea the session pass captured itself stands: `{state:
    /// open|parked|task, reason, task}`, `reason` the user's own words
    /// parking it; `null` for every other entry.
    pub(crate) outcome: Option<scribe_core::Outcome>,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
    pub(crate) decision: Option<scribe_core::Decision>,
    pub(crate) item: Option<scribe_core::OpenItem>,
    /// For an entry under `open-item`, its derived state: `open`,
    /// `answered`, `closed` or `withdrawn` (2_design.md §18.4); `null`
    /// for every other entry.
    pub(crate) state: Option<&'static str>,
    /// The user's checked instruction that retired or forgot this entry;
    /// `null` otherwise.
    pub(crate) removal_instruction: Option<InstructionPayload>,
}

impl EntryPayload {
    /// With the action state an `entry` command computed.
    pub(crate) fn with_action(mut self, state: Option<&scribe_core::action::ActionState>) -> Self {
        self.action = state.map(ActionPayload::from);
        self
    }

    /// With the admission of the claim it was approved from, for `entry show`.
    pub(crate) fn with_admitted(mut self, admitted: Option<scribe_core::Admitted>) -> Self {
        self.admitted = admitted;
        self
    }

    /// With the window included, for `--window`.
    pub(crate) fn with_window(mut self, entry: &Entry) -> Self {
        self.window = entry.window.as_ref().map(WindowPayload::from);
        self
    }
}

impl From<&Entry> for EntryPayload {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.to_string(),
            tag: entry.tag.to_string(),
            status: entry.status.as_str(),
            project: entry.project.to_string(),
            topics: strings(entry.topics.as_slice()),
            related: strings(&entry.related),
            provider: entry.provider.to_string(),
            host: entry.host.to_string(),
            created: entry.created,
            updated: entry.updated,
            said: entry.said.clone(),
            content: entry.content.clone(),
            content_from: ContentFromPayload::from(&entry.content_from),
            cite: entry.cite.as_ref().map(CitePayload::from),
            superseded_by: entry.superseded_by.as_ref().map(ToString::to_string),
            review_after: entry.review_after.map(date),
            author: AuthorPayload::from(&entry.author),
            updated_by: entry.updated_by.as_ref().map(AuthorPayload::from),
            retirement_reason: entry.retirement_reason.clone(),
            quote: entry.quote.clone(),
            evidence: entry.evidence.clone(),
            as_of: entry.as_of.map(date),
            window_turns: entry.window.as_ref().map(|w| w.turns.len()),
            window: None,
            action: None,
            admitted: None,
            approved_by: entry.approved_by.as_ref().map(AuthorPayload::from),
            instruction: entry.instruction.as_ref().map(InstructionPayload::from),
            stated_by: entry.stated_by.as_ref().map(AuthorPayload::from),
            outcome: entry.outcome.clone(),
            handoff_mode: entry.handoff_mode,
            decision: entry.decision.clone(),
            item: entry.item.clone(),
            state: scribe_core::ItemState::of(entry).map(scribe_core::ItemState::as_str),
            removal_instruction: entry
                .removal_instruction
                .as_ref()
                .map(InstructionPayload::from),
        }
    }
}

/// Render an `entry list` result.
pub(crate) fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    items: &[EntryPayload],
    total: usize,
    warnings: &[WarningPayload],
) -> Result {
    let json = envelope("entries", items, total, warnings);
    let mode = sink.terminal().mode();
    sink.emit(&json, |_| {
        // A STATE column, last so the other columns keep their places, only
        // when the list holds an open item (2_design.md §18.5).
        let (columns, rows) = if items.iter().any(|e| e.state.is_some()) {
            (
                entry_columns().into_iter().chain([left("STATE")]).collect(),
                items
                    .iter()
                    .map(|entry| {
                        let mut row = entry_row(entry, mode);
                        row.push(Cell::plain(entry.state.unwrap_or("-")));
                        row
                    })
                    .collect(),
            )
        } else {
            (
                entry_columns(),
                items.iter().map(|entry| entry_row(entry, mode)).collect(),
            )
        };
        table(mode, columns, rows)
    })
}

/// Render a single entry (`entry show` and entry write verbs).
pub(crate) fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    entry: &EntryPayload,
) -> Result {
    emit_detail(sink, entry, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => entry_plain(out, shorts, entry),
        Mode::Table => entry_detail(out, entry),
    })
}

fn entry_columns() -> Vec<Column> {
    vec![
        left("ID"),
        left("STATUS"),
        left("PROJECT"),
        left("TOPICS"),
        left("CREATED"),
        cut("CONTENT"),
        left("AUTHOR"),
        right("AGE"),
    ]
}

fn entry_row(e: &EntryPayload, mode: Mode) -> Vec<Cell> {
    vec![
        Cell::plain(e.id.clone()),
        Cell {
            text: e.status.to_owned(),
            role: role_for_status(e.status),
        },
        Cell::plain(e.project.clone()),
        Cell::plain(joined(&e.topics)),
        Cell::plain(list_timestamp(e.created, mode)),
        Cell::plain(one_line(&e.content)),
        Cell::plain(author(&e.author)),
        Cell::plain(
            e.action
                .as_ref()
                .map_or_else(|| String::from("-"), |a| age(a.age_seconds)),
        ),
    ]
}

fn entry_plain(out: &mut Detail, shorts: &ShortIds, entry: &EntryPayload) -> fmt::Result {
    writeln!(out, "{}", plain_line(&entry_row(entry, Mode::Plain)))?;
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

fn entry_detail(out: &mut Detail, e: &EntryPayload) -> fmt::Result {
    entry_metadata(out, e)?;
    field(out, "content", or_dash(&e.content))
}

fn entry_metadata(out: &mut Detail, e: &EntryPayload) -> fmt::Result {
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
    field(out, "id", &e.id)?;
    field(out, "tag", &e.tag)?;
    field_role(out, "status", e.status, role_for_status(e.status))?;
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

/// `approved by <aide> on your instruction "<quote>" (<session>, <date>)`
/// for an entry an agent approved on the user's checked instruction
/// (2_design.md §12.3); `None` when they typed the approval.
fn approved_line(e: &EntryPayload, shorts: Option<&ShortIds>) -> Option<String> {
    let by = e.approved_by.as_ref()?;
    let inst = e.instruction.as_ref()?;
    Some(scribe_core::voice::instructed_line(
        "approved",
        &by.name,
        scribe_core::voice::INSTRUCTION_YOURS,
        inst.quote.as_deref(),
        &shown_id(&inst.session, shorts),
        inst.at,
    ))
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
