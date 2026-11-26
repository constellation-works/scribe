//! The audit log's record shape: one JSON line per mutating command.
//!
//! The `audit` table of `scribe.db` (`<root>/audit.jsonl` before
//! 2_design.md §15.9 phase 7) answers "what changed this store, when, and
//! did it work". The CLI's audit middleware builds each [`AuditEvent`] and
//! [`crate::Store::record_audit`] or a [`Tx`](crate::Tx)'s
//! [`Audit::record`] records it. Read-only commands are never
//! audited: they must not write (STD-01 R31).
//!
//! An event holds stable identifiers only: the command path, the record it
//! acted on, the outcome and a stable error code. Never argument values or
//! error text, which can carry record contents or paths (STD-05 R13). Each line
//! carries its own `format` so a reader can refuse a newer one (STD-03 R10);
//! a crash can leave a torn last line, which a reader skips.

use super::Audit;
use super::{Access, ReadWrite};
use crate::Store;
use crate::db::{self, docs};
use crate::error::{Error, Result};
use std::path::PathBuf;

pub use crate::contracts::audit::{AUDIT_FILE, AuditEvent, AuditStatus, IssueAudit};
pub(crate) use crate::db::codec::AUDIT_FORMAT;
pub use crate::db::codec::decode_formatted;
#[cfg(test)]
pub(crate) use crate::db::codec::{AuditLine, encode_line};

impl<A: Access> Audit<'_, A> {
    /// Every audit event and how many lines did not read.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        self.store.audit_events()
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub fn path(&self) -> std::path::PathBuf {
        self.store.audit_path()
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl<'s> Audit<'s, ReadWrite> {
    /// The audit log's write side for [`Store::record_audit`], the guard's
    /// line outside a [`Tx`](super::Tx).
    pub(crate) fn over(store: &'s Store) -> Self {
        Audit {
            store,
            access: std::marker::PhantomData,
        }
    }

    /// Record one audit line. Inside a write's transaction it commits with
    /// the write. Otherwise it is inserted without waiting, and spooled to
    /// a file of its own when another writer holds the database or the
    /// store is still files, so no line waits on a lock or is lost to one.
    ///
    /// A store too new for this build takes no line.
    ///
    /// # Errors
    ///
    /// An unsafe data directory, or a spool file that cannot be written.
    pub fn record(&self, event: &AuditEvent) -> Result<()> {
        if self.store.open_conn().is_some() {
            return self.store.with_conn(|conn, path| {
                docs::put_audit(conn, path, AUDIT_FORMAT, event, None, |e| {
                    db::db_error("audit to", path, e)
                })
                .map(drop)
            });
        }
        self.store.prepare_dir(&self.store.root)?;
        let moved = match self.store.documents_moved() {
            Ok(moved) => moved,
            // A store a newer build migrated takes no write from this one,
            // its log included; the command already reported the refusal.
            Err(Error::NewerFormat { .. }) => return Ok(()),
            Err(other) => return Err(other),
        };
        if moved {
            match self.store.audit_without_waiting(event) {
                Ok(()) => return Ok(()),
                Err(Error::Locked { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        self.store.spool_audit(event)
    }
}

impl Store {
    /// Record one audit line: the command's audit guard's write
    /// ([`Audit::record`](crate::repository::Audit::record)). Inside the
    /// command's write it commits with that write; outside one it is its
    /// own insert, which waits for no lock. The one audit write a caller
    /// makes without a [`Tx`](crate::Tx): a [`Snapshot`](crate::Snapshot)'s
    /// audit repository only reads.
    ///
    /// # Errors
    ///
    /// As [`Audit::record`](crate::repository::Audit::record).
    pub fn record_audit(&self, event: &AuditEvent) -> Result<()> {
        Audit::<ReadWrite>::over(self).record(event)
    }

    /// The audit log, `<root>/audit.jsonl`.
    #[must_use]
    pub(crate) fn audit_path(&self) -> PathBuf {
        self.root.join(AUDIT_FILE)
    }

    /// The audit log's events, oldest first, the spooled ones last, and how
    /// many lines did not parse. Reads only: no lock, no write; a store with
    /// none has none.
    ///
    /// # Errors
    ///
    /// An unsafe file (a link, loose modes) as for any store read, or a
    /// line of a newer audit format (`store_too_new`).
    pub(crate) fn audit_events(&self) -> Result<(Vec<AuditEvent>, usize)> {
        let (lines, torn) = self.audit_lines()?;
        Ok((lines.into_iter().map(|(_, event)| event).collect(), torn))
    }

    /// Audit lines with their original formats, in the same order as `audit_events`.
    pub(crate) fn audit_lines(&self) -> Result<(Vec<(u32, AuditEvent)>, usize)> {
        if !self.root_exists()? {
            return Ok((Vec::new(), 0));
        }
        let (mut lines, drained) = match self.read_db(|conn, _version, path| {
            let drained = docs::spooled_names(conn, path)?;
            Ok((docs::audit(conn, path)?, drained))
        })? {
            Some(found) => found,
            None => {
                self.refuse_file_store()?;
                (Vec::new(), Vec::new())
            }
        };
        let (spooled, torn) = self.spooled_audit()?;
        lines.extend(
            spooled
                .into_iter()
                .filter(|(name, _, _)| !drained.contains(name))
                .map(|(_, format, event)| (format, event)),
        );
        Ok((lines, torn))
    }
}
