//! `scribe group backfill` (ORB-27707 step 3c): store earlier typed turns for
//! each current seat holder.

use crate::cases::group::GroupVerbs;
use crate::cases::group_cmd::{GroupError, locate, redactor};
use crate::cases::group_turns;
use crate::runtime::{Ctx, PortError};
use crate::{Provider, WriteRefusal};

/// One seat's backfill.
#[derive(Debug)]
pub struct BackfilledMember {
    /// The seat.
    pub seat: String,
    /// Its newest holder.
    pub session: String,
    /// Turns stored.
    pub stored: usize,
    /// Turns already stored, skipped.
    pub skipped: usize,
    /// No transcript of the holder could be read as turns.
    pub transcript_unavailable: bool,
}

/// What `group backfill` did.
#[derive(Debug)]
pub struct GroupBackfilled {
    /// The group's canonical id.
    pub group: String,
    /// One per seat with a holder.
    pub members: Vec<BackfilledMember>,
    /// Turns stored in all.
    pub stored: usize,
    /// Turns skipped in all.
    pub skipped: usize,
}

/// Backfill group `id`. `provider` is the calling provider, whose config
/// redacts.
///
/// # Errors
///
/// [`GroupError`]: the group is closed, a store error, or a port.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    provider: Provider,
) -> Result<GroupBackfilled, GroupError<E>> {
    let store = ctx.store;
    let group = store.read().groups().get(id)?;
    if group.closed.is_some() {
        return Err(crate::Error::AgentWrite {
            kind: WriteRefusal::GroupMembership,
            message: "the group is already closed".into(),
        }
        .into());
    }
    let mut report = GroupBackfilled {
        group: group.id.clone(),
        members: Vec::new(),
        stored: 0,
        skipped: 0,
    };
    let mut batches = Vec::new();
    for seat in &group.seats {
        let Some(session) = seat.transcripts.last() else {
            continue;
        };
        let mut turns = None;
        for found_in in [Provider::ClaudeCode, Provider::Codex] {
            if let Ok(Some(path)) = locate(ctx, found_in, session)
                && let Ok(Some(bytes)) = ctx.transcripts.read_checked(found_in, &path, session)
                && let Ok(found) = group_turns::typed_turns::<E>(found_in, session, &bytes)
            {
                turns = Some(found);
                break;
            }
        }
        let unavailable = turns.is_none();
        let turns = turns.unwrap_or_default();
        report.members.push(BackfilledMember {
            seat: seat.name.clone(),
            session: session.to_string(),
            stored: 0,
            skipped: 0,
            transcript_unavailable: unavailable,
        });
        batches.push((session.clone(), turns));
    }
    let counts = store.backfill_group_turns(&group.id, &batches, &redactor(ctx, provider)?)?;
    for ((member, (_, turns)), stored) in report.members.iter_mut().zip(&batches).zip(counts) {
        member.stored = stored;
        member.skipped = turns.len() - stored;
        report.stored += stored;
        report.skipped += member.skipped;
    }
    Ok(report)
}
