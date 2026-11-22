//! The memory items of formats 19–24 (`memory/m-<n>.md`, 2_design.md §13),
//! kept only so phase 5 can read them once and migrate them (§15.11):
//! [`super::migrate`] turns them into `now` and milestone files and
//! `memory_history` rows. Nothing writes this shape any more.

use super::SensitiveKind;
use crate::record::{Author, Label, Project, nullable, nullable_date};
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
use time::{Date, OffsetDateTime};

/// The longest statement a stored item carries, in characters.
pub const STATEMENT_CHARS: usize = 200;

/// The five sections, in page order (§13.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// Role, skills, hosts and environment; with `personal` on, household
    /// facts as he states them. Global, no decay.
    Profile,
    /// A project's purpose, status and focus, with an as-of date. No
    /// longer written (§15.9 phase 1); stored items stay readable.
    Projects,
    /// How the user steers agents, as observed. No longer written (§15.9
    /// phase 1): a standing habit is a preference claim, not memory.
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

    /// Whether the session pass may write an item of this section. Since
    /// phase 1 of 2_design.md §15.9 it writes no `projects` or `working`
    /// item: a project decision belongs on its task and a working habit is
    /// a preference claim. Stored items of both still load and print.
    #[must_use]
    pub fn written(self) -> bool {
        match self {
            Self::Profile | Self::Now | Self::Milestones => true,
            Self::Projects | Self::Working => false,
        }
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
    /// `working`: the `preferences` candidate opened for it when it
    /// recurred (format 21, §13.7), so it opens once.
    #[serde(default, deserialize_with = "nullable")]
    pub preference_proposal: Option<crate::ProposalId>,
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
    /// The newest occurrence that still holds its quote.
    #[must_use]
    pub fn latest_occurrence(&self) -> Option<&Occurrence> {
        self.occurrences
            .iter()
            .filter(|o| !o.quote.is_empty())
            .max_by_key(|o| (o.date, o.n))
    }

    /// When the latest occurrence happened, or `created` without one.
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

/// Which rule promoted an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionRule {
    /// (a) the event changes a profile fact.
    ProfileChange,
    /// (b) the event recurs in enough sessions.
    RecursInSessions,
    /// (c) the user says it is a milestone.
    UserSays,
}

impl PromotionRule {
    /// The rule's name, as `[memory.promote]` keys it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProfileChange => "profile_change",
            Self::RecursInSessions => "recurs_in_sessions",
            Self::UserSays => "user_says",
        }
    }
}

/// A promotion's journal: on the milestone from its first write, and on the
/// `now` item once it is `promoted`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promotion {
    /// The rule that held.
    pub rule: PromotionRule,
    /// The milestone.
    pub to: MemoryId,
    /// Rule (a): the profile item the change supersedes, when there was one.
    #[serde(deserialize_with = "nullable")]
    pub profile_from: Option<MemoryId>,
    /// Rule (a): the new profile item.
    #[serde(deserialize_with = "nullable")]
    pub profile_to: Option<MemoryId>,
}
