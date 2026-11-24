//! CLI fields and transcript checks for decision and open-item writes.

use crate::app::Context;
use crate::commands::hook::payload::{self, Unreadable};
use crate::error::CliError;
use clap::Args;
use scribe_core::{
    AnswerIn, AuthorName, Decision, EntryId, OpenItem, Provider, Related, RelatedKind, SessionId,
    TagName, WriteRefusal, instruction,
};

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

/// Verify each quote in a referenced transcript, using the host's typed-turn
/// provenance checks. Short quotes may match only a whole typed turn.
pub(super) fn verify_decision_quotes(
    ctx: &Context,
    related: &[Related],
    quote: &str,
    reason: Option<&str>,
) -> Result<(), CliError> {
    let mut quote_found = false;
    let mut reason_found = reason.is_none();
    for session in related
        .iter()
        .filter(|r| r.kind() == RelatedKind::Session)
        .filter_map(|r| r.target().parse::<SessionId>().ok())
    {
        let Some(path) = payload::locate(ctx, &session)? else {
            continue;
        };
        let bytes = match payload::read_checked_bytes(ctx, &path, &session) {
            Ok(Some(bytes)) => bytes,
            Ok(None) | Err(Unreadable::Refused(_)) => continue,
            Err(Unreadable::Io(source)) => {
                return Err(CliError::Read {
                    what: path.display().to_string(),
                    source,
                });
            }
        };
        for words in typed_words(ctx.provider(), &session, &bytes) {
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
    Err(refusal(
        WriteRefusal::QuoteNotFound,
        &format!(
            "{flag} must be verbatim in a typed turn in a named session on this host; a short quote must be the whole turn"
        ),
    ))
}

fn typed_words(provider: Provider, session: &SessionId, bytes: &[u8]) -> Vec<String> {
    match provider {
        Provider::ClaudeCode => instruction::claude_session_conversation_raw_typed(bytes, session)
            .into_iter()
            .filter(|turn| turn.interactive || turn.entrypoint.is_none())
            .filter_map(|turn| turn.typed)
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
            let mut turns = std::collections::BTreeMap::<String, Vec<String>>::new();
            for typed in rollout.typed {
                if let Some(turn) = scribe_core::transcript::user_turn(&typed.message) {
                    turns.entry(typed.turn_id).or_default().push(turn.own);
                }
            }
            turns.into_values().map(|parts| parts.join("\n")).collect()
        }
    }
}
