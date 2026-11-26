//! `scribe entry add` (ORB-27707 step 3d), checked before the store lock
//! where it can be, and again by the store under it.

use super::verbs::EntryVerbs;
use super::verify as entry_verify;
use super::written::{EntryWritten, written};
use crate::apps::tag::redactor;
use crate::handoff::Mode as HandoffMode;
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError, resolve};
use crate::{
    AgentEntry, AnswerIn, AuthorName, Decision, EntryId, Label, OpenItem, Project, Provider,
    Related, TagName, Topic, WriteRefusal,
};

/// `entry add`'s decision and open-item fields, as typed.
#[derive(Debug, Default)]
pub struct RecordFields {
    /// For a decision, the user or the deciding agent's session name.
    pub decided_by: Option<AuthorName>,
    /// For an agent decision, why the agent chose it.
    pub rationale: Option<String>,
    /// For a user decision, their verbatim reason from a related session.
    pub reason_quote: Option<String>,
    /// The active decision this decision reverses.
    pub supersedes: Option<EntryId>,
    /// The open item this decision answers.
    pub settles: Option<EntryId>,
    /// For an open item, the asking agent's session name.
    pub asked_by: Option<AuthorName>,
    /// For an open item, what waits meanwhile.
    pub blocked: Option<String>,
    /// Where to answer an open item.
    pub answer_in: Option<AnswerIn>,
    /// The options shown in full.
    pub options: Vec<String>,
    /// The 1-based index of the recommended option.
    pub recommended: Option<usize>,
    /// Why the recommended option is preferred for an open item.
    pub why: Option<String>,
}

fn refusal<E>(kind: WriteRefusal, message: &str) -> WriteError<E> {
    WriteError::Core(crate::Error::AgentWrite {
        kind,
        message: message.to_owned(),
    })
}

impl RecordFields {
    /// The decision or open item these fields make under `tag`. A session
    /// given as a unique prefix in `--answer-in` is stored as the full id.
    fn into_records<E: PortError>(
        self,
        ctx: &Ctx<'_, E>,
        provider: Provider,
        tag: &TagName,
    ) -> Result<(Option<Decision>, Option<OpenItem>), WriteError<E>> {
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
                        AnswerIn::Session(resolve::session_or_typed(ctx, provider, &id)?)
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

/// What `entry add` runs.
pub struct AddInput {
    /// The tag.
    pub tag: TagName,
    /// The content.
    pub content: String,
    /// The agent.
    pub author: AuthorName,
    /// The project.
    pub project: Project,
    /// The topics, as typed.
    pub topics: Vec<Topic>,
    /// `--related`, as typed.
    pub related: Vec<Related>,
    /// The user's words.
    pub quote: Option<String>,
    /// A handoff's mode.
    pub mode: Option<HandoffMode>,
    /// The evidence pointer.
    pub evidence: Option<String>,
    /// The day the entry is true as of.
    pub as_of: Option<time::Date>,
    /// Whether the user dictated it.
    pub spoken: bool,
    /// The decision and open-item fields.
    pub record: RecordFields,
    /// The provider whose transcripts hold the related sessions.
    pub provider: Provider,
    /// This host.
    pub host: Label,
    /// The calling session (`CLAUDE_CODE_SESSION_ID`), read by a quote-less
    /// refresh.
    pub current_session: Option<String>,
}

/// `entry add`. The author and the arguments are checked first, so a bad
/// tag or empty content is reported before an unset `[redact]` env name;
/// then the related sessions are resolved, the entry and its records are
/// checked, the quote is verified against the transcripts, and the entry is
/// written.
///
/// # Errors
///
/// [`WriteError`]: a refusal, a transcript read, the redactor, or a store
/// error.
pub fn add<E: PortError>(ctx: &Ctx<'_, E>, input: AddInput) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let topics =
        crate::Topics::try_from(input.topics).map_err(|e| WriteError::Usage(e.to_string()))?;
    let provider = input.provider;
    // A session given as a unique prefix is stored as the full id.
    let related = resolve::relateds(ctx, provider, &input.related)?;
    let def = ctx.store.check_new_entry_full(
        &input.tag,
        &input.content,
        input.quote.as_deref(),
        input.evidence.as_deref(),
        input.spoken,
    )?;
    crate::check_handoff_related(&def.name, &related)?;
    let handoff = def.name.as_str() == crate::HANDOFF_TAG;
    let self_refresh_verified = if handoff && input.quote.is_none() {
        entry_verify::self_refresh(
            ctx,
            provider,
            &related,
            input.mode,
            input.current_session.as_deref(),
        )?;
        true
    } else {
        false
    };
    let (decision, item) = input.record.into_records(ctx, provider, &def.name)?;
    let draft = AgentEntry {
        tag: input.tag,
        content: input.content,
        project: input.project,
        topics,
        related,
        host: input.host,
        quote: input.quote,
        evidence: input.evidence,
        as_of: input.as_of,
        spoken: input.spoken,
        handoff_mode: input.mode,
        self_refresh_verified,
        decision,
        item,
    };
    ctx.store.check_new_entry_records(&def.name, &draft)?;
    if let Some(quote) = &draft.quote {
        if let Some(decision) = &draft.decision {
            entry_verify::decision_quotes(
                ctx,
                provider,
                &draft.related,
                quote,
                decision.reason_quote.as_deref(),
            )?;
        } else {
            entry_verify::quote(ctx, provider, &draft.related, quote, handoff)?;
        }
    }
    let redactor = redactor(ctx)?;
    let entry = ctx
        .store
        .add_agent_entry(draft, input.author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, true)
}
