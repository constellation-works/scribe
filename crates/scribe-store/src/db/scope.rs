//! The write transaction every store write runs in (2_design.md §15.7,
//! phase 7), in place of the store lock.
//!
//! - **One connection per write.** [`Scope`] holds the writer connection of
//!   the store write in progress on this thread. Every step of that write
//!   ([`super::open_writer`] called again, the session and memory writers,
//!   the message upserts) gets the same connection back, so it joins the
//!   write's transaction instead of waiting on it.
//! - **Nesting.** [`Tx::begin`] takes `BEGIN IMMEDIATE` on a connection with
//!   no transaction, and a `SAVEPOINT` inside one. A step that refuses rolls
//!   back to its savepoint, and only the outermost commit makes anything
//!   durable.
//! - **After the commit.** A WAL `TRUNCATE` checkpoint cannot run inside a
//!   transaction, so a forget inside one asks for it here
//!   ([`Scope::truncate_later`]) and the outermost commit runs it.
//!
//! Thread-local, so a second thread (or process) opens its own connection
//! and waits on SQLite's lock for the lock wait, as a second writer did on
//! the store lock.

use super::{db_error, write_error};
use crate::error::Result;
use cw_sqlite::Writer as SqliteWriter;
use cw_sqlite::rusqlite::{self, Connection};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

/// The writer connection of the store write in progress on this thread.
struct Active {
    root: PathBuf,
    conn: Rc<SqliteWriter>,
    truncate: bool,
}

thread_local! {
    static ACTIVE: RefCell<Option<Active>> = const { RefCell::new(None) };
}

/// The connection of the write in progress under `root` on this thread.
pub(crate) fn active(root: &Path) -> Option<Rc<SqliteWriter>> {
    ACTIVE.with(|a| {
        a.borrow()
            .as_ref()
            .filter(|a| a.root == root)
            .map(|a| Rc::clone(&a.conn))
    })
}

/// Holds `conn` as this thread's write connection for `root` until dropped.
pub(crate) struct Scope {
    previous: Option<Active>,
}

impl Scope {
    /// Make `conn` the connection every step of this write under `root`
    /// uses.
    pub(crate) fn enter(root: &Path, conn: Rc<SqliteWriter>) -> Self {
        let previous = ACTIVE.with(|a| {
            a.replace(Some(Active {
                root: root.to_path_buf(),
                conn,
                truncate: false,
            }))
        });
        Self { previous }
    }

    /// Ask the outermost commit under `root` to empty the WAL; `false` when
    /// no write is in progress there, so the caller checkpoints itself.
    pub(crate) fn truncate_later(root: &Path) -> bool {
        ACTIVE.with(|a| {
            let mut active = a.borrow_mut();
            match active.as_mut().filter(|a| a.root == root) {
                Some(a) => {
                    a.truncate = true;
                    true
                }
                None => false,
            }
        })
    }

    /// Leave the scope; `true` when a step asked for a WAL truncate.
    pub(crate) fn leave(mut self) -> bool {
        let previous = self.previous.take();
        ACTIVE.with(|a| a.replace(previous).is_some_and(|a| a.truncate))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE.with(|a| {
            a.replace(previous);
        });
    }
}

/// A transaction on a writer connection: `BEGIN IMMEDIATE` when none is
/// open, a savepoint inside one. Dropped without [`Tx::commit`], it rolls
/// back (to its savepoint).
pub(crate) struct Tx<'c> {
    conn: &'c Connection,
    nested: bool,
    done: bool,
    path: PathBuf,
    started: Instant,
    action: &'static str,
}

impl<'c> Tx<'c> {
    /// Begin. `action` names the write in a [`crate::Error::Locked`] or
    /// database error; `path` is the database.
    pub(crate) fn begin(conn: &'c Connection, path: &Path, action: &'static str) -> Result<Self> {
        let started = Instant::now();
        let nested = !conn.is_autocommit();
        let sql = if nested {
            "SAVEPOINT scribe_step"
        } else {
            "BEGIN IMMEDIATE"
        };
        conn.execute_batch(sql)
            .map_err(|e| write_error(action, path, started, e))?;
        Ok(Self {
            conn,
            nested,
            done: false,
            path: path.to_path_buf(),
            started,
            action,
        })
    }

    /// When it began, for a [`crate::Error::Locked`] wait.
    pub(crate) fn started(&self) -> Instant {
        self.started
    }

    /// A rusqlite error of this transaction as the store's error.
    pub(crate) fn fail(&self, e: rusqlite::Error) -> crate::Error {
        write_error(self.action, &self.path, self.started, e)
    }

    /// Commit, or release the savepoint into the outer transaction.
    pub(crate) fn commit(mut self) -> Result<()> {
        let sql = if self.nested {
            "RELEASE scribe_step"
        } else {
            "COMMIT"
        };
        self.done = true;
        self.conn.execute_batch(sql).map_err(|e| {
            // A failed COMMIT leaves the transaction open: roll it back so
            // the connection can be used again.
            if !self.nested {
                let _ = self.conn.execute_batch("ROLLBACK");
            }
            write_error(self.action, &self.path, self.started, e)
        })
    }
}

impl std::ops::Deref for Tx<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
    }
}

impl Drop for Tx<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let sql = if self.nested {
            "ROLLBACK TO scribe_step; RELEASE scribe_step"
        } else {
            "ROLLBACK"
        };
        if let Err(e) = self.conn.execute_batch(sql) {
            let error = db_error("roll back", &self.path, e);
            tracing::warn!(%error, "a scribe.db rollback failed");
        }
    }
}

/// Begin the outermost transaction of a store write: `BEGIN IMMEDIATE`,
/// waiting up to the lock wait for another writer.
pub(crate) fn begin_outer(conn: &Connection, path: &Path) -> Result<()> {
    let started = Instant::now();
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| write_error("write to", path, started, e))
}

/// Commit the outermost transaction; a failed commit is rolled back, so the
/// connection is never left inside one.
pub(crate) fn commit_outer(conn: &Connection, path: &Path) -> Result<()> {
    conn.execute_batch("COMMIT").map_err(|e| {
        rollback_outer(conn);
        write_error("commit to", path, Instant::now(), e)
    })
}

/// Roll the outermost transaction back. A failure is logged: dropping the
/// connection rolls back anyway.
pub(crate) fn rollback_outer(conn: &Connection) {
    if let Err(error) = conn.execute_batch("ROLLBACK") {
        tracing::debug!(%error, "a scribe.db rollback found no transaction");
    }
}
