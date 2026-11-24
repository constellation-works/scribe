//! `scribe group join` (ORB-27707 step 3c): seat the calling session in a
//! group under its own author name.

use super::verbs::GroupVerbs;
use super::{Error as GroupError, earlier_turns, ended, redactor};
use crate::apps::session::{self, ClearedPredecessor};
use crate::group::{JoinGroup, automatic_seat};
use crate::runtime::{Ctx, PortError};
use crate::{AuthorName, Label, Provider, SessionId};

/// What `group join` runs.
pub struct GroupJoinInput {
    /// The calling session.
    pub session: SessionId,
    /// Group id, unique prefix, or project group slug.
    pub id: String,
    /// Seat name; `None` takes the automatic seat of a project group.
    pub seat: Option<Label>,
    /// The caller's agent author name.
    pub author: AuthorName,
    /// Role; `None` keeps an existing seat's role.
    pub role: Option<String>,
    /// Replace the newest holder of an occupied seat.
    pub replace: bool,
    /// The provider whose session calls.
    pub provider: Provider,
}

/// What `group join` did.
#[derive(Debug)]
pub struct GroupJoined {
    /// The group's canonical id.
    pub id: String,
    /// Whether the join changed the store.
    pub changed: bool,
    /// Notices for the user.
    pub notices: Vec<String>,
}

/// Join the group.
///
/// # Errors
///
/// [`GroupError`]: a usage error, a store refusal, a transcript read, or a
/// port.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: GroupJoinInput,
) -> Result<GroupJoined, GroupError<E>> {
    let GroupJoinInput {
        session: caller,
        id,
        seat,
        author,
        role,
        replace,
        provider,
    } = input;
    let store = ctx.store;
    let mut notices = Vec::new();
    let group = store.read().groups().get(&id)?;
    let seat = match seat {
        Some(seat) => seat,
        None if group.slug.is_some() => {
            // A retry keeps the seat this session already holds, even
            // when the short name it first missed has since been freed.
            match group.seats.iter().find(|s| s.transcripts.contains(&caller)) {
                Some(held) => Label::sanitized(&held.name, "member"),
                None => automatic_seat(&caller, |name| {
                    Ok::<_, std::convert::Infallible>(group.seats.iter().any(|s| s.name == name))
                })
                .unwrap_or_else(|e| match e {}),
            }
        }
        None => return Err(GroupError::Usage("pass --as for a named seat".into())),
    };
    let seat_exists = group.seats.iter().any(|s| s.name == seat.as_str());
    let role = role.or_else(|| (group.slug.is_some() && !seat_exists).then(|| "member".to_owned()));
    let latest = group
        .seats
        .iter()
        .find(|s| s.name == seat.as_str())
        .and_then(|s| s.transcripts.last());
    let (ended, predecessor) = match latest {
        Some(previous) if !replace && previous != &caller => {
            let predecessor = if provider == Provider::ClaudeCode {
                let path = ctx
                    .transcripts
                    .locate_own(provider, &caller)
                    .map_err(GroupError::Port)?;
                match session::cleared_predecessor(ctx, provider, path.as_deref(), &caller) {
                    ClearedPredecessor::One(parent) if &parent == previous => Some(parent),
                    ClearedPredecessor::None
                    | ClearedPredecessor::One(_)
                    | ClearedPredecessor::Multiple(_) => None,
                }
            } else {
                None
            };
            let ended = if predecessor.is_none() && ended(ctx, previous)? {
                Some(previous)
            } else {
                None
            };
            (ended, predecessor)
        }
        _ => (None, None),
    };
    // Taken before the transcript is read, so a first turn appended
    // after an empty snapshot is not dated before the membership.
    let snapshot_at = ctx.clock.now();
    let earlier = earlier_turns(ctx, provider, &caller)?;
    let join = JoinGroup {
        group: &group.id,
        session: &caller,
        seat: &seat,
        author: &author,
        role: role.as_deref(),
        replace,
        ended,
        predecessor: predecessor.as_ref(),
        earlier_turns: &earlier,
        snapshot_at,
    };
    let changed = store.join_group(join, &redactor(ctx, provider)?)?;
    if !changed {
        notices.push("session already holds this seat".into());
    }
    Ok(GroupJoined {
        id: group.id,
        changed,
        notices,
    })
}
