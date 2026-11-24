//! The transcript checks of `entry add` and `entry answer` (ORB-27707 step
//! 3d): a quote, a decision's quotes, a quote-less refresh's ancestry and an
//! open item's answer, each read from the named sessions' transcripts on
//! this host before the store lock.

use crate::cases::write_error::WriteError;
use crate::cases::{Ctx, PortError, TranscriptState, Unreadable};
use crate::{Provider, Related, RelatedKind, SessionId, WriteRefusal, instruction};
use std::path::PathBuf;
use time::OffsetDateTime;

/// A user's typed turns in one session: each turn's time and words.
pub type TypedTurns = Vec<(OffsetDateTime, String)>;

fn refusal<E>(kind: WriteRefusal, message: String) -> WriteError<E> {
    WriteError::Core(crate::Error::AgentWrite { kind, message })
}

/// `session`'s `provider` transcript on this host, among those
/// [`Transcripts::files`](crate::cases::Transcripts::files) lists.
fn locate<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
) -> Result<Option<PathBuf>, E> {
    Ok(ctx
        .transcripts
        .files(provider)?
        .into_iter()
        .find(|(found, _)| found == session)
        .map(|(_, path)| path))
}

/// Refuse a quote that is not verbatim in the user's typed turns of a session
/// the entry names (`--related session:<id>`), or whose transcript cannot be
/// read on this host (docs/design/scribe/2_design.md §4.6). When
/// `require_all_sessions` is true (used for `handoff`, §16.1), every
/// `session:` reference in `related` must parse as a `SessionId` and verify
/// the quote against its transcript.
///
/// # Errors
///
/// `quote_required` or `quote_not_found`, or a port error.
pub(crate) fn quote<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
    quote: &str,
    require_all_sessions: bool,
) -> Result<(), WriteError<E>> {
    let session_refs: Vec<&Related> = related
        .iter()
        .filter(|r| r.kind() == RelatedKind::Session)
        .collect();
    let sessions: Vec<SessionId> = if require_all_sessions {
        let mut parsed = Vec::with_capacity(session_refs.len());
        for rel in &session_refs {
            let Ok(id) = rel.target().parse::<SessionId>() else {
                return Err(refusal(
                    WriteRefusal::QuoteNotFound,
                    format!("cannot verify quote for invalid session reference '{rel}'"),
                ));
            };
            if !parsed.contains(&id) {
                parsed.push(id);
            }
        }
        parsed
    } else {
        session_refs
            .iter()
            .filter_map(|r| r.target().parse().ok())
            .collect()
    };
    if sessions.is_empty() {
        return Err(refusal(
            WriteRefusal::QuoteRequired,
            String::from(
                "a quote is checked against the session it was typed in; pass --related session:<id>",
            ),
        ));
    }
    let check_one = |id: &SessionId| -> Result<bool, WriteError<E>> {
        let Some(path) = locate(ctx, provider, id).map_err(WriteError::Port)? else {
            return Ok(false);
        };
        if !matches!(
            ctx.transcripts.check(provider, &path, id),
            Ok(TranscriptState::Present)
        ) {
            return Ok(false);
        }
        match ctx.transcripts.read(provider, &path) {
            Ok(bytes) => {
                let messages = crate::transcript::parse(&bytes);
                Ok(crate::action::quote_found(&messages, quote))
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), %err, "cannot read the transcript");
                Ok(false)
            }
        }
    };
    let not_found = || {
        refusal(
            WriteRefusal::QuoteNotFound,
            String::from(
                "the quote is not word for word in the user's typed turns of the session named, or its transcript is not readable on this host; quote the user's own words exactly, or file a friction instead",
            ),
        )
    };
    if require_all_sessions {
        for id in &sessions {
            if !check_one(id)? {
                return Err(not_found());
            }
        }
        return Ok(());
    }
    for id in &sessions {
        if check_one(id)? {
            return Ok(());
        }
    }
    Err(not_found())
}

/// Check a quote-less refresh: `--mode` is given, `current` (the calling
/// session, `CLAUDE_CODE_SESSION_ID`) is the session `related` names, and
/// its transcript, or an ancestor's reached through clear or launch
/// predecessors within 16 transcripts, holds a typed user turn.
///
/// # Errors
///
/// `quote_required` or `quote_not_found`.
pub(crate) fn self_refresh<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
    mode: Option<crate::handoff::Mode>,
    current: Option<&str>,
) -> Result<(), WriteError<E>> {
    let refuse = |kind, message: &str| -> WriteError<E> { refusal(kind, message.to_owned()) };
    let target = related.iter().find(|r| r.kind() == RelatedKind::Session);
    if mode.is_none() || current != target.map(Related::target) || current.is_none() {
        return Err(refuse(
            WriteRefusal::QuoteRequired,
            "a quote-less refresh requires --mode and the current CLAUDE_CODE_SESSION_ID matching the related session",
        ));
    }
    let id = target
        .and_then(|r| r.target().parse::<SessionId>().ok())
        .ok_or_else(|| refuse(WriteRefusal::QuoteRequired, "invalid refresh session"))?;
    // Walk launchers in metadata order with one budget and cycle guard. A
    // clear predecessor takes precedence over every launch edge at that node.
    // A cycle is a link already on the current path; a session an earlier
    // branch already examined (a shared ancestor) is skipped, not refused.
    enum Step {
        Enter(SessionId),
        Leave(SessionId),
    }
    let mut last_failure: WriteError<E> = refusal(
        WriteRefusal::QuoteNotFound,
        format!("refresh session {id}: transcript is not readable on this host"),
    );
    let mut pending = vec![Step::Enter(id)];
    let mut seen = std::collections::HashSet::new();
    let mut on_path = std::collections::HashSet::new();
    while let Some(step) = pending.pop() {
        let link = match step {
            Step::Leave(link) => {
                on_path.remove(&link);
                continue;
            }
            Step::Enter(link) => link,
        };
        let failed = |reason: &str| -> WriteError<E> {
            refusal(
                WriteRefusal::QuoteNotFound,
                format!("refresh session {link}: {reason}"),
            )
        };
        if on_path.contains(&link) {
            return Err(failed("ancestry contains a cycle"));
        }
        if !seen.insert(link.clone()) {
            continue;
        }
        on_path.insert(link.clone());
        pending.push(Step::Leave(link.clone()));
        let path = locate(ctx, provider, &link).ok().flatten();
        let bytes = path.as_ref().and_then(|path| {
            ctx.transcripts
                .read_checked(provider, path, &link)
                .ok()
                .flatten()
        });
        if let Some(bytes) = bytes {
            let typed = if provider == Provider::ClaudeCode {
                instruction::claude_conversation_raw_typed(&bytes)
                    .iter()
                    .any(|turn| turn.typed.is_some())
            } else {
                crate::transcript::parse(&bytes)
                    .iter()
                    .filter_map(crate::transcript::user_turn)
                    .any(|turn| !turn.own.trim().is_empty())
            };
            if typed {
                return Ok(());
            }
            let parents = if provider == Provider::ClaudeCode {
                crate::transcript::predecessors(&bytes)
            } else {
                crate::transcript::Predecessors::default()
            };
            if parents.clear.len() > 1 {
                return Err(failed("ambiguous: more than one clear predecessor"));
            }
            if let Some(parent) = parents.clear.first() {
                pending.push(Step::Enter(parent.clone()));
            } else {
                pending.extend(parents.launch.into_iter().rev().map(Step::Enter));
            }
            last_failure = failed(
                "transcript must contain a typed user turn or a clear or launch predecessor",
            );
        } else {
            last_failure = failed("transcript failed provenance or is not readable on this host");
        }
        if seen.len() == 16
            && pending
                .iter()
                .any(|step| matches!(step, Step::Enter(next) if !seen.contains(next)))
        {
            return Err(failed("ancestry exceeds the 16-transcript limit"));
        }
    }
    Err(last_failure)
}

/// Read each referenced session only when its turns are needed. Missing or
/// refused transcripts are skipped; I/O errors are returned to the caller.
fn typed_sessions<'a, E: PortError>(
    ctx: &'a Ctx<'a, E>,
    provider: Provider,
    related: &'a [Related],
) -> impl Iterator<Item = Result<(SessionId, TypedTurns), WriteError<E>>> + 'a {
    related
        .iter()
        .filter(|r| r.kind() == RelatedKind::Session)
        .filter_map(|r| r.target().parse::<SessionId>().ok())
        .filter_map(move |session| {
            let read = || -> Result<Option<(SessionId, TypedTurns)>, WriteError<E>> {
                let Some(path) = locate(ctx, provider, &session).map_err(WriteError::Port)? else {
                    return Ok(None);
                };
                let bytes = match ctx
                    .transcripts
                    .read_checked_unparsed(provider, &path, &session)
                {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) | Err(Unreadable::Refused(_)) => return Ok(None),
                    Err(Unreadable::Io(source)) => {
                        return Err(WriteError::Read {
                            what: path.display().to_string(),
                            source,
                        });
                    }
                };
                let turns = typed_turns(provider, &session, &bytes);
                Ok(Some((session, turns)))
            };
            read().transpose()
        })
}

/// Verify each quote in a referenced transcript, using the host's typed-turn
/// provenance checks. Short quotes may match only a whole typed turn.
///
/// # Errors
///
/// `quote_not_found`, or a transcript read error.
pub(crate) fn decision_quotes<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
    quote: &str,
    reason: Option<&str>,
) -> Result<(), WriteError<E>> {
    verify_quotes_in_sessions(typed_sessions(ctx, provider, related), quote, reason)
}

/// The decision-quote check over sessions already read: stops at the first
/// session that completes both quotes, before a later read error.
///
/// # Errors
///
/// `quote_not_found`, or the first read error before both are found.
pub fn verify_quotes_in_sessions<Er: From<crate::Error>>(
    sessions: impl Iterator<Item = Result<(SessionId, TypedTurns), Er>>,
    quote: &str,
    reason: Option<&str>,
) -> Result<(), Er> {
    let mut quote_found = false;
    let mut reason_found = reason.is_none();
    for session in sessions {
        let (_, turns) = session?;
        for (_, words) in turns {
            quote_found |= instruction::quoted_in(&words, quote);
            reason_found |= reason.is_some_and(|reason| instruction::quoted_in(&words, reason));
        }
        if quote_found && reason_found {
            return Ok(());
        }
    }
    let flag = if quote_found {
        "--reason-quote"
    } else {
        "--quote"
    };
    Err(not_found(flag).into())
}

/// Find the user's answer to an open item, checked as a decision quote is
/// (2_design.md §18.4): the latest typed turn holding it across the named
/// sessions, with the session it was typed in. An exact tie in time goes to
/// the session named first.
///
/// # Errors
///
/// `quote_required` or `quote_not_found`, or a transcript read error.
pub(crate) fn answer<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    related: &[Related],
    quote: &str,
) -> Result<(SessionId, OffsetDateTime), WriteError<E>> {
    if !related.iter().any(|r| r.kind() == RelatedKind::Session) {
        return Err(refusal(
            WriteRefusal::QuoteRequired,
            String::from(
                "an answer is checked against the session the user typed it in; pass --related session:<id>",
            ),
        ));
    }
    let mut latest: Option<(SessionId, OffsetDateTime)> = None;
    for session in typed_sessions(ctx, provider, related) {
        let (session, turns) = session?;
        for (at, words) in turns {
            if instruction::quoted_in(&words, quote)
                && latest.as_ref().is_none_or(|(_, best)| at > *best)
            {
                latest = Some((session.clone(), at));
            }
        }
    }
    latest.ok_or_else(|| WriteError::Core(not_found("--quote")))
}

fn not_found(flag: &str) -> crate::Error {
    crate::Error::AgentWrite {
        kind: WriteRefusal::QuoteNotFound,
        message: format!(
            "{flag} must be verbatim in a typed turn in a named session on this host; a short quote must be the whole turn"
        ),
    }
}

fn typed_turns(provider: Provider, session: &SessionId, bytes: &[u8]) -> TypedTurns {
    match provider {
        Provider::ClaudeCode => instruction::claude_session_conversation_raw_typed(bytes, session)
            .into_iter()
            .filter(|turn| turn.interactive || turn.entrypoint.is_none())
            .filter_map(|turn| Some((turn.message.at, turn.typed?)))
            .collect(),
        Provider::Codex => {
            let Ok(rollout) = crate::codex::parse(bytes) else {
                return Vec::new();
            };
            if rollout.session != *session {
                return Vec::new();
            }
            // Several input messages may belong to the same typed turn. A
            // short match must cover the entire turn, including its other parts.
            let mut turns =
                std::collections::BTreeMap::<String, (OffsetDateTime, Vec<String>)>::new();
            for typed in rollout.typed {
                if let Some(turn) = crate::transcript::user_turn(&typed.message) {
                    turns
                        .entry(typed.turn_id)
                        .or_insert_with(|| (typed.message.at, Vec::new()))
                        .1
                        .push(turn.own);
                }
            }
            turns
                .into_values()
                .map(|(at, parts)| (at, parts.join("\n")))
                .collect()
        }
    }
}
