//! `scribe entry` writes (ORB-27707 step 3d): `add`, `pickup`, `update`,
//! `retire`, `forget`, `answer` and `close`, each checked before the store
//! lock where it can be, and again by the store under it.

use crate::action::{self, ActionState, Pickup};
use crate::cases::entry::EntryVerbs;
use crate::cases::entry_verify;
use crate::cases::instruction_check::{self, Instruction};
use crate::cases::pickup::PickupVerbs;
use crate::handoff::Mode as HandoffMode;
use crate::instruction::{Act, Target};
use crate::runtime::errors::{WriteError, redactor};
use crate::runtime::{Ctx, PortError, resolve};
use crate::{
    AgentEntry, AnswerIn, AuthorName, Decision, Entry, EntryId, EntryPatch, ItemAnswer, Label,
    OpenItem, Project, Provider, Related, TagName, TagTable, Topic, WriteRefusal,
};
use time::OffsetDateTime;

/// What an entry command needs to report action state: the tag table, the
/// pickups and the time.
pub struct Actions {
    tags: TagTable,
    pickups: Vec<Pickup>,
    now: OffsetDateTime,
}

impl Actions {
    /// Read them.
    ///
    /// # Errors
    ///
    /// A store read error.
    pub fn load<E>(ctx: &Ctx<'_, E>) -> crate::Result<Self> {
        Ok(Self {
            tags: ctx.store.read().tags().config()?.tags,
            pickups: ctx.store.read().pickups().all()?,
            now: ctx.clock.now(),
        })
    }

    /// `entry`'s action state when its tag is an action tag.
    #[must_use]
    pub fn state(&self, entry: &Entry) -> Option<ActionState> {
        self.tags
            .action(&entry.tag)
            .map(|_| action::state(entry, &self.pickups, self.now))
    }
}

/// What an entry write did: the entry, whether it changed, its action state
/// read after the write, and notices.
#[derive(Debug)]
pub struct EntryWritten {
    /// The entry as written.
    pub entry: Entry,
    /// Whether the write changed it.
    pub changed: bool,
    /// Its action state, when its tag is an action tag.
    pub action: Option<ActionState>,
    /// Notices for the user.
    pub notices: Vec<String>,
}

fn written<E>(
    ctx: &Ctx<'_, E>,
    entry: Entry,
    changed: bool,
) -> Result<EntryWritten, WriteError<E>> {
    let action = Actions::load(ctx)?.state(&entry);
    Ok(EntryWritten {
        entry,
        changed,
        action,
        notices: Vec::new(),
    })
}

/// What `entry pickup` runs.
pub struct PickupInput<'i> {
    /// The action entry's id or a unique prefix.
    pub id: &'i str,
    /// The consumer.
    pub author: AuthorName,
    /// What the consumer made of it.
    pub result: Option<&'i Related>,
    /// The user rejected the result.
    pub rejected: bool,
    /// The provider whose transcripts resolve a `session:` result.
    pub provider: Provider,
}

/// `entry pickup`.
///
/// # Errors
///
/// [`WriteError`]: a refusal or a store error.
pub fn pickup<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: PickupInput<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let result = input
        .result
        .map(|r| resolve::related(ctx, input.provider, r))
        .transpose()?;
    let (pickup, changed) = ctx.store.record_pickup(
        input.id,
        input.author,
        result,
        input.rejected,
        ctx.clock.now(),
    )?;
    let entry = ctx.store.read().entries().get(&pickup.entry)?;
    written(ctx, entry, changed)
}

/// `entry update`: `patch`'s references are resolved here, the ones to
/// remove kept as given too.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn update<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    mut patch: EntryPatch,
    author: AuthorName,
    provider: Provider,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    patch.add_related = resolve::relateds(ctx, provider, &patch.add_related)?;
    patch.remove_related =
        resolve::relateds_preserving_input(ctx, provider, &patch.remove_related)?;
    ctx.store.check_agent_entry_update(id, &patch)?;
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .update_agent_entry(id, patch, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}

/// `entry close`: settle an open item with a decision or a note.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn close<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    decision: Option<&EntryId>,
    note: Option<&str>,
    author: AuthorName,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    if let Some(note) = note {
        crate::check_reason(note)?;
    }
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .close_open_item(id, decision, note, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}

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

/// `entry retire` on the agent's reason, and on the user's instruction when
/// one is given. The instruction's session is noted first; the instruction
/// is checked before the store lock, so a refusal changes nothing. A passing
/// check lets retire accept Daniel's entry (2_design.md §12.1).
///
/// # Errors
///
/// [`WriteError`]: a refusal, the instruction check, the redactor, or a
/// store error.
pub fn retire<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    reason: &str,
    author: AuthorName,
    instruction: &Instruction<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    instruction_check::note(ctx, instruction);
    crate::check_agent_author(&author)?;
    crate::check_reason(reason)?;
    let target = ctx.store.resolve_entry(id)?;
    let instructed = instruction_check::check(
        ctx,
        instruction,
        Act::Retire,
        &Target::new(target.id.to_string(), id),
    )?;
    let redactor = redactor(ctx)?;
    let (entry, changed) = ctx.store.retire_agent_entry(
        id,
        reason,
        instructed.as_ref(),
        author,
        ctx.clock.now(),
        &redactor,
    )?;
    written(ctx, entry, changed)
}

/// `entry forget`: runs only with an instruction and `confirm`; both
/// refusals, and every check of the instruction, come before the store lock
/// and change nothing (2_design.md §12.4).
///
/// # Errors
///
/// [`WriteError::Usage`] without an instruction,
/// [`WriteError::ConfirmRequired`] without `confirm`, the instruction
/// check, or a store error.
pub fn forget<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: &AuthorName,
    confirm: bool,
    instruction: &Instruction<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    instruction_check::note(ctx, instruction);
    crate::check_agent_author(author)?;
    let target = ctx.store.resolve_entry(id)?;
    let uninstructed = || {
        WriteError::Usage(String::from(
            "entry forget runs only on the user's instruction: pass --session <id> and --instruction '<their words>'",
        ))
    };
    if instruction.session.is_none() || instruction.quote.is_none() {
        return Err(uninstructed());
    }
    if !confirm {
        return Err(WriteError::ConfirmRequired(target.id.to_string()));
    }
    let Some(instructed) = instruction_check::check(
        ctx,
        instruction,
        Act::Forget,
        &Target::new(target.id.to_string(), id),
    )?
    else {
        return Err(uninstructed());
    };
    let (entry, changed) =
        ctx.store
            .forget_entry(id, &instructed, author.clone(), ctx.clock.now())?;
    written(ctx, entry, changed)
}

/// `entry answer`: the item is checked before any transcript is read, and
/// again by the store under the lock (2_design.md §18.4). An answer
/// recorded here is not an approval in the session that must act (§18.8),
/// so a notice says so when the item asks for its answer in another session.
///
/// # Errors
///
/// [`WriteError`]: a refusal, a transcript read, the redactor, or a store
/// error.
pub fn answer<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    author: &AuthorName,
    quote: &str,
    related: &[Related],
    provider: Provider,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(author)?;
    let target = ctx.store.resolve_entry(id)?;
    crate::check_unsettled_item(&target)?;
    let related = resolve::relateds(ctx, provider, related)?;
    let (session, at) = entry_verify::answer(ctx, provider, &related, quote)?;
    let answer = ItemAnswer {
        words: quote.to_owned(),
        session,
        at,
        recorded_by: author.clone(),
    };
    let redactor = redactor(ctx)?;
    let (entry, changed) = ctx
        .store
        .answer_open_item(id, answer, ctx.clock.now(), &redactor)?;
    // Base the reminder on the choice that remains stored: an older answer
    // may have been ignored by the store.
    let notice = entry
        .item
        .as_ref()
        .and_then(|item| match (&item.answer_in, &item.answer) {
            (AnswerIn::Session(only), Some(answer)) if *only != answer.session => Some(format!(
                "{} asks for its answer in session:{only}, whose auto mode accepts only what is typed there; the user must also type it in that session",
                entry.id
            )),
            _ => None,
        });
    let mut written = written(ctx, entry, changed)?;
    written.notices.extend(notice);
    Ok(written)
}
