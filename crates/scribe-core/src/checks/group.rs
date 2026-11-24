//! The checks of a group write that run before its transaction: the
//! consent, the turns a member brings, the names it uses (2_design.md §17).
//! The checks that read the group's rows run inside its write.

use super::{check_agent_author, check_content, refusal};
use crate::group::{GroupAddition, GroupConsent, JoinGroup, NewGroup};
use crate::message::NewMessage;
use crate::{Redactor, RelatedKind, Result, SessionId, WriteRefusal};

pub(crate) fn number(id: &str) -> Option<i64> {
    let n = id.strip_prefix("g-")?.parse::<i64>().ok()?;
    (n > 0 && format!("g-{n}") == id).then_some(n)
}

/// A project group slug: lowercase letters, digits and interior hyphens, not a
/// group id, and free of configured sensitive text.
pub(crate) fn check_project_slug(slug: &str, redactor: &Redactor) -> Result<()> {
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || slug.starts_with('-')
        || slug.ends_with('-')
        || number(slug).is_some()
    {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug must be lowercase letters, digits and interior hyphens, and not a group id",
        ));
    }
    if redactor.mask(slug) != slug {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug contains sensitive text",
        ));
    }
    Ok(())
}

/// A backfill takes only the named session's own user turns.
pub(crate) fn check_own_user_turns(session: &SessionId, turns: &[NewMessage]) -> Result<()> {
    if turns
        .iter()
        .any(|m| &m.session != session || m.role != crate::Speaker::User)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group backfill takes only the member's own user turns",
        ));
    }
    Ok(())
}

/// A group opens on consent given for opening one, with the opener's own
/// user turns, a title and kind as content, and no session among its
/// references.
pub(crate) fn check_new_group(
    draft: &NewGroup,
    consent: &GroupConsent,
    earlier_turns: &[NewMessage],
) -> Result<()> {
    if !consent.authorizes_open() {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "the quoted consent was given for another operation, not opening a group",
        ));
    }
    check_own_user_turns(consent.session(), earlier_turns)?;
    check_content(&draft.title)?;
    check_content(&draft.kind)?;
    if draft
        .related
        .iter()
        .any(|r| r.kind() == RelatedKind::Session)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group references accept task:, pr: and repo:; use group join for a session",
        ));
    }
    Ok(())
}

/// A project-group backfill takes only each named session's user turns.
pub(crate) fn check_additions(additions: &[GroupAddition]) -> Result<()> {
    if additions.iter().any(|addition| {
        addition.turns.as_ref().is_some_and(|turns| {
            turns
                .iter()
                .any(|turn| turn.session != addition.session || turn.role != crate::Speaker::User)
        })
    }) {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "project group backfill takes only the named session's user turns",
        ));
    }
    Ok(())
}

/// A join names an agent author that is not reserved, a role that is
/// content, and the member's own user turns.
pub(crate) fn check_join(join: &JoinGroup<'_>) -> Result<()> {
    check_agent_author(join.author)?;
    if let Some(role) = join.role {
        check_content(role)?;
    }
    check_own_user_turns(join.session, join.earlier_turns)
}

/// A capture takes only the member's own verified user turns.
pub(crate) fn check_capture_turns(session: &SessionId, messages: &[NewMessage]) -> Result<()> {
    if messages
        .iter()
        .any(|m| &m.session != session || m.role != crate::Speaker::User)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group capture takes only the member's verified user turns",
        ));
    }
    Ok(())
}
