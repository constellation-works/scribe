//! Group turn capture and backfill: a member's typed turns reach the group
//! only while it holds its seat in an open group, within its consent window,
//! and each write rechecks the membership the hook read.

use crate::checks::group::{check_capture_turns, check_own_user_turns};
use crate::checks::refusal;
use crate::instruction::ClaudeGroupTurns;
use crate::message::NewMessage;
use crate::repo::CaptureState;
use crate::{
    ClaudeCapturePosition, ClaudeGroupCapture, Redactor, Result, SessionId, Store, WriteRefusal,
};

pub(super) fn backfill_group_turns(
    store: &Store,
    id: &str,
    members: &[(SessionId, Vec<NewMessage>)],
    redactor: &Redactor,
) -> Result<Vec<usize>> {
    for (session, turns) in members {
        check_own_user_turns(session, turns)?;
    }
    let n = store.read().groups().number(id)?;
    store.write(|tx| {
        tx.groups().rows("backfill group turns in", |rows| {
            if !rows.is_open(n)? {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    "the group is already closed",
                ));
            }
            let holders = rows.holder_count(n)?;
            if usize::try_from(holders).ok() != Some(members.len()) {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    "group membership changed during backfill; retry",
                ));
            }
            let mut counts = Vec::with_capacity(members.len());
            for (session, turns) in members {
                // A restored displaced holder restarted its consent window at
                // its new join time: turns typed before it, during the
                // displacement, were never authorized, so backfill keeps the
                // same cutoff as capture.
                let Some(since) = rows.holder_restart(n, session)? else {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        "group membership changed during backfill; retry",
                    ));
                };
                let eligible = turns
                    .iter()
                    .filter(|m| since.is_none_or(|s| m.at >= s))
                    .collect();
                counts.push(rows.store_selected(session, eligible, redactor)?);
            }
            Ok(counts)
        })
    })
}

pub(super) fn captures_group_turns(store: &Store, session: &SessionId) -> Result<bool> {
    store
        .read()
        .groups()
        .read_rows(|rows| Ok(rows.capture_state(session)?.is_some()))
}

pub(super) fn capture_group_turns(
    store: &Store,
    session: &SessionId,
    messages: &[NewMessage],
    redactor: &Redactor,
) -> Result<usize> {
    check_capture_turns(session, messages)?;
    let Some(before) = store
        .read()
        .groups()
        .read_rows(|rows| rows.capture_state(session))?
    else {
        return Ok(0);
    };
    if !before.holds {
        return skip_displaced_turns(store, session, &before, messages);
    }
    if next_turns(&before, messages).is_empty() {
        return Ok(0);
    }
    store.write(|tx| {
        tx.groups().rows("store group turns in", |rows| {
            let Some(state) = rows.capture_state(session)? else {
                return Ok(0);
            };
            if state.position != before.position || !state.holds {
                return Ok(0);
            }
            let turns = next_turns(&state, messages);
            let Some(last) = turns.last().copied() else {
                return Ok(0);
            };
            let inserted = rows.store_selected(session, turns, redactor)?;
            rows.set_cursor_at(state.position, &last.message)?;
            rows.clear_progress(state.position)?;
            Ok(inserted)
        })
    })
}

/// Move a displaced member's cursor past the turns it typed while a later
/// member held its seat, storing none of them, so they stay out of the
/// group if the seat comes back to it.
fn skip_displaced_turns(
    store: &Store,
    session: &SessionId,
    before: &CaptureState,
    messages: &[NewMessage],
) -> Result<usize> {
    let Some(last) = messages.last() else {
        return Ok(0);
    };
    if before.cursor.as_deref() == Some(last.message.as_str()) {
        return Ok(0);
    }
    store.write(|tx| {
        tx.groups().rows("skip displaced group turns in", |rows| {
            let Some(state) = rows.capture_state(session)? else {
                return Ok(0);
            };
            if state.position == before.position && !state.holds {
                rows.set_cursor_at(state.position, &last.message)?;
                rows.clear_progress(state.position)?;
            }
            Ok(0)
        })
    })
}

pub(super) fn claude_group_capture(
    store: &Store,
    session: &SessionId,
) -> Result<Option<ClaudeGroupCapture>> {
    store
        .read()
        .groups()
        .read_rows(|rows| rows.claude_snapshot(session))
}

pub(super) fn capture_claude_group_turns(
    store: &Store,
    before: &ClaudeGroupCapture,
    resumed: bool,
    position: &ClaudeCapturePosition,
    batch: ClaudeGroupTurns,
    redactor: &Redactor,
) -> Result<Option<usize>> {
    store.write(|tx| {
        tx.groups().rows("store Claude group progress in", |rows| {
            let session = batch.session().clone();
            if rows.claude_snapshot(&session)?.as_ref() != Some(before) {
                return Ok(None);
            }
            let member = before.member();
            let resolved = rows.resolve_seen(member.position, batch, resumed)?;

            let after = if resumed {
                Some(resolved.turns.as_slice())
            } else {
                after_cursor(member, &resolved.turns)
            };
            let skipped = (!member.holds).then(|| resolved.turns.last()).flatten();
            if after.is_none() && skipped.is_none() {
                // A replacement transcript without the old cursor does not
                // establish continuity. Wait for a replay that contains it.
                return Ok(Some(0));
            }
            let turns = if member.holds {
                turns_since(member, after.unwrap_or_default())
            } else {
                Vec::new()
            };
            let last = skipped.or_else(|| turns.last().copied());
            let inserted = rows.store_selected(&session, turns, redactor)?;
            if let Some(last) = last {
                rows.set_cursor_at(member.position, &last.message)?;
            }
            if !resumed {
                rows.clear_progress(member.position)?;
            }
            rows.save_progress(member.position, position)?;
            rows.save_seen(member.position, resolved.seen)?;
            Ok(Some(inserted))
        })
    })
}

fn next_turns<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Vec<&'a NewMessage> {
    turns_since(state, after_cursor(state, messages).unwrap_or_default())
}

fn after_cursor<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Option<&'a [NewMessage]> {
    match &state.cursor {
        Some(cursor) => messages
            .iter()
            .position(|m| &m.message == cursor)
            .map(|i| &messages[i + 1..]),
        None => Some(messages),
    }
}

/// The turns a member's capture may store. A member with no cursor holds its
/// join time as a cutoff, so older turns stay out. Once a cursor is set (by a
/// backfill or an earlier capture) it is the continuity point: every turn after
/// it is new whatever its timestamp, since a turn appended while the cursor's
/// snapshot was being read can be dated before the membership was committed.
/// Restoring a displaced holder's seat clears its cursor (`leave_group`,
/// `forget_session`), so the new join time cuts off the turns typed during the
/// displacement.
fn turns_since<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Vec<&'a NewMessage> {
    if state.cursor.is_some() {
        return messages.iter().collect();
    }
    messages.iter().filter(|m| m.at >= state.since).collect()
}
