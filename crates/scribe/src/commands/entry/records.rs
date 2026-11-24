//! CLI fields and transcript checks for decision and open-item writes.

use crate::app::Context;
use crate::commands::hook::payload::{self, Unreadable};
use crate::error::CliError;
use clap::Args;
use scribe_core::{
    AnswerIn, AuthorName, Decision, EntryId, OpenItem, Provider, Related, RelatedKind, SessionId,
    TagName, WriteRefusal, instruction,
};
use time::OffsetDateTime;

#[derive(Debug, Args)]
pub(super) struct RecordArgs {
    /// For a decision, user or the deciding agent's session name
    #[arg(long)]
    decided_by: Option<AuthorName>,
    /// For an agent decision, why the agent chose it
    #[arg(long)]
    rationale: Option<String>,
    /// For a user decision, their verbatim reason from a related session
    #[arg(long)]
    reason_quote: Option<String>,
    /// The active decision this decision reverses
    #[arg(long)]
    supersedes: Option<EntryId>,
    /// The open item this decision answers
    #[arg(long)]
    settles: Option<EntryId>,
    /// For an open item, the asking agent's session name
    #[arg(long)]
    asked_by: Option<AuthorName>,
    /// For an open item, what waits meanwhile (or 'nothing')
    #[arg(long)]
    blocked: Option<String>,
    /// Where to answer an open item: any (default), session:<id>, or hand
    #[arg(long)]
    answer_in: Option<AnswerIn>,
    /// An option shown in full, repeatable; supply zero or two to six
    #[arg(long = "option")]
    options: Vec<String>,
    /// The 1-based index of the recommended option
    #[arg(long)]
    recommended: Option<usize>,
    /// Why the recommended option is preferred for an open item
    #[arg(long)]
    why: Option<String>,
}

fn refusal(kind: WriteRefusal, message: &str) -> CliError {
    scribe_core::Error::AgentWrite {
        kind,
        message: message.to_owned(),
    }
    .into()
}

impl RecordArgs {
    pub(super) fn into_records(
        self,
        ctx: &Context,
        tag: &TagName,
    ) -> Result<(Option<Decision>, Option<OpenItem>), CliError> {
        let decision_flags = self.decided_by.is_some()
            || self.rationale.is_some()
            || self.reason_quote.is_some()
            || self.supersedes.is_some()
            || self.settles.is_some();
        let item_flags = self.asked_by.is_some()
            || self.blocked.is_some()
            || self.answer_in.is_some()
            || self.why.is_some();
        let choice_flags = !self.options.is_empty() || self.recommended.is_some();
        match tag.as_str() {
            "decision" if !item_flags => {
                let decided_by = self.decided_by.ok_or_else(|| {
                    refusal(
                        WriteRefusal::DecidedByRequired,
                        "a decision requires --decided-by",
                    )
                })?;
                let decision = Decision {
                    decided_by,
                    rationale: self.rationale,
                    reason_quote: self.reason_quote,
                    options: self.options,
                    recommended: self.recommended,
                    supersedes: self.supersedes,
                    settles: self.settles,
                };
                Ok((Some(decision), None))
            }
            "open-item" if !decision_flags => {
                let asked_by = self.asked_by.ok_or_else(|| {
                    refusal(
                        WriteRefusal::AskedByRequired,
                        "an open item requires --asked-by",
                    )
                })?;
                let blocked = self.blocked.ok_or_else(|| {
                    refusal(WriteRefusal::InvalidText, "an open item requires --blocked")
                })?;
                let answer_in = match self.answer_in.unwrap_or_default() {
                    AnswerIn::Session(id) => {
                        AnswerIn::Session(super::super::resolve_session_or_typed(ctx, &id)?)
                    }
                    location @ (AnswerIn::Any | AnswerIn::Hand) => location,
                };
                let item = OpenItem {
                    asked_by,
                    options: self.options,
                    recommended: self.recommended,
                    why: self.why,
                    blocked,
                    answer_in,
                    answer: None,
                    closed: None,
                };
                Ok((None, Some(item)))
            }
            _ if !decision_flags && !item_flags && !choice_flags => Ok((None, None)),
            _ => Err(refusal(
                WriteRefusal::RecordFields,
                "use decision flags only with --tag decision and open-item flags only with --tag open-item",
            )),
        }
    }
}

/// A user's typed turns in one session: each turn's time and words.
type TypedTurns = Vec<(OffsetDateTime, String)>;

/// Read each referenced session only when its turns are needed. Missing or
/// refused transcripts are skipped; I/O errors are returned to the caller.
fn typed_sessions<'a>(
    ctx: &'a Context,
    related: &'a [Related],
) -> impl Iterator<Item = Result<(SessionId, TypedTurns), CliError>> + 'a {
    related
        .iter()
        .filter(|r| r.kind() == RelatedKind::Session)
        .filter_map(|r| r.target().parse::<SessionId>().ok())
        .filter_map(move |session| {
            let read = || -> Result<Option<(SessionId, TypedTurns)>, CliError> {
                let Some(path) = payload::locate(ctx, &session)? else {
                    return Ok(None);
                };
                let bytes = match payload::read_checked_bytes(ctx, &path, &session) {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) | Err(Unreadable::Refused(_)) => return Ok(None),
                    Err(Unreadable::Io(source)) => {
                        return Err(CliError::Read {
                            what: path.display().to_string(),
                            source,
                        });
                    }
                };
                let turns = typed_turns(ctx.provider(), &session, &bytes);
                Ok(Some((session, turns)))
            };
            read().transpose()
        })
}

/// Verify each quote in a referenced transcript, using the host's typed-turn
/// provenance checks. Short quotes may match only a whole typed turn.
pub(super) fn verify_decision_quotes(
    ctx: &Context,
    related: &[Related],
    quote: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
    verify_quotes_in_sessions(typed_sessions(ctx, related), quote, reason)
}

fn verify_quotes_in_sessions(
    sessions: impl Iterator<Item = Result<(SessionId, TypedTurns), CliError>>,
    quote: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
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
    Err(not_found(flag))
}

/// Find the user's answer to an open item, checked as a decision quote is
/// (2_design.md §18.4): the latest typed turn holding it across the named
/// sessions, with the session it was typed in. An exact tie in time goes to
/// the session named first.
pub(super) fn find_answer(
    ctx: &Context,
    related: &[Related],
    quote: &str,
) -> Result<(SessionId, OffsetDateTime), CliError> {
    if !related.iter().any(|r| r.kind() == RelatedKind::Session) {
        return Err(refusal(
            WriteRefusal::QuoteRequired,
            "an answer is checked against the session the user typed it in; pass --related session:<id>",
        ));
    }
    let mut latest: Option<(SessionId, OffsetDateTime)> = None;
    for session in typed_sessions(ctx, related) {
        let (session, turns) = session?;
        for (at, words) in turns {
            if instruction::quoted_in(&words, quote)
                && latest.as_ref().is_none_or(|(_, best)| at > *best)
            {
                latest = Some((session.clone(), at));
            }
        }
    }
    latest.ok_or_else(|| not_found("--quote"))
}

fn not_found(flag: &str) -> CliError {
    refusal(
        WriteRefusal::QuoteNotFound,
        &format!(
            "{flag} must be verbatim in a typed turn in a named session on this host; a short quote must be the whole turn"
        ),
    )
}

fn typed_turns(provider: Provider, session: &SessionId, bytes: &[u8]) -> TypedTurns {
    match provider {
        Provider::ClaudeCode => instruction::claude_session_conversation_raw_typed(bytes, session)
            .into_iter()
            .filter(|turn| turn.interactive || turn.entrypoint.is_none())
            .filter_map(|turn| Some((turn.message.at, turn.typed?)))
            .collect(),
        Provider::Codex => {
            let Ok(rollout) = scribe_core::codex::parse(bytes) else {
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
                if let Some(turn) = scribe_core::transcript::user_turn(&typed.message) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_quotes_stop_before_a_later_read_error() {
        let id: SessionId = "8dc9df2b-5808-47cf-8be4-190c9c6e0711".parse().unwrap();
        let turns = vec![
            (OffsetDateTime::UNIX_EPOCH, String::from("yes")),
            (OffsetDateTime::UNIX_EPOCH, String::from("because")),
        ];
        let sessions = || {
            [
                Ok((id.clone(), turns.clone())),
                Err(CliError::Read {
                    what: String::from("later transcript"),
                    source: std::io::Error::other("injected read failure"),
                }),
            ]
            .into_iter()
        };
        assert!(verify_quotes_in_sessions(sessions(), "yes", Some("because")).is_ok());
        assert!(verify_quotes_in_sessions(sessions(), "yes", None).is_ok());
        assert!(matches!(
            verify_quotes_in_sessions(sessions(), "yes", Some("missing")),
            Err(CliError::Read { what, .. }) if what == "later transcript"
        ));
        assert!(matches!(
            verify_quotes_in_sessions(sessions(), "missing", Some("because")),
            Err(CliError::Read { what, .. }) if what == "later transcript"
        ));
    }
}
