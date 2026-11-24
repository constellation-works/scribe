//! `#scribe` as an annotation (docs/design/scribe/2_design.md §14): a
//! marker anywhere in a sentence Daniel typed, cataloged at the end of the
//! same turn by one bounded model call into a paragraph of context with
//! his verbatim quote.
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

pub use crate::capture::{MAX_MARKS, Mark, MarkProblem, marks, turn_marks};
use crate::entry::TagName;
use crate::models::{
    ANNOTATION_BUDGET, AnnotationMark, AnnotationMessage, AnnotationRequest, AnnotationTag,
    MarkAnswer,
};
use crate::tag::TagTable;
use crate::transcript::Message;
use crate::{Speaker, verbatim};
use std::time::Duration;

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
/// The longest context an annotation records, in characters (the claim
/// bound, 2_design.md §15.3).
pub const CONTEXT_CHARS: usize = crate::CONTEXT_CHARS;
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

/// Refuse a marker explicitly naming a typed record before asking a model.
#[must_use]
pub fn record_tag_refusal(mark: &Mark, tags: &TagTable) -> Option<CheckRefusal> {
    mark.hint(tags)
        .filter(|tag| crate::entry::is_decision_or_item(tag.as_str()))
        .map(|_| CheckRefusal::RecordTag)
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

/// Whether `def` may be written by `#scribe` marker capture. Handoffs,
/// decisions and open items are written through `entry add`.
fn is_annotatable(def: &crate::tag::TagDef) -> bool {
    let tag = def.name.as_str();
    tag != crate::handoff::HANDOFF_TAG && !crate::entry::is_decision_or_item(tag)
}

/// The call for one marked turn: its markers the model reads (those with
/// no problem), the window and every annotatable configured tag with its
/// criteria, voiced with `voice`.
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
            .filter(|def| is_annotatable(def))
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
    /// Scribe's paragraph of context: the entry's content.
    pub context: String,
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
    /// The marker names a decision or open item, written through entry add.
    RecordTag,
    /// A context (or one under two sentences), subject, quote or reason
    /// out of bounds, or a kind that
    /// is neither `entry` nor `none`.
    OutOfBounds,
}

impl CheckRefusal {
    /// The refusal code it is recorded and audited with.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotVerbatim => "quote_not_verbatim",
            Self::WrongTag | Self::RecordTag => "unknown_tag",
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
            Self::RecordTag => "record decisions and open items through scribe entry add",
            Self::OutOfBounds => "the model's answer is out of bounds",
        }
    }
}

/// `own_span` for an annotation (§14.3): a line-start `#scribe` line is a
/// marker whose words are Daniel's own (§14.6), so a quote may come from
/// it; a settling command's line is still left out.
pub(crate) fn own_marked_span(text: &str, quote: &str) -> bool {
    verbatim::contains(text, quote)
        && text.contains(quote)
        && crate::claim::own_lines_where(text, |line| !crate::capture::settles(line))
            .contains(quote)
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
    if let Some(refused) = record_tag_refusal(mark, tags) {
        return Err(refused);
    }
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
            let context = answer.context.trim();
            let subject = answer.subject.trim();
            let quote = answer.quote.trim();
            let quote_chars = quote.chars().count();
            if !crate::context_stands_alone(context)
                || subject.is_empty()
                || subject.chars().count() > SUBJECT_CHARS
                || !(QUOTE_MIN_CHARS..=QUOTE_MAX_CHARS).contains(&quote_chars)
            {
                return Err(CheckRefusal::OutOfBounds);
            }
            let tag = tags
                .resolve(answer.tag.trim())
                .filter(|def| is_annotatable(def))
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
            if !own_marked_span(&own, quote) || !verbatim::contains(&message.text(), quote) {
                return Err(CheckRefusal::NotVerbatim);
            }
            Ok(Checked::Entry(Annotated {
                tag,
                context: context.to_owned(),
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
/// id (§14.4): Daniel's, with scribe's context as its content, his
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
        content: annotated.context,
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

    /// Whether every call for it failed: refused `model_failed` after
    /// [`MAX_ATTEMPTS`] (§14.5). `scribe mark retry` opens it again.
    #[must_use]
    pub fn gave_up(&self) -> bool {
        self.outcome == MarkState::Refused && self.code.as_deref() == Some(crate::MODEL_FAILED)
    }

    /// Whether no entry or answer settled it yet: pending a retry, or given
    /// up ([`MarkRecord::gave_up`]). `scribe mark list` lists these.
    #[must_use]
    pub fn uncataloged(&self) -> bool {
        self.outcome == MarkState::Pending || self.gave_up()
    }
}
