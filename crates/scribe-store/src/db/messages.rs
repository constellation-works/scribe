//! Message rows and their full-text index.

use super::scope::Tx;
use super::{
    DB_FILE, database, db_error, from_sqlite_error, memory, open_writer, scope, sessions, stamp,
    truncate_wal, write_error,
};
use crate::Redactor;
use crate::entry::Speaker;
use crate::error::{Error, Result};
use crate::message::{MessageMatch, MessageSearch, NewMessage};
use crate::session::SessionId;
use cw_sqlite::rusqlite::{self, Connection};
use std::path::Path;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Merge the FTS5 index after a forget removes messages.
pub(crate) fn optimize_fts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO messages_fts (messages_fts) VALUES ('optimize')",
        [],
    )
    .map(|_| ())
}

/// Upsert `messages` on (session, message) in one immediate transaction,
/// each text redacted first. Returns how many rows were inserted or
/// changed: a window written again with the same text changes none.
pub(crate) fn upsert_messages(
    root: &Path,
    wait: Duration,
    messages: &[NewMessage],
    redactor: &Redactor,
) -> Result<usize> {
    let path = root.join(DB_FILE);
    let conn = open_writer(root, wait)?;
    let tx = Tx::begin(&conn, &path, "write messages to")?;
    let changed = upsert_in(&tx, &path, tx.started(), messages, redactor)?;
    tx.commit()?;
    Ok(changed)
}

/// [`upsert_messages`] inside a transaction the caller opened.
pub(crate) fn upsert_in(
    conn: &Connection,
    path: &Path,
    started: Instant,
    messages: &[NewMessage],
    redactor: &Redactor,
) -> Result<usize> {
    write_messages(
        conn,
        path,
        started,
        messages.iter(),
        redactor,
        MessageConflict::Replace,
    )
}

/// Write turns captured on group consent alone, inside a transaction the
/// caller opened. A turn already stored under the same `(session, message)`
/// is left as it is: it keeps its content and its provenance, so a pass's
/// turn is never marked group-only and a group-only turn is never rewritten
/// by a second group capture.
pub(crate) fn insert_group_only_in<'a>(
    conn: &Connection,
    path: &Path,
    started: Instant,
    messages: impl Iterator<Item = &'a NewMessage>,
    redactor: &Redactor,
) -> Result<usize> {
    write_messages(
        conn,
        path,
        started,
        messages,
        redactor,
        MessageConflict::KeepExisting,
    )
}

/// What a message write does when the `(session, message)` row exists. It
/// also fixes the row's provenance: [`Replace`](Self::Replace) writes a
/// pass's turn, [`KeepExisting`](Self::KeepExisting) a group-only one.
#[derive(Clone, Copy)]
enum MessageConflict {
    /// A pass's turn takes the row: content is replaced and group-only
    /// provenance is cleared, so a turn kept on group consent becomes
    /// session-consented content once a pass records it.
    Replace,
    /// A group-only turn never touches an existing row.
    KeepExisting,
}

fn write_messages<'a>(
    conn: &Connection,
    path: &Path,
    started: Instant,
    messages: impl Iterator<Item = &'a NewMessage>,
    redactor: &Redactor,
    on_conflict: MessageConflict,
) -> Result<usize> {
    let group_only = matches!(on_conflict, MessageConflict::KeepExisting);
    let insert = "INSERT INTO messages (session, message, role, at, text, sha256, group_only)
                  VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
    let conflict = match on_conflict {
        MessageConflict::KeepExisting => "ON CONFLICT (session, message) DO NOTHING",
        MessageConflict::Replace => {
            "ON CONFLICT (session, message) DO UPDATE SET
                 group_only = 0,
                 role = excluded.role, at = excluded.at,
                 text = excluded.text, sha256 = excluded.sha256
             WHERE messages.sha256 <> excluded.sha256
                OR messages.role <> excluded.role
                OR messages.at <> excluded.at
                OR messages.group_only = 1"
        }
    };
    let mut insert = conn
        .prepare(&format!("{insert} {conflict}"))
        .map_err(|e| db_error("write messages to", path, e))?;
    let mut changed = 0;
    let mut checked = std::collections::BTreeSet::new();
    for m in messages {
        if checked.insert(&m.session)
            && sessions::is_forgotten(conn, &m.session)
                .map_err(|e| db_error("write messages to", path, e))?
        {
            return Err(sessions::forgotten_refusal(
                &m.session,
                "nothing is written to it",
            ));
        }
        let text = redactor.message(&m.text);
        let sha256 = crate::digest::prompt_sha256(&text);
        changed += insert
            .execute((
                m.session.as_str(),
                &m.message,
                m.role.as_str(),
                stamp(m.at),
                &text,
                &sha256,
                group_only,
            ))
            .map_err(|e| write_error("write messages to", path, started, e))?;
    }
    Ok(changed)
}

/// Delete the messages keyed by `keys` (session, message id) for a forget,
/// and leave no trace of their text: `secure_delete` overwrites the freed
/// pages, an FTS5 `optimize` rewrites the index without their terms, and a
/// `TRUNCATE` checkpoint empties the WAL that held the old pages. A missing
/// database deletes nothing. Returns how many rows were deleted.
pub(crate) fn delete_messages(
    root: &Path,
    wait: Duration,
    keys: &[(SessionId, String)],
) -> Result<usize> {
    let path = root.join(DB_FILE);
    if keys.is_empty() {
        return Ok(0);
    }
    if scope::active(root).is_none() {
        let started = Instant::now();
        match database(root, wait).read() {
            Ok(_) => {}
            Err(cw_sqlite::Error::Missing { .. }) => return Ok(0),
            Err(e) => return Err(from_sqlite_error(e, started)),
        }
    }
    let conn = open_writer(root, wait)?;
    let tx = Tx::begin(&conn, &path, "forget messages in")?;
    let started = tx.started();
    let mut deleted = 0;
    {
        let mut delete = tx
            .prepare("DELETE FROM messages WHERE session = ?1 AND message = ?2")
            .map_err(|e| db_error("forget messages in", &path, e))?;
        for (session, message) in keys {
            deleted += delete
                .execute((session.as_str(), message))
                .map_err(|e| write_error("forget messages in", &path, started, e))?;
        }
    }
    if deleted > 0 {
        optimize_fts(&tx).map_err(|e| write_error("forget messages in", &path, started, e))?;
    }
    tx.commit()?;
    if deleted > 0 {
        truncate_wal(&conn, &path)?;
    }
    Ok(deleted)
}

/// Every message whose text matches one of `tokens` as a word prefix, best
/// BM25 score first. Read-only: no lock, no write; a missing database
/// matches nothing.
pub(crate) fn search_messages(
    root: &Path,
    tokens: &[String],
    group: Option<i64>,
) -> Result<MessageSearch> {
    if tokens.is_empty() {
        return Ok(MessageSearch::default());
    }
    let query = fts_query(tokens);
    let (matches, read_only_copy) = memory::read(root, |conn, version, path| {
        read_matches(conn, version, path, &query, group)
    })?;
    Ok(MessageSearch {
        matches,
        read_only_copy,
    })
}

fn read_matches(
    conn: &Connection,
    version: u32,
    path: &Path,
    query: &str,
    group: Option<i64>,
) -> Result<Vec<MessageMatch>> {
    if version == 0 || (version < 9 && group.is_some()) {
        return Ok(Vec::new());
    }
    let group_project = if version >= 11 {
        "(SELECT g.project FROM groups g JOIN group_members gm ON gm.group_id = g.id
          WHERE gm.session = m.session AND m.group_only = 1)"
    } else {
        "NULL"
    };
    let membership = if group.is_some() {
        "AND EXISTS (SELECT 1 FROM group_members gm WHERE gm.session = m.session AND gm.group_id = ?2)"
    } else {
        ""
    };
    let mut select = conn
        .prepare(&format!(
            "SELECT m.session, m.message, m.role, m.at, m.text, bm25(messages_fts), {group_project}
             FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid
             WHERE messages_fts MATCH ?1 {membership}
             ORDER BY bm25(messages_fts), m.at, m.id"
        ))
        .map_err(|e| db_error("read", path, e))?;
    let params = rusqlite::params![query, group];
    let rows = select
        .query_map(&params[..if group.is_some() { 2 } else { 1 }], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, f64>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|e| db_error("read", path, e))?;
    let mut matches = Vec::new();
    for row in rows {
        let (session, message, role, at, text, bm25, project) =
            row.map_err(|e| db_error("read", path, e))?;
        let corrupt = |what: &str| Error::corrupt(path, format!("a message row has {what}"));
        matches.push(MessageMatch {
            session: session.parse().map_err(|_| corrupt("a bad session id"))?,
            message,
            role: match role.as_str() {
                "user" => Speaker::User,
                "assistant" => Speaker::Assistant,
                _ => return Err(corrupt("an unknown role")),
            },
            at: OffsetDateTime::parse(&at, &Rfc3339).map_err(|_| corrupt("a bad time"))?,
            text,
            score: -bm25,
            group_project: project
                .map(|p| p.parse())
                .transpose()
                .map_err(|_| corrupt("a bad group project"))?,
        });
    }
    Ok(matches)
}

/// An FTS5 query: each token as a quoted prefix, any of them.
fn fts_query(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}
