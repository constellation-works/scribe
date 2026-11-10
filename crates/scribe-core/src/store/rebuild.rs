//! Isolated rebuild checkpoints. They use the session format and live beside
//! the visible record; all readers skip dot-files. `updated` in a checkpoint
//! pins the visible record revision it may replace, not the checkpoint time.

use super::{Store, format, fsio};
use crate::{Error, Redactor, Result, SessionId, SessionRecord};
use std::path::PathBuf;
use time::OffsetDateTime;

/// Serializes rebuilders of one session without holding the store lock over
/// model calls. The OS releases it on interruption.
#[derive(Debug)]
pub struct RebuildGuard {
    _lock: fsio::LockGuard,
}

impl Store {
    /// Acquire the session's rebuild lock, with the store's bounded wait.
    pub fn lock_rebuild(&self, record: &SessionRecord) -> Result<RebuildGuard> {
        self.session(&record.id)?;
        let path = self.rebuild_path(record).with_extension("lock");
        fsio::inspect_file(&path)?;
        Ok(RebuildGuard {
            _lock: fsio::LockGuard::acquire(&path, "session rebuild", self.lock_wait)?,
        })
    }

    fn rebuild_path(&self, record: &SessionRecord) -> PathBuf {
        self.session_path(record)
            .with_file_name(format!(".{}.rebuild.md", record.id))
    }

    /// The partial rebuilt record, if present. This read never locks or writes.
    pub fn rebuild_checkpoint(&self, original: &SessionRecord) -> Result<Option<SessionRecord>> {
        // Validate the root and parent directories through the visible record.
        self.session(&original.id)?;
        let path = self.rebuild_path(original);
        if !fsio::inspect_file(&path)? {
            return Ok(None);
        }
        let year = format!("{:04}", original.started.year());
        let month = format!("{:02}", u8::from(original.started.month()));
        format::decode_session(
            &path,
            &fsio::read(&path)?,
            original.id.as_str(),
            (&year, &month),
        )
        .map(Some)
    }

    /// Persist one rebuild window without touching the visible record or control.
    /// Callers hold `lock_rebuild` for the whole rebuild.
    pub fn checkpoint_rebuild(
        &self,
        mut record: SessionRecord,
        redactor: &Redactor,
    ) -> Result<SessionRecord> {
        let _lock = self.lock("rebuild checkpoint")?;
        self.refuse_busy(&record.id)?;
        self.session(&record.id)?;
        redactor.session(&mut record);
        let path = self.rebuild_path(&record);
        fsio::write_atomic(&path, &format::encode_session(&path, &record)?)?;
        Ok(record)
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
        let _lock = self.lock("finish rebuild")?;
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
        let path = self.session_path(&record);
        fsio::write_atomic(&path, &format::encode_session(&path, &record)?)?;
        fsio::remove_private(&self.rebuild_path(&record))?;
        Ok(Some(record))
    }

    fn refuse_busy(&self, id: &SessionId) -> Result<()> {
        if self.control()?.get(id).is_some_and(|c| c.on && c.due) {
            return Err(Error::SessionBusy(id.clone()));
        }
        Ok(())
    }
}
