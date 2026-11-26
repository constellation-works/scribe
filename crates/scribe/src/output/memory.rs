//! Payloads and rendering for `scribe memory`.

use super::common::console::{Sink, notice};
use super::common::detail::{Detail, emit_detail, field, or_none};
use super::common::record::{OriginPayload, Timestamp, origin_text};
use super::common::table::{Cell, Result, one_line, plain_line, table, timestamp, yes_no};
use cw_cli::{Column, Mode, Table};
use serde::Serialize;
use std::fmt::{self, Write as _};
use std::io::Write;

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

/// `memory rules`: the fixed memory rules (2_design.md §15.5). Field names
/// are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRulesPayload {
    /// In a fixed order.
    pub(crate) rules: Vec<MemoryRulePayload>,
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

/// Render a single memory item (`memory show <id>` and write verbs).
pub(crate) fn show<O: Write, E: Write>(sink: &mut Sink<O, E>, item: &MemoryPayload) -> Result {
    emit_detail(sink, item, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => {
            writeln!(
                out,
                "{}",
                plain_line(&[
                    Cell::plain(item.id.clone()),
                    Cell::plain(item.kind.to_owned()),
                    Cell::plain(item.date.clone().unwrap_or_else(|| String::from("-"))),
                    Cell::plain(item.title.clone().unwrap_or_else(|| String::from("-"))),
                ])
            )?;
            for line in item.text.lines() {
                writeln!(out, "text\t{line}")?;
            }
            Ok(())
        }
        Mode::Table => memory_detail(out, item),
    })
}

/// Render the memory block (`memory show` with no id), emitting its warnings
/// to stderr first.
pub(crate) fn show_block<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    block: &MemoryBlockPayload,
) -> Result {
    for warning in &block.warnings {
        notice(sink, warning);
    }
    emit_detail(sink, block, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain | Mode::Table => write!(out, "{}", block.text),
    })
}

/// Render a profile revision (`memory show --revision <n>`).
pub(crate) fn show_revision<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    revision: &RevisionPayload,
) -> Result {
    emit_detail(sink, revision, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain | Mode::Table => {
            write!(out, "{}", revision.text)?;
            if !revision.text.ends_with('\n') {
                writeln!(out)?;
            }
            Ok(())
        }
    })
}

/// Render a `memory refresh` or `memory apply` written result.
pub(crate) fn show_refresh<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    refresh: &RefreshPayload,
) -> Result {
    emit_detail(sink, refresh, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{}",
            refresh.result,
            refresh
                .revision
                .map_or_else(|| String::from("-"), |n| n.to_string()),
            refresh.sessions,
            refresh.backlog,
            refresh.milestones.len(),
            refresh.removed.len()
        ),
        Mode::Table => {
            field(out, "result", refresh.result)?;
            field(
                out,
                "revision",
                &refresh
                    .revision
                    .map_or_else(|| String::from("-"), |n| n.to_string()),
            )?;
            field(out, "sessions", &refresh.sessions.to_string())?;
            field(out, "backlog", &refresh.backlog.to_string())?;
            field(out, "milestones", &or_none(&refresh.milestones))?;
            field(out, "removed", &or_none(&refresh.removed))
        }
    })
}

/// Render a `memory prepare` result.
pub(crate) fn show_prepare<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    prepared: &scribe_core::apps::memory::PreparedRefresh,
) -> Result {
    emit_detail(sink, prepared, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}\t{}\t{}\t{}",
            prepared.nothing_new,
            prepared.session_ids.len(),
            prepared.backlog,
            prepared.passed_stems.len()
        ),
        Mode::Table => {
            field(
                out,
                "nothing_new",
                if prepared.nothing_new {
                    "true"
                } else {
                    "false"
                },
            )?;
            field(out, "sessions", &prepared.session_ids.len().to_string())?;
            field(out, "backlog", &prepared.backlog.to_string())?;
            field(out, "passed", &prepared.passed_stems.len().to_string())?;
            field(
                out,
                "corrections",
                yes_no(
                    prepared
                        .inputs
                        .as_ref()
                        .is_some_and(|i| i.corrections.is_some()),
                ),
            )
        }
    })
}

/// Render a `memory apply` retry refusal.
pub(crate) fn show_refresh_retry<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    retry: &RefreshRetryPayload,
) -> Result {
    emit_detail(sink, retry, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain => writeln!(
            out,
            "{}\t{}\t{}",
            retry.status,
            retry.code,
            one_line(&retry.correction)
        ),
        Mode::Table => {
            field(out, "status", retry.status)?;
            field(out, "code", retry.code)?;
            field(out, "correction", &retry.correction)
        }
    })
}

/// Render `memory rules`.
pub(crate) fn show_rules<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    rules: &MemoryRulesPayload,
) -> Result {
    let mode = sink.terminal().mode();
    sink.emit(rules, |_| memory_rules(mode, rules))
}

fn memory_detail(out: &mut Detail, m: &MemoryPayload) -> fmt::Result {
    let or_dash = |v: Option<&str>| v.unwrap_or("-").to_owned();
    field(out, "id", &m.id)?;
    field(out, "kind", m.kind)?;
    if m.kind == "profile" {
        field(out, "as_of", &or_dash(m.as_of.as_deref()))?;
        return writeln!(out, "\n{}", m.text.trim_end());
    }
    field(out, "date", &or_dash(m.date.as_deref()))?;
    if let Some(kind) = m.now_kind {
        field(out, "now_kind", kind)?;
    }
    field(out, "title", &or_dash(m.title.as_deref()))?;
    field(out, "text", &m.text)?;
    field(out, "sources", &or_none(&m.sources))?;
    if let Some(updated) = m.updated {
        field(out, "updated", &timestamp(updated))?;
    }
    if m.kind == "now" {
        field(
            out,
            "raised",
            &m.raised.as_ref().map_or_else(
                || String::from("-"),
                |r| format!("{} in {}", timestamp(r.at), r.session),
            ),
        )?;
    }
    Ok(())
}

fn memory_rule_columns() -> Vec<Column> {
    vec![
        Column::new("RULE"),
        Column::new("VALUE"),
        Column::new("ORIGIN"),
        Column::new("MEANING").truncatable(),
    ]
}

/// `memory rules`: plain is `<rule>\t<value>\t<origin>` lines, like `tag
/// show`; the table adds each rule's meaning, cut to fit (`--json` keeps
/// it whole).
fn memory_rules(mode: Mode, rules: &MemoryRulesPayload) -> Table {
    let rows: Vec<Vec<Cell>> = rules
        .rules
        .iter()
        .map(|r| {
            let value = r.value.as_array().map_or_else(
                || r.value.to_string(),
                |list| {
                    list.iter()
                        .map(|v| format!("{:?}", v.as_str().unwrap_or_default()))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );
            let mut row = vec![
                Cell::plain(r.rule),
                Cell::plain(value),
                Cell::plain(origin_text(&r.origin)),
            ];
            if mode == Mode::Table {
                row.push(Cell::plain(r.meaning));
            }
            row
        })
        .collect();
    let mut columns = memory_rule_columns();
    columns.truncate(rows.first().map_or(0, Vec::len));
    table(mode, columns, rows)
}

fn day(date: time::Date) -> String {
    date.format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}
