//! Locked direct writes from the CLI. Authors always describe the agent channel.
use super::ids::resolve;
use super::{Redactor, Store};
use crate::WriteRefusal;
use crate::entry::{AgentEntry, EntryPatch};
use crate::record::patch;
use crate::{
    Author, AuthorKind, AuthorName, ContentFrom, Entry, EntryId, EntryStatus, Error, Label,
    Proposal, ProposalId, ProposalKind, ProposalState, Result, TagName, Topics,
};
use time::OffsetDateTime;

pub(super) fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

/// Refuse a reserved name on an agent-channel write: `daniel`, `scribe`,
/// `consistency`, `session-pass` and `tag-retired` name Daniel's and
/// scribe's own channels, so a record
/// written with one could be mistaken for theirs.
///
/// # Errors
///
/// [`WriteRefusal::ReservedAuthor`] when `author` is reserved.
pub fn check_agent_author(author: &AuthorName) -> Result<()> {
    if author.is_reserved() {
        return Err(refusal(
            WriteRefusal::ReservedAuthor,
            format!(
                "'{author}' is reserved for Daniel's and scribe's own channels; pass your own crew or model name as --author"
            ),
        ));
    }
    Ok(())
}

/// Refuse blank free text: an entry's content or a proposal's text.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] when `text` is empty or only whitespace.
pub fn check_content(text: &str) -> Result<()> {
    required(text)
}

/// Refuse a blank reason, or one over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_reason(reason: &str) -> Result<()> {
    required_reason(reason)
}

fn unknown_tag() -> Error {
    refusal(
        WriteRefusal::TagNotFound,
        "unknown tag; run 'scribe tag list'",
    )
}

/// A quote belongs on an action entry, and a tag that requires one gets
/// one. Whether it is verbatim in Daniel's turns is the caller's check: it
/// needs the transcript.
fn check_quote(def: &crate::TagDef, quote: Option<&str>) -> Result<()> {
    match (def.action, quote) {
        (None, Some(_)) => Err(refusal(
            WriteRefusal::NotActionTag,
            format!(
                "{} is not an action tag; --quote applies only to an action tag",
                def.name
            ),
        )),
        (Some(action), None) if action.require_quote => Err(refusal(
            WriteRefusal::QuoteRequired,
            format!(
                "{} requires Daniel's words: pass --quote with Daniel's correction and --related session:<id>; without a quote, file a friction instead",
                def.name
            ),
        )),
        (Some(_), Some(quote)) if quote.trim().is_empty() => Err(refusal(
            WriteRefusal::QuoteRequired,
            "--quote must not be empty",
        )),
        _ => Ok(()),
    }
}

/// An evidence pointer belongs on a `require_evidence` (finding) entry,
/// and such a tag gets one: a result with nothing to check it against is
/// just an agent's memory of itself.
fn check_evidence(def: &crate::TagDef, evidence: Option<&str>) -> Result<()> {
    match (def.require_evidence, evidence) {
        (true, None) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} records a measured or verified result: pass --evidence with a pointer to its evidence (a path, task, run, experiment or commit)",
                def.name
            ),
        )),
        (true, Some(evidence)) if evidence.trim().is_empty() => Err(refusal(
            WriteRefusal::EvidenceRequired,
            "--evidence must not be empty",
        )),
        (false, Some(_)) => Err(refusal(
            WriteRefusal::EvidenceRequired,
            format!(
                "{} does not take an evidence pointer; --evidence applies only to the finding tag",
                def.name
            ),
        )),
        _ => Ok(()),
    }
}

/// The session a direct finding write names (a `session:` reference), as
/// the per-session cap counts it.
fn draft_session(related: &[crate::Related]) -> Option<String> {
    related
        .iter()
        .find(|r| r.kind() == crate::RelatedKind::Session)
        .map(|r| r.target().to_owned())
}

/// Whether `entry` counts against this write's per-session finding cap:
/// same named session, or, when the write names none, the same author's
/// unnamed-session findings of the same day.
fn counts_toward_cap(
    entry: &Entry,
    tag: &TagName,
    session: Option<&str>,
    author: &AuthorName,
    now: OffsetDateTime,
) -> bool {
    if &entry.tag != tag || !entry.is_finding() {
        return false;
    }
    match (session, entry.session_key()) {
        (Some(named), Some(held)) => named == held,
        (None, None) => &entry.author.name == author && entry.created.date() == now.date(),
        _ => false,
    }
}

fn required(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err(refusal(WriteRefusal::InvalidText, "text must not be empty"));
    }
    Ok(())
}

fn required_reason(reason: &str) -> Result<()> {
    required(reason)?;
    if reason.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "reason must contain at most 300 characters",
        ));
    }
    Ok(())
}

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

    /// Check a new note's arguments (content and tag) as `add_agent_entry`
    /// does, without locking or writing, so a caller can refuse a bad
    /// argument before it sets up anything a write needs.
    ///
    /// # Errors
    ///
    /// `invalid_text` for blank content; `tag_not_found` for a tag no
    /// config or store tag names.
    pub fn check_new_entry(
        &self,
        tag: &TagName,
        content: &str,
        quote: Option<&str>,
    ) -> Result<crate::TagDef> {
        self.check_new_entry_with_evidence(tag, content, quote, None)
    }

    /// [`Store::check_new_entry`] with an evidence pointer, for the
    /// `require_evidence` (`finding`) tag.
    pub fn check_new_entry_with_evidence(
        &self,
        tag: &TagName,
        content: &str,
        quote: Option<&str>,
        evidence: Option<&str>,
    ) -> Result<crate::TagDef> {
        required(content)?;
        let def = self
            .config()?
            .tags
            .resolve(tag.as_str())
            .cloned()
            .ok_or_else(unknown_tag)?;
        check_quote(&def, quote)?;
        check_evidence(&def, evidence)?;
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
        let _lock = self.lock("entry add")?;
        let config = self.config()?;
        let tag = config
            .tags
            .resolve(draft.tag.as_str())
            .ok_or_else(unknown_tag)?;
        check_quote(tag, draft.quote.as_deref())?;
        check_evidence(tag, draft.evidence.as_deref())?;
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
            author: Author::agent(author),
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
            stated_by: None,
            outcome: None,
            content: draft.content,
        };
        redactor.entry(&mut entry);
        if let Some(cover) =
            crate::pass::covering_entry(&entries, &entry.content, &entry.tag, &entry.project)
        {
            return Err(refusal(
                WriteRefusal::AlreadyCovered,
                format!("already covered by {}; nothing written", cover.id),
            ));
        }
        self.write_entry(&entry, true)?;
        Ok(entry)
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
        let _lock = self.lock("entry update")?;
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
        if let Some(content) = patching.content {
            if entry.author.kind != AuthorKind::Agent {
                return Err(refusal(
                    WriteRefusal::DanielOnly,
                    "Daniel's content changes only through #scribe",
                ));
            }
            required(&content)?;
            if entry.status == EntryStatus::Forgotten {
                return Err(refusal(
                    WriteRefusal::EntryNotActive,
                    "a forgotten entry cannot regain content",
                ));
            }
            entry.content = redactor.mask(&content);
        }
        let mut topics = entry.topics.as_slice().to_vec();
        patch(&mut topics, &patching.add_topics, &patching.remove_topics);
        entry.topics = Topics::try_from(topics)
            .map_err(|e| refusal(WriteRefusal::InvalidTopics, e.to_string()))?;
        patch(
            &mut entry.related,
            &patching.add_related,
            &patching.remove_related,
        );
        let changed = entry != before;
        if changed {
            entry.updated = now;
            entry.updated_by = Some(Author::agent(author));
            self.write_entry(&entry, false)?;
        }
        Ok((entry, changed))
    }

    /// Retire an entry, preserving the reason and author. An agent's note
    /// retires on the agent's word; Daniel's entry only on his checked
    /// `instruction` (docs/design/scribe/2_design.md §12.1), which the entry
    /// then records with its quote redacted. Retiring keeps the text.
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
        let _lock = self.lock("entry retire")?;
        let mut entry = self.resolve_entry(id)?;
        if entry.author.kind != AuthorKind::Agent && instruction.is_none() {
            return Err(refusal(
                WriteRefusal::DanielOnly,
                "Daniel retires his entries through #scribe retire, or on his instruction with --session and --instruction",
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
            entry.instruction = Some(crate::Instruction {
                session: instructed.session.clone(),
                message: instructed.message.clone(),
                at: instructed.at,
                quote: Some(redactor.mask(&instructed.quote)),
            });
        }
        self.write_entry(&entry, false)?;
        Ok((entry, true))
    }

    /// Forget an entry for good on Daniel's checked `instruction`
    /// (docs/design/scribe/2_design.md §12.4): its text and evidence are
    /// blanked with every proposal quote, candidate quote and reason about it
    /// ([`crate::consistency::forget`]). The entry keeps its id, tag, dates
    /// and author, records the agent in `updated_by` and the instruction's
    /// session and message ids, but not its quote, which may repeat the words
    /// he wants gone. No file is removed. `proposals.json` is written first,
    /// then the entry. Forgetting a forgotten entry changes nothing.
    pub fn forget_entry(
        &self,
        id: &str,
        instruction: &crate::instruction::Instructed,
        author: AuthorName,
        now: OffsetDateTime,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("entry forget")?;
        let target = self.resolve_entry(id)?;
        if target.status == EntryStatus::Forgotten {
            return Ok((target, false));
        }
        let mut proposals = self.proposals()?;
        let (mut entries, _) = self.load_entries()?;
        let before = proposals.clone();
        let forgotten = crate::consistency::forget(&target.id, &mut proposals, &mut entries, now);
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
        entry.instruction = Some(crate::Instruction {
            session: instruction.session.clone(),
            message: instruction.message.clone(),
            at: instruction.at,
            quote: None,
        });
        if proposals != before {
            self.write_proposals(&proposals)?;
        }
        self.write_entry(&entry, false)?;
        Ok((entry, true))
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
        let _lock = self.lock("proposal add")?;
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
        let _lock = self.lock("proposal change")?;
        let mut proposals = self.proposals()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = self.proposal_index(id, &ids)?;
        let p = &mut proposals[i];
        if p.kind == ProposalKind::Candidate {
            let Some(instructed) = instruction.filter(|_| change != crate::ProposalChange::Update)
            else {
                return Err(refusal(
                    WriteRefusal::DanielOnly,
                    if change == crate::ProposalChange::Update {
                        "Daniel settles candidate proposals through #scribe approve or reject; their reason is scribe's"
                    } else {
                        "Daniel settles candidate proposals through #scribe approve or reject, or on his instruction with --session and --instruction"
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
                    WriteRefusal::DanielOnly,
                    "Daniel settles proposals about his entries through #scribe approve or reject",
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
