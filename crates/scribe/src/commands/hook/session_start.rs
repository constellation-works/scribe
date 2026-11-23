//! SessionStart reports open proposals and unpicked actions to the user.
//! An interactive Claude session also receives the ordinary brief. A clear
//! with a continue handoff restores it before the brief (§16.7). On other
//! starts the same-project reminder is advisory and changes no switch.

use super::continuation::{self, ClearedPredecessor};
use super::payload::{self, SessionStartPayload};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::group::SeatStart;
use scribe_core::{ProposalState, Provider, SessionFilter, SessionId, TagName};
use std::collections::BTreeMap;
use std::path::Path;
use time::OffsetDateTime;

/// The hook event this runs for.
const EVENT: &str = "SessionStart";

/// How recently a session record must have been updated to count as the
/// one a new session may continue. A day covers a same-day resume and a
/// next-morning continuation, and stays quiet about older work.
const RECENT_SECS: i64 = 24 * 60 * 60;

/// Run the hook on its stdin payload.
pub(super) fn run(input: &[u8], ctx: &Context) -> Result<Output, CliError> {
    let payload: SessionStartPayload = payload::decode(input)?;
    let session = payload::session(&payload.session_id)?;
    let open = match ctx.store().proposals() {
        Ok(proposals) => proposals
            .iter()
            .filter(|p| p.state == ProposalState::Open)
            .count(),
        Err(err) => {
            tracing::warn!(%err, "cannot read the proposals; not reporting them");
            0
        }
    };
    let proposals = match open {
        0 => None,
        1 => Some(String::from(
            "scribe: 1 proposal to review — scribe proposal list",
        )),
        n => Some(format!(
            "scribe: {n} proposals to review — scribe proposal list"
        )),
    };
    let stuck = match ctx.store().stuck_actions(ctx.now()) {
        Ok(counts) => stuck_line(&counts),
        Err(err) => {
            tracing::warn!(%err, "cannot read the action entries; not reporting them");
            None
        }
    };
    let start_ancestry = ctx.interactive()
        && ctx.provider() == Provider::ClaudeCode
        && matches!(payload.source.as_deref(), None | Some("startup" | "clear"));
    let predecessor = if start_ancestry {
        continuation::cleared_predecessor(ctx, payload.transcript_path.as_deref(), &session)
    } else {
        ClearedPredecessor::None
    };
    let refresh = continuation::at_start(ctx, &session, &predecessor);
    let (continuation, handoff, mut outcomes) = match refresh {
        Some(reply) => (reply.message, reply.context, reply.outcomes),
        None => {
            let reminder = (payload.source.as_deref() != Some("clear"))
                .then(|| continuation_line(ctx, &session, payload.cwd.as_deref()))
                .flatten();
            (reminder, None, Vec::new())
        }
    };
    let seat = if start_ancestry {
        seat_at_start(ctx, &session, &predecessor)
    } else {
        SeatReply::default()
    };
    outcomes.extend(seat.outcomes);
    let lines: Vec<String> = proposals
        .into_iter()
        .chain(stuck)
        .chain(continuation)
        .chain(seat.message)
        .collect();
    let message = (!lines.is_empty()).then(|| lines.join("\n"));
    let brief = ctx
        .interactive()
        .then(|| brief_text(ctx, &session, payload.cwd.as_deref()))
        .flatten();
    let context_parts: Vec<String> = handoff
        .into_iter()
        .chain(seat.context)
        .chain(brief)
        .collect();
    let context = (!context_parts.is_empty()).then(|| context_parts.join("\n\n"));
    Ok(Output::Hook(HookReply {
        event: EVENT,
        message,
        context,
        block: None,
        outcomes,
    }))
}

#[derive(Default)]
struct SeatReply {
    context: Option<String>,
    message: Option<String>,
    outcomes: Vec<HookOutcome>,
}

fn seat_at_start(ctx: &Context, child: &SessionId, predecessor: &ClearedPredecessor) -> SeatReply {
    let parent = match predecessor {
        ClearedPredecessor::One(parent) => Some(parent),
        ClearedPredecessor::None | ClearedPredecessor::Multiple(_) => None,
    };
    match ctx.store().inherit_seat(child, parent) {
        Ok(SeatStart::NoOpenGroups) => SeatReply::default(),
        Ok(SeatStart::AlreadySeated(hold)) => SeatReply {
            context: Some(format!(
                "scribe: session {child} holds seat {} ({}) in {}",
                hold.seat, hold.role, hold.group
            )),
            message: None,
            outcomes: Vec::new(),
        },
        Ok(SeatStart::Replaced) => SeatReply {
            context: Some(format!(
                "scribe: session {child} was replaced on its group seat and holds none now; nothing inherited"
            )),
            message: None,
            outcomes: Vec::new(),
        },
        Ok(SeatStart::Inherited { hold, from }) => SeatReply {
            context: Some(format!(
                "scribe: session {child} holds seat {} ({}) in {} (inherited from {from})",
                hold.seat, hold.role, hold.group
            )),
            message: None,
            outcomes: vec![HookOutcome {
                target: Some(hold.group),
                error_code: None,
            }],
        },
        Ok(SeatStart::Unjoined) => {
            let reason = match predecessor {
                ClearedPredecessor::None => {
                    "scribe: no predecessor line in this transcript; no group seat was inherited — re-run `scribe group join` if your brief names a group"
                        .to_owned()
                }
                ClearedPredecessor::One(parent) => format!(
                    "scribe: predecessor {parent} holds no open group seat; nothing inherited — re-run `scribe group join` if your brief names a group"
                ),
                ClearedPredecessor::Multiple(_) => {
                    "scribe: multiple cleared predecessors in this transcript; no group seat was inherited — re-run `scribe group join` if your brief names a group"
                        .to_owned()
                }
            };
            SeatReply {
                context: Some(reason),
                message: None,
                outcomes: Vec::new(),
            }
        }
        Err(err) => {
            tracing::warn!(%err, "cannot inherit group seat");
            SeatReply {
                context: Some(format!(
                    "scribe: could not inherit group seat ({err}) — re-run `scribe group join` if your brief names a group"
                )),
                message: Some(format!("scribe: could not inherit group seat — {err}")),
                outcomes: vec![HookOutcome {
                    target: Some(format!("session:{child}")),
                    error_code: Some(err.code()),
                }],
            }
        }
    }
}

/// The memory block's text, or `None` when memory holds nothing or the
/// store cannot be read (logged, so the session still starts).
fn memory_block(ctx: &Context) -> Option<String> {
    match crate::commands::memory::block(ctx) {
        Ok(block) => {
            for warning in &block.warnings {
                tracing::warn!("{warning}");
            }
            let empty =
                block.profile.is_none() && block.now.is_empty() && block.milestones.is_empty();
            (!empty).then_some(block.text)
        }
        Err(err) => {
            tracing::warn!(%err, "cannot read the memory; not injecting the memory block");
            None
        }
    }
}

/// The brief for this session's project, or `None` when it has no memory,
/// no session and no claim, or the store cannot be read (logged, so the
/// session still starts). Memory that cannot be read is left out (logged by
/// [`memory_block`]); the rest is still given. With no `cwd`,
/// every project's sessions by date alone.
fn brief_text(ctx: &Context, session: &SessionId, cwd: Option<&str>) -> Option<String> {
    let project = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| project::of(Path::new(cwd)));
    let memory = memory_block(ctx);
    match crate::commands::brief::brief(
        ctx,
        memory,
        project.as_ref(),
        Some(session),
        scribe_core::brief::DEFAULT_SESSIONS,
    ) {
        Ok(brief)
            if brief.memory.is_none() && brief.sessions.is_empty() && brief.claims.is_empty() =>
        {
            None
        }
        Ok(brief) => Some(brief.text()),
        Err(err) => {
            tracing::warn!(%err, "cannot read the sessions or proposals; not injecting the brief");
            None
        }
    }
}

/// The continuation line, or `None` when this session should not be reminded.
///
/// Only a single active record in the project within [`RECENT_SECS`] is
/// suggested; several candidates are named for the user to choose. A session
/// whose switch is on, or that already has a record, is already opted in.
/// No `cwd`, no such record, or a store that cannot be listed: nothing.
fn continuation_line(ctx: &Context, session: &SessionId, cwd: Option<&str>) -> Option<String> {
    let cwd = cwd.map(str::trim).filter(|cwd| !cwd.is_empty())?;
    let project = project::of(Path::new(cwd));
    let listed = match ctx.store().list_sessions(&SessionFilter {
        include_inactive: true,
        ..SessionFilter::default()
    }) {
        Ok(listed) => listed,
        Err(err) => {
            tracing::warn!(%err, "cannot read the sessions; not suggesting a continuation");
            return None;
        }
    };
    if listed
        .control
        .get(session)
        .is_some_and(|control| control.on)
        || listed
            .listing
            .items
            .iter()
            .any(|record| &record.id == session)
    {
        return None;
    }
    let now = ctx.now();
    let ids: Vec<_> = listed
        .listing
        .items
        .into_iter()
        .filter(|record| record.active && record.project == project && recent(now, record.updated))
        .map(|record| record.id)
        .collect();
    let id = match ids.as_slice() {
        [] => return None,
        [id] => id,
        _ => {
            let ids = ids
                .iter()
                .map(SessionId::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            return Some(format!(
                "scribe: possible recorded predecessors {ids} — type /scribe on --from <id> to choose one"
            ));
        }
    };
    Some(format!(
        "scribe: this may continue {id} (recorded) — type /scribe on --from {id} to keep recording"
    ))
}

/// Whether `updated` is within [`RECENT_SECS`] of `now`. A record stamped
/// after `now` still counts: the clock moved, and the session was just written.
fn recent(now: OffsetDateTime, updated: OffsetDateTime) -> bool {
    now.unix_timestamp()
        .saturating_sub(updated.unix_timestamp())
        <= RECENT_SECS
}

/// The stuck-entries line, or `None` when there are none.
fn stuck_line(counts: &BTreeMap<TagName, usize>) -> Option<String> {
    let total: usize = counts.values().sum();
    if total == 0 {
        return None;
    }
    let hours = scribe_core::action::STUCK_AFTER.whole_hours();
    let age = if hours == 1 {
        String::from("1 hour")
    } else {
        format!("{hours} hours")
    };
    let per_tag: Vec<String> = counts.iter().map(|(tag, n)| format!("{tag} {n}")).collect();
    let noun = if total == 1 { "entry" } else { "entries" };
    Some(format!(
        "scribe: {total} action {noun} not picked up after {age} ({}) — scribe entry list --tag <tag> --status active",
        per_tag.join(", ")
    ))
}
