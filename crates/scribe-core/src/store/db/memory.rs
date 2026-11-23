//! The memory tables of `scribe.db` (docs/design/scribe/2_design.md §15.6,
//! §15.7): the journal every memory write commits before it touches a file,
//! the profile's revisions and cursor, `now_pending`, `memory_history` and
//! the pass's signals.
//!
//! A memory write commits one transaction ([`commit`]) holding its rows and
//! one `memory_journal` row listing its file operations with their full
//! content; the store then applies them and [`close`]s the journal. The next
//! write replays a journal left open by a crash. Reads ([`memory_view`],
//! [`revision_at`]) open read-only and never replay.

use super::{DB_FILE, Fallible, db_error, inspect, open_reader, refuse_newer, stamp, write_error};
use crate::error::{Error, Result};
use crate::memory::Pending;
use crate::session::SessionId;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;
use time::OffsetDateTime;

/// One file operation of a journal: write `path` (relative to `memory/`) to
/// `content`, or remove it when `content` is `None`. Applying one twice
/// leaves the same file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct FileOp {
    /// The path under `memory/`: `profile.md`, `now/<stem>.md` or
    /// `milestones/<stem>.md`.
    pub path: String,
    /// The file's whole content; `None` removes it.
    pub content: Option<String>,
}

/// One `memory_history` row: a removed item's text and sources, kept so
/// `ask` and search still recall it; also the hints a migration leaves for
/// the first refresh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct HistoryRow {
    /// The item's id: a file stem, or `m-<n>`.
    pub item: String,
    /// `now`, `milestones`, `profile`, `projects` or `working`.
    pub kind: String,
    /// Its text.
    pub text: String,
    /// The sessions it came from.
    pub sources: Vec<String>,
    /// When it left memory.
    pub removed_at: OffsetDateTime,
}

/// One undated `now` candidate to keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct PendingRow {
    /// Its session.
    pub session: SessionId,
    /// Its turn.
    pub message: String,
    /// What it is about.
    pub subject: String,
    /// Its prose.
    pub text: String,
    /// When the pass kept it.
    pub created: OffsetDateTime,
}

/// Everything one memory write commits before it touches a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::store) struct MemoryCommit {
    /// The file operations, in order.
    pub ops: Vec<FileOp>,
    /// The history rows to add.
    pub history: Vec<HistoryRow>,
    /// The items whose history rows a forget deletes.
    pub forget_history: Vec<String>,
    /// Messages an item cites: item (file stem or `m-<n>`), session and
    /// message id.
    pub links: Vec<(String, String, String)>,
    /// Links to copy from one item to another (a `now` item promoted to a
    /// milestone): from, to.
    pub link_copies: Vec<(String, String)>,
    /// Items a forget erases: the messages they cite are deleted, with the
    /// links, leaving no trace of their text (§15.7).
    pub forget_messages: Vec<String>,
    /// A new profile revision's content; its file op is in `ops`.
    pub revision: Option<String>,
    /// The profile job's new cursor: the last session read, by its last
    /// message's time and id.
    pub cursor: Option<(OffsetDateTime, SessionId)>,
    /// Undated candidates to keep.
    pub pending_add: Vec<PendingRow>,
    /// Pending candidates to drop by id.
    pub pending_drop: Vec<i64>,
    /// A session whose pending candidates all go (its final pass).
    pub pending_clear: Option<SessionId>,
    /// The pass's signal for a window: session, the window's last message
    /// id, and 0–3.
    pub signals: Vec<(SessionId, String, u8)>,
}

impl MemoryCommit {
    /// Whether it changes nothing.
    pub(in crate::store) fn is_empty(&self) -> bool {
        self.ops.is_empty()
            && self.history.is_empty()
            && self.forget_history.is_empty()
            && self.links.is_empty()
            && self.link_copies.is_empty()
            && self.forget_messages.is_empty()
            && self.revision.is_none()
            && self.cursor.is_none()
            && self.pending_add.is_empty()
            && self.pending_drop.is_empty()
            && self.pending_clear.is_none()
            && self.signals.is_empty()
    }
}

/// A committed journal row whose file operations may not all be applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct Journal {
    /// Its row id.
    pub id: i64,
    /// Its file operations, in order.
    pub ops: Vec<FileOp>,
    /// The profile revision its `profile.md` write renders.
    pub revision: Option<i64>,
}

/// One profile revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct Revision {
    /// Its number, from 1.
    pub n: i64,
    /// When it was committed.
    pub created: OffsetDateTime,
    /// The profile's text.
    pub content: String,
    /// The sha256 of the file once written; `None` while its journal is
    /// open.
    pub file_sha256: Option<String>,
}

/// What the memory reads need from the database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::store) struct MemoryDb {
    /// The newest profile revision.
    pub latest: Option<Revision>,
    /// The newest revision whose journal closed.
    pub closed: Option<Revision>,
    /// Whether a journal is open: the files may not match the database.
    pub open_journal: bool,
    /// The database was opened as a read-only copy.
    pub read_only_copy: bool,
}

fn parse_time(path: &Path, text: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|e| Error::corrupt(path, e))
}

fn revision_row(path: &Path, row: (i64, String, String, Option<String>)) -> Result<Revision> {
    Ok(Revision {
        n: row.0,
        created: parse_time(path, &row.1)?,
        content: row.2,
        file_sha256: row.3,
    })
}

fn has_journal(conn: &Connection) -> rusqlite::Result<bool> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(version >= 2)
}

const SELECT_REVISION: &str = "SELECT n, created, content, file_sha256 FROM profile_revisions";

fn read_view(conn: &Connection, path: &Path) -> Result<MemoryDb, Fallible> {
    let fail = |e| Fallible::from_sqlite("read", path, e);
    let version = refuse_newer(conn, path)?;
    if version == 0 {
        return Ok(MemoryDb::default());
    }
    let one = |sql: &str| -> Result<Option<Revision>, Fallible> {
        conn.query_row(sql, [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()
        .map_err(fail)?
        .map(|row| revision_row(path, row))
        .transpose()
        .map_err(Fallible::Failed)
    };
    let latest = one(&format!("{SELECT_REVISION} ORDER BY n DESC LIMIT 1"))?;
    let closed = one(&format!(
        "{SELECT_REVISION} WHERE file_sha256 IS NOT NULL ORDER BY n DESC LIMIT 1"
    ))?;
    let open_journal = version >= 2
        && conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM memory_journal WHERE done = 0)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(fail)?;
    Ok(MemoryDb {
        latest,
        closed,
        open_journal,
        read_only_copy: false,
    })
}

/// Read `f` from the database under `root` read-only, retrying as a
/// read-only copy (`immutable=1`) when the WAL cannot be opened. A missing
/// database reads as `T::default()`.
pub(super) fn read<T: Default>(
    root: &Path,
    f: impl Fn(&Connection, &Path) -> Result<T, Fallible>,
) -> Result<(T, bool)> {
    let path = root.join(DB_FILE);
    // Inside a store write on this thread, read what it has written so far.
    if let Some(conn) = super::scope::active(root) {
        return Ok((f(&conn, &path).map_err(Fallible::into_error)?, false));
    }
    if !inspect(root)? {
        return Ok((T::default(), false));
    }
    match open_reader(&path, false).and_then(|conn| f(&conn, &path)) {
        Ok(value) => Ok((value, false)),
        Err(Fallible::NeedsImmutable(_)) => {
            let conn = open_reader(&path, true).map_err(Fallible::into_error)?;
            Ok((f(&conn, &path).map_err(Fallible::into_error)?, true))
        }
        Err(Fallible::Failed(e)) => Err(e),
    }
}

/// The profile revisions and journal state, read-only.
pub(in crate::store) fn memory_view(root: &Path) -> Result<MemoryDb> {
    let (mut view, copy) = read(root, read_view)?;
    view.read_only_copy = copy;
    Ok(view)
}

/// Revision `n`, read-only.
pub(in crate::store) fn revision_at(root: &Path, n: i64) -> Result<Option<Revision>> {
    let (found, _) = read(root, |conn, path| {
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| Fallible::from_sqlite("read", path, e))?;
        if version == 0 {
            return Ok(None);
        }
        conn.query_row(&format!("{SELECT_REVISION} WHERE n = ?1"), [n], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()
        .map_err(|e| Fallible::from_sqlite("read", path, e))?
        .map(|row| revision_row(path, row))
        .transpose()
        .map_err(Fallible::Failed)
    })?;
    Ok(found)
}

/// The pending candidates of `session`, read-only; none without a
/// database or before schema 1.
pub(in crate::store) fn pending_read(root: &Path, session: &SessionId) -> Result<Vec<Pending>> {
    let (found, _) = read(root, |conn, path| {
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| Fallible::from_sqlite("read", path, e))?;
        if version == 0 {
            return Ok(Vec::new());
        }
        pending_rows(conn, session).map_err(|e| Fallible::Failed(db_error("read", path, e)))
    })?;
    Ok(found)
}

fn pending_rows(conn: &Connection, session: &SessionId) -> rusqlite::Result<Vec<Pending>> {
    let mut select = conn.prepare(
        "SELECT id, message, subject, text FROM now_pending WHERE session = ?1 ORDER BY id",
    )?;
    let rows = select.query_map([session.as_str()], |row| {
        Ok(Pending {
            id: row.get(0)?,
            session: session.clone(),
            message: row.get(1)?,
            subject: row.get(2)?,
            text: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// The writer's view of the memory tables, on an open writer connection.
pub(in crate::store) struct Writer<'a> {
    conn: &'a Connection,
    path: std::path::PathBuf,
}

impl<'a> Writer<'a> {
    /// Wrap a writer connection for the database under `root`.
    pub(in crate::store) fn new(conn: &'a Connection, root: &Path) -> Self {
        Self {
            conn,
            path: root.join(DB_FILE),
        }
    }

    fn err(&self, e: rusqlite::Error) -> Error {
        db_error("write memory to", &self.path, e)
    }

    /// Every journal left open, oldest first.
    pub(in crate::store) fn open_journals(&self) -> Result<Vec<Journal>> {
        if !has_journal(self.conn).map_err(|e| self.err(e))? {
            return Ok(Vec::new());
        }
        let mut select = self
            .conn
            .prepare("SELECT id, ops, revision FROM memory_journal WHERE done = 0 ORDER BY id")
            .map_err(|e| self.err(e))?;
        let rows = select
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(|e| self.err(e))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, ops, revision) = row.map_err(|e| self.err(e))?;
            let ops: Vec<FileOp> =
                serde_json::from_str(&ops).map_err(|e| Error::corrupt(&self.path, e))?;
            out.push(Journal { id, ops, revision });
        }
        Ok(out)
    }

    /// Remove the session's memory links and history sources. History with
    /// no source left must not supply text or a migration hint.
    pub(in crate::store) fn forget_session(&self, session: &SessionId) -> Result<bool> {
        let fail = |e| self.err(e);
        let id = session.as_str();
        let mut changed = self
            .conn
            .execute("DELETE FROM memory_messages WHERE session = ?1", [id])
            .map_err(fail)?;
        changed += self.conn.execute(
            "DELETE FROM memory_history WHERE EXISTS (SELECT 1 FROM json_each(sources) WHERE value = ?1)
             AND NOT EXISTS (SELECT 1 FROM json_each(sources) WHERE value <> ?1)",
            [id],
        ).map_err(fail)?;
        changed += self
            .conn
            .execute(
                "UPDATE memory_history SET sources = (SELECT json_group_array(value)
                FROM json_each(sources) WHERE value <> ?1)
             WHERE EXISTS (SELECT 1 FROM json_each(sources) WHERE value = ?1)",
                [id],
            )
            .map_err(fail)?;
        Ok(changed > 0)
    }

    /// Older journal operations are copies of affected files. A new forget
    /// operation takes their place during replay, including across a crash.
    /// The caller plans from the latest state after all open operations,
    /// so an older open operation carrying the erased source already has
    /// either a replacement or a later operation shadowing it.
    pub(in crate::store) fn scrub_journal(
        &self,
        replace: Option<&Journal>,
        contains_source: impl Fn(&FileOp) -> Result<bool>,
    ) -> Result<bool> {
        let fail = |e| self.err(e);
        let mut select = self
            .conn
            .prepare("SELECT id, ops FROM memory_journal WHERE ops <> '[]'")
            .map_err(fail)?;
        let rows = select
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(fail)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(fail)?;
        let mut changed = false;
        for (id, raw) in rows {
            if replace.is_some_and(|j| j.id == id) {
                continue;
            }
            let ops: Vec<FileOp> =
                serde_json::from_str(&raw).map_err(|e| Error::corrupt(&self.path, e))?;
            let mut keep = Vec::with_capacity(ops.len());
            for op in &ops {
                let superseded =
                    replace.is_some_and(|j| j.ops.iter().any(|new| new.path == op.path));
                if !(superseded || contains_source(op)?) {
                    keep.push(op);
                }
            }
            if keep.len() != ops.len() {
                let raw =
                    serde_json::to_string(&keep).map_err(|e| Error::corrupt(&self.path, e))?;
                self.conn
                    .execute(
                        "UPDATE memory_journal SET ops = ?2 WHERE id = ?1",
                        (id, raw),
                    )
                    .map_err(fail)?;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Commit `commit` in one immediate transaction: its rows, and a journal
    /// row when it has file operations, which this returns.
    pub(in crate::store) fn commit(
        &mut self,
        commit: &MemoryCommit,
        now: OffsetDateTime,
    ) -> Result<Option<Journal>> {
        let path = self.path.clone();
        let tx = super::scope::Tx::begin(self.conn, &path, "write memory to")?;
        let started = tx.started();
        let (journal, erased) = stage(&tx, &path, started, commit, now)?;
        tx.commit()?;
        if erased > 0 {
            super::truncate_wal(self.conn, &path, started)?;
        }
        Ok(journal)
    }

    /// Close a journal whose operations are applied: `done`, and the
    /// written profile's sha256 on its revision.
    pub(in crate::store) fn close(
        &mut self,
        journal: &Journal,
        file_sha256: Option<&str>,
    ) -> Result<()> {
        let path = self.path.clone();
        let tx = super::scope::Tx::begin(self.conn, &path, "close the memory journal in")?;
        let fail = |e| tx.fail(e);
        tx.execute(
            "UPDATE memory_journal SET done = 1 WHERE id = ?1",
            [journal.id],
        )
        .map_err(fail)?;
        if let (Some(n), Some(sha)) = (journal.revision, file_sha256) {
            tx.execute(
                "UPDATE profile_revisions SET file_sha256 = ?2 WHERE n = ?1",
                (n, sha),
            )
            .map_err(fail)?;
        }
        tx.commit()
    }

    /// The pending candidates of `session`, oldest first.
    pub(in crate::store) fn pending(&self, session: &SessionId) -> Result<Vec<Pending>> {
        pending_rows(self.conn, session).map_err(|e| self.err(e))
    }

    /// The newest revision and the newest closed one.
    pub(in crate::store) fn revisions(&self) -> Result<(Option<Revision>, Option<Revision>)> {
        let view = read_view(self.conn, &self.path).map_err(Fallible::into_error)?;
        Ok((view.latest, view.closed))
    }

    /// Revision `n`.
    pub(in crate::store) fn revision(&self, n: i64) -> Result<Option<Revision>> {
        self.conn
            .query_row(&format!("{SELECT_REVISION} WHERE n = ?1"), [n], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(|e| self.err(e))?
            .map(|row| revision_row(&self.path, row))
            .transpose()
    }

    /// The profile job's cursor.
    pub(in crate::store) fn cursor(&self) -> Result<Option<(OffsetDateTime, String)>> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT through_at, through_session FROM profile_cursor WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| self.err(e))?;
        row.map(|(at, session)| Ok((parse_time(&self.path, &at)?, session)))
            .transpose()
    }

    /// Each session's signal, summed over its windows.
    pub(in crate::store) fn signals(&self) -> Result<BTreeMap<String, i64>> {
        let mut select = self
            .conn
            .prepare("SELECT session, sum(signal) FROM pass_signals GROUP BY session")
            .map_err(|e| self.err(e))?;
        let rows = select
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| self.err(e))?;
        rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(|e| self.err(e))
    }

    /// `session`'s stored user turns, oldest first, joined by blank lines.
    /// Turns kept only on group consent (`group_only`) are left out: a
    /// derived profile would outlive the group.
    pub(in crate::store) fn user_text(&self, session: &str) -> Result<String> {
        let mut select = self
            .conn
            .prepare(
                "SELECT text FROM messages
                 WHERE session = ?1 AND role = 'user' AND group_only = 0 ORDER BY at, id",
            )
            .map_err(|e| self.err(e))?;
        let rows = select
            .query_map([session], |row| row.get::<_, String>(0))
            .map_err(|e| self.err(e))?;
        let texts = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| self.err(e))?;
        Ok(texts.join("\n\n"))
    }

    /// The history rows of `kinds`, oldest first.
    pub(in crate::store) fn history(&self, kinds: &[&str]) -> Result<Vec<HistoryRow>> {
        let mut select = self
            .conn
            .prepare("SELECT item, kind, text, sources, removed_at FROM memory_history ORDER BY id")
            .map_err(|e| self.err(e))?;
        let rows = select
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| self.err(e))?;
        let mut out = Vec::new();
        for row in rows {
            let (item, kind, text, sources, removed_at) = row.map_err(|e| self.err(e))?;
            if !kinds.contains(&kind.as_str()) {
                continue;
            }
            out.push(HistoryRow {
                item,
                kind,
                text,
                sources: serde_json::from_str(&sources)
                    .map_err(|e| Error::corrupt(&self.path, e))?,
                removed_at: parse_time(&self.path, &removed_at)?,
            });
        }
        Ok(out)
    }
}

/// Stage `commit` in a transaction the caller opened (`conn`): its rows,
/// and a journal row when it has file operations. Returns the journal and
/// how many messages a forget erased; after committing, the caller
/// truncates the WAL when that is more than none.
pub(super) fn stage(
    conn: &Connection,
    path: &Path,
    started: Instant,
    commit: &MemoryCommit,
    now: OffsetDateTime,
) -> Result<(Option<Journal>, usize)> {
    let fail = |e| write_error("write memory to", path, started, e);
    let revision = match &commit.revision {
        Some(content) => {
            conn.execute(
                    "INSERT INTO profile_revisions (created, content, file_sha256) VALUES (?1, ?2, NULL)",
                    (stamp(now), content),
                )
                .map_err(fail)?;
            Some(conn.last_insert_rowid())
        }
        None => None,
    };
    if let Some((at, session)) = &commit.cursor {
        conn.execute(
            "INSERT INTO profile_cursor (id, through_at, through_session) VALUES (1, ?1, ?2)
                 ON CONFLICT (id) DO UPDATE SET through_at = excluded.through_at,
                     through_session = excluded.through_session",
            (stamp(*at), session.as_str()),
        )
        .map_err(fail)?;
    }
    for item in &commit.forget_history {
        conn.execute("DELETE FROM memory_history WHERE item = ?1", [item])
            .map_err(fail)?;
    }
    for (item, session, message) in &commit.links {
        conn.execute(
            "INSERT INTO memory_messages (item, session, message) VALUES (?1, ?2, ?3)
                 ON CONFLICT DO NOTHING",
            (item, session, message),
        )
        .map_err(fail)?;
    }
    for (from, to) in &commit.link_copies {
        conn.execute(
            "INSERT INTO memory_messages (item, session, message)
                 SELECT ?2, session, message FROM memory_messages WHERE item = ?1
                 ON CONFLICT DO NOTHING",
            (from, to),
        )
        .map_err(fail)?;
    }
    let mut erased = 0;
    for item in &commit.forget_messages {
        erased += conn
            .execute(
                "DELETE FROM messages WHERE EXISTS (SELECT 1 FROM memory_messages l
                         WHERE l.item = ?1 AND l.session = messages.session
                         AND l.message = messages.message)",
                [item],
            )
            .map_err(fail)?;
        conn.execute("DELETE FROM memory_messages WHERE item = ?1", [item])
            .map_err(fail)?;
    }
    if erased > 0 {
        conn.execute(
            "INSERT INTO messages_fts (messages_fts) VALUES ('optimize')",
            [],
        )
        .map_err(fail)?;
    }
    for row in &commit.history {
        let sources = serde_json::to_string(&row.sources).map_err(|e| Error::corrupt(path, e))?;
        conn.execute(
            "INSERT INTO memory_history (item, kind, text, sources, removed_at)
                 SELECT ?1, ?2, ?3, ?4, ?5
                 WHERE NOT EXISTS (SELECT 1 FROM memory_history
                     WHERE item = ?1 AND kind = ?2 AND text = ?3)",
            (
                &row.item,
                &row.kind,
                &row.text,
                &sources,
                stamp(row.removed_at),
            ),
        )
        .map_err(fail)?;
    }
    if let Some(session) = &commit.pending_clear {
        conn.execute(
            "DELETE FROM now_pending WHERE session = ?1",
            [session.as_str()],
        )
        .map_err(fail)?;
    }
    for id in &commit.pending_drop {
        conn.execute("DELETE FROM now_pending WHERE id = ?1", [id])
            .map_err(fail)?;
    }
    for row in &commit.pending_add {
        conn.execute(
            "INSERT INTO now_pending (session, message, subject, text, created)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (session, subject) DO NOTHING",
            (
                row.session.as_str(),
                &row.message,
                &row.subject,
                &row.text,
                stamp(row.created),
            ),
        )
        .map_err(fail)?;
    }
    for (session, through, signal) in &commit.signals {
        conn.execute(
            "INSERT INTO pass_signals (session, through, signal) VALUES (?1, ?2, ?3)
                 ON CONFLICT (session, through) DO UPDATE SET signal = excluded.signal",
            (session.as_str(), through, i64::from(*signal)),
        )
        .map_err(fail)?;
    }
    let journal = if commit.ops.is_empty() {
        None
    } else {
        let ops = serde_json::to_string(&commit.ops).map_err(|e| Error::corrupt(path, e))?;
        conn.execute(
            "INSERT INTO memory_journal (created, ops, done, revision) VALUES (?1, ?2, 0, ?3)",
            (stamp(now), &ops, revision),
        )
        .map_err(fail)?;
        Some(Journal {
            id: conn.last_insert_rowid(),
            ops: commit.ops.clone(),
            revision,
        })
    };
    Ok((journal, erased))
}
