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
    let additions = match additions(ctx, named) {
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
        ctx.now(),
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
/// named, with the typed turns of those whose transcript is on disk. An id or
/// prefix a transcript matches resolves against transcripts first; any other
/// is resolved against the store and otherwise taken as typed, since a session
/// that was never recorded has no other source.
fn additions(ctx: &Context, named: &[SessionId]) -> Result<Vec<GroupAddition>, CliError> {
    let files = payload::files_for(ctx)?;
    let ids: Vec<String> = files.iter().map(|(id, _)| id.to_string()).collect();
    let store: &Store = ctx.store();
    let mut additions: Vec<GroupAddition> = Vec::new();
    for input in named {
        let id = match scribe_core::resolve_prefix(input.as_str(), &ids)? {
            Some(full) => full
                .parse::<SessionId>()
                .map_err(|e| CliError::Usage(e.to_string()))?,
            None => store.resolve_session_or_typed(input)?,
        };
        if additions.iter().any(|a| a.session == id) {
            continue;
        }
        let turns = match files.iter().find(|(found, _)| *found == id) {
            Some((_, path)) => group_turns::past(ctx, &id, path)?,
            None => None,
        };
        additions.push(GroupAddition { session: id, turns });
    }
    Ok(additions)
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
