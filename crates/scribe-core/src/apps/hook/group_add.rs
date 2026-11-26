//! `/scribe group add <slug> <session-id>...`: add past sessions to a project
//! group on a typed line (docs/design/scribe/2_design.md §4.4; ORB-27707
//! step 3b).
//!
//! The caller has already checked that the prompt is a typed user turn and
//! that its transcript passes the provenance check, exactly as for
//! `/scribe group <slug>`; this module acts only on a line that passed both.
//! It opens the group when absent and adds each named session, whether or not
//! the session was ever recorded. A session whose transcript is on disk has its
//! typed turns stored, chosen as a member's own turns are chosen
//! (the group app's `past_turns`); one without a transcript gets membership
//! only. The reply says which, per session. A session already in another
//! group refuses the whole line. The session the line is typed in is added
//! only when it names itself.
//!
//! The reply is a refusal or a success, never an error exit, and the prompt is
//! blocked as a `/scribe` line always is. Audit outcomes carry session ids and
//! error codes only.

use super::reply::{HookPortError, HookReply, USER_PROMPT_SUBMIT_EVENT as EVENT};
use crate::apps::group as group_app;
use crate::apps::group::HookError;
use crate::apps::tag::ConfigReads;
use crate::group::{Added, GroupAddition};
use crate::message::NewMessage;
use crate::runtime::audit::HookOutcome;
use crate::runtime::{Ctx, Discovered};
use crate::{Provider, SessionId, Store};
use std::path::Path;

/// The most sessions one line may name: each is a transcript read.
pub const MAX_SESSIONS: usize = 32;

/// What a typed `group add` line runs on.
pub struct GroupAddInput<'i> {
    /// The group's slug, as typed.
    pub slug: &'i str,
    /// The sessions the line names.
    pub named: &'i [SessionId],
    /// The session the line is typed in.
    pub session: &'i SessionId,
    /// The prompt's working directory.
    pub cwd: &'i str,
    /// The provider whose hook this is.
    pub provider: Provider,
}

/// Apply a typed `group add` line, reading each named session's past
/// transcript with `read_past`.
///
/// # Errors
///
/// [`HookError`]: the config or the redactor failed. A refusal is a reply.
pub fn run<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    input: &GroupAddInput<'_>,
    mut read_past: impl FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>,
) -> Result<HookReply, HookError<E>> {
    let GroupAddInput {
        slug,
        named,
        session,
        cwd,
        provider,
    } = *input;
    if named.len() > MAX_SESSIONS {
        return Ok(refusal(
            format!(
                "scribe: group add refused — {} sessions named; at most {MAX_SESSIONS} per line; nothing changed",
                named.len()
            ),
            "group_membership",
            session,
        ));
    }
    let config = ctx.store.read().tags().config()?.for_provider(provider);
    let redactor = ctx.redaction.redactor(&config).map_err(HookError::Port)?;
    // Joined time is taken before any transcript is read, so a turn appended
    // while the snapshots are read is never dated before the membership.
    let membership_at = ctx.clock.now();
    let additions = match additions(ctx, named, &mut read_past) {
        Ok(additions) => additions,
        Err(err) => {
            return Ok(refusal(
                format!("scribe: group add refused — {err}; nothing changed"),
                err.code(),
                session,
            ));
        }
    };
    let project = ctx.transcripts.project_of(Path::new(cwd));
    let added = group_app::add_project_group_sessions(
        ctx.store,
        slug,
        &project,
        session,
        &additions,
        membership_at,
        &redactor,
    );
    Ok(match added {
        Ok(added) => {
            let mut lines = vec![format!(
                "scribe: group {slug} ({}) {}",
                added.id,
                if added.created { "opened" } else { "open" }
            )];
            let mut outcomes = Vec::new();
            for (id, result) in &added.sessions {
                lines.push(match result {
                    Added::Joined { stored: Some(n) } => {
                        outcomes.push(joined(id));
                        format!(
                            "  {id}: added; transcript on disk, {n} typed {} stored",
                            if *n == 1 { "turn" } else { "turns" }
                        )
                    }
                    Added::Joined { stored: None } => {
                        outcomes.push(joined(id));
                        format!("  {id}: added; membership only, no transcript on disk")
                    }
                    Added::AlreadyMember => {
                        format!("  {id}: already a member; nothing changed")
                    }
                });
            }
            block(lines.join("\n"), outcomes)
        }
        Err(err) => refusal(
            format!("scribe: group add refused — {err}"),
            err.code(),
            session,
        ),
    })
}

/// The named sessions, each resolved to a full id, deduplicated in the order
/// named, with the typed turns of those whose transcript is on disk. Resolution
/// searches the union of known store sessions, existing group members, and
/// transcripts on disk so an exact id wins across every source and a shared
/// prefix refuses the line before writes; unknown ids stay as typed, since a
/// session that was never recorded has no other source.
fn additions<E: HookPortError>(
    ctx: &Ctx<'_, E>,
    named: &[SessionId],
    read_past: &mut impl FnMut(Provider, &SessionId, &Path) -> Result<Option<Vec<NewMessage>>, E>,
) -> Result<Vec<GroupAddition>, E> {
    let mut files = std::collections::BTreeMap::new();
    // Sessions with a regular transcript in more than one provider: kept in the
    // id universe for prefix resolution, and refused only if a named session
    // resolves to one.
    let mut cross_provider = std::collections::BTreeSet::new();
    // Entries named like a transcript whose file type is refused: present, so
    // a named session among them is read through the checked open, which
    // refuses it, never treated as membership only.
    let mut refused: std::collections::BTreeMap<SessionId, Vec<(std::path::PathBuf, Provider)>> =
        std::collections::BTreeMap::new();
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let found: Discovered = ctx.transcripts.discovered(provider)?;
        for (id, path) in found.files {
            match files.entry(id) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert((path, provider));
                }
                std::collections::btree_map::Entry::Occupied(slot) => {
                    cross_provider.insert(slot.key().clone());
                }
            }
        }
        for (id, path) in found.refused {
            refused.entry(id).or_default().push((path, provider));
        }
    }
    let store: &Store = ctx.store;
    let mut ids: Vec<String> = files
        .keys()
        .chain(refused.keys())
        .map(ToString::to_string)
        .collect();
    for group in store.read().groups().all()? {
        ids.extend(group.members().into_iter().map(|id| id.to_string()));
    }
    let mut additions: Vec<GroupAddition> = Vec::new();
    for input in named {
        let id = store.read().sessions().resolve_or_typed_with(input, &ids)?;
        if additions.iter().any(|a| a.session == id) {
            continue;
        }
        if cross_provider.contains(&id) {
            return Err(collision_error(&id, true));
        }
        // A refused entry takes precedence over a regular one for the same id,
        // so a session with both is read through the checked open and refuses
        // the whole line, never backfilled from its regular transcript.
        let turns = match refused.get(&id) {
            Some(entries) => {
                let mut resolved: Option<(Vec<NewMessage>, Provider)> = None;
                for (path, provider) in entries {
                    let Some(turns) = read_past(*provider, &id, path)? else {
                        return Err(E::from(HookError::TranscriptRefused {
                            id,
                            reason: String::from(
                                "the refused transcript disappeared after discovery",
                            ),
                        }));
                    };
                    if let Some((_, other)) = files.get(&id) {
                        return Err(collision_error(&id, *other != *provider));
                    }
                    if let Some((_, prev_provider)) = &resolved {
                        return Err(collision_error(&id, *prev_provider != *provider));
                    }
                    resolved = Some((turns, *provider));
                }
                resolved.map(|(turns, _)| turns)
            }
            None => match files.get(&id) {
                Some((path, provider)) => read_past(*provider, &id, path)?,
                None => None,
            },
        };
        additions.push(GroupAddition { session: id, turns });
    }
    Ok(additions)
}

fn collision_error<E: HookPortError>(id: &SessionId, cross_provider: bool) -> E {
    E::from(HookError::Usage(if cross_provider {
        format!("session {id} exists in multiple transcript providers")
    } else {
        String::from("ambiguous duplicate transcript session id")
    }))
}

fn joined(id: &SessionId) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{id}")),
        error_code: None,
    }
}

fn refusal(reason: String, code: &'static str, session: &SessionId) -> HookReply {
    block(
        reason,
        vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: Some(code),
        }],
    )
}

fn block(reason: String, outcomes: Vec<HookOutcome>) -> HookReply {
    HookReply {
        event: EVENT,
        message: None,
        context: None,
        block: Some(reason),
        outcomes,
    }
}
