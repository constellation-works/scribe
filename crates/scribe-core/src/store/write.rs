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
//! - **The move.** The first write moves every record kind still in files
//!   into the database in its own transaction ([`Store::import_at_write`]),
//!   checking the row count of each kind. A write that refuses rolls the
//!   move back with it. After the commit the files go to
//!   `pre-sqlite-<date>/`, which scribe never deletes, and each JSON
//!   document a format-26 build reads is left as an empty stamp at the
//!   current format, so that build refuses the store instead of writing
//!   files beside the database (STD-03 R10).
//! - **After the commit.** File work that must follow the commit runs
//!   there: moving files aside, the memory journal's files (§15.6), issue
//!   files moved aside, spooled audit lines removed, and the WAL emptied
//!   after a forget. A crash before it leaves work the next write finds
//!   and finishes.
//! - **Audit lines that cannot wait.** A line written outside a write's
//!   transaction is inserted without waiting. When another writer holds the
//!   database, or the store is still files, it is spooled to one file of
//!   its own under `audit-spool/`, which every reader merges and the next
//!   write drains into the table. So a command refused for a locked store
//!   waits once, and its line is kept.

use super::agent::refusal;
use super::audit::{self, AuditEvent};
use super::db::{self, Conn as Connection, DB_FILE, docs, scope};
use super::{Store, fsio};
use crate::WriteRefusal;
use crate::error::{Error, Result};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;
use time::OffsetDateTime;

/// A spooled audit line: its file name, its format and the event.
pub(super) type Spooled = (String, u32, AuditEvent);

/// Spooled audit lines, one file each, under the root.
pub(super) const AUDIT_SPOOL_DIR: &str = "audit-spool";

/// What a write leaves to do once it commits.
#[derive(Debug, Default)]
pub(super) struct After {
    /// Files to rename, in order: issue records moved aside.
    pub(super) renames: Vec<(PathBuf, PathBuf)>,
    /// Apply the memory journals left open.
    pub(super) journals: bool,
    /// Move files that now live in the database aside, and stamp.
    pub(super) aside: bool,
    /// Rewrite the format stamps.
    pub(super) stamp: bool,
    /// Spool files drained into the table.
    pub(super) spooled: Vec<PathBuf>,
}

/// Which move a write makes: its own, or `store migrate`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Move {
    /// At a write: never refused, so hooks keep working; a rebuild
    /// checkpoint becomes a row.
    AtWrite,
    /// `store migrate`: refused while a session is on, while a rebuild
    /// checkpoint waits, or once every kind moved.
    ByHand,
}

/// The write transaction of this thread, until its command ends.
struct Pending {
    root: PathBuf,
    mode: Move,
    /// When the write began: it dates the move and `pre-sqlite-<date>/`.
    now: OffsetDateTime,
    open: Option<Open>,
    after: After,
    moved: Option<super::Moved>,
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
        self.transaction_as(Move::AtWrite, OffsetDateTime::now_utc(), f)
    }

    /// [`Store::transaction`] making `mode`'s move, dated `now`.
    pub(super) fn transaction_as<T, E: From<Error>>(
        &self,
        mode: Move,
        now: OffsetDateTime,
        f: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        if self.in_write() {
            if mode == Move::ByHand {
                // `store migrate`'s refusals run at its own move; inside
                // another write that move has already been made.
                return Err(Error::corrupt(
                    self.root.join(DB_FILE),
                    "store migrate runs as a transaction of its own, not inside another write",
                )
                .into());
            }
            return f();
        }
        let mut restore = Restore {
            previous: Some(PENDING.with(|p| {
                p.replace(Some(Pending {
                    root: self.root.clone(),
                    mode,
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
            return out;
        };
        let Some(open) = pending.open else {
            return out;
        };
        let path = self.root.join(DB_FILE);
        match out {
            Ok(value) => {
                let committed = scope::commit_outer(&open.conn, &path);
                let truncate = open.scope.leave();
                committed?;
                self.after_commit(pending.after, pending.now, truncate)?;
                Ok(value)
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
    pub(super) fn write<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction(|| {
            let conn = self.begin()?;
            let step = scope::Tx::begin(&conn, &self.root.join(DB_FILE), "write to")?;
            let out = f()?;
            step.commit()?;
            Ok(out)
        })
    }

    /// Whether a write transaction is pending on this thread for this
    /// store (begun or not).
    pub(super) fn in_write(&self) -> bool {
        PENDING.with(|p| p.borrow().as_ref().is_some_and(|p| p.root == self.root))
    }

    /// The connection of the transaction under way, if it began.
    fn open_conn(&self) -> Option<Rc<Connection>> {
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
            Some((pending.mode, pending.now))
        });
        let Some((mode, now)) = begun else {
            // Not reachable: `write` always runs inside `transaction`.
            scope::rollback_outer(&conn);
            return Err(Error::corrupt(
                path,
                "a store write began outside its transaction",
            ));
        };
        self.check_stamp()?;
        let moved = self.import_at_write(mode, now)?;
        PENDING.with(|p| {
            if let Some(pending) = p.borrow_mut().as_mut() {
                pending.moved = Some(moved);
            }
        });
        Ok(conn)
    }

    /// Note work for after the commit of the pending write; `false` when
    /// there is none, so the caller does it now.
    pub(super) fn after(&self, f: impl FnOnce(&mut After)) -> bool {
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
    pub(super) fn moved_now(&self) -> Option<super::Moved> {
        PENDING.with(|p| {
            p.borrow()
                .as_ref()
                .filter(|p| p.root == self.root)
                .and_then(|p| p.moved.clone())
        })
    }

    /// Run `f` on the connection of the write under way, beginning it if
    /// needed: the document writers' way in.
    pub(super) fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection, &Path) -> Result<T>,
    ) -> Result<T> {
        self.write(|| {
            let conn = self.begin()?;
            f(&conn, &self.root.join(DB_FILE))
        })
    }

    /// The file work a commit leaves: issue files moved aside, the files
    /// that moved in moved aside and the stamps written, spooled audit
    /// lines removed, open memory journals applied, and the WAL emptied
    /// after a forget.
    fn after_commit(&self, after: After, now: OffsetDateTime, truncate: bool) -> Result<()> {
        for (from, to) in &after.renames {
            if let Some(dir) = to.parent() {
                self.prepare_subdir(dir)?;
            }
            fsio::rename(from, to)?;
        }
        if after.aside {
            self.move_files_aside(now)?;
        } else if after.stamp {
            self.stamp_format()?;
        }
        for spooled in &after.spooled {
            fsio::remove_private(spooled)?;
        }
        if after.journals {
            self.apply_open_journals()?;
        }
        if truncate {
            let path = self.root.join(DB_FILE);
            let conn = db::open_new_writer(&self.root, self.lock_wait)?;
            db::truncate_wal(&conn, &path, std::time::Instant::now())?;
        }
        Ok(())
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
    pub fn record_audit(&self, event: &AuditEvent) -> Result<()> {
        if self.open_conn().is_some() {
            return self.with_conn(|conn, path| {
                docs::put_audit(conn, path, audit::AUDIT_FORMAT, event, None, |e| {
                    db::db_error("audit to", path, e)
                })
                .map(drop)
            });
        }
        self.prepare_dir(&self.root)?;
        let moved = match self.documents_moved() {
            Ok(moved) => moved,
            // A store a newer build migrated takes no write from this one,
            // its log included; the command already reported the refusal.
            Err(Error::NewerFormat { .. }) => return Ok(()),
            Err(other) => return Err(other),
        };
        if moved {
            match self.audit_without_waiting(event) {
                Ok(()) => return Ok(()),
                Err(Error::Locked { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        self.spool_audit(event)
    }

    /// Insert `event` in a transaction of its own that waits for no other
    /// writer.
    fn audit_without_waiting(&self, event: &AuditEvent) -> Result<()> {
        let path = self.root.join(DB_FILE);
        let conn = db::open_new_writer(&self.root, Duration::ZERO)?;
        let tx = scope::Tx::begin(&conn, &path, "audit to")?;
        docs::put_audit(&tx, &path, audit::AUDIT_FORMAT, event, None, |e| tx.fail(e))?;
        tx.commit()
    }

    /// Write `event` to `audit-spool/<time>-<pid>-<n>.json`.
    fn spool_audit(&self, event: &AuditEvent) -> Result<()> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        self.prepare_subdir(&dir)?;
        let line = audit::encode_line(event)
            .map_err(|e| Error::corrupt(&dir, format!("an audit line does not encode: {e}")))?;
        let name = format!(
            "{:020}-{}-{}.json",
            OffsetDateTime::now_utc().unix_timestamp_nanos().max(0),
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        fsio::create_new_private(&dir.join(name), &line)
    }

    /// The spooled audit lines, by file name, with how many did not parse.
    /// Reads only.
    pub(super) fn spooled_audit(&self) -> Result<(Vec<Spooled>, usize)> {
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        if !fsio::inspect_subdir(&dir)? {
            return Ok((Vec::new(), 0));
        }
        let mut names: Vec<String> = fsio::list_dir(&dir)?
            .into_iter()
            .filter(|n| !n.starts_with('.') && n.ends_with(".json"))
            .collect();
        names.sort();
        let mut out = Vec::new();
        let mut torn = 0;
        for name in names {
            let path = dir.join(&name);
            if !fsio::inspect_file(&path)? {
                continue;
            }
            let (mut lines, skipped) = audit::decode_formatted(&path, &fsio::read(&path)?)?;
            torn += skipped;
            if let Some((format, event)) = lines.pop() {
                out.push((name, format, event));
            }
        }
        Ok((out, torn))
    }

    /// Drain the spool into the table inside the write under way; the files
    /// are removed after the commit.
    pub(super) fn drain_spool(&self, conn: &Connection, path: &Path) -> Result<()> {
        let (lines, _) = self.spooled_audit()?;
        let dir = self.root.join(AUDIT_SPOOL_DIR);
        for (name, format, event) in lines {
            docs::put_audit(conn, path, format, &event, Some(&name), |e| {
                db::db_error("audit to", path, e)
            })?;
            let file = dir.join(&name);
            self.after(|a| a.spooled.push(file));
        }
        Ok(())
    }

    /// Refuse `store migrate` before anything moves: once every kind is in
    /// the database, while a session is on, or while a rebuild checkpoint
    /// waits.
    pub(super) fn refuse_migrate(&self, conn: &Connection) -> Result<()> {
        let sessions_moved = db::sessions::moved(conn)
            .map_err(|e| db::db_error("read", &self.root.join(DB_FILE), e))?;
        let documents_moved =
            docs::moved(conn).map_err(|e| db::db_error("read", &self.root.join(DB_FILE), e))?;
        if sessions_moved && documents_moved {
            return Err(refusal(
                WriteRefusal::MigrateBlocked,
                "the store is already in scribe.db; there is nothing to migrate",
            ));
        }
        let control = self.control()?;
        if let Some((id, _)) = control.sessions.iter().find(|(_, c)| c.on) {
            return Err(refusal(
                WriteRefusal::MigrateBlocked,
                format!(
                    "session {id} is switched on; switch it off (/scribe off) or let its pass settle, then migrate"
                ),
            ));
        }
        if !sessions_moved && let Some((id, path)) = self.checkpoint_files()?.first() {
            return Err(refusal(
                WriteRefusal::MigrateBlocked,
                format!(
                    "session {id} has a rebuild checkpoint at {}; finish it with `scribe session rebuild {id}` or delete the file, then migrate",
                    path.display()
                ),
            ));
        }
        Ok(())
    }
}
