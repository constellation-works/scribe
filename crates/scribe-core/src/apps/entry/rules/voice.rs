//! Whose words an entry holds and how a line names them: the entry-side
//! half of `voice`, used by the entry, proposal and search apps. The
//! prompt half (`[user]` and `[instructions]`) is in `config::voice`.
//!
//! No I/O (STD-02 R5).

use crate::config::voice::UserName;
use crate::entry::{ContentFrom, Entry, Speaker};
use time::OffsetDateTime;

/// Whose instruction a line the user reads names: `entry show`, `record
/// search` and `proposal list` speak to them.
pub const INSTRUCTION_YOURS: &str = "your";

/// Whose words an approved pass statement is, in `ask` evidence and in
/// `scribe search` snippets (2_design.md §11.1).
#[must_use]
pub fn stated_whose(user: &UserName) -> String {
    format!("scribe's statement of {}'s turn", user.noun())
}

/// Whose words an approved pass statement of `speaker`'s turn is, in
/// `ask` evidence, `scribe search` snippets and `entry show`
/// (2_design.md §11.1, §15.3).
#[must_use]
pub fn stated_whose_for(user: &UserName, speaker: Speaker) -> String {
    match speaker {
        Speaker::User => stated_whose(user),
        Speaker::Assistant => String::from("scribe's statement of the assistant's turn"),
    }
}

/// Whose words an agent's statement of the user's spoken instruction is, in
/// `ask` evidence, `scribe search` snippets and `entry show` (2_design.md
/// §16.6).
#[must_use]
pub fn agent_stated_whose(user: &UserName, agent: &str) -> String {
    format!("agent ({agent})'s statement of {}'s turn", user.noun())
}

/// The label before the user's own words beside such a statement.
#[must_use]
pub fn stated_quote_label(user: &UserName) -> String {
    format!("{}'s words:", user.title())
}

/// The label before a quote of `speaker`'s words beside scribe's
/// statement of their turn.
#[must_use]
pub fn words_label(user: &UserName, speaker: Speaker) -> String {
    match speaker {
        Speaker::User => stated_quote_label(user),
        Speaker::Assistant => String::from("The assistant's words:"),
    }
}

// --- whose words an entry holds -------------------------------------------

/// The calendar day of `at`, as a record writes a date.
pub fn day(at: OffsetDateTime) -> String {
    crate::record::nullable_date::format(at.date())
}

/// Whose words `entry`'s content is when `stated_quote` returned its quote.
#[must_use]
pub fn stated_whose_of(user: &UserName, entry: &Entry) -> String {
    match entry
        .stated_by
        .as_ref()
        .filter(|by| by.kind == crate::AuthorKind::Agent)
    {
        Some(by) => agent_stated_whose(user, by.name.as_str()),
        None => stated_whose_for(user, speaker_of(entry)),
    }
}

/// Whose turn an entry's content states: the assistant's only for a
/// message entry from an assistant turn.
#[must_use]
pub fn speaker_of(entry: &Entry) -> Speaker {
    match entry.content_from {
        ContentFrom::Message { role, .. } => role,
        ContentFrom::Agent | ContentFrom::Instruction => Speaker::User,
    }
}

/// Whose words an entry the session pass captured itself under a `write`
/// catcher is (2_design.md §11.6, §15.2): nobody typed or approved it, and
/// an assistant turn's words are never the user's.
#[must_use]
pub fn captured_whose(user: &UserName, speaker: Speaker) -> String {
    let noun = user.noun();
    match speaker {
        Speaker::User => format!(
            "scribe's statement of {noun}'s turn, captured by the session pass (not typed by {noun})"
        ),
        Speaker::Assistant => format!(
            "scribe's statement of the assistant's turn, captured by the session pass (not {noun}'s words)"
        ),
    }
}

/// The speaker's verbatim words beside an entry whose content is scribe's
/// or an agent's statement of their turn: one the user typed/marked,
/// approved, or spoke on instruction, or one the pass captured; `None` for
/// every other entry.
pub fn stated_quote(entry: &Entry) -> Option<&str> {
    entry
        .stated_by
        .as_ref()
        .and(entry.quote.as_deref())
        .filter(|_| {
            entry.author.kind == crate::AuthorKind::User
                || is_pass_captured(entry)
                || is_stated_assistant(entry)
                || is_agent_stated(entry)
        })
}

/// Whether `entry` is an agent-written entry on the user's spoken
/// instruction (2_design.md §16.6): `content` is the agent's statement and
/// `quote` is the user's verbatim words.
fn is_agent_stated(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Agent
        && entry.content_from == ContentFrom::Agent
        && entry
            .stated_by
            .as_ref()
            .is_some_and(|by| by.kind == crate::AuthorKind::Agent)
}

/// Whether `entry` is an approved non-finding assistant claim whose
/// content is scribe's context of the assistant's turn (2_design.md §15.3).
fn is_stated_assistant(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Agent
        && !entry.is_finding()
        && matches!(
            entry.content_from,
            ContentFrom::Message {
                role: Speaker::Assistant,
                ..
            }
        )
}

/// Whether the session pass wrote `entry` itself (an idea under a `write`
/// tag), rather than the user typing or approving it.
#[must_use]
pub fn is_pass_captured(entry: &Entry) -> bool {
    entry.author.kind == crate::AuthorKind::Scribe && entry.outcome.is_some()
}

/// An idea's outcome as one line of evidence or snippet text: `open`,
/// `parked, in the user's words: "<reason>"` or `task <id>`.
#[must_use]
pub fn outcome_text(outcome: &crate::Outcome, user: &UserName) -> String {
    match (outcome.state, &outcome.reason, &outcome.task) {
        (crate::OutcomeState::Parked, Some(reason), _) => {
            format!("parked, in {}'s words: \"{reason}\"", user.noun())
        }
        (crate::OutcomeState::Task, _, Some(task)) => format!("task {task}"),
        (state, ..) => state.as_str().to_owned(),
    }
}

/// Whose instruction a line in `ask` evidence names: the model reads it.
#[must_use]
pub fn instruction_theirs(user: &UserName) -> String {
    format!("{}'s", user.noun())
}

/// How a settlement on the user's checked instruction reads (2_design.md
/// §12.3): `approved by <aide> on your instruction "<quote>" (<session>,
/// <date>)`. `verb` is `approved` or `rejected`; `whose` is
/// [`INSTRUCTION_YOURS`] or [`instruction_theirs`]. A forgotten entry's
/// instruction has no quote, and the line then names only the turn.
#[must_use]
pub fn instructed_line(
    verb: &str,
    aide: &str,
    whose: &str,
    quote: Option<&str>,
    session: &str,
    at: OffsetDateTime,
) -> String {
    let quoted = quote
        .map(|q| {
            format!(
                " \"{}\"",
                q.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .unwrap_or_default();
    format!(
        "{verb} by {aide} on {whose} instruction{quoted} ({session}, {})",
        day(at)
    )
}

/// The approved-by line of an entry an agent approved on the user's
/// instruction; `None` for one they approved by typing, or never approved.
#[must_use]
pub fn approved_line(entry: &Entry, whose: &str) -> Option<String> {
    let by = entry.approved_by.as_ref()?;
    let inst = entry.instruction.as_ref()?;
    Some(instructed_line(
        "approved",
        &by.name.to_string(),
        whose,
        inst.quote.as_deref(),
        inst.session.as_str(),
        inst.at,
    ))
}
