//! The proposal repository: proposals and claims in `scribe.db`, their
//! lookup by id, and the session pass's candidate reservations.

use super::Proposals;
use super::ids::resolve;
use crate::checks::refusal;
use crate::db::{self, docs};
use crate::error::Result;
use crate::{EntryId, Proposal, ProposalId, ProposalKind, Store, WriteRefusal};

impl Proposals<'_> {
    /// The proposal an exact id or a unique prefix of at least four
    /// characters names.
    ///
    /// # Errors
    ///
    /// `invalid_id`, `id_ambiguous`, `proposal_not_found` naming the file
    /// searched, and the store's read errors.
    pub fn resolve(&self, input: &str) -> Result<Proposal> {
        self.store.find_proposal(input)
    }

    /// The index in `ids` of the proposal `input` names, as
    /// [`Proposals::resolve`] finds it.
    ///
    /// # Errors
    ///
    /// As [`Proposals::resolve`].
    pub fn position(&self, input: &str, ids: &[String]) -> Result<usize> {
        self.store.proposal_index(input, ids)
    }

    /// Where the proposals are kept, for an error to name.
    #[must_use]
    pub fn path(&self) -> std::path::PathBuf {
        self.store.root.join(crate::PROPOSALS_FILE)
    }

    /// The entry ids a new entry may not take: those reserved by candidate
    /// proposals and those of deleted handoffs.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn reserved_entry_ids(&self) -> Result<Vec<EntryId>> {
        self.store.candidate_reserved_ids()
    }

    /// Every proposal, by id.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<Proposal>> {
        self.store.proposals()
    }

    /// Write the proposals that changed against the stored ones.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, proposals: &[Proposal]) -> Result<()> {
        self.store.write_proposals(proposals)
    }
}

impl Store {
    /// Every proposal, by id; none when there are none.
    pub(crate) fn proposals(&self) -> Result<Vec<Proposal>> {
        let mut proposals = self.read_or_empty(docs::claims)?;
        proposals.sort_by_key(|p| p.id);
        Ok(proposals)
    }

    /// Write the proposals that changed, in the write under way.
    pub(crate) fn write_proposals(&self, proposals: &[Proposal]) -> Result<()> {
        self.with_conn(|conn, path| {
            let before = docs::claims(conn, path)?;
            docs::put_claims(conn, path, &before, proposals, |e| {
                db::db_error("write claims to", path, e)
            })
        })
    }

    /// The index in `ids` of the proposal `input` names; a miss is
    /// `proposal_not_found`, naming the id and the file searched.
    pub(crate) fn proposal_index(&self, input: &str, ids: &[String]) -> Result<usize> {
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
                    self.root.join(crate::PROPOSALS_FILE).display()
                ),
            )
        })
    }

    /// Resolve an exact proposal id or a unique prefix of at least four
    /// characters, as the write verbs do, without locking or writing.
    pub(crate) fn find_proposal(&self, input: &str) -> Result<Proposal> {
        let proposals = self.proposals()?;
        let ids: Vec<String> = proposals.iter().map(|p| p.id.to_string()).collect();
        let i = self.proposal_index(input, &ids)?;
        Ok(proposals[i].clone())
    }
}

impl Store {
    /// Include reserved candidate ids and deleted handoff ids when
    /// allocating an entry id.
    pub(crate) fn candidate_reserved_ids(&self) -> Result<Vec<EntryId>> {
        let mut ids: Vec<_> = self
            .proposals()?
            .into_iter()
            .filter(|p| p.kind == ProposalKind::Candidate)
            .map(|p| p.older)
            .collect();
        let deleted = self
            .read_db(crate::db::docs::deleted_handoffs)?
            .unwrap_or_default();
        ids.extend(deleted);
        Ok(ids)
    }
}
