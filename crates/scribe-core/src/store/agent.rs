//! Locked direct writes from the CLI. Authors always describe the agent channel.
use super::ids::resolve;
use super::{Redactor, Store};
use crate::WriteRefusal;
use crate::checks::entry::{
    check_evidence, check_handoff, check_handoff_related, check_handoff_related_update,
    check_quote, check_spoken_quote, check_spoken_tag, counts_toward_cap, draft_session,
};
use crate::checks::records::{
    check_decision_redaction, check_decision_size, check_decision_update, patch_decision,
};
use crate::checks::{
    check_agent_author, refusal, required, required_message, required_reason, unknown_tag,
};
use crate::entry::{AgentEntry, DECISION_TAG, EntryPatch, is_decision_or_item};
use crate::record::patch;
use crate::{
    Author, AuthorKind, AuthorName, ContentFrom, Entry, EntryId, EntryStatus, Error, Label,
    Proposal, ProposalId, ProposalKind, ProposalState, Result, TagName, Topics,
};
use time::OffsetDateTime;

impl Store {
    /// The index in `ids` of the proposal `input` names; a miss is
    /// `proposal_not_found`, naming the id and the file searched.
    fn proposal_index(&self, input: &str, ids: &[String]) -> Result<usize> {
        // A well-formed id is an exact lookup, as for entries; only other
        // input is a prefix, which needs four characters.
        let found = if input.parse::<ProposalId>().is_ok() {
            ids.iter().position(|id| id == input)
        } else {
            resolve(input, ids)?
        };
        found.ok_or_else(|| {
            refusal(
                WriteRefusal::ProposalNotFound,
                format!(
                    "no proposal {input} in {}; run 'scribe proposal list' with the same --root to see existing ids",
                    self.root.join(super::PROPOSALS_FILE).display()
                ),
            )
        })
    }

    /// Resolve an exact proposal id or a unique prefix of at least four
    /// characters, as the write verbs do, without locking or writing.
    pub fn resolve_proposal(&self, input: &str) -> Result<Proposal> {
        let proposals = self.proposals()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = self.proposal_index(input, &ids)?;
        Ok(proposals[i].clone())
    }

    /// Resolve an exact entry id or a unique prefix of at least four characters.
    pub fn resolve_entry(&self, input: &str) -> Result<Entry> {
        let exact = input.parse::<EntryId>().ok();
        if let Some(id) = &exact {
            match self.entry(id) {
                Ok(entry) => return Ok(entry),
                Err(Error::EntryNotFound { .. }) => {}
                Err(e) => return Err(e),
            }
        } else if input.len() < 4
            || !input
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || input
                .rsplit_once('-')
                .is_some_and(|(_, n)| n.starts_with('0'))
        {
            return Err(refusal(
                WriteRefusal::InvalidId,
                "use an entry id or a lowercase unique prefix of at least four characters",
            ));
        }
        let (entries, skipped) = self.load_entries()?;
        let mut ids: Vec<String> = entries.iter().map(|e| e.id.to_string()).collect();
        ids.extend(skipped.iter().filter_map(|s| {
            s.path
                .file_stem()?
                .to_str()?
                .parse::<EntryId>()
                .ok()
                .map(|id| id.to_string())
        }));
        ids.sort();
        ids.dedup();
        let Some(i) = resolve(input, &ids)? else {
            return Err(match exact {
                Some(id) => Error::EntryNotFound {
                    path: self.entry_path(&id),
                    id,
                },
                None => refusal(
                    WriteRefusal::EntryNotFound,
                    format!(
                        "no entry matches '{input}' under {}; run 'scribe entry list' with the same --root to see existing ids",
                        self.root.join(super::ENTRIES_DIR).display()
                    ),
                ),
            });
        };
        let id = ids[i]
            .parse::<EntryId>()
            .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string()))?;
        self.entry(&id)
    }

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
    pub fn check_new_entry_full(
        &self,
        tag: &TagName,
        content: &str,
        quote: Option<&str>,
        evidence: Option<&str>,
        spoken: bool,
    ) -> Result<crate::TagDef> {
        required(content)?;
        let def = self
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

    /// Check an agent entry patch without locking or writing, so invalid
    /// content, topics or ids can be reported before a caller constructs a
    /// redactor. The write path repeats these checks under the store lock.
    pub fn check_agent_entry_update(&self, id: &str, patching: &EntryPatch) -> Result<()> {
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

    /// Create a note with agent provenance, allocating an unused id under the lock.
    pub fn add_agent_entry(
        &self,
        draft: AgentEntry,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Entry> {
        check_agent_author(&author)?;
        required(&draft.content)?;
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
        let config = self.config()?;
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
                let resolved = match self.resolve_session(&id) {
                    Ok(resolved) => resolved,
                    Err(Error::SessionNotFound { .. }) => id,
                    Err(error) => return Err(error),
                };
                self.refuse_forgotten(&resolved)?;
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
            .map(|decision| self.decision_replacement(decision))
            .transpose()?
            .flatten();
        let (entries, skipped) = self.load_entries()?;
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
        ids.extend(self.candidate_reserved_ids()?);
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
        self.write_entry(&entry, true)?;
        if let Some(older) = &mut replaced {
            older.status = EntryStatus::Superseded;
            older.superseded_by = Some(entry.id.clone());
            older.inactive_since = Some(now);
            older.updated = now;
            older.updated_by = Some(entry.author.clone());
            self.write_entry(older, false)?;
        }
        Ok(entry)
            })
    }

    /// Apply a patch atomically; return whether the record changed.
    pub fn update_agent_entry(
        &self,
        id: &str,
        patching: EntryPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
            let mut entry = self.resolve_entry(id)?;
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
                self.write_entry(&entry, false)?;
            }
            Ok((entry, changed))
        })
    }

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
    pub fn retire_agent_entry(
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
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
        let mut entry = self.resolve_entry(id)?;
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
        self.write_entry(&entry, false)?;
        Ok((entry, true))
            })
    }

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
    pub fn forget_entry(
        &self,
        id: &str,
        instruction: &crate::instruction::Instructed,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        required_message(instruction)?;
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
            let target = self.resolve_entry(id)?;
            if target.status == EntryStatus::Forgotten {
                return Ok((target, false));
            }
            let mut proposals = self.proposals()?;
            let (mut entries, _) = self.load_entries()?;
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
            self.forget_entry_residue(&target, &before, now)?;
            if proposals != before {
                self.write_proposals(&proposals)?;
            }
            self.write_entry(&entry, false)?;
            Ok((entry, true))
        })
    }

    /// Open an agent proposal against either author's active entry.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit proposal inputs at the store boundary"
    )]
    pub fn add_agent_proposal(
        &self,
        kind: ProposalKind,
        older: &str,
        newer: Option<&str>,
        reason: &str,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)> {
        check_agent_author(&author)?;
        required_reason(reason)?;
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
        let older = self.resolve_entry(older)?;
        let newer = newer.map(|id| self.resolve_entry(id)).transpose()?;
        if kind == ProposalKind::Candidate || (kind == ProposalKind::Supersedes) != newer.is_some()
        {
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "supersedes requires --newer; stale forbids it",
            ));
        }
        if newer.as_ref().is_some_and(|e| e.id == older.id) {
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "older and newer must differ",
            ));
        }
        if older.status != EntryStatus::Active
            || newer
                .as_ref()
                .is_some_and(|e| e.status != EntryStatus::Active)
        {
            return Err(refusal(
                WriteRefusal::EntryNotActive,
                "proposal entries must be active",
            ));
        }
        let mut proposals = self.proposals()?;
        let newer = newer.map(|e| e.id);
        if let Some(p) = proposals
            .iter()
            .find(|p| p.kind == kind && p.older == older.id && p.newer == newer)
        {
            if p.state == ProposalState::Open
                && p.reason == redactor.mask(reason)
                && p.author == Author::agent(author.clone())
            {
                return Ok((p.clone(), false));
            }
            return Err(refusal(
                WriteRefusal::ProposalInvalid,
                "a proposal already covers these entries; use proposal update on an open agent proposal",
            ));
        }
        let n = proposals
            .iter()
            .map(|p| p.id.number())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| refusal(WriteRefusal::ProposalInvalid, "proposal ids exhausted"))?;
        let id: ProposalId = format!("p-{n}")
            .parse()
            .map_err(|e: crate::ProposalError| {
                refusal(WriteRefusal::ProposalInvalid, e.to_string())
            })?;
        let proposal = Proposal {
            id,
            kind,
            older: older.id,
            newer,
            reason: redactor.mask(reason),
            quotes: Vec::new(),
            state: ProposalState::Open,
            opened_at: now,
            settled_at: None,
            settled_by: None,
            author: Author::agent(author),
            updated_by: None,
            settled_author: None,
            closed_reason: None,
            instruction: None,
            candidate: None,
        };
        proposals.push(proposal.clone());
        self.write_proposals(&proposals)?;
        Ok((proposal, true))
            })
    }

    /// Change an open agent proposal's reason or settle an agent entry's
    /// proposal. With Daniel's checked `instruction` (2_design.md §12.1,
    /// §12.3), an approve or reject also settles a session-pass candidate
    /// ([`Store::settle_candidate_on_instruction`]); nothing else widens: a
    /// proposal about his entry is still his to settle.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit settlement inputs at the store boundary"
    )]
    pub fn change_agent_proposal(
        &self,
        id: &str,
        change: crate::ProposalChange,
        reason: Option<&str>,
        instruction: Option<&crate::instruction::Instructed>,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)> {
        check_agent_author(&author)?;
        if let Some(reason) = reason {
            required_reason(reason)?;
        }
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
        let mut proposals = self.proposals()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = self.proposal_index(id, &ids)?;
        let p = &mut proposals[i];
        if p.kind == ProposalKind::Candidate {
            let Some(instructed) = instruction.filter(|_| change != crate::ProposalChange::Update)
            else {
                // Without an instruction the refusal reads as it always has.
                return Err(refusal(
                    WriteRefusal::UserOnly,
                    if instruction.is_some() {
                        "The user settles candidate proposals through #scribe approve or reject; their reason is scribe's"
                    } else {
                        "The user settles candidate proposals through #scribe approve or reject"
                    },
                ));
            };
            let approve = change == crate::ProposalChange::Approve;
            return self.settle_candidate_on_instruction(
                proposals, i, approve, reason, instructed, author, now, redactor,
            );
        }
        let before = p.clone();
        let reason_changed = reason.is_some_and(|r| redactor.mask(r) != p.reason);
        let mut changed_entry = None;
        if change != crate::ProposalChange::Update {
            let approve = change == crate::ProposalChange::Approve;
            let mut older = self.entry(&p.older)?;
            if older.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::UserOnly,
                    "The user settles proposals about their entries through #scribe approve or reject",
                ));
            }
            let state = if approve {
                ProposalState::Approved
            } else {
                ProposalState::Rejected
            };
            if p.state == state {
                if reason.is_some_and(|r| redactor.mask(r) != p.reason) {
                    return Err(refusal(
                        WriteRefusal::ProposalSettled,
                        "a settled proposal's reason cannot be changed",
                    ));
                }
                return Ok((p.clone(), false));
            }
            if p.state != ProposalState::Open {
                return Err(refusal(
                    WriteRefusal::ProposalSettled,
                    "proposal is already settled",
                ));
            }
            // The settler's `--reason` replaces the proposal's reason below,
            // so it may only do so on an agent-opened proposal: a reason
            // Daniel or scribe wrote stays as written, and settling without
            // `--reason` (or repeating theirs) keeps it.
            if reason_changed && p.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::ProposalNotAgent,
                    "only an agent-opened proposal's reason can be changed; settle without --reason to keep the original reason",
                ));
            }
            if approve {
                if older.status != EntryStatus::Active {
                    return Err(refusal(
                        WriteRefusal::EntryNotActive,
                        "older entry is no longer active",
                    ));
                }
                if reason_changed {
                    p.updated_by = Some(Author::agent(author.clone()));
                }
                p.settled_author = Some(Author::agent(author.clone()));
                crate::consistency::apply_approval(p, &mut older, now);
                changed_entry = Some(older);
            } else if reason_changed {
                p.updated_by = Some(Author::agent(author.clone()));
            }
            p.state = state;
            p.settled_at = Some(now);
            p.settled_by = None;
            p.settled_author = Some(Author::agent(author));
        } else {
            if p.state != ProposalState::Open {
                return Err(refusal(
                    WriteRefusal::ProposalSettled,
                    "only open proposals can be updated",
                ));
            }
            if p.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::ProposalNotAgent,
                    "only agent-opened proposals can be updated",
                ));
            }
            // Preserve the creator while recording who revised the reason.
            if reason_changed {
                p.updated_by = Some(Author::agent(author));
            }
        }
        if let Some(reason) = reason {
            p.reason = redactor.mask(reason);
        }
        let result = p.clone();
        let changed = result != before;
        if changed {
            self.write_proposals(&proposals)?;
        }
        if let Some(entry) = changed_entry {
            self.write_entry(&entry, false)?;
        }
        Ok((result, changed))
            })
    }

    /// Settle candidate `proposals[i]` on Daniel's checked `instructed`
    /// (2_design.md §12.3). The caller holds the lock. The candidate must be
    /// open; the same instruction again changes nothing. An approval writes
    /// the reserved entry as a typed approval does (his for a claim from his
    /// turn, the assistant's for one from an assistant turn), with
    /// `approved_by` the agent; a claim an active entry already covers is
    /// closed `rejected` with `closed_reason: covered` and writes none. A
    /// rejection writes no entry and sets `closed_reason: instructed`. Either way the proposal records the agent
    /// in `settled_author`, the instruction turn's cite in `settled_by` and
    /// `instruction` with its quote redacted. `proposals.json` is written
    /// before the entry, so a crash between them leaves an approved proposal
    /// the next scan re-applies with the same attribution.
    #[allow(
        clippy::too_many_arguments,
        reason = "explicit settlement inputs at the store boundary"
    )]
    fn settle_candidate_on_instruction(
        &self,
        mut proposals: Vec<Proposal>,
        i: usize,
        approve: bool,
        reason: Option<&str>,
        instructed: &crate::instruction::Instructed,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Proposal, bool)> {
        let p = &mut proposals[i];
        // A candidate's reason is scribe's: an agent never replaces it.
        if reason.is_some_and(|r| redactor.mask(r) != p.reason) {
            return Err(refusal(
                WriteRefusal::ProposalNotAgent,
                "only an agent-opened proposal's reason can be changed; settle without --reason to keep the original reason",
            ));
        }
        let same_instruction = p.instruction.as_ref().is_some_and(|held| {
            held.session == instructed.session && held.message == instructed.message
        });
        required_message(instructed)?;
        if p.state != ProposalState::Open {
            // The same command again, or an approval an active entry was
            // found to cover, is done; any other settlement stands.
            let done = if approve {
                p.state == ProposalState::Approved
                    || p.closed_reason == Some(crate::ClosedReason::Covered)
            } else {
                p.closed_reason == Some(crate::ClosedReason::Instructed)
            };
            if same_instruction && done {
                return Ok((p.clone(), false));
            }
            return Err(refusal(
                WriteRefusal::ProposalSettled,
                format!("{} is already {}", p.id, p.state.as_str()),
            ));
        }
        // An instruction cannot settle a claim opened after he typed it
        // (2_design.md §12.2 check 3): he never saw it.
        if instructed.at < p.opened_at {
            return Err(refusal(
                WriteRefusal::InstructionStale,
                format!(
                    "the instruction was typed before {} opened; ask the user again",
                    p.id
                ),
            ));
        }
        p.state = if approve {
            ProposalState::Approved
        } else {
            ProposalState::Rejected
        };
        p.settled_at = Some(now);
        p.settled_by = Some(instructed.cite());
        p.settled_author = Some(Author::agent(author));
        p.instruction = Some(instructed.record(|q| redactor.mask(q)));
        if !approve {
            p.closed_reason = Some(crate::ClosedReason::Instructed);
            let result = p.clone();
            self.write_proposals(&proposals)?;
            return Ok((result, true));
        }
        self.write_proposals(&proposals)?;
        let snapshot = proposals[i].clone();
        if let super::candidates::CandidateWrite::Covered { .. } =
            self.write_candidate_entry(&snapshot)?
        {
            // An active entry already says it: the approval is kept, with
            // its instruction, and closed as covered without an entry.
            proposals[i].state = ProposalState::Rejected;
            proposals[i].closed_reason = Some(crate::ClosedReason::Covered);
            self.write_proposals(&proposals)?;
        }
        Ok((proposals[i].clone(), true))
    }
}
