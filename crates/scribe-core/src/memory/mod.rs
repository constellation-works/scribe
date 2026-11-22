//! Memory: who the user is, what they have coming up, and what happened
//! (docs/design/scribe/2_design.md §15.5, §15.6). One fixed shape, the same
//! for every user, with no switches:
//!
//! - `memory/profile.md`: one document of seven sections fixed in code
//!   ([`profile::HEADINGS`]), rebuilt weekly by `scribe memory refresh`
//!   ([`profile`]); its `Pinned` section is the user's own and is copied
//!   byte for byte.
//! - `memory/now/<date>-<slug>.md`: something the user plans or must meet,
//!   always with a date ([`NowItem`]). It stays [`NOW_KEEP_DAYS`] days past
//!   its date; then the refresh promotes it to a milestone or moves it to
//!   `memory_history`.
//! - `memory/milestones/<date>-<slug>.md`: a dated event that changed a
//!   profile fact or that the user called a milestone ([`Milestone`]).
//!
//! The session pass writes only `now` items and stated milestones, from the
//! user's own turns ([`check`], [`drafts`], [`plan`]). An undated `now`
//! candidate waits in `now_pending` until a later window of the same session
//! dates it, and is dropped when the session goes final. [`block`] renders
//! the `## Profile`, `## Now` and `## Milestones` block that `scribe memory
//! show` prints and `scribe brief` embeds. The items of formats 19–24
//! ([`legacy`]) are read once and migrated ([`migrate`], §15.11).
//!
//! No I/O (STD-02 R5): the store reads and writes the files and the
//! database, the CLI reads the transcript and the clock.

pub mod legacy;
pub mod migrate;
pub mod profile;

use crate::entry::Speaker;
use crate::models::{MemoryAnswer, PassMessage};
use crate::record::nullable;
use crate::session::SessionId;
use crate::transcript;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use time::{Date, Duration, OffsetDateTime};

/// The most memory elements one pass invocation takes per session, across
/// its windows; a refused element uses its slot (§15.5).
pub const MAX_PER_PASS: usize = 3;
/// The most undated `now` candidates one session keeps pending.
pub const MAX_PENDING: usize = 5;
/// Days a `now` item stays after its date.
pub const NOW_KEEP_DAYS: i64 = 14;
/// How many milestones the block shows, newest first.
pub const MILESTONE_LINES: usize = 5;
/// The longest title, in characters.
pub const TITLE_CHARS: usize = 80;
/// The longest item text, in characters.
pub const TEXT_CHARS: usize = 400;
/// The shortest quote, in characters.
pub const QUOTE_MIN_CHARS: usize = 8;
/// The longest quote, in characters.
pub const QUOTE_CHARS: usize = 400;
/// The longest message id the model may name.
pub const MESSAGE_CHARS: usize = 128;
/// The longest slug of a file stem, in characters.
pub const SLUG_CHARS: usize = 48;
/// The phrases with which the user calls an event a milestone (§15.5): a
/// stated milestone is written only when its turn holds one, matched as a
/// capture trigger.
pub const USER_SAYS: &[&str] = &[
    "milestone",
    "big day",
    "remember this",
    "never forget",
    "life changing",
];

/// What kind of sensitive event an item records (§13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensitiveKind {
    /// A death or a loss.
    Loss,
    /// A health event, never its detail.
    Health,
    /// A relationship event.
    Relationship,
    /// A family event.
    Family,
}

impl SensitiveKind {
    /// The kind's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loss => "loss",
            Self::Health => "health",
            Self::Relationship => "relationship",
            Self::Family => "family",
        }
    }
}

impl fmt::Display for SensitiveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Words that mark financial detail; an element holding one is refused
/// (§13.8). Matched on whole words, case aside.
pub const FINANCE_WORDS: &[&str] = &[
    "salary",
    "salaries",
    "paycheck",
    "wage",
    "wages",
    "income",
    "balance",
    "debt",
    "debts",
    "owe",
    "owes",
    "owed",
    "loan",
    "loans",
    "mortgage",
    "iban",
    "net worth",
    "bank account",
    "account number",
    "card number",
    "credit card",
    "routing number",
];

/// Words that mark a sensitive event, with its kind: when one matches, the
/// element is sensitive whatever the model said (§13.8).
pub const SENSITIVE_WORDS: &[(&str, SensitiveKind)] = &[
    ("died", SensitiveKind::Loss),
    ("passed away", SensitiveKind::Loss),
    ("funeral", SensitiveKind::Loss),
    ("death", SensitiveKind::Loss),
    ("miscarriage", SensitiveKind::Loss),
    ("hospital", SensitiveKind::Health),
    ("surgery", SensitiveKind::Health),
    ("diagnosed", SensitiveKind::Health),
    ("diagnosis", SensitiveKind::Health),
    ("cancer", SensitiveKind::Health),
    ("divorce", SensitiveKind::Relationship),
    ("divorced", SensitiveKind::Relationship),
    ("breakup", SensitiveKind::Relationship),
    ("separated", SensitiveKind::Relationship),
    ("pregnant", SensitiveKind::Family),
    ("pregnancy", SensitiveKind::Family),
];

/// Mood words a `now` statement may hold only when its quote does (§13.4):
/// scribe stores what was said, never an inferred state.
pub const MOOD_WORDS: &[&str] = &[
    "sad",
    "solemn",
    "upset",
    "depressed",
    "anxious",
    "stressed",
    "grieving",
    "happy",
    "excited",
    "angry",
    "mood",
    "worried",
    "frustrated",
    "lonely",
    "nervous",
    "devastated",
];

/// Whether `text` holds `phrase` as whole words, case aside.
fn has_phrase(text: &str, phrase: &str) -> bool {
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect();
    let wanted: Vec<&str> = phrase.split(' ').collect();
    !wanted.is_empty()
        && words
            .windows(wanted.len())
            .any(|w| w.iter().zip(&wanted).all(|(a, b)| a == b))
}

/// The kind of the first sensitive word in `text`, if any.
#[must_use]
pub fn sensitive_word(text: &str) -> Option<SensitiveKind> {
    SENSITIVE_WORDS
        .iter()
        .find(|(word, _)| has_phrase(text, word))
        .map(|(_, kind)| *kind)
}

/// Whether `text` holds a listed finance word.
#[must_use]
pub fn finance_word(text: &str) -> bool {
    FINANCE_WORDS.iter().any(|word| has_phrase(text, word))
}

/// Whether `statement` names a mood that `quote` does not.
#[must_use]
pub fn inferred_mood(statement: &str, quote: &str) -> bool {
    MOOD_WORDS
        .iter()
        .any(|word| has_phrase(statement, word) && !has_phrase(quote, word))
}

/// What a `now` item is: a deadline the user must meet, an event they
/// attend, or something they plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NowKind {
    /// A date by which something is due.
    Deadline,
    /// Something that happens on the date.
    Event,
    /// Something the user plans to do by or on the date.
    Plan,
}

impl NowKind {
    /// The kind's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deadline => "deadline",
            Self::Event => "event",
            Self::Plan => "plan",
        }
    }
}

impl FromStr for NowKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "deadline" => Ok(Self::Deadline),
            "event" => Ok(Self::Event),
            "plan" => Ok(Self::Plan),
            other => Err(format!(
                "'{other}' is not a now kind; use deadline, event or plan"
            )),
        }
    }
}

/// A `now` or milestone file's stem and id: `<YYYY-MM-DD>-<slug>`, the slug
/// lowercase ASCII letters and digits in runs joined by `-`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Stem(String);

impl Stem {
    /// The stem for an item dated `date` titled `title`: the title's words
    /// as a slug, cut at [`SLUG_CHARS`] on a word, or `item` when it has
    /// none.
    #[must_use]
    pub fn new(date: Date, title: &str) -> Self {
        let mut slug = String::new();
        for word in title
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            let extra = usize::from(!slug.is_empty()) + word.len();
            if slug.len() + extra > SLUG_CHARS {
                break;
            }
            if !slug.is_empty() {
                slug.push('-');
            }
            slug.push_str(word);
        }
        if slug.is_empty() {
            slug.push_str("item");
        }
        Self(format!(
            "{}-{slug}",
            crate::record::nullable_date::format(date)
        ))
    }

    /// The stem text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The date the stem starts with.
    #[must_use]
    pub fn date(&self) -> Option<Date> {
        self.0
            .get(..10)
            .and_then(|d| crate::record::parse_day(d).ok())
    }
}

impl TryFrom<String> for Stem {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let slug = value.get(11..).unwrap_or_default();
        let valid = value.len() <= 11 + SLUG_CHARS
            && value
                .get(..10)
                .is_some_and(|d| crate::record::parse_day(d).is_ok())
            && value.as_bytes().get(10) == Some(&b'-')
            && !slug.is_empty()
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && !slug.contains("--")
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(format!(
                "'{value}' is not a memory file stem (<YYYY-MM-DD>-<slug>)"
            ))
        }
    }
}

impl FromStr for Stem {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl From<Stem> for String {
    fn from(stem: Stem) -> Self {
        stem.0
    }
}

impl fmt::Display for Stem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// When an agent last raised a `now` item (`scribe memory ack`): it limits
/// unprompted mentions, never what the block shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raised {
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// In which session.
    pub session: SessionId,
}

/// One `now` item: `memory/now/<stem>.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NowItem {
    /// Its file stem and id.
    pub stem: Stem,
    /// The event date or deadline.
    pub date: Date,
    /// What it is.
    pub kind: NowKind,
    /// The sessions it came from, in the order they were added.
    pub sources: Vec<SessionId>,
    /// When it last changed: the turn that wrote it.
    pub updated: OffsetDateTime,
    /// When an agent last raised it.
    pub raised: Option<Raised>,
    /// Its one-line title.
    pub title: String,
    /// Its prose.
    pub text: String,
}

/// One milestone: `memory/milestones/<stem>.md`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Milestone {
    /// Its file stem and id.
    pub stem: Stem,
    /// When it happened.
    pub date: Date,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    pub updated: OffsetDateTime,
    /// Its one-line title.
    pub title: String,
    /// Its prose.
    pub text: String,
}

/// The frontmatter of a `now` file, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NowFront {
    /// The event date or deadline.
    #[serde(with = "crate::record::nullable_date::required")]
    pub date: Date,
    /// What it is.
    pub kind: NowKind,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// When an agent last raised it.
    #[serde(
        default,
        deserialize_with = "nullable",
        skip_serializing_if = "Option::is_none"
    )]
    pub raised: Option<Raised>,
}

/// The frontmatter of a milestone file, as stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneFront {
    /// When it happened.
    #[serde(with = "crate::record::nullable_date::required")]
    pub date: Date,
    /// The sessions it came from.
    pub sources: Vec<SessionId>,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// What kind of memory a pass element writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementKind {
    /// A `now` item, or a pending candidate when it has no date.
    Now,
    /// A milestone the user called one.
    Milestone,
}

/// Why the pass refused a memory element; counted in the pass report by
/// code and audited by code only, never with its text (§13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// `memory_invalid`: an unknown kind, or a field empty or past its
    /// bound.
    Invalid,
    /// `memory_unverified`: the quote is not the user's own verbatim span
    /// of the turn named, in this window and in the original transcript.
    Unverified,
    /// `memory_sensitive`: a credential or anything the redactor changes,
    /// or a listed finance word.
    Sensitive,
    /// `memory_inferred`: a `now` text naming a mood its quote does not.
    Inferred,
    /// `memory_undated`: a milestone, an update or a completion with no
    /// real date within a year before and two years after the turn.
    Undated,
    /// `memory_not_stated`: a milestone whose turn does not call it one.
    NotStated,
    /// `memory_unknown`: an update naming no listed `now` item, or a
    /// completion naming no pending candidate of this session.
    Unknown,
    /// `memory_pending_full`: the session already holds [`MAX_PENDING`]
    /// undated candidates.
    PendingFull,
}

impl Refusal {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "memory_invalid",
            Self::Unverified => "memory_unverified",
            Self::Sensitive => "memory_sensitive",
            Self::Inferred => "memory_inferred",
            Self::Undated => "memory_undated",
            Self::NotStated => "memory_not_stated",
            Self::Unknown => "memory_unknown",
            Self::PendingFull => "memory_pending_full",
        }
    }
}

/// One element that passed every model-free check, on its way to the
/// original-transcript check and the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// What it writes.
    pub kind: ElementKind,
    /// The title, on one line.
    pub title: String,
    /// The prose, on one line.
    pub text: String,
    /// The user's words.
    pub quote: String,
    /// The turn named.
    pub message: String,
    /// The date as the model gave it, checked later against the turn.
    pub date: Option<String>,
    /// A `now` item's kind.
    pub now_kind: NowKind,
    /// The listed `now` item this updates.
    pub update: Option<Stem>,
    /// The pending candidate this dates.
    pub completes: Option<i64>,
}

/// What [`check`] made of a window's elements.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    /// The elements that passed.
    pub checked: Vec<Checked>,
    /// One code per refused element.
    pub refused: Vec<Refusal>,
    /// The slots used: every element taken, written or refused.
    pub used: usize,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Check the model's memory elements for one window (§15.5, §13.8), at most
/// `slots` of them (the rest are ignored and use no slot). In order: the
/// shape and bounds; the quote, a verbatim span of the user's own words of
/// the turn it names in `messages`; the redactor (`masks` says whether it
/// would change a text); the finance words; and the mood rule. Sensitive
/// events are kept: `now` items and milestones may hold them.
#[must_use]
pub fn check(
    answers: &[MemoryAnswer],
    messages: &[PassMessage],
    slots: usize,
    masks: &dyn Fn(&str) -> bool,
) -> CheckReport {
    let mut report = CheckReport::default();
    for answer in answers.iter().take(slots) {
        report.used = report.used.saturating_add(1);
        match check_one(answer, messages, masks) {
            Ok(checked) => report.checked.push(checked),
            Err(refusal) => report.refused.push(refusal),
        }
    }
    report
}

fn check_one(
    answer: &MemoryAnswer,
    messages: &[PassMessage],
    masks: &dyn Fn(&str) -> bool,
) -> Result<Checked, Refusal> {
    let kind = match answer.kind.trim() {
        "now" => ElementKind::Now,
        "milestone" => ElementKind::Milestone,
        _ => return Err(Refusal::Invalid),
    };
    let title = one_line(&answer.title);
    let text = one_line(&answer.text);
    let quote = answer.quote.clone();
    let quote_chars = quote.trim().chars().count();
    if title.is_empty()
        || title.chars().count() > TITLE_CHARS
        || text.is_empty()
        || text.chars().count() > TEXT_CHARS
        || !(QUOTE_MIN_CHARS..=QUOTE_CHARS).contains(&quote_chars)
        || answer.message.is_empty()
        || answer.message.chars().count() > MESSAGE_CHARS
    {
        return Err(Refusal::Invalid);
    }
    let now_kind = match kind {
        ElementKind::Now => answer
            .now_kind
            .as_deref()
            .unwrap_or("plan")
            .parse()
            .map_err(|_| Refusal::Invalid)?,
        ElementKind::Milestone => NowKind::Event,
    };
    let update = match answer
        .update
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(stem) => Some(stem.parse::<Stem>().map_err(|_| Refusal::Unknown)?),
        None => None,
    };
    let completes = match answer
        .completes
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(id) => Some(
            id.strip_prefix("pending-")
                .unwrap_or(id)
                .parse::<i64>()
                .map_err(|_| Refusal::Unknown)?,
        ),
        None => None,
    };
    if kind == ElementKind::Milestone && (update.is_some() || completes.is_some()) {
        return Err(Refusal::Invalid);
    }
    let own = messages.iter().any(|m| {
        m.uuid == answer.message
            && m.role == Speaker::User
            && crate::pass::own_span(&m.text, &quote)
    });
    if !own {
        return Err(Refusal::Unverified);
    }
    if masks(&title) || masks(&text) || masks(&quote) {
        return Err(Refusal::Sensitive);
    }
    if finance_word(&title) || finance_word(&text) || finance_word(&quote) {
        return Err(Refusal::Sensitive);
    }
    if inferred_mood(&format!("{title} {text}"), &quote) {
        return Err(Refusal::Inferred);
    }
    Ok(Checked {
        kind,
        title,
        text,
        quote,
        message: answer.message.clone(),
        date: answer
            .date
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned),
        now_kind,
        update,
        completes,
    })
}

/// A checked element tied to its turn in the original transcript, ready for
/// the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The element.
    pub checked: Checked,
    /// The date, kept only when real and within a year before and two years
    /// after the turn.
    pub date: Option<Date>,
    /// The user's own words of the turn (pasted blocks left out), against
    /// which the store repeats the verbatim check under its lock.
    pub turn: String,
    /// When the turn was sent.
    pub at: OffsetDateTime,
}

/// Tie each checked element to its turn in the original transcript, never
/// the model's metadata: only the user's turns, and only when the quote is
/// their own span of that turn's original text. An element that fails here
/// is refused `memory_unverified`.
#[must_use]
pub fn drafts(
    checked: Vec<Checked>,
    messages: &[transcript::Message],
) -> (Vec<Draft>, Vec<Refusal>) {
    let mut out = Vec::new();
    let mut refused = Vec::new();
    for element in checked {
        let found = messages
            .iter()
            .find(|m| m.uuid == element.message && m.role == Speaker::User)
            .and_then(|m| Some((m, transcript::user_turn(m)?)))
            .filter(|(m, turn)| {
                crate::pass::own_span(&turn.own, &element.quote)
                    && m.text().contains(&element.quote)
            });
        let Some((message, turn)) = found else {
            refused.push(Refusal::Unverified);
            continue;
        };
        let date = element
            .date
            .as_deref()
            .and_then(|text| crate::record::parse_day(text).ok())
            .filter(|date| plausible_date(*date, message.at.date()));
        out.push(Draft {
            checked: element,
            date,
            turn: turn.own,
            at: message.at,
        });
    }
    (out, refused)
}

/// Whether `date` lies within a year before and two years after `turn`.
#[must_use]
pub fn plausible_date(date: Date, turn: Date) -> bool {
    date >= turn - Duration::days(365) && date <= turn + Duration::days(730)
}

/// Whether the user's `turn` calls its event a milestone: one of
/// [`USER_SAYS`] occurs in it, matched as a capture trigger.
#[must_use]
pub fn user_says_milestone(turn: &str) -> bool {
    USER_SAYS
        .iter()
        .any(|phrase| crate::trigger::find(phrase, turn).is_some())
}

/// An undated `now` candidate kept in `now_pending` (§15.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// Its row id; the model names it `pending-<id>` to date it.
    pub id: i64,
    /// The session it came from.
    pub session: SessionId,
    /// The turn it came from.
    pub message: String,
    /// What it is about: the element's title.
    pub subject: String,
    /// The element's prose.
    pub text: String,
}

/// One change a pass window's memory makes (§15.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PassChange {
    /// Write a `now` file whole.
    Now(NowItem),
    /// Write a milestone file whole.
    Milestone(Milestone),
    /// Keep an undated candidate.
    AddPending {
        /// The turn.
        message: String,
        /// The title.
        subject: String,
        /// The prose.
        text: String,
    },
    /// Drop a pending candidate a dated element completed.
    DropPending(i64),
}

/// The changes `drafts` make for `session` against the current `now` items,
/// milestones and this session's pending candidates, and the elements
/// refused under the lock. In order, per draft:
///
/// - **milestone**: needs a date and a turn that calls it one; written by
///   stem (`<date>-<slug of title>`), merging its sources into one already
///   there.
/// - **dated `now`**: an update rewrites the listed item in place, keeping
///   its stem and raised mark; a completion writes the item with the
///   candidate's session among its sources and drops the candidate;
///   otherwise the item is written by stem, joining one already there.
/// - **undated `now`**: kept pending, one per subject, at most
///   [`MAX_PENDING`] per session; an undated update or completion is
///   refused.
///
/// Each file's `updated` is its turn's time, so a replayed window writes
/// the same bytes.
#[must_use]
pub fn plan(
    drafts: &[Draft],
    now_items: &[NowItem],
    milestones: &[Milestone],
    pending: &[Pending],
    session: &SessionId,
) -> (Vec<PassChange>, Vec<Refusal>) {
    let mut changes: Vec<PassChange> = Vec::new();
    let mut refused = Vec::new();
    let mut now: Vec<NowItem> = now_items.to_vec();
    let mut miles: Vec<Milestone> = milestones.to_vec();
    let mut pending: Vec<Pending> = pending
        .iter()
        .filter(|p| &p.session == session)
        .cloned()
        .collect();
    let add_source = |sources: &mut Vec<SessionId>, s: &SessionId| {
        if !sources.contains(s) {
            sources.push(s.clone());
        }
    };
    for draft in drafts {
        let element = &draft.checked;
        if !crate::pass::own_span(&draft.turn, &element.quote) {
            refused.push(Refusal::Unverified);
            continue;
        }
        match (element.kind, draft.date) {
            (ElementKind::Milestone, None) => refused.push(Refusal::Undated),
            (ElementKind::Milestone, Some(date)) => {
                if !user_says_milestone(&draft.turn) {
                    refused.push(Refusal::NotStated);
                    continue;
                }
                let stem = Stem::new(date, &element.title);
                let item = match miles.iter_mut().find(|m| m.stem == stem) {
                    Some(m) => {
                        add_source(&mut m.sources, session);
                        m.title.clone_from(&element.title);
                        m.text.clone_from(&element.text);
                        m.updated = draft.at;
                        m.clone()
                    }
                    None => {
                        let m = Milestone {
                            stem,
                            date,
                            sources: vec![session.clone()],
                            updated: draft.at,
                            title: element.title.clone(),
                            text: element.text.clone(),
                        };
                        miles.push(m.clone());
                        m
                    }
                };
                changes.push(PassChange::Milestone(item));
            }
            (ElementKind::Now, None) => {
                if element.update.is_some() || element.completes.is_some() {
                    refused.push(Refusal::Undated);
                    continue;
                }
                if pending.iter().any(|p| p.subject == element.title) {
                    continue;
                }
                if pending.len() >= MAX_PENDING {
                    refused.push(Refusal::PendingFull);
                    continue;
                }
                pending.push(Pending {
                    id: 0,
                    session: session.clone(),
                    message: element.message.clone(),
                    subject: element.title.clone(),
                    text: element.text.clone(),
                });
                changes.push(PassChange::AddPending {
                    message: element.message.clone(),
                    subject: element.title.clone(),
                    text: element.text.clone(),
                });
            }
            (ElementKind::Now, Some(date)) => {
                let mut sources = Vec::new();
                if let Some(id) = element.completes {
                    let Some(at) = pending.iter().position(|p| p.id == id && p.id != 0) else {
                        refused.push(Refusal::Unknown);
                        continue;
                    };
                    let done = pending.remove(at);
                    add_source(&mut sources, &done.session);
                    changes.push(PassChange::DropPending(id));
                }
                add_source(&mut sources, session);
                let target = match &element.update {
                    Some(stem) => match now.iter().position(|n| &n.stem == stem) {
                        Some(i) => Some(i),
                        None => {
                            refused.push(Refusal::Unknown);
                            continue;
                        }
                    },
                    None => {
                        let stem = Stem::new(date, &element.title);
                        now.iter().position(|n| n.stem == stem)
                    }
                };
                let item = match target {
                    Some(i) => {
                        let n = &mut now[i];
                        for s in &sources {
                            add_source(&mut n.sources, s);
                        }
                        n.date = date;
                        n.kind = element.now_kind;
                        n.title.clone_from(&element.title);
                        n.text.clone_from(&element.text);
                        n.updated = draft.at;
                        n.clone()
                    }
                    None => {
                        let n = NowItem {
                            stem: Stem::new(date, &element.title),
                            date,
                            kind: element.now_kind,
                            sources,
                            updated: draft.at,
                            raised: None,
                            title: element.title.clone(),
                            text: element.text.clone(),
                        };
                        now.push(n.clone());
                        n
                    }
                };
                changes.push(PassChange::Now(item));
            }
        }
    }
    (changes, refused)
}

/// Whether a `now` item dated `date` is past its stay on `today`: more than
/// [`NOW_KEEP_DAYS`] days after its date.
#[must_use]
pub fn expired(date: Date, today: Date) -> bool {
    date + Duration::days(NOW_KEEP_DAYS) < today
}

/// The profile as the block shows it: its text and the date of the
/// revision it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileView {
    /// `profile.md`, or the committed revision while a journal is open.
    pub text: String,
    /// The day of its last revision; `None` for a file no revision wrote.
    pub as_of: Option<Date>,
}

/// The memory block (§15.4): `## Profile (as of <date>)` with the profile
/// in full, `## Now` with every item nearest date first, and `## Milestones`
/// with the last [`MILESTONE_LINES`], newest first. An empty part prints
/// its heading and `(none)`.
#[must_use]
pub fn block(profile: Option<&ProfileView>, now: &[NowItem], milestones: &[Milestone]) -> String {
    let mut out = String::new();
    match profile {
        Some(p) => {
            match p.as_of {
                Some(date) => out.push_str(&format!(
                    "## Profile (as of {})\n",
                    crate::record::nullable_date::format(date)
                )),
                None => out.push_str("## Profile\n"),
            }
            out.push_str(p.text.trim_end());
            out.push('\n');
        }
        None => out.push_str("## Profile\n(none)\n"),
    }
    out.push_str("## Now\n");
    let mut now: Vec<&NowItem> = now.iter().collect();
    now.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.stem.cmp(&b.stem)));
    if now.is_empty() {
        out.push_str("(none)\n");
    }
    for n in now {
        out.push_str(&format!(
            "- {} · due {} · {}\n",
            n.stem,
            crate::record::nullable_date::format(n.date),
            n.title
        ));
    }
    out.push_str("## Milestones\n");
    let shown = newest_milestones(milestones);
    if shown.is_empty() {
        out.push_str("(none)\n");
    }
    for m in shown {
        out.push_str(&format!(
            "- {} · {} · {}\n",
            m.stem,
            crate::record::nullable_date::format(m.date),
            m.title
        ));
    }
    out
}

/// The milestones the block shows: the last [`MILESTONE_LINES`], newest
/// first.
#[must_use]
pub fn newest_milestones(milestones: &[Milestone]) -> Vec<&Milestone> {
    let mut shown: Vec<&Milestone> = milestones.iter().collect();
    shown.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.stem.cmp(&a.stem)));
    shown.truncate(MILESTONE_LINES);
    shown
}
