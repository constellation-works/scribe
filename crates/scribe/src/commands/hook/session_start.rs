//! `scribe hook session-start`: tell Daniel how many proposals wait for him
//! (docs/design/scribe/2_design.md §7), how many action entries no consumer
//! has picked up (§4.6), and, when this session may continue a recorded
//! one, the command that keeps recording it (§5.4, §7).
//!
//! Read-only and model-free: it reads `proposals.json` and, when any
//! proposal (including a session-pass candidate) is open, prints one
//! `systemMessage` line for Daniel, `scribe: 3 proposals to review —
//! scribe proposal list`. It then counts the active entries under action
//! tags older than `action::STUCK_AFTER` with no pickup in `pickups.json`
//! and, when there are any, adds one line, `scribe: 2 action entries not
//! picked up after 1 hour (standard 1, remind 1) — scribe entry list --tag
//! <tag> --status active`. When this session is not opted in and an active
//! session in the same project was recorded within the last day, it adds
//! `scribe: this may continue <id> (recorded) — type /scribe on --from <id>
//! to keep recording`, naming the one updated most recently. The agent is
//! not told: settling is Daniel's, in a typed turn, and so is the link.
//! Nothing to report, no store yet, or a store that cannot be read
//! (logged) prints nothing, so a session always starts. It writes nothing
//! and is not audited.

use super::payload::{self, SessionStartPayload};
use super::project;
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookReply, Output};
use scribe_core::{ProposalState, SessionFilter, SessionId, TagName};
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
    let continuation = continuation_line(ctx, &session, payload.cwd.as_deref());
    let lines: Vec<String> = proposals
        .into_iter()
        .chain(stuck)
        .chain(continuation)
        .collect();
    let message = (!lines.is_empty()).then(|| lines.join("\n"));
    Ok(Output::Hook(HookReply {
        event: EVENT,
        message,
        context: None,
        block: None,
        outcomes: Vec::new(),
    }))
}

/// The continuation line, or `None` when this session should not be reminded.
///
/// The predecessor is the active record in `cwd`'s project, other than
/// this session, with the newest `updated` inside [`RECENT_SECS`]. A session
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
    let previous = listed
        .listing
        .items
        .iter()
        .filter(|record| record.active && record.project == project)
        .filter(|record| recent(now, record.updated))
        .max_by(|left, right| {
            left.updated
                .cmp(&right.updated)
                .then_with(|| left.id.cmp(&right.id))
        })?;
    let id = previous.id.as_str();
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
