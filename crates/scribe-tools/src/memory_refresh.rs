//! Pure memory-refresh request, answer, basis and Orbit apply shapes
//! (docs/design/scribe/2_design.md §15.6, §19.3.5).
//!
//! No I/O and no store writes (STD-02 R5).

use scribe_store::record::nullable;
use scribe_store::{MemoryContext, Stem};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// The JSON schema of a profile answer (2_design.md §15.6).
pub const PROFILE_SCHEMA: &str = r#"{"type":"object","properties":{"profile":{"type":"string","description":"the whole profile.md: the seven sections in order, each a '## ' heading, Pinned copied exactly; at most 6000 characters"},"milestones":{"type":"array","items":{"type":"object","properties":{"now":{"type":["string","null"],"description":"the stem of the passed now item this promotes, or null for a profile change"},"date":{"type":"string","description":"YYYY-MM-DD"},"title":{"type":"string","maxLength":80},"text":{"type":"string","maxLength":400}},"required":["now","date","title","text"],"additionalProperties":false}}},"required":["profile","milestones"],"additionalProperties":false}"#;

/// The profile's section headings, in order.
pub const HEADINGS: [&str; 7] = [
    "Role and work",
    "Current focus",
    "Projects",
    "Goals",
    "Environment",
    "How they work",
    "Pinned",
];

/// The longest profile the job writes, in characters.
pub const PROFILE_CHARS: usize = 6000;

/// The shipped instruction text of a profile refresh (`[instructions] profile`
/// replaces it).
pub const PROFILE_INSTRUCTIONS: &str = "You keep {user}'s profile: one short document saying who \
     they are now, rebuilt from their recent sessions.";

/// One session the profile job reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSession {
    /// Its id.
    pub id: String,
    /// Its project.
    pub project: String,
    /// Its last change's date, `YYYY-MM-DD`.
    pub date: String,
    /// Its title.
    pub title: String,
    /// Its summary and goals, bounded.
    pub summary: String,
    /// The user's stored turns, for the sessions with the highest signal.
    pub verbatim: Option<String>,
}

/// One milestone the profile job returns.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileMilestoneAnswer {
    /// The stem of the passed `now` item it promotes; `None` for a profile
    /// change.
    #[serde(default, deserialize_with = "nullable")]
    pub now: Option<String>,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// One line.
    pub title: String,
    /// Its prose.
    pub text: String,
}

/// The model's answer to a profile refresh, unchecked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileAnswer {
    /// The whole `profile.md`.
    pub profile: String,
    /// Milestones to write.
    #[serde(default)]
    pub milestones: Vec<ProfileMilestoneAnswer>,
}

/// The refresh cursor position on the wire: an RFC 3339 timestamp and session id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshCursor {
    /// The last read session's updated timestamp.
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    /// The last read session's id.
    pub session: String,
}

mod cursor_serde {
    use super::RefreshCursor;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use time::OffsetDateTime;

    pub(super) fn serialize<S: Serializer>(
        value: &Option<(OffsetDateTime, String)>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let wire = value.as_ref().map(|(at, session)| RefreshCursor {
            at: *at,
            session: session.clone(),
        });
        wire.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<(OffsetDateTime, String)>, D::Error> {
        let wire = Option::<RefreshCursor>::deserialize(deserializer)?;
        Ok(wire.map(|c| (c.at, c.session)))
    }
}

/// What a refresh's write checks has not changed since its inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Basis {
    /// The latest profile revision's number.
    pub latest: Option<i64>,
    /// The digest of the profile file.
    pub file: Option<String>,
    /// The refresh cursor.
    #[serde(default, with = "cursor_serde")]
    pub cursor: Option<(OffsetDateTime, String)>,
    /// The stems of the `now` files.
    pub now: Vec<Stem>,
}

/// The bounded data passed to the `scribe_profile_refresh` activity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedProfileInputs {
    /// The current profile, or the empty template.
    pub profile: String,
    /// The user's hand edits since the last revision, as a line diff.
    pub corrections: Option<String>,
    /// Hints for a first profile: migrated `profile` and `projects` items.
    pub hints: Vec<String>,
    /// The sessions read, oldest first.
    pub sessions: Vec<ProfileSession>,
    /// The `now` items still ahead or within 14 days.
    pub now: Vec<MemoryContext>,
    /// The `now` items past their 14 days, which may become milestones.
    pub passed: Vec<MemoryContext>,
    /// Every milestone.
    pub milestones: Vec<MemoryContext>,
    /// Active preference entries: id and what they say, for links.
    pub preferences: Vec<(String, String)>,
    /// How prompts name the user (`[user] name`, or `"the user"`).
    pub user_noun: String,
    /// Optional `[instructions] profile` override from `config.toml`, with `{user}`/`{User}` filled.
    pub instructions: Option<String>,
}

/// Output of `memory prepare` / `scribe.memory-refresh-prepare`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedRefresh {
    /// True when there is no new session, hand edit or passed `now` item.
    pub nothing_new: bool,
    /// Optimistic concurrency basis; `None` when `nothing_new` is true.
    pub basis: Option<Basis>,
    /// Where the cursor moves once committed; `None` when no session was read.
    pub cursor_to: Option<RefreshCursor>,
    /// Session ids read in this batch.
    pub session_ids: Vec<String>,
    /// Sessions after the cursor left for later runs.
    pub backlog: usize,
    /// Stems of passed `now` items being settled in this batch.
    pub passed_stems: Vec<Stem>,
    /// Bounded inputs for `scribe_profile_refresh`; `None` when `nothing_new` is true.
    pub inputs: Option<PreparedProfileInputs>,
}

impl PreparedRefresh {
    /// A prepared refresh with nothing to do.
    #[must_use]
    pub fn nothing_new() -> Self {
        Self {
            nothing_new: true,
            basis: None,
            cursor_to: None,
            session_ids: Vec::new(),
            backlog: 0,
            passed_stems: Vec::new(),
            inputs: None,
        }
    }
}

/// Input to `memory apply` / `scribe.memory-refresh-apply`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRefreshRequest {
    /// Optimistic concurrency basis captured by `prepare`.
    pub basis: Basis,
    /// Where the cursor moves once committed; `None` when no session was read.
    #[serde(default)]
    pub cursor_to: Option<RefreshCursor>,
    /// Session ids read by `prepare`.
    #[serde(default)]
    pub session_ids: Vec<String>,
    /// Sessions after the cursor left for later runs; `apply` recounts it.
    #[serde(default)]
    pub backlog: usize,
    /// Stems of passed `now` items captured by `prepare`.
    #[serde(default)]
    pub passed_stems: Vec<Stem>,
    /// The model's unchecked answer.
    pub answer: ProfileAnswer,
    /// When true, a `profile_*` validation refusal returns `{status: "retry", correction}`
    /// instead of failing, so an Orbit job can run one conditional retry.
    #[serde(default)]
    pub allow_retry: bool,
}
