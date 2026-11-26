//! Payloads for memory reads, rules and refresh results.

use super::{OriginPayload, Timestamp};
use serde::Serialize;

/// One memory file (`memory show <id>`, and the writes on one;
/// docs/design/scribe/specs/output-contract.md). Field names are a
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryPayload {
    /// `profile`, or the file stem of a `now` item or milestone.
    pub(crate) id: String,
    /// `profile`, `now` or `milestone`.
    pub(crate) kind: &'static str,
    /// The event date or deadline, `YYYY-MM-DD`; `null` for the profile.
    pub(crate) date: Option<String>,
    /// `now` only: `deadline`, `event` or `plan`; `null` otherwise.
    pub(crate) now_kind: Option<&'static str>,
    /// The title; `null` for the profile.
    pub(crate) title: Option<String>,
    /// The prose, or the whole profile.
    pub(crate) text: String,
    /// The sessions it came from; empty for the profile.
    pub(crate) sources: Vec<String>,
    /// When it last changed; `null` for the profile.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) updated: Option<Timestamp>,
    /// `now` only: when an agent last raised it; `null` when never.
    pub(crate) raised: Option<scribe_core::memory::Raised>,
    /// The profile only: the date of its last revision; `null` otherwise.
    pub(crate) as_of: Option<String>,
}

impl MemoryPayload {
    /// The payload of a shown file.
    pub(crate) fn from_shown(shown: &scribe_core::MemoryShown) -> Self {
        match shown {
            scribe_core::MemoryShown::Profile(p) => Self::profile(p),
            scribe_core::MemoryShown::Now(n) => Self::now(n),
            scribe_core::MemoryShown::Milestone(m) => Self::milestone(m),
        }
    }

    pub(crate) fn profile(p: &scribe_core::memory::ProfileView) -> Self {
        Self {
            id: String::from("profile"),
            kind: "profile",
            date: None,
            now_kind: None,
            title: None,
            text: p.text.clone(),
            sources: Vec::new(),
            updated: None,
            raised: None,
            as_of: p.as_of.map(day),
        }
    }

    pub(crate) fn now(n: &scribe_core::memory::NowItem) -> Self {
        Self {
            id: n.stem.to_string(),
            kind: "now",
            date: Some(day(n.date)),
            now_kind: Some(n.kind.as_str()),
            title: Some(n.title.clone()),
            text: n.text.clone(),
            sources: n.sources.iter().map(ToString::to_string).collect(),
            updated: Some(n.updated),
            raised: n.raised.clone(),
            as_of: None,
        }
    }

    pub(crate) fn milestone(m: &scribe_core::memory::Milestone) -> Self {
        Self {
            id: m.stem.to_string(),
            kind: "milestone",
            date: Some(day(m.date)),
            now_kind: None,
            title: Some(m.title.clone()),
            text: m.text.clone(),
            sources: m.sources.iter().map(ToString::to_string).collect(),
            updated: Some(m.updated),
            raised: None,
            as_of: None,
        }
    }
}

/// The profile in the memory block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProfileBlockPayload {
    /// `profile.md`, or its committed revision while a journal is open.
    pub(crate) text: String,
    /// The date of its last revision; `null` for a file no revision wrote.
    pub(crate) as_of: Option<String>,
}

/// One line of the memory block's Now or Milestones part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryLinePayload {
    /// The file stem.
    pub(crate) id: String,
    /// `YYYY-MM-DD`.
    pub(crate) date: String,
    /// Its title.
    pub(crate) title: String,
}

/// The memory block (2_design.md §15.4): `memory show` with no id prints
/// it, and `scribe brief` embeds it. `--json` prints `{profile, now,
/// milestones}`. Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryBlockPayload {
    /// The profile; `null` when there is none.
    pub(crate) profile: Option<ProfileBlockPayload>,
    /// Every `now` item, nearest date first.
    pub(crate) now: Vec<MemoryLinePayload>,
    /// The last five milestones, newest first.
    pub(crate) milestones: Vec<MemoryLinePayload>,
    /// The block as text.
    #[serde(skip)]
    pub(crate) text: String,
    /// Warning codes and lines for stderr: `memory_pending`, a read-only
    /// copy, files that did not parse, items not yet migrated.
    #[serde(skip)]
    pub(crate) warnings: Vec<String>,
}

impl MemoryBlockPayload {
    /// The block of `view`, with the root it was read from for notices.
    pub(crate) fn from_view(view: &scribe_core::MemoryView, root: &std::path::Path) -> Self {
        let mut now: Vec<&scribe_core::memory::NowItem> = view.now.iter().collect();
        now.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.stem.cmp(&b.stem)));
        let warnings = scribe_core::apps::memory::warnings(view, root);
        Self {
            profile: view.profile.as_ref().map(|p| ProfileBlockPayload {
                text: p.text.clone(),
                as_of: p.as_of.map(day),
            }),
            now: now
                .iter()
                .map(|n| MemoryLinePayload {
                    id: n.stem.to_string(),
                    date: day(n.date),
                    title: n.title.clone(),
                })
                .collect(),
            milestones: scribe_core::memory::newest_milestones(&view.milestones)
                .into_iter()
                .map(|m| MemoryLinePayload {
                    id: m.stem.to_string(),
                    date: day(m.date),
                    title: m.title.clone(),
                })
                .collect(),
            text: scribe_core::apps::memory::text(view),
            warnings,
        }
    }
}

/// `memory rules`: the fixed memory rules (2_design.md §15.5). Field names
/// are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRulesPayload {
    /// In a fixed order.
    pub(crate) rules: Vec<MemoryRulePayload>,
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRulePayload {
    /// Its name.
    pub(crate) rule: &'static str,
    /// Its value: a number or the list of phrases.
    pub(crate) value: serde_json::Value,
    /// Where it came from: always shipped, as memory has no switches.
    pub(crate) origin: OriginPayload,
    /// What it does, in one line.
    pub(crate) meaning: &'static str,
}

impl MemoryRulesPayload {
    /// The fixed rules.
    pub(crate) fn new() -> Self {
        use scribe_core::memory as m;
        let rule = |rule, value, meaning| MemoryRulePayload {
            rule,
            value,
            origin: OriginPayload::SHIPPED,
            meaning,
        };
        Self {
            rules: vec![
                rule(
                    "max_per_pass",
                    serde_json::Value::from(m::MAX_PER_PASS),
                    "the most now and milestone elements one pass invocation writes per session",
                ),
                rule(
                    "max_pending",
                    serde_json::Value::from(m::MAX_PENDING),
                    "the most undated now candidates a session holds until a later turn dates one; dropped when the session ends",
                ),
                rule(
                    "now_keep_days",
                    serde_json::Value::from(m::NOW_KEEP_DAYS),
                    "a now item stays this many days after its date, then the weekly refresh makes it a milestone or history",
                ),
                rule(
                    "milestone_lines",
                    serde_json::Value::from(m::MILESTONE_LINES),
                    "the memory block shows this many milestones, newest first",
                ),
                rule(
                    "profile_chars",
                    serde_json::Value::from(m::profile::PROFILE_CHARS),
                    "the longest profile the weekly refresh writes",
                ),
                rule(
                    "user_says",
                    serde_json::Value::from(
                        m::USER_SAYS
                            .iter()
                            .map(|p| (*p).to_owned())
                            .collect::<Vec<_>>(),
                    ),
                    "one of these phrases in the user's turn makes the event a milestone",
                ),
            ],
        }
    }
}

/// `memory refresh`: what the weekly profile job did. Field names are a
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RefreshPayload {
    /// `written`, or `nothing_new` when nothing changed since the last run
    /// and no model was called.
    pub(crate) result: &'static str,
    /// The new profile revision; `null` when the profile did not change.
    pub(crate) revision: Option<i64>,
    /// Sessions read.
    pub(crate) sessions: usize,
    /// Sessions after the cursor left for the next run.
    pub(crate) backlog: usize,
    /// Milestones written, by stem.
    pub(crate) milestones: Vec<String>,
    /// Passed `now` items removed, by stem.
    pub(crate) removed: Vec<String>,
}

/// `memory apply` when `allow_retry` is true and a `profile_*` check refused
/// the rebuilt answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RefreshRetryPayload {
    /// Always `"retry"`.
    pub(crate) status: &'static str,
    /// Stable `profile_*` refusal code.
    pub(crate) code: &'static str,
    /// Actionable correction for the retry prompt.
    pub(crate) correction: String,
}

/// `memory show --revision <n>`: one profile revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RevisionPayload {
    /// Its number.
    pub(crate) revision: i64,
    /// When it was committed.
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    /// The profile's text.
    pub(crate) text: String,
}

fn day(date: time::Date) -> String {
    date.format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}
