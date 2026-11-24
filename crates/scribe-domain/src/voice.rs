//! Who prompts name and what each model use is told to do: the
//! configurable half of every prompt (`[user]` and `[instructions]` in
//! `config.toml`).
//!
//! A prompt is the use's instruction text followed by its output contract.
//! The instruction text says what to do and what to catch; `config.toml`
//! may replace it per use. The contract (the answer's JSON shape, the
//! verbatim-quote rule, what is never kept, messages being data) is fixed
//! here in code and always follows. Both halves name the person whose
//! sessions scribe records as "the user", or by `[user] name` when the file
//! sets one: `{user}` and `{User}` in any text stand for that noun, the
//! second capitalised to open a sentence.
//!
//! No I/O (STD-02 R5).

use crate::entry::{ContentFrom, Entry, Speaker};
use time::OffsetDateTime;

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

/// The most characters `[user] name` may hold.
pub const USER_NAME_CHARS: usize = 64;

/// The most characters one use's instruction text may hold.
pub const INSTRUCTION_CHARS: usize = 4_000;

/// How prompts name the user: "the user" by default, or `[user] name`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserName(Option<String>);

impl UserName {
    /// A configured name, checked: nonempty after trimming, at most
    /// [`USER_NAME_CHARS`] characters, no control characters and no braces
    /// (which would read as a placeholder).
    ///
    /// # Errors
    ///
    /// Why the name was refused.
    pub fn named(name: &str) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(String::from("user.name must not be empty"));
        }
        if name.chars().count() > USER_NAME_CHARS {
            return Err(format!(
                "user.name must be at most {USER_NAME_CHARS} characters"
            ));
        }
        if name.chars().any(|c| c.is_control() || c == '{' || c == '}') {
            return Err(String::from(
                "user.name must contain no control characters or braces",
            ));
        }
        Ok(Self(Some(name.to_owned())))
    }

    /// The configured name, when there is one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// The noun inside a sentence: `the user`, or the name.
    #[must_use]
    pub fn noun(&self) -> &str {
        self.0.as_deref().unwrap_or("the user")
    }

    /// The noun opening a sentence: `The user`, or the name.
    #[must_use]
    pub fn title(&self) -> &str {
        self.0.as_deref().unwrap_or("The user")
    }

    /// `text` with `{User}` and `{user}` replaced by [`Self::title`] and
    /// [`Self::noun`]. Other braces are left as they are.
    #[must_use]
    pub fn fill(&self, text: &str) -> String {
        text.replace("{User}", self.title())
            .replace("{user}", self.noun())
    }
}

/// One model use whose instruction text `config.toml` may replace, keyed
/// by its name under `[instructions]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUse {
    /// The session pass that writes a session record.
    Session,
    /// Shortening a refused session header.
    SessionHeader,
    /// `memory refresh`'s profile rebuild (2_design.md §15.6).
    Profile,
}

impl ModelUse {
    /// Every use, in the order `config.toml` documents them.
    pub const ALL: [Self; 3] = [Self::Session, Self::SessionHeader, Self::Profile];

    /// Its key under `[instructions]`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::SessionHeader => "session_header",
            Self::Profile => "profile",
        }
    }
}

/// The instruction texts `[instructions]` sets, per use; a use it leaves
/// out keeps its shipped text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instructions {
    texts: Vec<(ModelUse, String)>,
}

impl Instructions {
    /// Set `model_use`'s text, checked: nonempty after trimming, at most
    /// [`INSTRUCTION_CHARS`] characters, and no control characters other
    /// than newlines and tabs.
    ///
    /// # Errors
    ///
    /// Why the text was refused.
    pub fn set(&mut self, model_use: ModelUse, text: &str) -> Result<(), String> {
        let text = check_instruction(model_use.key(), text)?;
        self.texts.retain(|(u, _)| *u != model_use);
        self.texts.push((model_use, text.to_owned()));
        Ok(())
    }

    /// The text `config.toml` set for `model_use`, when it set one.
    #[must_use]
    pub fn get(&self, model_use: ModelUse) -> Option<&str> {
        self.texts
            .iter()
            .find(|(u, _)| *u == model_use)
            .map(|(_, text)| text.as_str())
    }
}

/// `text` trimmed, if it is a valid `instructions.<key>` value: nonempty, at
/// most [`INSTRUCTION_CHARS`] characters, and no control characters other
/// than newlines and tabs.
pub fn check_instruction<'a>(key: &str, text: &'a str) -> Result<&'a str, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("instructions.{key} must not be empty"));
    }
    if text.chars().count() > INSTRUCTION_CHARS {
        return Err(format!(
            "instructions.{key} must be at most {INSTRUCTION_CHARS} characters"
        ));
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(format!(
            "instructions.{key} must contain no control characters other than newlines and tabs"
        ));
    }
    Ok(text)
}

/// What one request's prompt is voiced with: who it names and, when
/// `config.toml` replaced it, the use's instruction text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    /// How the prompt names the user.
    pub user: UserName,
    /// The configured instruction text; `None` keeps the shipped one.
    pub instructions: Option<String>,
}

impl Voice {
    /// The instruction text to send: the configured one, or `shipped`, with
    /// the user's name filled in.
    #[must_use]
    pub fn instructions(&self, shipped: &str) -> String {
        self.user
            .fill(self.instructions.as_deref().unwrap_or(shipped))
    }

    /// Fixed contract text with the user's name filled in.
    #[must_use]
    pub fn fill(&self, text: &str) -> String {
        self.user.fill(text)
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

/// Whose instruction a line the user reads names: `entry show`, `record
/// search` and `proposal list` speak to them.
pub const INSTRUCTION_YOURS: &str = "your";

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
