//! Session-pass candidates, reservations and interrupted approval recovery.

use super::Store;
use crate::{EntryId, ProposalKind, Result};

impl Store {
    /// Include reserved candidate ids and deleted handoff ids when
    /// allocating an entry id.
    pub(super) fn candidate_reserved_ids(&self) -> Result<Vec<EntryId>> {
        let mut ids: Vec<_> = self
            .proposals()?
            .into_iter()
            .filter(|p| p.kind == ProposalKind::Candidate)
            .map(|p| p.older)
            .collect();
        let deleted = self
            .read_db(super::db::docs::deleted_handoffs)?
            .unwrap_or_default();
        ids.extend(deleted);
        Ok(ids)
    }
}
