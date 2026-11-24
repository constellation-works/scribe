//! Isolated rebuild checkpoints: a `rebuild_checkpoints` row of
//! `scribe.db` holding the partial record in the session file format
//! (2_design.md §15.7, phase 6); a store whose sessions are still files
//! keeps it beside the record as a dot-file. `updated` in a checkpoint pins
//! the visible record revision it may replace, not the checkpoint time.
//! The per-session locks are lock files in `locks/`, held across model
//! calls, which no SQLite transaction may be.

use crate::repository::sessions::encode_checkpoint;
use crate::{Result, SessionId, SessionRecord};
use crate::{Store, fsio};
use time::OffsetDateTime;

/// Serializes rebuilders of one session without holding the store lock over
/// model calls. The OS releases it on interruption.
#[derive(Debug)]
pub struct SessionLock {
    _lock: fsio::LockGuard,
}

impl Store {
    /// Take the lock file `name` under `locks/`, with the store's bounded
    /// wait; `what` names its holder in a timeout.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Locked`] past the wait, and the privacy and I/O errors.
    pub fn session_lock(&self, name: &str, what: &str) -> Result<SessionLock> {
        let path = self.lock_path(name)?;
        Ok(SessionLock {
            _lock: fsio::LockGuard::acquire(&path, what, self.lock_wait)?,
        })
    }

    /// Keep `record` as its session's rebuild checkpoint; `false` when the
    /// session has no row.
    pub(crate) fn keep_checkpoint(&self, record: &SessionRecord) -> Result<bool> {
        let text = encode_checkpoint(&self.root, record)?;
        let id = record.id.clone();
        self.sessions_tx(|tx| tx.put_checkpoint(&id, &text, OffsetDateTime::now_utc()))
    }

    /// Serialize Codex prompt ingestion for one session across lifecycle hooks.
    pub fn lock_codex_ingestion(&self, id: &SessionId) -> Result<SessionLock> {
        let path = self.lock_path(&format!("codex-{id}.lock"))?;
        Ok(SessionLock {
            _lock: fsio::LockGuard::acquire(&path, "codex ingestion", self.lock_wait)?,
        })
    }
}
