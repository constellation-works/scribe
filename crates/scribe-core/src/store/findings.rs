//! Findings the session pass writes itself, and the open candidates a pass
//! settles because they can no longer be approved
//! (docs/design/scribe/2_design.md §4.7).

use super::{Redactor, Store};
use crate::pass::{FindingDraft, same_statement};
use crate::{
    Author, AuthorKind, ContentFrom, Entry, EntryId, EntryStatus, ProposalId, ProposalKind,
    ProposalState, Result, SessionRecord, Speaker, TagName,
};
use time::OffsetDateTime;

/// The findings one pass window wrote and the earlier findings they
/// superseded, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassFindings {
    /// The new `finding` entries.
    pub written: Vec<EntryId>,
    /// Earlier agent-authored findings now `superseded` by one of them.
    pub superseded: Vec<EntryId>,
}

/// What [`Store::settle_stale_candidates`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettledCandidates {
    /// Open `finding` candidates written as findings: the proposal and the
    /// entry (its reserved id).
    pub findings: Vec<(ProposalId, EntryId)>,
    /// Open candidates closed: under a tag the store no longer defines, or
    /// a finding an entry already records. Each with the tag.
    pub closed: Vec<(ProposalId, TagName)>,
}

impl SettledCandidates {
    /// Whether nothing was settled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.closed.is_empty()
    }
}

/// The longest proposal reason, as everywhere else.
const REASON_CHARS: usize = 300;

impl Store {
    /// Write a pass window's findings. The caller holds the pass lock and
    /// has checked the record revision; the findings are written before the
    /// record, so an interrupted window is retried and the duplicate guard
    /// keeps the retry from writing one twice.
    ///
    /// A draft is skipped (nothing written, the record still advances) when
    /// the session is not on, an entry is unreadable, the store has no
    /// `require_evidence` finding tag, its provenance is not this session's,
    /// its content or evidence would need redaction (the entry stays an
    /// exact copy), or an entry or candidate already records it. A named
    /// supersede applies only to an active agent-authored finding of the
    /// same project; any other id is ignored and the finding still written.
    pub(super) fn write_pass_findings(
        &self,
        record: &SessionRecord,
        drafts: &[FindingDraft],
        redactor: &Redactor,
    ) -> Result<PassFindings> {
        let mut done = PassFindings::default();
        if drafts.is_empty() || !self.control()?.get(&record.id).is_some_and(|c| c.on) {
            return Ok(done);
        }
        let config = self.config()?;
        let Some(tag) = config
            .tags
            .resolve(crate::FINDING_TAG)
            .filter(|t| t.require_evidence)
            .cloned()
        else {
            return Ok(done);
        };
        let (mut entries, skipped) = self.load_entries()?;
        if !skipped.is_empty() {
            tracing::warn!(
                skipped = skipped.len(),
                "not writing findings: some entries are unreadable"
            );
            return Ok(done);
        }
        let proposals = self.proposals()?;
        let mut ids: Vec<EntryId> = entries.iter().map(|e| e.id.clone()).collect();
        ids.extend(self.candidate_reserved_ids()?);
        let now = record.updated;
        for draft in drafts.iter().take(crate::pass::MAX_PASS_FINDINGS) {
            let Some(message) = draft.cite.message.clone() else {
                continue;
            };
            // The finding a correction revises is near-identical by design
            // (one number changed), so against it only the same text counts
            // as a duplicate; every other entry takes the usual guard.
            let target = draft.supersedes.as_ref().and_then(|older| {
                entries.iter().position(|e| {
                    &e.id == older
                        && e.tag == tag.name
                        && e.is_finding()
                        && e.author.kind == AuthorKind::Agent
                        && e.status == EntryStatus::Active
                        && e.project == record.project
                })
            });
            let duplicate = entries.iter().enumerate().any(|(i, e)| {
                if Some(i) == target {
                    crate::verbatim::normalize(&e.content).trim()
                        == crate::verbatim::normalize(&draft.content).trim()
                } else {
                    same_statement(&e.content, &draft.content)
                }
            });
            if draft.cite.session != record.id
                || draft.project != record.project
                || draft.content.trim().is_empty()
                || draft.content != redactor.mask(&draft.content)
                || draft.evidence.trim().is_empty()
                || draft.evidence != redactor.mask(&draft.evidence)
                || duplicate
                || proposals
                    .iter()
                    .filter_map(|p| p.candidate.as_ref())
                    .any(|c| same_statement(&c.quote, &draft.content))
            {
                continue;
            }
            let id = EntryId::next_in(&tag.name, &ids);
            let mut entry = Entry {
                id: id.clone(),
                tag: tag.name.clone(),
                status: EntryStatus::Active,
                project: draft.project.clone(),
                topics: crate::Topics::default(),
                related: Vec::new(),
                provider: draft.provider.clone(),
                host: draft.host.clone(),
                author: Author::assistant(),
                updated_by: None,
                created: now,
                updated: now,
                said: String::new(),
                content_from: ContentFrom::Message {
                    role: Speaker::Assistant,
                    message,
                    at: draft.cite.at,
                },
                cite: Some(draft.cite.clone()),
                retirement_reason: None,
                superseded_by: None,
                review_after: tag.review_after(now.date()),
                quote: None,
                window: None,
                evidence: Some(draft.evidence.clone()),
                as_of: Some(draft.cite.at.date()),
                content: draft.content.clone(),
            };
            redactor.entry(&mut entry);
            if entry.content != draft.content {
                continue;
            }
            self.write_entry(&entry, true)?;
            ids.push(id.clone());
            done.written.push(id.clone());
            if let Some(target) = target.and_then(|i| entries.get_mut(i)) {
                target.status = EntryStatus::Superseded;
                target.superseded_by = Some(id.clone());
                target.updated = now;
                self.write_entry(target, false)?;
                done.superseded.push(target.id.clone());
            }
            entries.push(entry);
        }
        Ok(done)
    }

    /// At the start of a pass, under the lock, settle the open candidates
    /// that can no longer be approved: one under the finding tag is written
    /// as a finding (its reserved id; closed instead when an entry already
    /// records it), and one under a tag the store no longer defines is
    /// closed with a reason naming the tag. Both are settled by scribe
    /// (`{kind: scribe, name: session-pass}`), never by Daniel; a closed
    /// candidate stays in the duplicate check, so it is never proposed
    /// again. Other open candidates are untouched. Nothing to settle takes
    /// no lock and writes nothing.
    pub fn settle_stale_candidates(&self, now: OffsetDateTime) -> Result<SettledCandidates> {
        let mut done = SettledCandidates::default();
        if !self.has_stale_candidates()? {
            return Ok(done);
        }
        let _lock = self.lock("session pass")?;
        let config = self.config()?;
        let (entries, skipped) = self.load_entries()?;
        let mut proposals = self.proposals()?;
        for proposal in &mut proposals {
            if proposal.kind != ProposalKind::Candidate || proposal.state != ProposalState::Open {
                continue;
            }
            let Some(candidate) = proposal.candidate.clone() else {
                continue;
            };
            let reason = match config.tags.get(&candidate.tag) {
                None => format!(
                    "Closed by scribe: the store no longer defines the tag {}",
                    candidate.tag
                ),
                Some(tag) if tag.require_evidence && candidate.role == Speaker::Assistant => {
                    // Without every entry, absence of a duplicate is unknown.
                    if !skipped.is_empty() {
                        continue;
                    }
                    if entries
                        .iter()
                        .any(|e| same_statement(&e.content, &candidate.quote))
                    {
                        String::from("Closed by scribe: an entry already records this finding")
                    } else {
                        proposal.state = ProposalState::Approved;
                        proposal.settled_at = Some(now);
                        proposal.settled_by = None;
                        proposal.settled_author = Some(Author::session_pass());
                        done.findings.push((proposal.id, proposal.older.clone()));
                        continue;
                    }
                }
                Some(_) => continue,
            };
            proposal.state = ProposalState::Rejected;
            proposal.settled_at = Some(now);
            proposal.settled_by = None;
            proposal.settled_author = Some(Author::session_pass());
            proposal.reason = reason.chars().take(REASON_CHARS).collect();
            done.closed.push((proposal.id, candidate.tag));
        }
        if done.is_empty() {
            return Ok(done);
        }
        // The approved proposals are the journal: written first, so an
        // interruption is completed by the next candidate reapply.
        self.write_proposals(&proposals)?;
        for (id, _) in &done.findings {
            if let Some(proposal) = proposals.iter().find(|p| p.id == *id) {
                self.write_candidate_entry(proposal)?;
            }
        }
        Ok(done)
    }

    /// Whether any open candidate is under the finding tag or a tag the
    /// store no longer defines. Reads only.
    fn has_stale_candidates(&self) -> Result<bool> {
        let config = self.config()?;
        Ok(self.proposals()?.iter().any(|p| {
            p.kind == ProposalKind::Candidate
                && p.state == ProposalState::Open
                && p.candidate.as_ref().is_some_and(|c| {
                    config
                        .tags
                        .get(&c.tag)
                        .is_none_or(|t| t.require_evidence && c.role == Speaker::Assistant)
                })
        }))
    }
}
