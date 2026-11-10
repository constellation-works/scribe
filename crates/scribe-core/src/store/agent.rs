//! Locked direct writes from the CLI. Authors always describe the agent channel.
use super::ids::resolve;
use super::{Redactor, Store};
use crate::WriteRefusal;
use crate::entry::{AgentEntry, EntryPatch};
use crate::record::patch;
use crate::{
    Author, AuthorKind, AuthorName, ContentFrom, Entry, EntryId, EntryStatus, Error, Label,
    Proposal, ProposalId, ProposalKind, ProposalState, Result, Topics,
};
use time::OffsetDateTime;

pub(super) fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

/// Refuse a reserved name on an agent-channel write: `daniel`, `scribe`
/// and `consistency` name Daniel's and scribe's own channels, so a record
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
        let i = match resolve(input, &ids, WriteRefusal::EntryNotFound) {
            Ok(i) => i,
            Err(Error::AgentWrite {
                kind: WriteRefusal::EntryNotFound,
                ..
            }) if exact.is_some() => {
                let id =
                    exact.ok_or_else(|| refusal(WriteRefusal::EntryNotFound, "entry missing"))?;
                return Err(Error::EntryNotFound {
                    path: self.entry_path(&id),
                    id,
                });
            }
            Err(e) => return Err(e),
        };
        let id = ids[i]
            .parse::<EntryId>()
            .map_err(|e| refusal(WriteRefusal::InvalidId, e.to_string()))?;
        self.entry(&id)
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
        let tag = config.tags.resolve(draft.tag.as_str()).ok_or_else(|| {
            refusal(
                WriteRefusal::TagNotFound,
                "unknown tag; run 'scribe tag list'",
            )
        })?;
        let (entries, skipped) = self.load_entries()?;
        let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
        ids.extend(
            skipped
                .iter()
                .filter_map(|s| s.path.file_stem()?.to_str()?.parse().ok()),
        );
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
            said: String::new(),
            content_from: ContentFrom::Agent,
            cite: None,
            retirement_reason: None,
            superseded_by: None,
            review_after: tag.review_after(now.date()),
            content: draft.content,
        };
        redactor.entry(&mut entry);
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
        // An agent edits only its own note, and only while it is active;
        // a Daniel entry keeps the metadata-only rule below.
        if entry.author.kind == AuthorKind::Agent {
            if entry.author.name != author {
                return Err(refusal(
                    WriteRefusal::NotAuthor,
                    "only the entry's author may update it; record your own note with 'scribe entry add'",
                ));
            }
            if entry.status != EntryStatus::Active {
                return Err(refusal(
                    WriteRefusal::EntryNotActive,
                    "only an active entry can be updated",
                ));
            }
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

    /// Retire an agent note, preserving the reason and author.
    pub fn retire_agent_entry(
        &self,
        id: &str,
        reason: &str,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(Entry, bool)> {
        check_agent_author(&author)?;
        required_reason(reason)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("entry retire")?;
        let mut entry = self.resolve_entry(id)?;
        if entry.author.kind != AuthorKind::Agent {
            return Err(refusal(
                WriteRefusal::DanielOnly,
                "Daniel retires his entries through #scribe retire",
            ));
        }
        if entry.author.name != author {
            return Err(refusal(
                WriteRefusal::NotAuthor,
                "only the entry's author may retire it",
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
        entry.updated_by = Some(Author::agent(author));
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
        if (kind == ProposalKind::Supersedes) != newer.is_some() {
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
        };
        proposals.push(proposal.clone());
        self.write_proposals(&proposals)?;
        Ok((proposal, true))
    }

    /// Change an open agent proposal's reason or settle an agent entry's proposal.
    pub fn change_agent_proposal(
        &self,
        id: &str,
        change: crate::ProposalChange,
        reason: Option<&str>,
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
        let i = resolve(id, &ids, WriteRefusal::ProposalNotFound)?;
        let p = &mut proposals[i];
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
            if p.author == Author::agent(author.clone()) {
                return Err(refusal(
                    WriteRefusal::NotAuthor,
                    "the proposal's author cannot settle it; another agent must settle it",
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
            if p.author.name != author {
                return Err(refusal(
                    WriteRefusal::NotAuthor,
                    "only the proposal's author may update it",
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
}
