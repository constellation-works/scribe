//! `/scribe group add <slug> <session-id>...`: add past sessions to a project
//! group on a typed line (docs/design/scribe/2_design.md §4.4).
//!
//! The caller has already checked that the prompt is a typed user turn and
//! that its transcript passes the provenance check, exactly as for
//! `/scribe group <slug>`; this module acts only on a line that passed both.
//! It opens the group when absent and adds each named session, whether or not
//! the session was ever recorded. A session whose transcript is on disk has its
//! typed turns stored, chosen as a member's own turns are chosen
//! ([`super::group_turns::past`]); one without a transcript gets membership
//! only. The reply says which, per session. A session already in another
//! group refuses the whole line. The session the line is typed in is added
//! only when it names itself.
//!
//! The reply is a refusal or a success, never an error exit, and the prompt is
//! blocked as a `/scribe` line always is. Audit outcomes carry session ids and
//! error codes only.

use super::payload::{self, PromptPayload};
use super::{group_turns, user_prompt_submit};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{HookOutcome, HookReply, Output};
use scribe_core::GroupVerbs;
use scribe_core::group::{Added, GroupAddition};
use scribe_core::{SessionId, Store};

/// The most sessions one line may name: each is a transcript read.
pub(super) const MAX_SESSIONS: usize = 32;

/// Apply a typed `group add` line.
pub(super) fn run(
    slug: &str,
    named: &[SessionId],
    payload: &PromptPayload,
    session: &SessionId,
    ctx: &Context,
) -> Result<Output, CliError> {
    run_with_transcript_reader(slug, named, payload, session, ctx, |ctx, id, path| {
        #[cfg(test)]
        super::tests::group_add::before_past_read(path);
        group_turns::past(ctx, id, path)
    })
}

pub(super) fn run_with_transcript_reader(
    slug: &str,
    named: &[SessionId],
    payload: &PromptPayload,
    session: &SessionId,
    ctx: &Context,
    mut read_past: impl FnMut(
        &Context,
        &SessionId,
        &std::path::Path,
    ) -> Result<Option<Vec<scribe_core::message::NewMessage>>, CliError>,
) -> Result<Output, CliError> {
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
    let redactor = ctx.redactor(&ctx.config()?)?;
    // Joined time is taken before any transcript is read, so a turn appended
    // while the snapshots are read is never dated before the membership.
    let membership_at = ctx.now();
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
    let project = super::project::of(std::path::Path::new(&payload.cwd));
    let added = ctx.store().add_project_group_sessions(
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
fn additions(
    ctx: &Context,
    named: &[SessionId],
    read_past: &mut impl FnMut(
        &Context,
        &SessionId,
        &std::path::Path,
    ) -> Result<Option<Vec<scribe_core::message::NewMessage>>, CliError>,
) -> Result<Vec<GroupAddition>, CliError> {
    let mut files = std::collections::BTreeMap::new();
    // Sessions with a regular transcript in more than one provider: kept in the
    // id universe for prefix resolution, and refused only if a named session
    // resolves to one.
    let mut cross_provider = std::collections::BTreeSet::new();
    // Entries named like a transcript whose file type is refused: present, so
    // a named session among them is read through the checked open, which
    // refuses it, never treated as membership only.
    let mut refused: std::collections::BTreeMap<
        SessionId,
        Vec<(std::path::PathBuf, scribe_core::Provider)>,
    > = std::collections::BTreeMap::new();
    for provider in [
        scribe_core::Provider::ClaudeCode,
        scribe_core::Provider::Codex,
    ] {
        let provider_ctx = ctx.with_provider(provider);
        let found = payload::discovered_for(&provider_ctx)?;
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
    let store: &Store = ctx.store();
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
                let mut resolved: Option<(
                    Vec<scribe_core::message::NewMessage>,
                    scribe_core::Provider,
                )> = None;
                for (path, provider) in entries {
                    let Some(turns) = read_past(&ctx.with_provider(*provider), &id, path)? else {
                        return Err(CliError::TranscriptRefused {
                            id,
                            reason: String::from(
                                "the refused transcript disappeared after discovery",
                            ),
                        });
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
                Some((path, provider)) => read_past(&ctx.with_provider(*provider), &id, path)?,
                None => None,
            },
        };
        additions.push(GroupAddition { session: id, turns });
    }
    Ok(additions)
}

fn collision_error(id: &SessionId, cross_provider: bool) -> CliError {
    if cross_provider {
        CliError::Usage(format!(
            "session {id} exists in multiple transcript providers"
        ))
    } else {
        CliError::Usage(String::from("ambiguous duplicate transcript session id"))
    }
}

fn joined(id: &SessionId) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{id}")),
        error_code: None,
    }
}

fn refusal(reason: String, code: &'static str, session: &SessionId) -> Output {
    block(
        reason,
        vec![HookOutcome {
            target: Some(format!("session:{session}")),
            error_code: Some(code),
        }],
    )
}

fn block(reason: String, outcomes: Vec<HookOutcome>) -> Output {
    Output::Hook(HookReply {
        event: user_prompt_submit::EVENT,
        message: None,
        context: None,
        block: Some(reason),
        outcomes,
    })
}
