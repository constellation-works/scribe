//! Store writes as `scribe.db` transactions (docs/design/scribe/2_design.md
//! §15.7, phase 7), which replace the store lock and the whole-file
//! rewrite of every document.
//!
//! - **One transaction per write.** [`Store::write`] runs its step inside
//!   one `BEGIN IMMEDIATE` transaction with `busy_timeout` equal to the
//!   lock wait; a timeout is [`Error::Locked`], as the lock's was. Its
//!   reads see what it has written so far, and a step that refuses rolls
//!   back to its savepoint, so a refused write changes nothing.
//! - **One transaction per command.** [`Store::transaction`] lets the CLI
//!   run a whole command, and the audit rows it writes, as one
//!   transaction: started at the command's first write and committed when
//!   it ends, so a write and its audit row commit together. A command that
//!   calls a model does not use it, because no transaction may be held
//!   across a model call; each of its steps commits on its own.
//! - **The archive import.** The first write imports the file store's
//!   `pre-sqlite-<date>/` and `pre-issue-<date>/` archives into the archive
//!   tables in its own transaction ([`Store::import_at_write`]), checking
//!   the row count of each kind. A write that refuses rolls the import back
//!   with it. Each JSON document a format-26 build reads is kept as an
//!   empty stamp at the current format, so that build refuses the store
//!   instead of writing files beside the database (STD-03 R10).
//! - **After the commit.** File work that must follow the commit runs
//!   there: the imported archive directories removed, the stamps written,
//!   the memory journal's files (§15.6), spooled audit lines removed and
//!   the WAL emptied after a forget. A crash before it leaves work the next
//!   write finds and finishes: an archive already imported is only
//!   removed.
//! - **Audit lines that cannot wait.** A line written outside a write's
//!   transaction is inserted without waiting. When another writer holds the
//!   database, or the store is still files, it is spooled to one file of
//!   its own under `audit-spool/`, which every reader merges and the next
//!   write drains into the table. So a command refused for a locked store
//!   waits once, and its line is kept.

use crate::db::files;
use crate::db::{self, Conn as Connection, DB_FILE, docs, scope};
use crate::error::{Error, Result};
use crate::repository::audit::{self, AuditEvent};
use crate::session::{ControlState, SessionId};
use crate::{Imported, Store};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use time::OffsetDateTime;

/// A spooled audit line: its file name, its format and the event.
pub(crate) type Spooled = (String, u32, AuditEvent);

/// Spooled audit lines, one file each, under the root.
pub const AUDIT_SPOOL_DIR: &str = "audit-spool";

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    /// Test-only synchronization immediately before the spool is listed.
    pub static BEFORE_SPOOL_DRAIN: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

#[cfg(test)]
thread_local! {
    /// Tests: fail the next write after its transaction commits and before
    /// the imported archives are removed, as an interruption there would.
    pub(crate) static CRASH_BEFORE_REMOVAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// What a write leaves to do once it commits.
#[derive(Debug, Default)]
pub(crate) struct After {
    /// Apply the memory journals left open.
    pub(crate) journals: bool,
    /// Archive directories (and leftover files) whose records are now in
    /// the archive tables, to remove.
    pub(crate) archive_removals: Vec<PathBuf>,
    /// Rewrite the format stamps.
    pub(crate) stamp: bool,
    /// Spool files drained into the table.
    pub(crate) spooled: Vec<PathBuf>,
}

/// Which move a write makes: its own, or `store migrate`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Move {
    /// At a write: never refused, so hooks keep working; a rebuild
    /// checkpoint becomes a row.
    AtWrite,
    /// `store migrate`: refused while a session is on, while a rebuild
    /// checkpoint file waits, or when there is no archive to import.
    ByHand,
}

/// The check `store migrate` runs before its import, inside its write
/// ([`Store::import_file_store`]).
pub(crate) type ImportCheck = Box<dyn FnOnce(&ImportFacts<'_>) -> Result<()>>;

/// What `store migrate`'s check reads, inside its write and before
/// anything is imported. Each read is made when asked for.
pub struct ImportFacts<'a> {
    pub(crate) store: &'a Store,
    pub(crate) conn: &'a Connection,
    pub(crate) leftovers: &'a [String],
}

impl ImportFacts<'_> {
    /// Whether anything is left to import: items only a file store holds,
    /// or a `pre-sqlite-*`/`pre-issue-*` archive not yet in the tables.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn pending(&self) -> Result<bool> {
        let db = self.store.root.join(DB_FILE);
        let mut pending = !self.leftovers.is_empty();
        for name in self.store.archive_dirs()? {
            pending |= !db::archive::imported(self.conn, &db, &name)?;
        }
        Ok(pending)
    }

    /// The sessions' switches.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn control(&self) -> Result<ControlState> {
        self.store.control()
    }

    /// Every rebuild checkpoint file of a file store, by session.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn checkpoint_files(&self) -> Result<Vec<(SessionId, PathBuf)>> {
        self.store.checkpoint_files()
    }
}

/// The write transaction of this thread, until its command ends.
struct Pending {
    root: PathBuf,
    mode: Move,
    /// `store migrate`'s check, until its move runs it.
    check: Option<ImportCheck>,
    /// When the write began: it dates the archive import.
    now: OffsetDateTime,
    open: Option<Open>,
    after: After,
    moved: Option<crate::Imported>,
}

/// The transaction, once the first write began it.
struct Open {
    conn: Rc<Connection>,
    scope: scope::Scope,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Restores the thread's previous pending write and rolls back an
/// unfinished transaction, also on a panic.
struct Restore {
    previous: Option<Option<Pending>>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        let Some(previous) = self.previous.take() else {
            return;
        };
        if let Some(Pending {
            open: Some(open), ..
        }) = PENDING.with(|p| p.replace(previous))
        {
            scope::rollback_outer(&open.conn);
            drop(open.scope);
        }
    }
}

impl Store {
    /// Run `f` as one transaction on `scribe.db`: every store write inside
    /// it joins one `BEGIN IMMEDIATE`, begun at the first of them, and
    /// commits when `f` returns `Ok`; an `Err` rolls every write back.
    /// Audit lines recorded inside commit with the writes
    /// ([`Store::record_audit`]). A call inside another joins it.
    ///
    /// No model call may run inside: the transaction holds the database's
    /// write lock until `f` returns.
    ///
    /// # Errors
    ///
    /// `f`'s error; [`Error::Locked`] when the commit waited past the lock
    /// wait; store failures from the commit or the work after it.
    pub fn transaction<T, E: From<Error>>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        self.transaction_as(Move::AtWrite, None, OffsetDateTime::now_utc(), f)
    }

    /// `scribe store migrate`'s write: import the file store's
    /// `pre-sqlite-*` and `pre-issue-*` archives into `scribe.db` in one
    /// transaction, then remove them. `check` runs inside it first, under
    /// its lock and before anything is imported, and its refusal leaves the
    /// store as it was.
    ///
    /// # Errors
    ///
    /// `check`'s refusal; `store_corrupt` for a file store whose records
    /// never moved; store failures.
    pub fn import_file_store(
        &self,
        now: OffsetDateTime,
        check: impl FnOnce(&ImportFacts<'_>) -> Result<()> + 'static,
    ) -> Result<Imported> {
        self.transaction_as(Move::ByHand, Some(Box::new(check)), now, || {
            self.with_conn(|_, _| Ok(()))?;
            Ok::<_, Error>(self.moved_now().unwrap_or_default())
        })
    }

    /// [`Store::transaction`] making `mode`'s move, dated `now`; `check`
    /// is `store migrate`'s.
    pub(crate) fn transaction_as<T, E: From<Error>>(
        &self,
        mode: Move,
        check: Option<ImportCheck>,
        now: OffsetDateTime,
        f: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let (value, cleanup) = self.transaction_with_cleanup(mode, check, now, f)?;
        cleanup?;
        Ok(value)
    }

    /// Return the transaction result and any post-commit error separately.
    /// Nested calls leave cleanup to the outer transaction.
    fn transaction_with_cleanup<T, E: From<Error>>(
        &self,
        mode: Move,
        check: Option<ImportCheck>,
        now: OffsetDateTime,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<(T, Result<()>), E> {
        if self.in_write() {
            if mode == Move::ByHand {
                let adopted = PENDING.with(|p| {
                    let mut pending = p.borrow_mut();
                    let Some(pending) = pending.as_mut().filter(|p| p.root == self.root) else {
                        return false;
                    };
                    if pending.open.is_some() {
                        return false;
                    }
                    pending.mode = Move::ByHand;
                    pending.check = check;
                    pending.now = now;
                    true
                });
                if !adopted {
                    // `store migrate`'s refusals run at its own move; inside
                    // another write that move has already been made.
                    return Err(Error::corrupt(
                        self.root.join(DB_FILE),
                        "store migrate runs as a transaction of its own, not inside another write",
                    )
                    .into());
                }
            }
            return f().map(|value| (value, Ok(())));
        }
        let mut restore = Restore {
            previous: Some(PENDING.with(|p| {
                p.replace(Some(Pending {
                    root: self.root.clone(),
                    mode,
                    check,
                    now,
                    open: None,
                    after: After::default(),
                    moved: None,
                }))
            })),
        };
        let out = f();
        let previous = restore.previous.take().unwrap_or_default();
        let Some(pending) = PENDING.with(|p| p.replace(previous)) else {
            return out.map(|value| (value, Ok(())));
        };
        let Some(open) = pending.open else {
            return out.map(|value| (value, Ok(())));
        };
        let path = self.root.join(DB_FILE);
        match out {
            Ok(value) => {
                let committed = scope::commit_outer(&open.conn, &path);
                let truncate = open.scope.leave();
                committed?;
                Ok((
                    value,
                    self.after_commit(pending.after, pending.now, truncate),
                ))
            }
            Err(e) => {
                scope::rollback_outer(&open.conn);
                drop(open.scope);
                Err(e)
            }
        }
    }

    /// Run `f` as one store write: inside the command's transaction when
    /// there is one, else in a transaction of its own. A refusal rolls back
    /// everything `f` wrote.
    pub(crate) fn write_step<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.step(f)
    }

    /// [`Store::write_step`] for any error a store error converts into: the
    /// same lock, transaction and savepoint ([`Store::write`] runs on it).
    pub(crate) fn step<T, E: From<Error>>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        self.transaction(|| {
            let conn = self.begin()?;
            let step = scope::Tx::begin(&conn, &self.root.join(DB_FILE), "write to")?;
            let out = f()?;
            step.commit()?;
            Ok(out)
        })
    }

    /// Run a write and retain its committed result even if cleanup fails.
    /// The caller must run outside a pending write to observe the commit.
    pub(crate) fn write_step_with_cleanup<T>(
        &self,
        f: impl FnOnce() -> Result<T>,
    ) -> Result<(T, Result<()>)> {
        self.transaction_with_cleanup(Move::AtWrite, None, OffsetDateTime::now_utc(), || {
            self.write_step(f)
        })
    }

    /// Whether a write transaction is pending on this thread for this
    /// store (begun or not): the transaction state [`Store::write`] keeps.
    #[must_use]
    pub fn in_write(&self) -> bool {
        PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.root == self.root))
    }

    /// The connection of the transaction under way, if it began.
    pub(crate) fn open_conn(&self) -> Option<Rc<Connection>> {
        PENDING.with(|p| {
            p.borrow()
                .as_ref()
                .filter(|p| p.root == self.root)
                .and_then(|p| p.open.as_ref().map(|o| Rc::clone(&o.conn)))
        })
    }

    /// Begin the pending transaction at its first write, moving any record
    /// kind still in files in first; or the one already begun.
    fn begin(&self) -> Result<Rc<Connection>> {
        if let Some(conn) = self.open_conn() {
            return Ok(conn);
        }
        self.prepare_dir(&self.root)?;
        let path = self.root.join(DB_FILE);
        let conn = Rc::new(db::open_new_writer(&self.root, self.lock_wait)?);
        scope::begin_outer(&conn, &path)?;
        let scope = scope::Scope::enter(&self.root, Rc::clone(&conn));
        let begun = PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            let pending = pending.as_mut().filter(|p| p.root == self.root)?;
            pending.open = Some(Open {
                conn: Rc::clone(&conn),
                scope,
            });
            Some((pending.mode, pending.check.take(), pending.now))
        });
        let Some((mode, check, now)) = begun else {
            // Not reachable: `write` always runs inside `transaction`.
            scope::rollback_outer(&conn);
            return Err(Error::corrupt(
                path,
                "a store write began outside its transaction",
            ));
        };
        self.check_stamp()?;
        let moved = self.import_at_write(mode, check, now)?;
        PENDING.with(|p| {
            if let Some(pending) = p.borrow_mut().as_mut() {
                pending.moved = Some(moved);
            }
        });
        Ok(conn)
    }

    /// Note work for after the commit of the pending write; `false` when
    /// there is none, so the caller does it now.
    pub(crate) fn after(&self, f: impl FnOnce(&mut After)) -> bool {
        PENDING.with(|p| {
            let mut pending = p.borrow_mut();
            match pending
                .as_mut()
                .filter(|p| p.root == self.root && p.open.is_some())
            {
                Some(pending) => {
                    f(&mut pending.after);
                    true
                }
                None => false,
            }
        })
    }

    /// What the pending write's move did, once it began.
    pub(crate) fn moved_now(&self) -> Option<crate::Imported> {
        PENDING.with(|p| {
            p.borrow()
                .as_ref()
                .filter(|p| p.root == self.root)
                .and_then(|p| p.moved.clone())
        })
    }

    /// Run `f` on the connection of the write under way, beginning it if
    /// needed: the document writers' way in.
    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection, &Path) -> Result<T>,
    ) -> Result<T> {
        self.write_step(|| {
            let conn = self.begin()?;
            f(&conn, &self.root.join(DB_FILE))
        })
    }

    /// The file work a commit leaves: the WAL emptied after a forget, the
    /// imported archives removed, the stamps written, spooled audit lines
    /// removed and open memory journals applied.
    fn after_commit(&self, after: After, _now: OffsetDateTime, truncate: bool) -> Result<()> {
        if truncate {
            let path = self.root.join(DB_FILE);
            let conn = db::open_new_writer(&self.root, self.lock_wait)?;
            db::truncate_wal(&conn, &path, std::time::Instant::now())?;
        }
        #[cfg(test)]
        if !after.archive_removals.is_empty() && CRASH_BEFORE_REMOVAL.with(|c| c.replace(false)) {
            return Err(Error::io(
                "remove",
                self.root.join(DB_FILE),
                std::io::Error::other("a crash injected by a test before the archives are removed"),
            ));
        }
        for path in &after.archive_removals {
            self.remove_archive(path)?;
        }
        if after.stamp || !after.archive_removals.is_empty() {
            self.stamp_format()?;
        }
        for spooled in &after.spooled {
            files::remove(&self.root, spooled)?;
        }
        if after.journals {
            self.apply_open_journals()?;
        }
        Ok(())
    }

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
        crate::repository::Audit::<crate::repository::ReadWrite>::over(self).record(event)
    }

    /// Insert `event` in a transaction of its own that waits for no other
    /// writer.
    pub(crate) fn audit_without_waiting(&self, event: &AuditEvent) -> Result<()> {
        let path = self.root.join(DB_FILE);
        let conn = db::open_new_writer(&self.root, Duration::ZERO)?;
        let tx = scope::Tx::begin(&conn, &path, "audit to")?;
        docs::put_audit(&tx, &path, audit::AUDIT_FORMAT, event, None, |e| tx.fail(e))?;
        tx.commit()
    }

    /// Write `event` to `audit-spool/<time>-<pid>-<n>.json`.
    pub(crate) fn spool_audit(&self, event: &AuditEvent) -> Result<()> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        self.prepare_subdir(&dir)?;
        let line = audit::encode_line(audit::AUDIT_FORMAT, event)
            .map_err(|e| Error::corrupt(&dir, format!("an audit line does not encode: {e}")))?;
        let name = format!(
            "{:020}-{}-{}.json",
            OffsetDateTime::now_utc().unix_timestamp_nanos().max(0),
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        cw_fs::create_new(&dir.join(name), &line)?;
        Ok(())
    }

    /// The spooled audit lines, by file name, with how many did not parse.
    /// Reads only.
    pub(crate) fn spooled_audit(&self) -> Result<(Vec<Spooled>, usize)> {
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        if !cw_fs::inspect_dir(&dir)? {
            return Ok((Vec::new(), 0));
        }
        let mut names: Vec<String> = cw_fs::list(&dir)?
            .into_iter()
            .filter(|n| !n.starts_with('.') && n.ends_with(".json"))
            .collect();
        names.sort();
        let mut out = Vec::new();
        let mut torn = 0;
        for name in names {
            let path = dir.join(&name);
            if !cw_fs::inspect_file(&path)? {
                continue;
            }
            let (mut lines, skipped) = audit::decode_formatted(&path, &files::read(&path)?)?;
            torn += skipped;
            if let Some((format, event)) = lines.pop() {
                out.push((name, format, event));
            }
        }
        Ok((out, torn))
    }

    /// Drain the spool into the table inside the write under way; the files
    /// are removed after the commit.
    pub(crate) fn drain_spool(&self, conn: &Connection, path: &Path) -> Result<usize> {
        #[cfg(any(test, feature = "test-support"))]
        BEFORE_SPOOL_DRAIN.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        let (lines, _) = self.spooled_audit()?;
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        let mut inserted = 0;
        for (name, format, event) in lines {
            if docs::put_audit(conn, path, format, &event, Some(&name), |e| {
                db::db_error("audit to", path, e)
            })? {
                inserted += 1;
            }
            let file = dir.join(&name);
            self.after(|a| a.spooled.push(file));
        }
        Ok(inserted)
    }
}
