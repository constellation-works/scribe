//! SessionStart (ORB-27707 step 3c) reports open proposals and unpicked
//! actions to the user.
//! An interactive session also receives the ordinary brief
//! ([`crate::apps::brief`]) after the rest of [`run`]'s context. A clear
//! with a continue handoff restores it before the brief (§16.7). On other
//! starts the same-project reminder is advisory and changes no switch.

use super::continuation;
use super::reply::HookReply;
use crate::apps::brief;
use crate::apps::group;
use crate::apps::session::{self, ClearedPredecessor};
use crate::apps::tag::ConfigReads;
use crate::group::SeatStart;
use crate::runtime::audit::HookOutcome;
use crate::runtime::{Ctx, PortError, TranscriptState};
use crate::{ProposalState, Provider, SessionFilter, SessionId, TagName};
use std::collections::BTreeMap;
use std::path::Path;
use time::OffsetDateTime;

/// The hook event this runs for.
const EVENT: &str = "SessionStart";

/// How recently a session record must have been updated to count as the
/// one a new session may continue. A day covers a same-day resume and a
/// next-morning continuation, and stays quiet about older work.
const RECENT_SECS: i64 = 24 * 60 * 60;

/// What `SessionStart` runs on: its parsed payload.
pub struct SessionStartInput<'i> {
    /// The session.
    pub session: &'i SessionId,
    /// Its transcript, when the payload names one.
    pub transcript_path: Option<&'i Path>,
    /// Why the session started (`startup`, `clear`, `resume`, ...).
    pub source: Option<&'i str>,
    /// The working directory, when the harness sends one.
    pub cwd: Option<&'i str>,
    /// Whether this runs under an interactive Claude Code session.
    pub interactive: bool,
    /// The provider whose hook this is.
    pub provider: Provider,
}

/// Run the hook on its parsed payload. The reply's context is the
/// restored handoff and the seat; the CLI appends the brief.
#[must_use]
pub fn run<E: PortError>(ctx: &Ctx<'_, E>, input: &SessionStartInput<'_>) -> HookReply {
    let SessionStartInput {
        session,
        transcript_path,
        source,
        cwd,
        interactive,
        provider,
    } = *input;
    let session = session.clone();
    let open = match ctx.store.read().proposals().all() {
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
    let items = match ctx.store.read().entries().list(&crate::EntryFilter {
        tag: crate::entry::OPEN_ITEM_TAG.parse().ok(),
        status: Some(crate::EntryStatus::Active),
        ..crate::EntryFilter::default()
    }) {
        Ok(listed) => {
            let n = crate::brief::open_items(&listed.listing.items).len();
            (n > 0).then(|| {
                format!(
                    "scribe: {n} open {} for you",
                    if n == 1 { "item" } else { "items" }
                )
            })
        }
        Err(err) => {
            tracing::warn!(%err, "cannot read open items; not reporting them");
            None
        }
    };
    let stuck = match stuck_actions(&ctx.store.read(), ctx.clock.now()) {
        Ok(counts) => stuck_line(&counts),
        Err(err) => {
            tracing::warn!(%err, "cannot read the action entries; not reporting them");
            None
        }
    };
    let start_ancestry = interactive
        && provider == Provider::ClaudeCode
        && matches!(source, None | Some("startup" | "clear"));
    let missing = start_ancestry
        && transcript_path.is_some_and(|path| {
            matches!(
                ctx.transcripts.check(provider, path, &session),
                Ok(TranscriptState::Missing)
            )
        });
    // A marker that cannot be recorded is reported, not fatal: the start
    // still prints its brief, memory, proposals and handoff.
    let deferral = missing.then(|| group::defer_ancestry(ctx.store, &session));
    if let Some(Err(err)) = &deferral {
        tracing::warn!(%err, "cannot record the deferred ancestry check");
    }
    let predecessor = if start_ancestry && !missing {
        session::cleared_predecessor(ctx, provider, transcript_path, &session)
    } else {
        ClearedPredecessor::None
    };
    let refresh = continuation::at_start(ctx, &session, &predecessor);
    let (continuation, handoff, mut outcomes) = match refresh {
        Some(reply) => (reply.message, reply.context, reply.outcomes),
        None => {
            let reminder = (source != Some("clear"))
                .then(|| continuation_line(ctx, &session, cwd))
                .flatten();
            (reminder, None, Vec::new())
        }
    };
    if matches!(deferral, Some(Ok(true))) {
        outcomes.push(HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: None,
        });
    }
    let seat = match deferral {
        Some(Ok(_)) => SeatReply {
            context: Some(
                "scribe: transcript not written yet; ancestry checked at the first prompt".into(),
            ),
            ..SeatReply::default()
        },
        Some(Err(err)) => deferral_failure(&session, &err),
        None if start_ancestry => seat_at_start(ctx, &session, &predecessor),
        None => SeatReply::default(),
    };
    outcomes.extend(seat.outcomes);
    let lines: Vec<String> = proposals
        .into_iter()
        .chain(items)
        .chain(stuck)
        .chain(continuation)
        .chain(seat.message)
        .collect();
    let message = (!lines.is_empty()).then(|| lines.join("\n"));
    let brief = interactive
        .then(|| brief::at_start(ctx, provider, &session, cwd))
        .flatten();
    let context_parts: Vec<String> = handoff
        .into_iter()
        .chain(seat.context)
        .chain(brief)
        .collect();
    let context = (!context_parts.is_empty()).then(|| context_parts.join("\n\n"));
    HookReply {
        event: EVENT,
        message,
        context,
        block: None,
        outcomes,
    }
}

/// The reply when the deferred check could not be recorded. No marker exists,
/// so the first prompt will not check this start's predecessor; no seat is
/// inherited here.
fn deferral_failure(child: &SessionId, err: &crate::Error) -> SeatReply {
    SeatReply {
        context: Some(format!(
            "scribe: could not record the deferred ancestry check ({err}) — re-run `scribe group join` if your brief names a group"
        )),
        message: Some(format!(
            "scribe: could not record the deferred ancestry check — {err}"
        )),
        outcomes: vec![HookOutcome {
            target: Some(format!("session:{child}")),
            error_code: Some(err.code()),
        }],
    }
}

/// What a session's group seat at its start adds to the hook's reply.
#[derive(Debug, Default)]
pub struct SeatReply {
    /// For the agent.
    pub context: Option<String>,
    /// For the user.
    pub message: Option<String>,
    /// For the audit log.
    pub outcomes: Vec<HookOutcome>,
}

/// Seat `child` at its start: keep the seat it holds, or inherit its one
/// cleared predecessor's, and say which.
#[must_use]
pub fn seat_at_start<E>(
    ctx: &Ctx<'_, E>,
    child: &SessionId,
    predecessor: &ClearedPredecessor,
) -> SeatReply {
    let parent = match predecessor {
        ClearedPredecessor::One(parent) => Some(parent),
        ClearedPredecessor::None | ClearedPredecessor::Multiple(_) => None,
    };
    match group::inherit_seat(ctx.store, child, parent) {
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

/// The continuation line, or `None` when this session should not be reminded.
///
/// Only a single active record in the project within [`RECENT_SECS`] is
/// suggested; several candidates are named for the user to choose. A session
/// whose switch is on, or that already has a record, is already opted in.
/// No `cwd`, no such record, or a store that cannot be listed: nothing.
fn continuation_line<E: PortError>(
    ctx: &Ctx<'_, E>,
    session: &SessionId,
    cwd: Option<&str>,
) -> Option<String> {
    let cwd = cwd.map(str::trim).filter(|cwd| !cwd.is_empty())?;
    let project = ctx.transcripts.project_of(Path::new(cwd));
    let listed = match crate::apps::session::list_sessions(
        &ctx.store.read(),
        &SessionFilter {
            include_inactive: true,
            ..SessionFilter::default()
        },
        None,
    ) {
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
    let now = ctx.clock.now();
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
/// Active action entries older than [`crate::action::STUCK_AFTER`] at
/// `now` with no pickup, counted per tag. Reads only.
fn stuck_actions(
    read: &crate::Snapshot<'_>,
    now: time::OffsetDateTime,
) -> crate::Result<BTreeMap<TagName, usize>> {
    if !read.exists()? {
        return Ok(BTreeMap::new());
    }
    let config = read.tags().config()?;
    let (entries, _) = read.entries().all()?;
    Ok(crate::action::stuck(
        &entries,
        &config.tags,
        &read.pickups().all()?,
        now,
    ))
}

fn stuck_line(counts: &BTreeMap<TagName, usize>) -> Option<String> {
    let total: usize = counts.values().sum();
    if total == 0 {
        return None;
    }
    let hours = crate::action::STUCK_AFTER.whole_hours();
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
