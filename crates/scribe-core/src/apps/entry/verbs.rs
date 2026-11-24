//! Entry verbs: notes written with agent provenance, and the decision and
//! open-item records (2_design.md §12, §18).

use crate::checks::entry::{
    check_evidence, check_handoff, check_handoff_related, check_handoff_related_update,
    check_quote, check_spoken_quote, check_spoken_tag, counts_toward_cap, draft_session,
};
use crate::checks::records::{
    check_decision, check_decision_redaction, check_decision_size, check_decision_update,
    check_open_item, check_unsettled_item, item_mut, patch_decision,
};
use crate::checks::{
    check_agent_author, check_reason, refusal, required, required_message, required_reason,
    unknown_tag,
};
use crate::entry::{
    AgentEntry, Cite, DECISION_TAG, EntryPatch, OPEN_ITEM_TAG, is_decision_or_item,
};
use crate::record::patch;
use crate::runtime::resolve::refuse_forgotten;
use crate::{
    Author, AuthorKind, AuthorName, ContentFrom, Decision, Entry, EntryId, EntryStatus, Error,
    ItemAnswer, ItemClosure, ItemState, Label, Redactor, RelatedKind, Result, SessionId, Store,
    TagName, Topics, Tx, WriteRefusal,
};
use time::OffsetDateTime;

/// The entry verbs, implemented for [`Store`]. Authors always describe the
/// agent channel.
pub trait EntryVerbs {
    /// Resolve an exact entry id or a unique prefix of at least four characters.
    fn resolve_entry(&self, input: &str) -> Result<Entry>;

    /// Check a new note's arguments (content, tag, quote and evidence) as
    /// `add_agent_entry` does, without locking or writing, so a caller can
    /// refuse a bad argument before it sets up anything a write needs.
    /// `spoken` checks an entry written on the user's spoken instruction
    /// (2_design.md §16.6).
    ///
    /// # Errors
    ///
    /// `invalid_text` for blank content; `tag_not_found` for a tag no
    /// config or store tag names.
    fn check_new_entry_full(
        &self,
        tag: &TagName,
        content: &str,
        quote: Option<&str>,
        evidence: Option<&str>,
        spoken: bool,
    ) -> Result<crate::TagDef>;

    /// Check an agent entry patch without locking or writing, so invalid
    /// content, topics or ids can be reported before a caller constructs a
    /// redactor. The write path repeats these checks under the store lock.
    fn check_agent_entry_update(&self, id: &str, patching: &EntryPatch) -> Result<()>;

    /// Check the fields specific to the resolved tag before checking transcripts.
    fn check_new_entry_records(&self, tag: &TagName, draft: &AgentEntry) -> Result<()>;

    /// Create a note with agent provenance, allocating an unused id under the lock.
    fn add_agent_entry(
        &self,
        draft: AgentEntry,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Entry>;

    /// Apply a patch atomically; return whether the record changed.
    fn update_agent_entry(
        &self,
        id: &str,
        patching: EntryPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)>;

    /// Retire an entry, preserving the reason and author. An agent's note
    /// retires on the agent's word; Daniel's entry only on his checked
    /// `instruction` (docs/design/scribe/2_design.md §12.1), which the entry
    /// then records in `removal_instruction` with its quote redacted, apart
    /// from the `instruction` an agent approved it on. Retiring keeps the
    /// text.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit retirement inputs at the store boundary"
    )]
    fn retire_agent_entry(
        &self,
        id: &str,
        reason: &str,
        instruction: Option<&crate::instruction::Instructed>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)>;

    /// Forget an entry for good on Daniel's checked `instruction`
    /// (docs/design/scribe/2_design.md §12.4): its text and evidence are
    /// blanked with every proposal quote, candidate quote and reason about it
    /// ([`crate::consistency::forget`]). The entry keeps its id, tag, dates
    /// and author, records the agent in `updated_by` and the instruction's
    /// session and message ids in `removal_instruction`, apart from any
    /// approval's `instruction`, but not its quote, which may repeat the words
    /// he wants gone. The database keeps the entry row. Any matching legacy
    /// files are erased after commit, and a repeat can finish interrupted
    /// archive cleanup.
    fn forget_entry(
        &self,
        id: &str,
        instruction: &crate::instruction::Instructed,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(Entry, bool)>;

    /// Record the user's checked answer on an open item (2_design.md §18.4).
    /// The caller has found `answer.words` in a typed turn of
    /// `answer.session`. A newer answer replaces an earlier one until the
    /// item closes; an older answer, or a replay of the stored typed turn
    /// (same words, same session, same time), changes nothing. Other equal
    /// timestamps retain submission order.
    ///
    /// # Errors
    ///
    /// `not_open_item`, `item_settled`, `needs_redaction` for words
    /// redaction would alter, `session_forgotten`, and the store's errors.
    fn answer_open_item(
        &self,
        id: &str,
        answer: ItemAnswer,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)>;

    /// Close an open item once its asker carried it out (2_design.md §18.4):
    /// retire it with `item.closed`, naming the decision that settles it or
    /// a note that none lasts.
    ///
    /// # Errors
    ///
    /// `not_open_item`, `item_settled`, `not_answered` for a choice with no
    /// answer, `invalid_decision` when `decision` is not an active decision
    /// whose `settles` names this item, `invalid_text` for a blank or long
    /// note, and the store's errors.
    fn close_open_item(
        &self,
        id: &str,
        decision: Option<&EntryId>,
        note: Option<&str>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)>;

    /// Whether any entry from `session` still waits for its cite's message
    /// uuid. Reads only, so the `Stop` hook, which runs after every reply,
    /// can return without locking or writing when nothing waits.
    fn has_unfilled_cites(&self, session: &SessionId) -> Result<bool>;

    /// Under the store lock, fill the cite's message uuid of every entry
    /// from `session` that has none yet with what `find` returns for its
    /// cite; `updated` becomes `now`. Returns the ids filled, oldest first.
    fn fill_cite_messages(
        &self,
        session: &SessionId,
        now: time::OffsetDateTime,
        find: impl Fn(&Cite) -> Option<String>,
    ) -> Result<Vec<EntryId>>;
}

impl EntryVerbs for Store {
    fn resolve_entry(&self, input: &str) -> Result<Entry> {
        self.read().entries().resolve(input)
    }

    fn check_new_entry_full(
        &self,
        tag: &TagName,
        content: &str,
        quote: Option<&str>,
        evidence: Option<&str>,
        spoken: bool,
    ) -> Result<crate::TagDef> {
        required(content)?;
        let def = self
            .read()
            .tags()
            .config()?
            .tags
            .resolve(tag.as_str())
            .cloned()
            .ok_or_else(unknown_tag)?;
        if spoken {
            check_spoken_tag(&def)?;
            check_spoken_quote(quote)?;
        } else if def.name.as_str() != DECISION_TAG
            && (def.name.as_str() != crate::handoff::HANDOFF_TAG || quote.is_some())
        {
            check_quote(&def, quote)?;
        }
        check_evidence(&def, evidence)?;
        check_handoff(&def.name, content)?;
        Ok(def)
    }

    fn check_agent_entry_update(&self, id: &str, patching: &EntryPatch) -> Result<()> {
        if let Some(content) = &patching.content {
            required(content)?;
        }
        let entry = self.resolve_entry(id)?;
        if let Some(content) = &patching.content {
            check_handoff(&entry.tag, content)?;
        }
        check_handoff_related_update(&entry, &patching.add_related, &patching.remove_related)?;
        let mut topics = entry.topics.as_slice().to_vec();
        patch(&mut topics, &patching.add_topics, &patching.remove_topics);
        Topics::try_from(topics)
            .map(|_| ())
            .map_err(|e| refusal(WriteRefusal::InvalidTopics, e.to_string()))
    }

    fn check_new_entry_records(&self, tag: &TagName, draft: &AgentEntry) -> Result<()> {
        match (tag.as_str(), &draft.decision, &draft.item) {
            (DECISION_TAG, Some(decision), None) => {
                check_decision(decision, draft)?;
                if decision.by_user() {
                    for session in draft
                        .related
                        .iter()
                        .filter(|r| r.kind() == RelatedKind::Session)
                        .filter_map(|r| r.target().parse::<SessionId>().ok())
                    {
                        refuse_forgotten(self, &session)?;
                    }
                }
                Ok(())
            }
            (OPEN_ITEM_TAG, None, Some(item)) => check_open_item(item),
            (DECISION_TAG, None, None) => Err(refusal(
                WriteRefusal::DecidedByRequired,
                "a decision requires --decided-by",
            )),
            (OPEN_ITEM_TAG, None, None) => Err(refusal(
                WriteRefusal::AskedByRequired,
                "an open item requires --asked-by and --blocked",
            )),
            (_, None, None) => Ok(()),
            _ => Err(refusal(
                WriteRefusal::RecordFields,
                "use decision fields only with --tag decision and open-item fields only with --tag open-item",
            )),
        }
    }

    fn add_agent_entry(
        &self,
        draft: AgentEntry,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Entry> {
        check_agent_author(&author)?;
        required(&draft.content)?;
        self.write(|tx| {
        let config = tx.tags().config()?;
        let tag = config
            .tags
            .resolve(draft.tag.as_str())
            .ok_or_else(unknown_tag)?;
        self.check_new_entry_records(&tag.name, &draft)?;
        check_decision_redaction(&draft, &|s| redactor.mask(s))?;
        if draft.spoken {
            check_spoken_tag(tag)?;
            check_spoken_quote(draft.quote.as_deref())?;
            if draft_session(&draft.related).is_none() {
                return Err(refusal(
                    WriteRefusal::QuoteRequired,
                    "a spoken-instruction quote is checked against the session it was typed in: pass --related session:<id>",
                ));
            }
            if let Some(quote) = &draft.quote
                && redactor.mask(quote) != *quote
            {
                return Err(refusal(
                    WriteRefusal::NeedsRedaction,
                    "the quoted text contains a secret that redaction would alter; nothing written",
                ));
            }
        } else {
            if tag.name.as_str() == crate::handoff::HANDOFF_TAG && draft.quote.is_none() {
                if !draft.self_refresh_verified {
                    return Err(refusal(WriteRefusal::QuoteRequired, "a quote-less handoff requires a verified self-refresh session"));
                }
            } else if tag.name.as_str() != DECISION_TAG {
                check_quote(tag, draft.quote.as_deref())?;
            }
            check_handoff_related(&tag.name, &draft.related)?;
            if tag.name.as_str() == crate::handoff::HANDOFF_TAG
                && let Some(id) = draft_session(&draft.related)
                && let Ok(id) = id.parse::<crate::SessionId>()
            {
                let resolved = match self.read().sessions().resolve(&id) {
                    Ok(resolved) => resolved,
                    Err(Error::SessionNotFound { .. }) => id,
                    Err(error) => return Err(error),
                };
                refuse_forgotten(self, &resolved)?;
            }
        }
        if draft.handoff_mode.is_some() && tag.name.as_str() != crate::handoff::HANDOFF_TAG {
            return Err(refusal(WriteRefusal::InvalidText, "--mode applies only to handoffs"));
        }
        check_evidence(tag, draft.evidence.as_deref())?;
        check_handoff(&tag.name, &draft.content)?;
        let mut replaced = draft
            .decision
            .as_ref()
            .map(|decision| decision_replacement(tx, decision))
            .transpose()?
            .flatten();
        let (entries, skipped) = tx.entries().all()?;
        if tag.require_evidence {
            let session = draft_session(&draft.related);
            let held = entries
                .iter()
                .filter(|e| counts_toward_cap(e, &tag.name, session.as_deref(), &author, now))
                .count();
            if held >= crate::MAX_FINDINGS_PER_SESSION {
                return Err(refusal(
                    WriteRefusal::FindingCap,
                    format!(
                        "at most {} findings per session (cap {}); this session already holds {held}: record only a result that is costly to re-derive",
                        crate::MAX_FINDINGS_PER_SESSION,
                        crate::MAX_FINDINGS_PER_SESSION
                    ),
                ));
            }
        }
        let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
        ids.extend(
            skipped
                .iter()
                .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
        );
        ids.extend(tx.proposals().reserved_entry_ids()?);
        let mut entry = Entry {
            id: EntryId::next_in(&tag.name, &ids),
            tag: tag.name.clone(),
            status: EntryStatus::Active,
            project: draft.project,
            topics: draft.topics,
            related: draft.related,
            provider: Label::sanitized("cli", "cli"),
            host: draft.host,
            author: Author::agent(author.clone()),
            updated_by: None,
            created: now,
            updated: now,
            inactive_since: None,
            said: String::new(),
            content_from: ContentFrom::Agent,
            cite: None,
            retirement_reason: None,
            superseded_by: None,
            review_after: tag.review_after(now.date()),
            quote: draft.quote,
            window: None,
            as_of: draft
                .evidence
                .as_ref()
                .map(|_| draft.as_of.unwrap_or_else(|| now.date())),
            evidence: draft.evidence,
            approved_by: None,
            instruction: None,
            removal_instruction: None,
            stated_by: draft.spoken.then(|| Author::agent(author)),
            outcome: None,
            handoff_mode: draft.handoff_mode,
            decision: draft.decision,
            item: draft.item,
            content: draft.content,
        };
        redactor.entry(&mut entry);
        if let Some(decision) = &entry.decision {
            check_decision_size(decision, &entry.content, entry.quote.as_deref())?;
        }
        if !is_decision_or_item(entry.tag.as_str())
            && let Some(cover) =
                crate::pass::covering_entry(&entries, &entry.content, &entry.tag, &entry.project)
        {
            return Err(refusal(
                WriteRefusal::AlreadyCovered,
                format!("already covered by {}; nothing written", cover.id),
            ));
        }
        tx.entries().put(&entry, true)?;
        if let Some(older) = &mut replaced {
            older.status = EntryStatus::Superseded;
            older.superseded_by = Some(entry.id.clone());
            older.inactive_since = Some(now);
            older.updated = now;
            older.updated_by = Some(entry.author.clone());
            tx.entries().put(older, false)?;
        }
        Ok(entry)
            })
    }

    fn update_agent_entry(
        &self,
        id: &str,
        patching: EntryPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        self.write(|tx| {
            let mut entry = tx.entries().resolve(id)?;
            let before = entry.clone();
            // Any agent may edit an agent note while it is active (author names
            // are attribution, not ownership); a Daniel entry keeps the
            // metadata-only rule below.
            if entry.author.kind == AuthorKind::Agent && entry.status != EntryStatus::Active {
                return Err(refusal(
                    WriteRefusal::EntryNotActive,
                    "only an active entry can be updated",
                ));
            }
            patch_decision(&mut entry, &patching, &|s| redactor.mask(s))?;
            if let Some(content) = patching.content {
                if entry.author.kind != AuthorKind::Agent {
                    return Err(refusal(
                        WriteRefusal::UserOnly,
                        "The user's content changes only through #scribe",
                    ));
                }
                if entry.content_from != ContentFrom::Agent {
                    return Err(refusal(
                        WriteRefusal::NotAgentNote,
                        "only direct agent notes can change content; approved claims allow metadata updates only (retire this entry and add a new note instead)",
                    ));
                }
                required(&content)?;
                check_handoff(&entry.tag, &content)?;
                if entry.status == EntryStatus::Forgotten {
                    return Err(refusal(
                        WriteRefusal::EntryNotActive,
                        "a forgotten entry cannot regain content",
                    ));
                }
                let masked = redactor.mask(&content);
                if masked != entry.content && entry.stated_by.is_some() {
                    entry.stated_by = Some(Author::agent(author.clone()));
                }
                entry.content = masked;
            }
            let mut topics = entry.topics.as_slice().to_vec();
            patch(&mut topics, &patching.add_topics, &patching.remove_topics);
            entry.topics = Topics::try_from(topics)
                .map_err(|e| refusal(WriteRefusal::InvalidTopics, e.to_string()))?;
            check_handoff_related_update(&entry, &patching.add_related, &patching.remove_related)?;
            patch(
                &mut entry.related,
                &patching.add_related,
                &patching.remove_related,
            );
            check_decision_update(&before, &entry)?;
            let changed = entry != before;
            if changed {
                entry.updated = now;
                entry.updated_by = Some(Author::agent(author));
                tx.entries().put(&entry, false)?;
            }
            Ok((entry, changed))
        })
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "explicit retirement inputs at the store boundary"
    )]
    fn retire_agent_entry(
        &self,
        id: &str,
        reason: &str,
        instruction: Option<&crate::instruction::Instructed>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        required_reason(reason)?;
        self.write(|tx| {
        let mut entry = tx.entries().resolve(id)?;
        if entry.author.kind != AuthorKind::Agent && instruction.is_none() {
            return Err(refusal(
                WriteRefusal::UserOnly,
                "The user retires their entries through #scribe retire, or on their instruction with --session and --instruction",
            ));
        }
        if entry.status == EntryStatus::Retired
            && entry.retirement_reason.as_deref() == Some(redactor.mask(reason).as_str())
        {
            return Ok((entry, false));
        }
        if entry.status != EntryStatus::Active {
            return Err(refusal(
                WriteRefusal::EntryNotActive,
                "only an active entry can be retired",
            ));
        }
        entry.status = EntryStatus::Retired;
        entry.retirement_reason = Some(redactor.mask(reason));
        entry.updated = now;
        entry.inactive_since = Some(now);
        entry.updated_by = Some(Author::agent(author));
        if let Some(instructed) = instruction {
            required_message(instructed)?;
            entry.removal_instruction = Some(crate::Instruction {
                session: instructed.session.clone(),
                message: instructed.message.clone(),
                at: instructed.at,
                quote: Some(redactor.mask(&instructed.quote)),
            });
        }
        tx.entries().put(&entry, false)?;
        Ok((entry, true))
            })
    }

    fn forget_entry(
        &self,
        id: &str,
        instruction: &crate::instruction::Instructed,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        required_message(instruction)?;
        self.write(|tx| {
            let target = tx.entries().resolve(id)?;
            if target.status == EntryStatus::Forgotten {
                return Ok((target, false));
            }
            let mut proposals = tx.proposals().all()?;
            let (mut entries, _) = tx.entries().all()?;
            let before = proposals.clone();
            let forgotten =
                crate::consistency::forget(&target.id, &mut proposals, &mut entries, now);
            let entry = entries.into_iter().find(|e| e.id == target.id);
            let (crate::consistency::Settled::Forgotten(_), Some(mut entry)) = (forgotten, entry)
            else {
                // Resolved above and not forgotten, but not in the listing: a
                // file that does not parse is never rewritten.
                return Err(refusal(
                    WriteRefusal::EntryNotFound,
                    format!("cannot forget {}: its file does not parse", target.id),
                ));
            };
            entry.updated_by = Some(Author::agent(author));
            entry.removal_instruction = Some(crate::Instruction {
                session: instruction.session.clone(),
                message: instruction.message.clone(),
                at: instruction.at,
                quote: None,
            });
            // The cited turns' stored messages go first: they hold the words
            // the forget removes, and deleting them again is a no-op; the
            // archived copies are erased in the same transaction.
            tx.entries().erase_residue(&target, &before, now)?;
            if proposals != before {
                tx.proposals().put_all(&proposals)?;
            }
            tx.entries().put(&entry, false)?;
            Ok((entry, true))
        })
    }

    fn answer_open_item(
        &self,
        id: &str,
        answer: ItemAnswer,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&answer.recorded_by)?;
        if redactor.mask(&answer.words) != answer.words {
            return Err(refusal(
                WriteRefusal::NeedsRedaction,
                "the answer has quoted text that redaction would alter; nothing written",
            ));
        }
        refuse_forgotten(self, &answer.session)?;
        self.write(|tx| {
            // Checked again inside the write: a forget committed after the
            // check above must not be followed by a fresh answer write.
            refuse_forgotten(self, &answer.session)?;
            let mut entry = tx.entries().resolve(id)?;
            check_unsettled_item(&entry)?;
            let author = Author::agent(answer.recorded_by.clone());
            let item = item_mut(&mut entry);
            // Only a replay of the stored typed turn (same words, same session,
            // same time) is idempotent; a later typed repetition of the words
            // is a later choice and moves the answer's time.
            if item.answer.as_ref().is_some_and(|old| {
                answer.at < old.at
                    || (answer.at == old.at
                        && old.words == answer.words
                        && old.session == answer.session)
            }) {
                return Ok((tx.entries().resolve(id)?, false));
            }
            item.answer = Some(answer);
            entry.updated = now;
            entry.updated_by = Some(author);
            tx.entries().put(&entry, false)?;
            Ok((entry, true))
        })
    }

    fn close_open_item(
        &self,
        id: &str,
        decision: Option<&EntryId>,
        note: Option<&str>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        let note = match (note, decision) {
            (Some(note), _) => {
                check_reason(note)?;
                redactor.mask(note)
            }
            (None, Some(decision)) => format!("settled by {decision}"),
            (None, None) => {
                return Err(refusal(
                    WriteRefusal::InvalidText,
                    "close names the decision written for the item (--decision) or says what was done (--note)",
                ));
            }
        };
        self.write(|tx| {
            let mut entry = tx.entries().resolve(id)?;
            let state = check_unsettled_item(&entry)?;
            let choice = entry.item.as_ref().is_some_and(|i| !i.options.is_empty());
            if choice && state != ItemState::Answered {
                return Err(refusal(
                    WriteRefusal::NotAnswered,
                    format!(
                        "{} is a choice and has no recorded answer; record the user's answer with entry answer first",
                        entry.id
                    ),
                ));
            }
            if let Some(decision) = decision {
                check_settling_decision(tx, decision, &entry.id)?;
            }
            let by = Author::agent(author.clone());
            item_mut(&mut entry).closed = Some(ItemClosure {
                at: now,
                by: author,
                decision: decision.cloned(),
                note: note.clone(),
            });
            entry.status = EntryStatus::Retired;
            entry.retirement_reason = Some(format!("closed: {note}"));
            entry.inactive_since = Some(now);
            entry.updated = now;
            entry.updated_by = Some(by);
            tx.entries().put(&entry, false)?;
            Ok((entry, true))
        })
    }

    fn has_unfilled_cites(&self, session: &SessionId) -> Result<bool> {
        Ok(self
            .read()
            .entries()
            .all()?
            .0
            .iter()
            .any(|e| unfilled(e, session)))
    }

    fn fill_cite_messages(
        &self,
        session: &SessionId,
        now: time::OffsetDateTime,
        find: impl Fn(&Cite) -> Option<String>,
    ) -> Result<Vec<EntryId>> {
        if !self.read().exists()? {
            return Ok(Vec::new());
        }
        self.write(|tx| {
            let (mut entries, _) = tx.entries().all()?;
            entries.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
            let mut filled = Vec::new();
            for mut entry in entries.into_iter().filter(|e| unfilled(e, session)) {
                let Some(cite) = entry.cite.as_mut() else {
                    continue;
                };
                let Some(message) = find(cite) else {
                    continue;
                };
                cite.message = Some(message);
                entry.updated = now;
                tx.entries().put(&entry, false)?;
                filled.push(entry.id);
            }
            Ok(filled)
        })
    }
}

/// Validate decision links while holding the transaction that writes them.
fn decision_replacement(tx: &Tx<'_>, decision: &Decision) -> Result<Option<Entry>> {
    if let Some(id) = &decision.settles {
        linked_entry(tx, id, OPEN_ITEM_TAG, WriteRefusal::InvalidSettles)?;
    }
    let Some(id) = &decision.supersedes else {
        return Ok(None);
    };
    let older = linked_entry(tx, id, DECISION_TAG, WriteRefusal::InvalidSupersedes)?;
    if older.status != EntryStatus::Active {
        return Err(refusal(
            WriteRefusal::InvalidSupersedes,
            "--supersedes must name an active decision",
        ));
    }
    if !decision.by_user() && older.decision.as_ref().is_none_or(Decision::by_user) {
        return Err(refusal(
            WriteRefusal::SupersedeUserDecision,
            "only a user decision may supersede a user decision or a trial decision with no typed decider",
        ));
    }
    Ok(Some(older))
}

fn linked_entry(tx: &Tx<'_>, id: &EntryId, tag: &str, kind: WriteRefusal) -> Result<Entry> {
    let invalid = || refusal(kind, format!("the link must name an existing {tag} entry"));
    if id.tag().as_str() != tag {
        return Err(invalid());
    }
    match tx.entries().get(id) {
        Err(crate::Error::EntryNotFound { .. }) => Err(invalid()),
        result => result,
    }
}

/// Refuse `decision` unless it is an active decision whose `settles`
/// names `item`, so the link between them runs both ways.
fn check_settling_decision(tx: &Tx<'_>, decision: &EntryId, item: &EntryId) -> Result<()> {
    let invalid = || {
        refusal(
            WriteRefusal::InvalidDecision,
            format!(
                "--decision must name an active decision written with --settles {item}; nothing written"
            ),
        )
    };
    if decision.tag().as_str() != DECISION_TAG {
        return Err(invalid());
    }
    let entry = match tx.entries().get(decision) {
        Err(crate::Error::EntryNotFound { .. }) => return Err(invalid()),
        result => result?,
    };
    let settles = entry.decision.as_ref().and_then(|d| d.settles.as_ref());
    if entry.status != EntryStatus::Active || settles != Some(item) {
        return Err(invalid());
    }
    Ok(())
}

/// Whether `entry` is from `session` and still waits for its message uuid.
fn unfilled(entry: &Entry, session: &SessionId) -> bool {
    entry
        .cite
        .as_ref()
        .is_some_and(|c| &c.session == session && c.message.is_none())
}
