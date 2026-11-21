//! Memory: what scribe has noticed about the user, by section
//! (docs/design/scribe/2_design.md §13, slice (a) of §13.10).
//!
//! A memory item is one statement about the user, written by the session
//! pass of a session switched on with `/scribe on`, with the occurrences
//! (the user's verbatim turns) that support it. Its section is one of five
//! fixed in code ([`Section`]); config switches a section off, never adds
//! one. Writing needs no approval: the pass's checks ([`check`]) and the
//! sensitive-content gate stand in for it (§13.3, §13.8). Whether an item is
//! on the session-start page is derived from the item and the day
//! ([`off_page`]), never stored, so no routine has to run for an item to
//! leave the page.
//!
//! Items carry no `layer`, `person` or `recurs` field: only the parked
//! parts of §13.12-13.14 read them. Slice (b) adds the routine's merges and
//! supersedes ([`consolidate`]) and the promotion of `now` items to
//! `milestones` ([`promote`]); the `working` to `preferences` proposal is
//! slice (c).
//!
//! No I/O (STD-02 R5): the store reads and writes the files, the CLI reads
//! the transcript and the clock.

pub mod consolidate;
pub mod promote;

pub use promote::{PromoteRules, Promotion, PromotionRule};

use crate::entry::Speaker;
use crate::models::{MemoryAnswer, PassMessage};
use crate::record::{Author, Label, Project, nullable, nullable_date};
use crate::session::SessionId;
use crate::transcript;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use time::{Date, Duration, OffsetDateTime};

/// The most memory elements one pass invocation takes per session, across
/// its windows; a refused element uses its slot (§13.3).
pub const MAX_PER_PASS: usize = 3;
/// The longest statement, in characters.
pub const STATEMENT_CHARS: usize = 200;
/// The shortest quote, in characters.
pub const QUOTE_MIN_CHARS: usize = 8;
/// The longest quote, in characters.
pub const QUOTE_CHARS: usize = 400;
/// The longest message id the model may name.
pub const MESSAGE_CHARS: usize = 128;
/// The most active statements a pass request lists for the model to name
/// a repeat by.
pub const MAX_CONTEXT_ITEMS: usize = 40;
/// The longest milestone line on the page, in characters.
pub const MILESTONE_LINE_CHARS: usize = 100;
/// How many `now` items the page shows at most.
pub const MAX_NOW_LINES: usize = 8;
/// The page budget of §13.6, in characters, every part but `milestones`.
pub const PAGE_CHARS: usize = 2400;
/// The characters of milestone lines past which the hook logs
/// `memory_page_over_budget`.
pub const MILESTONES_WARN_CHARS: usize = 600;

/// The five sections, in page order (§13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// Role, skills, hosts and environment; with `personal` on, household
    /// facts as he states them. Global, no decay.
    Profile,
    /// A project's purpose, status and focus, with an as-of date.
    Projects,
    /// How he steers agents, as observed. Describes, never binds.
    Working,
    /// Upcoming things and recent happenings.
    Now,
    /// Big, dated life or project events. Never leave the page.
    Milestones,
}

impl Section {
    /// Every section, in the order the page shows them.
    pub const ALL: [Self; 5] = [
        Self::Now,
        Self::Milestones,
        Self::Profile,
        Self::Projects,
        Self::Working,
    ];

    /// The section's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Projects => "projects",
            Self::Working => "working",
            Self::Now => "now",
            Self::Milestones => "milestones",
        }
    }

    /// Whether an item of this section may carry a sensitive event (§13.8).
    #[must_use]
    pub fn holds_sensitive(self) -> bool {
        matches!(self, Self::Now | Self::Milestones)
    }

    /// Whether an item of this section keeps an event date.
    #[must_use]
    pub fn dated(self) -> bool {
        matches!(self, Self::Now | Self::Milestones)
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Section {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "profile" => Ok(Self::Profile),
            "projects" => Ok(Self::Projects),
            "working" => Ok(Self::Working),
            "now" => Ok(Self::Now),
            "milestones" => Ok(Self::Milestones),
            other => Err(format!(
                "'{other}' is not a memory section; use one of: profile, projects, working, now, milestones"
            )),
        }
    }
}

/// Where an item stands (§13.2). Only an active item is on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryStatus {
    /// On record; on the page while its window lasts.
    Active,
    /// The routine judged it the same as an older item and moved its
    /// occurrences there (`merged_into`).
    Merged,
    /// A newer item contradicts it (`superseded_by`); text kept.
    Superseded,
    /// A `now` item that became a milestone (`promotion`); its occurrences
    /// moved to the milestone.
    Promoted,
    /// Hidden from the page, lists and `ask`; text kept.
    Retired,
    /// Statement and every quote blanked for good.
    Forgotten,
}

impl MemoryStatus {
    /// The status's name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Merged => "merged",
            Self::Superseded => "superseded",
            Self::Promoted => "promoted",
            Self::Retired => "retired",
            Self::Forgotten => "forgotten",
        }
    }
}

impl fmt::Display for MemoryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

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

    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "loss" => Some(Self::Loss),
            "health" => Some(Self::Health),
            "relationship" => Some(Self::Relationship),
            "family" => Some(Self::Family),
            _ => None,
        }
    }
}

impl fmt::Display for SensitiveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A memory item's id, `m-<n>`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MemoryId(String);

impl MemoryId {
    /// The id numbered `n`.
    #[must_use]
    pub fn numbered(n: u64) -> Self {
        Self(format!("m-{n}"))
    }

    /// Its number.
    #[must_use]
    pub fn number(&self) -> u64 {
        self.0
            .strip_prefix("m-")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MemoryId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = value.strip_prefix("m-").is_some_and(|n| {
            !n.is_empty()
                && n.len() <= 19
                && n.bytes().all(|b| b.is_ascii_digit())
                && !n.starts_with('0')
        });
        if valid {
            Ok(Self(value))
        } else {
            Err(format!("'{value}' is not a memory id (m-<n>)"))
        }
    }
}

impl FromStr for MemoryId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl From<MemoryId> for String {
    fn from(id: MemoryId) -> Self {
        id.0
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One turn that supports an item: the user's verbatim words with where and
/// when he said them, all but the quote derived by the pass (§13.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Occurrence {
    /// Its number on the item, from 1; never reused.
    pub n: u32,
    /// When the turn was sent (the transcript's timestamp, not the pass's).
    #[serde(with = "time::serde::rfc3339")]
    pub date: OffsetDateTime,
    /// The session the turn is in.
    pub session: SessionId,
    /// The turn's message uuid.
    pub message: String,
    /// The host the session ran on.
    pub host: Label,
    /// The session's project.
    pub project: Project,
    /// The user's words, verbatim; empty once forgotten.
    pub quote: String,
    /// Who wrote the occurrence: scribe's session pass.
    pub author: Author,
}

/// When a `now` item was last brought up (§13.4): it limits unprompted
/// mentions, never what the page shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acknowledged {
    /// When.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// In which session.
    pub session: SessionId,
    /// The agent that raised it (`memory ack`), or the session pass when
    /// the user raised it himself.
    pub author: Author,
}

/// One memory item: `memory/<id>.md`, frontmatter only (§13.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryItem {
    /// `m-<n>`.
    pub id: MemoryId,
    /// Its section.
    pub section: Section,
    /// Where it stands.
    pub status: MemoryStatus,
    /// scribe's statement about the user, at most [`STATEMENT_CHARS`]
    /// characters; empty once forgotten.
    pub statement: String,
    /// `projects` only: the project the sessions it came from belong to.
    #[serde(deserialize_with = "nullable")]
    pub project: Option<Project>,
    /// `now` and `milestones`: the event's date, when one was said.
    #[serde(with = "nullable_date")]
    pub event_date: Option<Date>,
    /// A sensitive event (§13.8), only ever in `now` or `milestones`.
    pub sensitive: bool,
    /// Its kind, when sensitive.
    #[serde(deserialize_with = "nullable")]
    pub sensitive_kind: Option<SensitiveKind>,
    /// `now` only: when it was last brought up.
    #[serde(deserialize_with = "nullable")]
    pub acknowledged: Option<Acknowledged>,
    /// Why an agent retired it; `null` otherwise.
    #[serde(deserialize_with = "nullable")]
    pub retirement_reason: Option<String>,
    /// `milestones`: the `now` item it was promoted from, when there was
    /// one (format 20).
    #[serde(default, deserialize_with = "nullable")]
    pub promoted_from: Option<MemoryId>,
    /// A promotion's journal (format 20): on the milestone from its first
    /// write, so a crash between the files is completed by the next scan;
    /// on the `now` item once it is `promoted`.
    #[serde(default, deserialize_with = "nullable")]
    pub promotion: Option<Promotion>,
    /// `merged`: the item its occurrences moved to (format 20).
    #[serde(default, deserialize_with = "nullable")]
    pub merged_into: Option<MemoryId>,
    /// `superseded`: the item that contradicts it (format 20).
    #[serde(default, deserialize_with = "nullable")]
    pub superseded_by: Option<MemoryId>,
    /// Items the routine judged unrelated to this one, newer than it, so
    /// the pair is never offered again (format 20).
    #[serde(default)]
    pub unrelated_to: Vec<MemoryId>,
    /// Who wrote it: scribe's session pass.
    pub author: Author,
    /// Who changed it last.
    pub updated_by: Author,
    /// When it was written.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// When it last changed.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// The turns that support it, newest first.
    pub occurrences: Vec<Occurrence>,
}

impl MemoryItem {
    /// The newest occurrence's date, or when the item was written.
    #[must_use]
    pub fn latest(&self) -> OffsetDateTime {
        self.occurrences
            .iter()
            .map(|o| o.date)
            .max()
            .unwrap_or(self.created)
    }

    /// How many distinct sessions its occurrences come from.
    #[must_use]
    pub fn sessions(&self) -> usize {
        self.occurrences
            .iter()
            .map(|o| o.session.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Whether the page labels it "not yet raised" on `today` (§13.4): a
    /// `now` item with no mark, or whose date has passed since the mark.
    #[must_use]
    pub fn not_yet_raised(&self, today: Date) -> bool {
        if self.section != Section::Now {
            return false;
        }
        match (&self.acknowledged, self.event_date) {
            (None, _) => true,
            (Some(ack), Some(date)) => date < today && ack.at.date() < date,
            (Some(_), None) => false,
        }
    }
}

/// `[memory]` in `config.toml` (§13.3), every key optional. The parked
/// settings of §13.12-13.14 are not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryConfig {
    /// `false` switches every section off.
    pub enabled: bool,
    /// Whether the pass may keep personal elements (§13.1); ships `true`.
    pub personal: bool,
    /// Days after their latest occurrence that `projects` and `working`
    /// items stay on the page.
    pub decay_days: u32,
    /// Days after its latest occurrence an undated `now` item stays on the
    /// page.
    pub now_undated_days: u32,
    /// Days after its date a dated `now` item stays on the page.
    pub now_after_days: u32,
    /// The session-start page's budget, every part but `milestones`.
    pub page_chars: usize,
    /// How many recent sessions the page lists; 0 lists none.
    pub recent_sessions: usize,
    /// `[memory.sections]`: each section switched on or off alone.
    pub sections: SectionSwitches,
    /// `[memory.promote]`: when a `now` item becomes a milestone (§13.4).
    pub promote: PromoteRules,
    /// Where `decay_days`, `now_undated_days` and `now_after_days` came
    /// from, for `memory rules`.
    pub windows: WindowOrigins,
}

/// Where each page window's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowOrigins {
    /// `decay_days`.
    pub decay_days: crate::tag::RuleOrigin,
    /// `now_undated_days`.
    pub now_undated_days: crate::tag::RuleOrigin,
    /// `now_after_days`.
    pub now_after_days: crate::tag::RuleOrigin,
}

impl Default for WindowOrigins {
    fn default() -> Self {
        Self {
            decay_days: crate::tag::RuleOrigin::Shipped,
            now_undated_days: crate::tag::RuleOrigin::Shipped,
            now_after_days: crate::tag::RuleOrigin::Shipped,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            personal: true,
            decay_days: 90,
            now_undated_days: 7,
            now_after_days: 14,
            page_chars: PAGE_CHARS,
            recent_sessions: 5,
            sections: SectionSwitches::default(),
            promote: PromoteRules::default(),
            windows: WindowOrigins::default(),
        }
    }
}

impl MemoryConfig {
    /// Whether `section` is written and shown: memory enabled and the
    /// section's own switch on.
    #[must_use]
    pub fn section_on(&self, section: Section) -> bool {
        self.enabled && self.sections.get(section)
    }

    /// The sections switched on, in page order.
    #[must_use]
    pub fn sections_on(&self) -> Vec<Section> {
        Section::ALL
            .into_iter()
            .filter(|s| self.section_on(*s))
            .collect()
    }
}

/// `[memory.sections]`, each `true` unless configured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per fixed section"
)]
pub struct SectionSwitches {
    /// `profile`.
    pub profile: bool,
    /// `projects`.
    pub projects: bool,
    /// `working`.
    pub working: bool,
    /// `now`.
    pub now: bool,
    /// `milestones`.
    pub milestones: bool,
}

impl Default for SectionSwitches {
    fn default() -> Self {
        Self {
            profile: true,
            projects: true,
            working: true,
            now: true,
            milestones: true,
        }
    }
}

impl SectionSwitches {
    /// The switch of `section`.
    #[must_use]
    pub fn get(&self, section: Section) -> bool {
        match section {
            Section::Profile => self.profile,
            Section::Projects => self.projects,
            Section::Working => self.working,
            Section::Now => self.now,
            Section::Milestones => self.milestones,
        }
    }
}

/// Why the pass refused a memory element; counted in the pass report by
/// code and audited by code only, never with its text (§13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Refusal {
    /// `memory_invalid`: an unknown section, or a field empty or past its
    /// bound.
    Invalid,
    /// `memory_unverified`: the quote is not the user's own verbatim span
    /// of the turn named, in this window and in the original transcript.
    Unverified,
    /// `memory_section_off`: the section is switched off.
    SectionOff,
    /// `memory_sensitive`: a credential or anything the redactor changes, a
    /// listed finance word, or a sensitive event outside `now` and
    /// `milestones`.
    Sensitive,
    /// `memory_personal_off`: a personal element while `[memory] personal`
    /// is off.
    PersonalOff,
    /// `memory_inferred`: a `now` statement naming a mood its quote does
    /// not.
    Inferred,
}

impl Refusal {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "memory_invalid",
            Self::Unverified => "memory_unverified",
            Self::SectionOff => "memory_section_off",
            Self::Sensitive => "memory_sensitive",
            Self::PersonalOff => "memory_personal_off",
            Self::Inferred => "memory_inferred",
        }
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

/// One element that passed every model-free check, on its way to the
/// original-transcript check and the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    /// The section.
    pub section: Section,
    /// The statement, trimmed.
    pub statement: String,
    /// The quote.
    pub quote: String,
    /// The turn named.
    pub message: String,
    /// The event date as the model gave it, checked later against the
    /// turn's timestamp.
    pub event_date: Option<String>,
    /// The item the model says this repeats.
    pub same_as: Option<MemoryId>,
    /// Sensitive, by the word list or the model.
    pub sensitive: bool,
    /// Its kind.
    pub sensitive_kind: Option<SensitiveKind>,
    /// `now` and `milestones`: the new profile statement promotion rule
    /// (a) writes, when it passed the profile checks.
    pub promote: Option<String>,
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

/// Check the model's memory elements for one window (§13.3, §13.8), at most
/// `slots` of them (the rest are ignored and use no slot). In order: the
/// shape and bounds; the quote, a verbatim span of the user's own words of
/// the turn it names in `messages`; the section switch; the redactor
/// (`masks` says whether it would change a text: a change drops the
/// element); the finance words; the sensitive words, which force the flag,
/// with `personal` off refusing a flagged or model-marked personal element;
/// the section rule for sensitive items; and the mood rule.
#[must_use]
pub fn check(
    answers: &[MemoryAnswer],
    messages: &[PassMessage],
    slots: usize,
    config: &MemoryConfig,
    masks: &dyn Fn(&str) -> bool,
) -> CheckReport {
    let mut report = CheckReport::default();
    for answer in answers.iter().take(slots) {
        report.used = report.used.saturating_add(1);
        match check_one(answer, messages, config, masks) {
            Ok(checked) => report.checked.push(checked),
            Err(refusal) => report.refused.push(refusal),
        }
    }
    report
}

fn check_one(
    answer: &MemoryAnswer,
    messages: &[PassMessage],
    config: &MemoryConfig,
    masks: &dyn Fn(&str) -> bool,
) -> Result<Checked, Refusal> {
    let section: Section = answer.section.parse().map_err(|_| Refusal::Invalid)?;
    let statement = answer
        .statement
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let quote = answer.quote.clone();
    let quote_chars = quote.trim().chars().count();
    if statement.is_empty()
        || statement.chars().count() > STATEMENT_CHARS
        || !(QUOTE_MIN_CHARS..=QUOTE_CHARS).contains(&quote_chars)
        || answer.message.is_empty()
        || answer.message.chars().count() > MESSAGE_CHARS
    {
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
    if !config.section_on(section) {
        return Err(Refusal::SectionOff);
    }
    if masks(&statement) || masks(&quote) {
        return Err(Refusal::Sensitive);
    }
    if finance_word(&statement) || finance_word(&quote) {
        return Err(Refusal::Sensitive);
    }
    let listed = sensitive_word(&statement).or_else(|| sensitive_word(&quote));
    let named = answer
        .sensitive_kind
        .as_deref()
        .and_then(SensitiveKind::parse);
    let sensitive = listed.is_some() || named.is_some();
    if !config.personal && (sensitive || answer.personal) {
        return Err(Refusal::PersonalOff);
    }
    if sensitive && !section.holds_sensitive() {
        return Err(Refusal::Sensitive);
    }
    if section == Section::Now && inferred_mood(&statement, &quote) {
        return Err(Refusal::Inferred);
    }
    Ok(Checked {
        section,
        statement,
        quote,
        message: answer.message.clone(),
        event_date: answer
            .event_date
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .filter(|_| section.dated()),
        same_as: answer
            .same_as
            .as_deref()
            .and_then(|id| id.trim().parse().ok()),
        sensitive,
        sensitive_kind: listed.or(named),
        promote: answer
            .promote
            .as_deref()
            .and_then(|text| profile_statement(text, section, masks)),
    })
}

/// Rule (a)'s new profile statement, when it may be kept (§13.4, §13.8):
/// on a `now` or `milestones` element only, at most
/// [`promote::PROMOTE_CHARS`] characters, unchanged by the redactor, with
/// no finance or sensitive word (a profile item holds nothing sensitive).
/// One that fails drops the promotion, not the element.
fn profile_statement(text: &str, section: Section, masks: &dyn Fn(&str) -> bool) -> Option<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (matches!(section, Section::Now | Section::Milestones)
        && !text.is_empty()
        && text.chars().count() <= promote::PROMOTE_CHARS
        && !masks(&text)
        && !finance_word(&text)
        && sensitive_word(&text).is_none())
    .then_some(text)
}

/// A checked element tied to its turn in the original transcript, ready for
/// the store: the turn's own words (against which the store repeats the
/// verbatim check under its lock) and its timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    /// The element.
    pub checked: Checked,
    /// The event date, kept only when real and within a year before and
    /// two years after the turn.
    pub event_date: Option<Date>,
    /// The user's own words of the turn (pasted blocks left out).
    pub turn: String,
    /// When the turn was sent.
    pub at: OffsetDateTime,
}

/// Tie each checked element to its turn in the original transcript, never
/// the model's metadata: only the user's turns, and only when the quote is
/// his own span of that turn's original text. An element that fails here is
/// refused `memory_unverified`.
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
            .and_then(|m| Some((m, transcript::daniel_turn(m)?)))
            .filter(|(m, turn)| {
                crate::pass::own_span(&turn.own, &element.quote)
                    && m.text().contains(&element.quote)
            });
        let Some((message, turn)) = found else {
            refused.push(Refusal::Unverified);
            continue;
        };
        let event_date = element
            .event_date
            .as_deref()
            .and_then(|text| crate::record::parse_day(text).ok())
            .filter(|date| plausible_date(*date, message.at.date()));
        out.push(Draft {
            checked: element,
            event_date,
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

/// The occurrence fields the pass derives: never the model's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// The session passed.
    pub session: SessionId,
    /// This host.
    pub host: Label,
    /// The session's project.
    pub project: Project,
}

/// What [`apply`] did with one draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// A new item.
    Created(MemoryId),
    /// An occurrence appended to an existing item.
    Appended(MemoryId),
    /// The turn is already an occurrence of the item: a replayed window.
    Replayed(MemoryId),
}

/// Write one draft into `items` (§13.3): an occurrence of the item the
/// model named (`same_as`) when it is active, in the same section (or, for
/// a `now` element, a milestone a `now` item was promoted to) and its
/// statement shares at least one content token in four with the new one;
/// else of an active item of the same section (and project, for
/// `projects`) whose statement's tokens equal, contain or overlap the new
/// one's (Jaccard at least 0.8); else a new item numbered past the highest.
/// A turn already on the item it would join appends nothing. A new
/// occurrence of a `now` item marks it raised by the user's turn.
pub fn apply(
    items: &mut Vec<MemoryItem>,
    draft: &Draft,
    provenance: &Provenance,
    now: OffsetDateTime,
) -> Applied {
    let element = &draft.checked;
    let project = (element.section == Section::Projects).then(|| provenance.project.clone());
    let new_tokens = token_set(&element.statement);
    let named = element.same_as.as_ref().and_then(|id| {
        items.iter().position(|item| {
            &item.id == id
                && item.status == MemoryStatus::Active
                && joins(element.section, item.section)
                && overlaps_quarter(&token_set(&item.statement), &new_tokens)
        })
    });
    let duplicate = || {
        items.iter().position(|item| {
            item.status == MemoryStatus::Active
                && item.section == element.section
                && item.project == project
                && near_duplicate(&token_set(&item.statement), &new_tokens)
        })
    };
    let author = Author::session_pass();
    if let Some(i) = named.or_else(duplicate) {
        let item = &mut items[i];
        if item
            .occurrences
            .iter()
            .any(|o| o.message == element.message)
        {
            return Applied::Replayed(item.id.clone());
        }
        let n = item
            .occurrences
            .iter()
            .map(|o| o.n)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        item.occurrences.insert(0, occurrence(n, draft, provenance));
        item.occurrences
            .sort_by(|a, b| b.date.cmp(&a.date).then(b.n.cmp(&a.n)));
        if item.event_date.is_none() {
            item.event_date = draft.event_date;
        }
        if element.sensitive && !item.sensitive {
            item.sensitive = true;
            item.sensitive_kind = element.sensitive_kind;
        }
        if item.section == Section::Now {
            item.acknowledged = Some(Acknowledged {
                at: draft.at,
                session: provenance.session.clone(),
                author: author.clone(),
            });
        }
        item.updated = now;
        item.updated_by = author;
        return Applied::Appended(item.id.clone());
    }
    // A replayed window may name no repeat and the statement may differ:
    // a turn already recorded in this section is not recorded twice.
    if let Some(item) = items.iter().find(|item| {
        joins(element.section, item.section)
            && item
                .occurrences
                .iter()
                .any(|o| o.message == element.message && o.quote == element.quote)
    }) {
        return Applied::Replayed(item.id.clone());
    }
    let next = items.iter().map(|i| i.id.number()).max().unwrap_or(0) + 1;
    let id = MemoryId::numbered(next);
    items.push(MemoryItem {
        id: id.clone(),
        section: element.section,
        status: MemoryStatus::Active,
        statement: element.statement.clone(),
        project,
        event_date: draft.event_date,
        sensitive: element.sensitive,
        sensitive_kind: element.sensitive_kind,
        acknowledged: None,
        retirement_reason: None,
        promoted_from: None,
        promotion: None,
        merged_into: None,
        superseded_by: None,
        unrelated_to: Vec::new(),
        author: author.clone(),
        updated_by: author,
        created: now,
        updated: now,
        occurrences: vec![occurrence(1, draft, provenance)],
    });
    Applied::Created(id)
}

/// Whether an element of `section` may join an item of `item`: its own
/// section, or, for a `now` element, a milestone, which holds the
/// occurrences of the `now` item promoted to it (§13.4).
fn joins(section: Section, item: Section) -> bool {
    section == item || (section == Section::Now && item == Section::Milestones)
}

fn occurrence(n: u32, draft: &Draft, provenance: &Provenance) -> Occurrence {
    Occurrence {
        n,
        date: draft.at,
        session: provenance.session.clone(),
        message: draft.checked.message.clone(),
        host: provenance.host.clone(),
        project: provenance.project.clone(),
        quote: draft.checked.quote.clone(),
        author: Author::session_pass(),
    }
}

fn token_set(text: &str) -> BTreeSet<String> {
    crate::ask::tokens(text).into_iter().collect()
}

/// At least one content token in four in common (§7's rule).
fn overlaps_quarter(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    let common = a.intersection(b).count();
    let union = a.union(b).count();
    common > 0 && common.saturating_mul(4) >= union
}

/// Equal, one containing the other, or Jaccard at least 0.8 (memory's own
/// duplicate rule, §13.3).
fn near_duplicate(a: &BTreeSet<String>, b: &BTreeSet<String>) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let common = a.intersection(b).count();
    let union = a.union(b).count();
    a.is_subset(b) || b.is_subset(a) || common.saturating_mul(5) >= union.saturating_mul(4)
}

/// The active items a pass request lists so the model can name a repeat:
/// those of the sections switched on, best word overlap with the window's
/// user turns first, then newest, at most [`MAX_CONTEXT_ITEMS`].
#[must_use]
pub fn context(
    items: &[MemoryItem],
    messages: &[PassMessage],
    config: &MemoryConfig,
) -> Vec<crate::models::MemoryContext> {
    let window: BTreeSet<String> = messages
        .iter()
        .filter(|m| m.role == Speaker::User)
        .flat_map(|m| crate::ask::tokens(&m.text))
        .collect();
    let mut active: Vec<(usize, &MemoryItem)> = items
        .iter()
        .filter(|i| i.status == MemoryStatus::Active && config.section_on(i.section))
        .map(|i| (token_set(&i.statement).intersection(&window).count(), i))
        .collect();
    active.sort_by(|(a, x), (b, y)| b.cmp(a).then(y.latest().cmp(&x.latest())));
    active
        .into_iter()
        .take(MAX_CONTEXT_ITEMS)
        .map(|(_, i)| crate::models::MemoryContext {
            id: i.id.to_string(),
            section: i.section.as_str(),
            statement: i.statement.clone(),
        })
        .collect()
}

/// Why an item is not on the page on a given day (§13.2: derived, never
/// stored).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffPage {
    /// Merged into an older item.
    Merged,
    /// Superseded by a newer item.
    Superseded,
    /// Promoted to a milestone.
    Promoted,
    /// Retired.
    Retired,
    /// Forgotten.
    Forgotten,
    /// Its section is switched off.
    SectionOff,
    /// `projects` or `working`, with no occurrence within `decay_days`.
    Decayed,
    /// `now`, past its window.
    PastWindow,
}

impl OffPage {
    /// The label `memory show --include-off-page` gives it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Superseded => "superseded",
            Self::Promoted => "promoted",
            Self::Retired => "retired",
            Self::Forgotten => "forgotten",
            Self::SectionOff => "section_off",
            Self::Decayed => "decayed",
            Self::PastWindow => "past_window",
        }
    }
}

/// Why `item` is off the page on `today`, or `None` when it is on it
/// (§13.4, §13.5): `profile` and `milestones` stay while active;
/// `projects` and `working` leave `decay_days` after their latest
/// occurrence; a dated `now` item leaves `now_after_days` after its date,
/// an undated one `now_undated_days` after its latest occurrence.
#[must_use]
pub fn off_page(item: &MemoryItem, config: &MemoryConfig, today: Date) -> Option<OffPage> {
    match item.status {
        MemoryStatus::Merged => return Some(OffPage::Merged),
        MemoryStatus::Superseded => return Some(OffPage::Superseded),
        MemoryStatus::Promoted => return Some(OffPage::Promoted),
        MemoryStatus::Retired => return Some(OffPage::Retired),
        MemoryStatus::Forgotten => return Some(OffPage::Forgotten),
        MemoryStatus::Active => {}
    }
    if !config.section_on(item.section) {
        return Some(OffPage::SectionOff);
    }
    let age = |from: Date| (today - from).whole_days();
    let latest = item.latest().date();
    match item.section {
        Section::Profile | Section::Milestones => None,
        Section::Projects | Section::Working => {
            (age(latest) > i64::from(config.decay_days)).then_some(OffPage::Decayed)
        }
        Section::Now => {
            let past = match item.event_date {
                Some(date) => age(date) > i64::from(config.now_after_days),
                None => age(latest) > i64::from(config.now_undated_days),
            };
            past.then_some(OffPage::PastWindow)
        }
    }
}

/// One recently active session, as the page lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentSession {
    /// The session.
    pub id: SessionId,
    /// Its title.
    pub title: String,
    /// Its last message.
    pub at: OffsetDateTime,
}

/// The session-start page (§13.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    /// What the hook injects.
    pub text: String,
    /// Whether the milestone lines pass [`MILESTONES_WARN_CHARS`]: the
    /// signal that a cap is needed.
    pub over_budget: bool,
}

const HEADER: &str = "scribe memory — what you know about Daniel. Descriptive, not rules: rules are his preferences. Raise milestones only when the conversation goes there.";
const FOOTER: &str = "More: scribe memory show · scribe session show <id> · scribe question ask";

/// The parts' base budgets of §13.6, scaled to `page_chars`.
const NOW_BUDGET: usize = 600;
const PROFILE_BUDGET: usize = 400;
const PROJECTS_BUDGET: usize = 300;
const WORKING_BUDGET: usize = 440;
const RECENT_BUDGET: usize = 400;

/// Build the page from the store's items and the recent sessions on
/// `today`; `project` is the session's own project, whose `projects` items
/// alone are shown. `None` when there is nothing to show. Sections in page
/// order, each line one item and its id; a part over its budget drops whole
/// items from its end and says `(+k more)`, and its unused budget passes to
/// `working`, then `profile`. Milestones are never cut.
#[must_use]
pub fn page(
    items: &[MemoryItem],
    recent: &[RecentSession],
    project: Option<&Project>,
    config: &MemoryConfig,
    today: Date,
) -> Option<Page> {
    let on: Vec<&MemoryItem> = items
        .iter()
        .filter(|i| off_page(i, config, today).is_none())
        .collect();
    let of = |section: Section| -> Vec<&MemoryItem> {
        on.iter()
            .copied()
            .filter(|i| i.section == section)
            .collect()
    };
    let scale = |base: usize| base.saturating_mul(config.page_chars) / PAGE_CHARS;

    let mut now = of(Section::Now);
    now.sort_by(|a, b| match (a.event_date, b.event_date) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.latest().cmp(&a.latest()),
    });
    let now_lines: Vec<String> = now.iter().map(|i| now_line(i, today)).collect();
    let (now_part, now_left) = lines_part("Now:", &now_lines, scale(NOW_BUDGET), MAX_NOW_LINES);

    let mut milestones = of(Section::Milestones);
    milestones.sort_by_key(|i| {
        (
            i.event_date.unwrap_or_else(|| i.latest().date()),
            i.id.number(),
        )
    });
    let milestone_lines: Vec<String> = milestones.iter().map(|i| milestone_line(i)).collect();
    let milestone_chars: usize = milestone_lines.iter().map(|l| l.chars().count() + 1).sum();
    let milestones_part = (!milestone_lines.is_empty())
        .then(|| format!("Milestones:\n{}", milestone_lines.join("\n")));

    let mut projects = of(Section::Projects);
    projects.retain(|i| project.is_some_and(|p| i.project.as_ref() == Some(p)));
    projects.sort_by_key(|i| std::cmp::Reverse(i.latest()));
    let projects_label = project.map_or_else(String::new, |p| {
        let as_of = projects
            .first()
            .map(|i| crate::record::nullable_date::format(i.latest().date()))
            .unwrap_or_default();
        format!("Project {p} (as of {as_of}):")
    });
    let (projects_part, projects_left) =
        inline_part(&projects_label, &projects, scale(PROJECTS_BUDGET));

    let recent_part = recent_part(recent, config.recent_sessions, scale(RECENT_BUDGET));
    let recent_left =
        scale(RECENT_BUDGET).saturating_sub(recent_part.as_ref().map_or(0, |p| p.chars().count()));

    let mut working = of(Section::Working);
    working.sort_by(|a, b| {
        b.sessions()
            .cmp(&a.sessions())
            .then(b.latest().cmp(&a.latest()))
    });
    let spare = now_left + projects_left + recent_left;
    let (working_part, working_left) =
        inline_part("Working:", &working, scale(WORKING_BUDGET) + spare);

    let mut profile = of(Section::Profile);
    profile.sort_by(|a, b| {
        b.occurrences
            .len()
            .cmp(&a.occurrences.len())
            .then(b.latest().cmp(&a.latest()))
    });
    let (profile_part, _) = inline_part("Profile:", &profile, scale(PROFILE_BUDGET) + working_left);

    let body: Vec<String> = [
        now_part,
        milestones_part,
        profile_part,
        projects_part,
        working_part,
        recent_part,
    ]
    .into_iter()
    .flatten()
    .collect();
    if body.is_empty() {
        return None;
    }
    let text = format!("{HEADER}\n{}\n{FOOTER}", body.join("\n"));
    Some(Page {
        text,
        over_budget: milestone_chars > MILESTONES_WARN_CHARS,
    })
}

fn now_line(item: &MemoryItem, today: Date) -> String {
    let raised = if item.not_yet_raised(today) {
        "; not yet raised"
    } else {
        ""
    };
    match item.event_date {
        Some(date) => {
            let days = (date - today).whole_days();
            let when = match days {
                0 => String::from("today"),
                d if d > 0 => format!("in {d} days"),
                d => format!("{} days ago", d.unsigned_abs()),
            };
            format!(
                "- {} {} ({when}{raised}) [{}]",
                crate::record::nullable_date::format(date),
                item.statement,
                item.id
            )
        }
        None => format!(
            "- {} (said {}{raised}) [{}]",
            item.statement,
            crate::record::nullable_date::format(item.latest().date()),
            item.id
        ),
    }
}

fn milestone_line(item: &MemoryItem) -> String {
    let date = item.event_date.unwrap_or_else(|| item.latest().date());
    let tail = format!(" [{}]", item.id);
    let head = format!("- {} ", crate::record::nullable_date::format(date));
    let room = MILESTONE_LINE_CHARS.saturating_sub(head.chars().count() + tail.chars().count());
    format!("{head}{}{tail}", cut(&item.statement, room))
}

/// `text` cut to `max` characters, with `…` when cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A part of one line per item under `label`, within `budget` characters
/// and `max` lines. Returns the part and the budget it left unused.
fn lines_part(label: &str, lines: &[String], budget: usize, max: usize) -> (Option<String>, usize) {
    if lines.is_empty() {
        return (None, budget);
    }
    let mut used = label.chars().count();
    let mut kept: Vec<&str> = Vec::new();
    for line in lines.iter().take(max) {
        let chars = line.chars().count() + 1;
        if used + chars > budget {
            break;
        }
        used += chars;
        kept.push(line);
    }
    let more = lines.len() - kept.len();
    let mut part = label.to_owned();
    for line in &kept {
        part.push('\n');
        part.push_str(line);
    }
    if more > 0 {
        part.push_str(&format!("\n(+{more} more)"));
    }
    if kept.is_empty() && more > 0 {
        return (Some(part), 0);
    }
    (Some(part), budget.saturating_sub(used))
}

/// A part of items joined on one line after `label`, within `budget`.
fn inline_part(label: &str, items: &[&MemoryItem], budget: usize) -> (Option<String>, usize) {
    if items.is_empty() || label.is_empty() {
        return (None, budget);
    }
    let mut line = label.to_owned();
    let mut kept = 0usize;
    for item in items {
        let piece = format!(
            "{} {} [{}]",
            if kept == 0 { "" } else { ";" },
            item.statement,
            item.id
        );
        let piece = if kept == 0 {
            piece.trim_start().to_owned()
        } else {
            piece
        };
        let addition = if kept == 0 {
            format!(" {piece}")
        } else {
            piece
        };
        if line.chars().count() + addition.chars().count() > budget {
            break;
        }
        line.push_str(&addition);
        kept += 1;
    }
    let more = items.len() - kept;
    if more > 0 {
        line.push_str(&format!(" (+{more} more)"));
    }
    let left = budget.saturating_sub(line.chars().count());
    (Some(line), left)
}

fn recent_part(recent: &[RecentSession], n: usize, budget: usize) -> Option<String> {
    if n == 0 || recent.is_empty() {
        return None;
    }
    let mut line = String::from("Recent sessions:");
    let mut kept = 0usize;
    for session in recent.iter().take(n) {
        let piece = format!(
            "{} {} \"{}\" ({})",
            if kept == 0 { "" } else { ";" },
            session.id,
            cut(
                &session
                    .title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                60
            ),
            crate::record::nullable_date::format(session.at.date())
        );
        let piece = if kept == 0 {
            format!(" {}", piece.trim_start())
        } else {
            piece
        };
        if line.chars().count() + piece.chars().count() > budget {
            break;
        }
        line.push_str(&piece);
        kept += 1;
    }
    (kept > 0).then_some(line)
}

/// Forget `item` for good (§13.9): its statement, every occurrence's quote
/// and any retirement reason blanked, its status `forgotten`; id, section,
/// dates and authors stay. Returns whether anything changed.
pub fn forget(item: &mut MemoryItem, by: Author, now: OffsetDateTime) -> bool {
    if item.status == MemoryStatus::Forgotten
        && item.statement.is_empty()
        && item.occurrences.iter().all(|o| o.quote.is_empty())
    {
        return false;
    }
    item.status = MemoryStatus::Forgotten;
    item.statement.clear();
    item.retirement_reason = None;
    for occurrence in &mut item.occurrences {
        occurrence.quote.clear();
    }
    item.updated = now;
    item.updated_by = by;
    true
}

/// The items whose words `forget` of `id` must also blank (§13.9): those
/// merged into it, whose statements still stand; and, across a promotion,
/// the milestone and the `now` item it came from, which hold the same
/// event and the same occurrences. By index into `items`, without `id`
/// itself.
#[must_use]
pub fn linked(items: &[MemoryItem], id: &MemoryId) -> Vec<usize> {
    let Some(target) = items.iter().find(|i| &i.id == id) else {
        return Vec::new();
    };
    let promoted_to = (target.status == MemoryStatus::Promoted)
        .then(|| target.promotion.as_ref().map(|p| p.to.clone()))
        .flatten();
    items
        .iter()
        .enumerate()
        .filter(|(_, i)| &i.id != id)
        .filter(|(_, i)| {
            i.merged_into.as_ref() == Some(id)
                || target.promoted_from.as_ref() == Some(&i.id)
                || promoted_to.as_ref() == Some(&i.id)
        })
        .map(|(n, _)| n)
        .collect()
}

/// Forget occurrence `n` of `item` for good: its quote blanked. Returns
/// whether anything changed; `None` when the item has no occurrence `n`.
pub fn forget_occurrence(
    item: &mut MemoryItem,
    n: u32,
    by: Author,
    now: OffsetDateTime,
) -> Option<bool> {
    let occurrence = item.occurrences.iter_mut().find(|o| o.n == n)?;
    if occurrence.quote.is_empty() {
        return Some(false);
    }
    occurrence.quote.clear();
    item.updated = now;
    item.updated_by = by;
    Some(true)
}
