//! `#scribe` as an annotation (docs/design/scribe/2_design.md §14): a
//! marker anywhere in a sentence Daniel typed, cataloged at the end of the
//! same turn by one bounded model call into a statement with his verbatim
//! quote.
//!
//! This module is pure: [`marks`] finds the markers of a turn and the
//! mentions that are not markers (§14.1), [`window`] builds the bounded
//! window the model reads (§14.3), [`request`] the call, and [`check`] the
//! deterministic checks of an answer before anything is written. The
//! prompt hook and the `Stop` hook do the I/O.
//!
//! A line that starts with `#scribe` is a marker too (§14.6): forms 1 to 3
//! fold into it, so `#scribe idea: …` hints `idea` and `#scribe add this to
//! preferences` hints `preferences`. Only a whole-line settling command
//! (form 4, `crate::capture::scan`) is not a marker.
//!
//! No I/O (STD-02 R5).

use crate::capture::{GLOBAL_OPENER, MARKER};
use crate::entry::TagName;
use crate::models::{
    ANNOTATION_BUDGET, AnnotationMark, AnnotationMessage, AnnotationRequest, AnnotationTag,
    MarkAnswer,
};
use crate::record::{Topic, Topics};
use crate::tag::TagTable;
use crate::transcript::Message;
use crate::{Speaker, verbatim};
use std::time::Duration;

/// The most markers one turn may hold; a fourth is refused
/// [`MarkProblem::TooMany`] before any model call (§14.5).
pub const MAX_MARKS: usize = 3;
/// The most marked turns one `Stop` sends to the model, oldest first; the
/// rest wait for the next `Stop` (§14.5).
pub const MAX_TURNS_PER_STOP: usize = 2;
/// The most model calls one marked turn gets across `Stop`s; after the
/// last failure its markers are refused `model_failed` (§14.5).
pub const MAX_ATTEMPTS: u32 = 3;
/// How many messages before the marked turn the window keeps (§14.3).
pub const WINDOW_BEFORE: usize = 8;
/// The most characters of the marked turn the window keeps.
pub const TURN_CHARS: usize = 8_000;
/// The most characters of any other message the window keeps.
pub const MESSAGE_CHARS: usize = 3_000;
/// The most characters the window holds in all; older messages go first.
pub const WINDOW_CHARS: usize = 40_000;
/// The longest statement an annotation records, in characters.
pub const STATEMENT_CHARS: usize = 300;
/// The longest subject, in characters.
pub const SUBJECT_CHARS: usize = 80;
/// The shortest quote, in characters (as §6.4's check).
pub const QUOTE_MIN_CHARS: usize = 8;
/// The longest quote, in characters.
pub const QUOTE_MAX_CHARS: usize = 1_500;
/// The longest reason a `none` answer gives, in characters.
pub const REASON_CHARS: usize = 200;
/// What the window shows where it cut a message.
const CUT: &str = "[… cut]";

/// Why a marker is refused before any model call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkProblem {
    /// A settling verb after a marker inside a sentence: settling needs
    /// its own line (§14.1).
    Settling,
    /// `#scribe/<word>` with a word other than `global`.
    BadOpener,
    /// A marker past [`MAX_MARKS`] in one turn.
    TooMany,
}

impl MarkProblem {
    /// The refusal code it is recorded and audited with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Settling | Self::BadOpener => "unparsed_instruction",
            Self::TooMany => "too_many_marks",
        }
    }

    /// Why, for Daniel's echo.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Settling => String::from(
                "settling needs its own line: #scribe approve|reject <p-id> or #scribe retire|forget <entry-id>",
            ),
            Self::BadOpener => format!("only {MARKER} and {GLOBAL_OPENER} mark a message"),
            Self::TooMany => format!("at most {MAX_MARKS} #scribe markers in one message"),
        }
    }
}

/// One `#scribe` marker of a typed turn (§14.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Its number in the turn, from 1, in text order.
    pub number: u32,
    /// The line it is on, from 1, as `capture::lines` numbers lines.
    pub line: u32,
    /// That line as typed.
    pub said: String,
    /// It was `#scribe/global`: the entry's project is `global`.
    pub global: bool,
    /// The word right after it, lowercased and without a trailing `:`,
    /// when there is one; a hint only when it names a tag ([`Mark::hint`]).
    pub word: Option<String>,
    /// For a marker that opens its line, the last word after `to`, `in` or
    /// `under` (and an optional `my` or `the`) at the end of the line: the
    /// tag of a folded form 2 or 3 (`#scribe add this to preferences`).
    pub tail: Option<String>,
    /// Hashtags after it on its line: the entry's topics.
    pub topics: Topics,
    /// Why it is refused before any call; `None` for a marker the model
    /// reads.
    pub problem: Option<MarkProblem>,
}

impl Mark {
    /// The tag its next word names (a tag or an alias), else the tag its
    /// line ends with ([`Mark::tail`]), which the model may not change;
    /// `None` when neither names one.
    #[must_use]
    pub fn hint<'a>(&self, tags: &'a TagTable) -> Option<&'a TagName> {
        [self.word.as_deref(), self.tail.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|word| tags.resolve(word))
            .map(|def| &def.name)
            .next()
    }
}

/// The tag word a folded form 2 or 3 line ends with: `… (to|in|under)
/// [my|the] <tag>`, before any trailing hashtags.
fn tail(rest: &str) -> Option<String> {
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    while words.last().is_some_and(|w| w.starts_with('#')) {
        words.pop();
    }
    let tag = words.pop()?.trim_end_matches(PUNCTUATION).to_lowercase();
    if words
        .last()
        .is_some_and(|w| DETERMINERS.contains(&w.to_lowercase().as_str()))
    {
        words.pop();
    }
    words
        .last()
        .is_some_and(|w| PREPOSITIONS.contains(&w.to_lowercase().as_str()))
        .then_some(tag)
        .filter(|tag| !tag.is_empty())
}

/// What a word after a marker sheds at its end.
const PUNCTUATION: [char; 9] = [':', ',', '.', ';', '!', '?', ')', ']', '}'];

/// The prepositions before the tag that ends a folded form 2 or 3 line.
const PREPOSITIONS: [&str; 3] = ["to", "in", "under"];
/// The optional determiners before that tag.
const DETERMINERS: [&str; 2] = ["my", "the"];

/// The markers of a typed turn, in text order, numbered from 1 (§14.1).
///
/// Lines inside code fences, blockquotes and `<pasted_content>` blocks hold
/// none (as `capture::plain_lines` leaves them out), nor does an indented
/// code line or a whole-line settling command (form 4). Within a line,
/// a token in an inline code span or a double-quoted span (`"…"`, `“…”`) is
/// a mention, not a marker; a backtick or quote with no partner on its line
/// (`a 12" pipe`) opens no span. A marker is [`MARKER`] or `#scribe/<word>`
/// that starts the line or follows whitespace or an opening bracket, and
/// is followed by the end of the line, whitespace, `:`, a closing bracket
/// or sentence punctuation.
#[must_use]
pub fn marks(text: &str) -> Vec<Mark> {
    let mut found = Vec::new();
    for (line, said) in crate::capture::plain_lines(text) {
        // A settling line is a command (form 4); an indented line is code.
        if crate::capture::settles(said) || said.starts_with("    ") || said.starts_with('\t') {
            continue;
        }
        for start in tokens(said) {
            let number = u32::try_from(found.len() + 1).unwrap_or(u32::MAX);
            let mut mark = parse(number, line, said, start);
            if found.len() >= MAX_MARKS {
                mark.problem = Some(MarkProblem::TooMany);
            }
            found.push(mark);
        }
    }
    found
}

/// Byte offsets in `line` where a marker starts, outside code and quotes.
fn tokens(line: &str) -> Vec<usize> {
    let lone = unpaired(line);
    let mut out = Vec::new();
    let (mut code, mut quoted, mut curly) = (false, false, false);
    let mut prev: Option<char> = None;
    for (at, c) in line.char_indices() {
        match c {
            _ if lone.contains(&at) => {}
            '`' => code = !code,
            '"' if !code => quoted = !quoted,
            '“' if !code => curly = true,
            '”' if !code => curly = false,
            '#' if !code && !quoted && !curly => {
                let opens = prev.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | '[' | '{'));
                let rest = &line[at..];
                let ends = rest.strip_prefix(MARKER).is_some_and(|after| {
                    after.is_empty()
                        || after.starts_with(char::is_whitespace)
                        || after.starts_with([':', '/', ')', ']', '}', ',', '.', ';', '!', '?'])
                });
                if opens && ends {
                    out.push(at);
                }
            }
            _ => {}
        }
        prev = Some(c);
    }
    out
}

/// Byte offsets of the span openers in `line` that no partner closes: the
/// last backtick of an odd count, the last straight quote of an odd count
/// outside code, and a `“` with no `”` after it. Such an opener is a stray
/// character (`a 12" pipe`), so it hides nothing that follows it.
fn unpaired(line: &str) -> Vec<usize> {
    let mut lone = Vec::new();
    let ticks: Vec<usize> = line.match_indices('`').map(|(at, _)| at).collect();
    if ticks.len() % 2 == 1 {
        lone.extend(ticks.last());
    }
    let mut code = false;
    let mut quotes = Vec::new();
    let mut open_curly = None;
    for (at, c) in line.char_indices() {
        match c {
            '`' if !lone.contains(&at) => code = !code,
            '"' if !code => quotes.push(at),
            '“' if !code && open_curly.is_none() => open_curly = Some(at),
            '”' if !code => open_curly = None,
            _ => {}
        }
    }
    if quotes.len() % 2 == 1 {
        lone.extend(quotes.last());
    }
    lone.extend(open_curly);
    lone
}

/// The marker starting at byte `start` of `said`.
fn parse(number: u32, line: u32, said: &str, start: usize) -> Mark {
    let rest = &said[start + MARKER.len()..];
    let mut words = rest.split_whitespace();
    let opener_tail = if rest.starts_with('/') {
        // Sentence punctuation may follow the opener: `… #scribe/global.`
        words.next().map(|w| w.trim_end_matches(PUNCTUATION))
    } else {
        None
    };
    let (global, bad) = match opener_tail {
        None => (false, false),
        Some(tail) => (tail == "/global", tail != "/global"),
    };
    // `#scribe: idea` and `#scribe idea:` both read `idea` as the word.
    let mut next = words.next().map(|w| w.trim_start_matches(':'));
    if next.is_some_and(str::is_empty) {
        next = words.next();
    }
    let word = next
        .map(|w| w.trim_end_matches(PUNCTUATION).to_lowercase())
        .filter(|w| !w.is_empty() && !w.starts_with('#'));
    let mut topics: Vec<Topic> = Vec::new();
    for hashtag in rest.split_whitespace().filter_map(|w| w.strip_prefix('#')) {
        let tag = hashtag.trim_end_matches(PUNCTUATION).to_lowercase();
        if let Ok(topic) = Topic::try_from(tag)
            && !topics.contains(&topic)
            && topics.len() < Topics::MAX
            && topic.as_str() != "scribe"
        {
            topics.push(topic);
        }
    }
    let problem = if bad {
        Some(MarkProblem::BadOpener)
    } else if word
        .as_deref()
        .is_some_and(|w| crate::capture::SETTLING.contains(&w))
    {
        Some(MarkProblem::Settling)
    } else {
        None
    };
    let opens_line = said[..start].trim().is_empty();
    Mark {
        number,
        line,
        said: said.to_owned(),
        global,
        word,
        tail: if opens_line { tail(rest) } else { None },
        topics: Topics::try_from(topics).unwrap_or_default(),
        problem,
    }
}

/// The window one call reads (§14.3): the [`WINDOW_BEFORE`] messages with
/// text before the marked turn, the marked turn (cut to [`TURN_CHARS`]),
/// and the assistant's reply after it up to the next user turn; every
/// other message cut to [`MESSAGE_CHARS`]. Over [`WINDOW_CHARS`] in all,
/// the oldest messages before the turn go first, then the reply's last.
/// Empty when `marked` is not in `messages`.
#[must_use]
pub fn window(messages: &[Message], marked: &str) -> Vec<AnnotationMessage> {
    let Some(at) = messages.iter().position(|m| m.uuid == marked) else {
        return Vec::new();
    };
    let item = |m: &Message, max: usize, is_marked: bool| AnnotationMessage {
        uuid: m.uuid.clone(),
        role: m.role,
        text: cut(m.text().trim(), max),
        marked: is_marked,
    };
    let has_text = |m: &&Message| !m.text().trim().is_empty();
    let before: Vec<&Message> = messages[..at].iter().filter(has_text).collect();
    let mut before: Vec<AnnotationMessage> = before[before.len().saturating_sub(WINDOW_BEFORE)..]
        .iter()
        .map(|m| item(m, MESSAGE_CHARS, false))
        .collect();
    let turn = item(&messages[at], TURN_CHARS, true);
    let mut after: Vec<AnnotationMessage> = messages[at + 1..]
        .iter()
        .take_while(|m| m.role == Speaker::Assistant)
        .filter(has_text)
        .map(|m| item(m, MESSAGE_CHARS, false))
        .collect();
    let size = |b: &[AnnotationMessage], a: &[AnnotationMessage]| {
        b.iter()
            .chain(a)
            .map(|m| m.text.chars().count())
            .sum::<usize>()
            + turn.text.chars().count()
    };
    while size(&before, &after) > WINDOW_CHARS && !before.is_empty() {
        before.remove(0);
    }
    while size(&before, &after) > WINDOW_CHARS && !after.is_empty() {
        after.pop();
    }
    before.into_iter().chain([turn]).chain(after).collect()
}

/// `text` cut to `max` characters, with [`CUT`] when it was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push(' ');
    out.push_str(CUT);
    out
}

/// The call for one marked turn: its markers the model reads (those with
/// no problem), the window and every configured tag with its criteria,
/// voiced with `voice`.
#[must_use]
pub fn request(
    marks: &[&Mark],
    window: Vec<AnnotationMessage>,
    tags: &TagTable,
    model: &str,
    voice: crate::voice::Voice,
) -> AnnotationRequest {
    AnnotationRequest {
        voice,
        model: model.to_owned(),
        budget: budget(),
        marks: marks
            .iter()
            .map(|m| AnnotationMark {
                marker: m.number,
                line: m.said.clone(),
                hint: m.hint(tags).map(ToString::to_string),
            })
            .collect(),
        messages: window,
        tags: tags
            .tags()
            .iter()
            .map(|def| AnnotationTag {
                name: def.name.to_string(),
                description: def.description.clone(),
                criteria: def.capture.criteria.as_ref().map(|c| c.value.clone()),
            })
            .collect(),
    }
}

/// The call's budget (§14.3).
#[must_use]
pub fn budget() -> Duration {
    ANNOTATION_BUDGET
}

/// What an answer for one marker comes to after the checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checked {
    /// An entry to write.
    Entry(Annotated),
    /// The model found nothing to record, with its reason.
    None(String),
}

/// A checked entry, before the store gives it an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotated {
    /// The tag.
    pub tag: TagName,
    /// Scribe's statement: the entry's content.
    pub statement: String,
    /// What it is about.
    pub subject: String,
    /// Daniel's verbatim span: the entry's `quote`.
    pub quote: String,
    /// The uuid of the typed turn the quote is from.
    pub message: String,
    /// When that turn was written.
    pub at: time::OffsetDateTime,
}

/// Why an answer for a marker is refused (§14.3, "Checks after the
/// answer"). Each is recorded `refused` with its code; the model is not
/// asked again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckRefusal {
    /// The quote is not a verbatim span of Daniel's own words in a typed
    /// turn of the window.
    NotVerbatim,
    /// The tag is not configured, or not the hint's.
    WrongTag,
    /// A statement, subject, quote or reason out of bounds, or a kind that
    /// is neither `entry` nor `none`.
    OutOfBounds,
}

impl CheckRefusal {
    /// The refusal code it is recorded and audited with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotVerbatim => "quote_not_verbatim",
            Self::WrongTag => "unknown_tag",
            Self::OutOfBounds => "annotation_out_of_bounds",
        }
    }

    /// Why, for Daniel's echo.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotVerbatim => "the model's quote is not word for word in a message you typed",
            Self::WrongTag => {
                "the model chose a tag that is not configured or not the one you named"
            }
            Self::OutOfBounds => "the model's answer is out of bounds",
        }
    }
}

/// The deterministic checks of one marker's answer (§14.3). `typed` are
/// the window's turns Daniel plainly typed, the marked one included; the
/// quote must be a verbatim span of his own words in the one it names.
///
/// # Errors
///
/// A [`CheckRefusal`] when a check fails.
pub fn check(
    mark: &Mark,
    answer: &MarkAnswer,
    typed: &[&Message],
    tags: &TagTable,
) -> Result<Checked, CheckRefusal> {
    match answer.kind.as_str() {
        "none" => {
            let reason = answer.reason.trim();
            if reason.chars().count() > REASON_CHARS {
                return Err(CheckRefusal::OutOfBounds);
            }
            Ok(Checked::None(if reason.is_empty() {
                String::from("nothing to record")
            } else {
                reason.to_owned()
            }))
        }
        "entry" => {
            let statement = answer.statement.trim();
            let subject = answer.subject.trim();
            let quote = answer.quote.trim();
            let quote_chars = quote.chars().count();
            if statement.is_empty()
                || statement.chars().count() > STATEMENT_CHARS
                || subject.is_empty()
                || subject.chars().count() > SUBJECT_CHARS
                || !(QUOTE_MIN_CHARS..=QUOTE_MAX_CHARS).contains(&quote_chars)
            {
                return Err(CheckRefusal::OutOfBounds);
            }
            let tag = tags
                .resolve(answer.tag.trim())
                .map(|def| def.name.clone())
                .ok_or(CheckRefusal::WrongTag)?;
            if mark.hint(tags).is_some_and(|hint| *hint != tag) {
                return Err(CheckRefusal::WrongTag);
            }
            let message = typed
                .iter()
                .find(|m| m.uuid == answer.message)
                .ok_or(CheckRefusal::NotVerbatim)?;
            let own = crate::transcript::user_turn(message)
                .map(|turn| turn.own)
                .ok_or(CheckRefusal::NotVerbatim)?;
            if !crate::pass::own_marked_span(&own, quote)
                || !verbatim::contains(&message.text(), quote)
            {
                return Err(CheckRefusal::NotVerbatim);
            }
            Ok(Checked::Entry(Annotated {
                tag,
                statement: statement.to_owned(),
                subject: subject.to_owned(),
                quote: quote.to_owned(),
                message: message.uuid.clone(),
                at: message.at,
            }))
        }
        _ => Err(CheckRefusal::OutOfBounds),
    }
}

/// The entry a checked annotation records, before the store gives it an
/// id (§14.4): Daniel's, with scribe's statement as its content, his
/// words in `quote`, the marker's line in `said`, and `turn` the marked
/// turn (its `at` when it was typed), cited with `message`, its uuid.
/// `#scribe/global` records it for every project.
#[must_use]
pub fn draft(
    mark: &Mark,
    annotated: Annotated,
    turn: &crate::capture::Turn,
    message: &str,
    tags: &TagTable,
) -> crate::NewEntry {
    let mut cite = turn.cite(mark.line);
    cite.message = Some(message.to_owned());
    crate::NewEntry {
        project: if mark.global {
            crate::Project::sanitized(crate::Project::GLOBAL)
        } else {
            turn.project.clone()
        },
        topics: mark.topics.clone(),
        related: Vec::new(),
        provider: turn.provider.clone(),
        host: turn.host.clone(),
        created: turn.at,
        said: mark.said.clone(),
        content: annotated.statement,
        content_from: crate::ContentFrom::Message {
            role: Speaker::User,
            message: annotated.message,
            at: annotated.at,
        },
        cite,
        review_after: tags
            .get(&annotated.tag)
            .and_then(|def| def.review_after(turn.at.date())),
        window: None,
        quote: Some(annotated.quote),
        stated_by: Some(crate::Author::annotation()),
        tag: annotated.tag,
    }
}

/// Where one marker stands in `marks.json` (2_design.md §14.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkState {
    /// An entry was written for it (or an earlier one already was).
    Entry,
    /// The model found nothing to record.
    None,
    /// Refused, with its code: never sent to the model again.
    Refused,
    /// The model call failed; the next `Stop` tries again.
    Pending,
}

impl MarkState {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::None => "none",
            Self::Refused => "refused",
            Self::Pending => "pending",
        }
    }

    /// Whether the marker is settled: anything but `pending`.
    #[must_use]
    pub fn is_final(self) -> bool {
        self != Self::Pending
    }
}

/// One marker's outcome as `marks.json` keeps it: ids and codes only,
/// never words (2_design.md §14.5).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkRecord {
    /// The session the marked turn is in.
    pub session: crate::SessionId,
    /// The marked turn's message uuid.
    pub message: String,
    /// The marked turn's `prompt_sha256`.
    pub prompt_sha256: String,
    /// The marker's number in the turn, from 1.
    pub marker: u32,
    /// Where it stands.
    pub outcome: MarkState,
    /// For `entry`, the entry's id; `null` otherwise.
    pub entry: Option<crate::EntryId>,
    /// For `refused`, the refusal code; `null` otherwise.
    pub code: Option<String>,
    /// Model calls made for it so far.
    pub attempts: u32,
    /// When the outcome was last written.
    #[serde(with = "time::serde::rfc3339")]
    pub at: time::OffsetDateTime,
}

impl MarkRecord {
    /// Whether the fields agree with the outcome.
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.marker >= 1
            && !self.message.is_empty()
            && match self.outcome {
                MarkState::Entry => self.entry.is_some() && self.code.is_none(),
                MarkState::Refused => {
                    self.entry.is_none() && self.code.as_deref().is_some_and(|c| !c.is_empty())
                }
                MarkState::None => self.entry.is_none() && self.code.is_none(),
                MarkState::Pending => {
                    self.entry.is_none() && self.code.is_none() && self.attempts < MAX_ATTEMPTS
                }
            }
    }

    /// Whether it is the marker `marker` of message `message`.
    #[must_use]
    pub fn is(&self, message: &str, marker: u32) -> bool {
        self.message == message && self.marker == marker
    }
}
