//! Rebuild verbs: a session record rebuilt in windows beside the visible
//! one, under its own lock (2_design.md §15.7).

use crate::checks::{forgotten_refusal, refuse_busy};
use crate::{Error, RebuildGuard, Redactor, Result, SessionRecord, Store};
use time::OffsetDateTime;

/// The rebuild verbs, implemented for [`Store`].
pub trait RebuildVerbs {
    /// Acquire the session's rebuild lock, with the store's bounded wait.
    fn lock_rebuild(&self, record: &SessionRecord) -> Result<RebuildGuard>;

    /// The partial rebuilt record, if present. This read never locks or writes.
    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>>;

    /// Persist one rebuild window without touching the visible record or control.
    /// Callers hold `lock_rebuild` for the whole rebuild.
    fn checkpoint_rebuild(
        &self,
        record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord>;

    /// Compare and replace under the store lock. `None` means the visible
    /// record advanced: read its new revision and fold in any newer messages
    /// before trying again. Rebuild is the sole sealed-segment replacement route.
    fn finish_rebuild(
        &self,
        record: SessionRecord,
        expected: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>>;
}

impl RebuildVerbs for Store {
    fn lock_rebuild(&self, record: &SessionRecord) -> Result<RebuildGuard> {
        let view = self.read().sessions().get(&record.id)?;
        if view.forgotten.is_some() {
            return Err(forgotten_refusal(&record.id, "nothing is written to it"));
        }
        let guard = self.session_lock(&format!("rebuild-{}.lock", record.id), "session rebuild")?;
        // Forget may have completed while acquisition was waiting.
        self.refuse_forgotten(&record.id)?;
        Ok(guard)
    }

    fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.read().sessions().get(&original.id)?;
        self.read().sessions().checkpoint(original)
    }

    fn checkpoint_rebuild(
        &self,
        mut record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            tx.sessions().get(&record.id)?;
            redactor.session(&mut record);
            let id = record.id.clone();
            let kept = tx.sessions().keep_checkpoint(&record)?;
            if !kept {
                return Err(Error::SessionNotFound {
                    id,
                    path: tx.sessions().db_path(),
                });
            }
            Ok(record)
        })
    }

    fn finish_rebuild(
        &self,
        mut record: SessionRecord,
        expected: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write(|tx| {
            refuse_busy(&tx.sessions().control()?, &record.id)?;
            let current = tx.sessions().get(&record.id)?.record;
            if current.updated != expected {
                return Ok(None);
            }
            record.started = current.started;
            record.created = current.created;
            record.links = current.links;
            record.active = current.active;
            record.keep_active = current.keep_active;
            record.active_change = None;
            record.cataloged_by = current.cataloged_by;
            redactor.session(&mut record);
            // Activity attribution belongs to the latest visible record, not this rebuild.
            record.active_change = current.active_change;
            // The record and the end of its checkpoint commit together.
            tx.sessions().put_rebuilt(&record)?;
            Ok(Some(record))
        })
    }
}
