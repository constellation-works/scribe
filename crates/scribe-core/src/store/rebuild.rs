//! Isolated rebuild checkpoints: a `rebuild_checkpoints` row of
//! `scribe.db` holding the partial record in the session file format
//! (2_design.md §15.7, phase 6); a store whose sessions are still files
//! keeps it beside the record as a dot-file. `updated` in a checkpoint pins
//! the visible record revision it may replace, not the checkpoint time.
//! The per-session locks are lock files in `locks/`, held across model
//! calls, which no SQLite transaction may be.

use super::sessions::encode_checkpoint;
use super::{Store, fsio};
use crate::{Error, Redactor, Result, SessionId, SessionRecord};
use time::OffsetDateTime;

/// Serializes rebuilders of one session without holding the store lock over
/// model calls. The OS releases it on interruption.
#[derive(Debug)]
pub struct RebuildGuard {
    _lock: fsio::LockGuard,
}

impl Store {
    /// Serialize Codex prompt ingestion for one session across lifecycle hooks.
    pub fn lock_codex_ingestion(&self, id: &SessionId) -> Result<RebuildGuard> {
        let path = self.lock_path(&format!("codex-{id}.lock"))?;
        Ok(RebuildGuard {
            _lock: fsio::LockGuard::acquire(&path, "codex ingestion", self.lock_wait)?,
        })
    }

    /// Acquire the session's rebuild lock, with the store's bounded wait.
    pub fn lock_rebuild(&self, record: &SessionRecord) -> Result<RebuildGuard> {
        self.session(&record.id)?;
        let path = self.lock_path(&format!("rebuild-{}.lock", record.id))?;
        Ok(RebuildGuard {
            _lock: fsio::LockGuard::acquire(&path, "session rebuild", self.lock_wait)?,
        })
    }

    /// The partial rebuilt record, if present. This read never locks or writes.
    pub fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        self.session(&original.id)?;
        self.checkpoint_of(original)
    }

    /// Persist one rebuild window without touching the visible record or control.
    /// Callers hold `lock_rebuild` for the whole rebuild.
    pub fn checkpoint_rebuild(
        &self,
        mut record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord> {
        self.write(|| {
            self.refuse_busy(&record.id)?;
            self.session(&record.id)?;
            redactor.session(&mut record);
            let text = encode_checkpoint(&self.root, &record)?;
            let id = record.id.clone();
            let kept =
                self.sessions_tx(|tx| tx.put_checkpoint(&id, &text, OffsetDateTime::now_utc()))?;
            if !kept {
                return Err(Error::SessionNotFound {
                    id,
                    path: self.root.join(super::DB_FILE),
                });
            }
            Ok(record)
        })
    }

    /// Compare and replace under the store lock. `None` means the visible
    /// record advanced: read its new revision and fold in any newer messages
    /// before trying again. Rebuild is the sole sealed-segment replacement route.
    pub fn finish_rebuild(
        &self,
        mut record: SessionRecord,
        expected: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<Option<SessionRecord>> {
        self.write(|| {
            self.refuse_busy(&record.id)?;
            let current = self.session(&record.id)?.record;
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
            self.sessions_tx(|tx| {
                tx.put_record(&record)?;
                tx.drop_checkpoint(&record.id)
            })?;
            Ok(Some(record))
        })
    }

    fn refuse_busy(&self, id: &SessionId) -> Result<()> {
        if self.control()?.get(id).is_some_and(|c| c.on && c.due) {
            return Err(Error::SessionBusy(id.clone()));
        }
        Ok(())
    }
}
