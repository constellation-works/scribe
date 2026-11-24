//! The session tables of `scribe.db` (docs/design/scribe/2_design.md
//! §15.7, phase 6): one `sessions` row per session scribe knows, holding
//! its record (when it has one) and its switch (when it was ever switched
//! on); its `segments`, `session_links` and `session_terms`; the Codex
//! cursors; the rebuild checkpoints; and `record_kinds`, which says the
//! sessions have moved in from files.
//!
//! - **Ids.** A row's `id` is `INTEGER PRIMARY KEY AUTOINCREMENT`, shown
//!   as `s-<n>`, so an id is never reused. The provider's session id (the
//!   transcript UUID) is `provider_session`, unique, and stays the key
//!   every cite, message and memory link uses.
//! - **Columns.** A record or switch field is its own column, named as the
//!   field. A scalar is stored as itself (a bool as 0 or 1, a time as its
//!   RFC 3339 text); a list or an object as its JSON text. [`RECORD`],
//!   [`SWITCH`] and [`SEGMENT`] list them, so a record is written and read
//!   back through its serde shape, field for field.
//! - **Header-only rows.** A session switched on but never passed has a row
//!   with `has_record = 0`; a cataloged one never switched on has
//!   `has_switch = 0`.
//! - **Forgotten rows.** The provider id, short id and forget timestamp
//!   stay. The record and switch are removed, and their id cannot be reused.
//!
//! The functions take a connection or a transaction the caller opened; the
//! store holds its lock around every write.

use super::memory::read;
use super::{DB_FILE, Fallible, db_error, optimize_fts, stamp};
use crate::error::{Error, Result};
use crate::session::{ControlState, SessionControl, SessionId, SessionRecord, check_log};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// How a field is stored.
#[derive(Clone, Copy, Debug)]
pub(super) enum Col {
    /// A string, as TEXT.
    Text,
    /// A bool, as 0 or 1.
    Bool,
    /// A whole number, as INTEGER.
    Int,
    /// A list or an object, as its JSON text.
    Json,
}

/// A record's frontmatter fields, as columns of `sessions`. `id`, `links`,
/// `terms` and `log` have their own homes; the body sections are
/// `summary`, `goals` and `next`.
const RECORD: &[(&str, Col)] = &[
    ("status", Col::Text),
    ("active", Col::Bool),
    ("keep_active", Col::Bool),
    ("active_change", Col::Json),
    ("cataloged_by", Col::Json),
    ("rebuilt_by", Col::Json),
    ("project", Col::Text),
    ("topics", Col::Json),
    ("related", Col::Json),
    ("provider", Col::Text),
    ("host", Col::Text),
    ("created", Col::Text),
    ("updated", Col::Text),
    ("title", Col::Text),
    ("cwd", Col::Text),
    ("started", Col::Text),
    ("on_since", Col::Text),
    ("scribed_through", Col::Text),
    ("passes", Col::Int),
    ("next_open", Col::Int),
];

/// A switch's fields, as `sw_<field>` columns of `sessions`.
const SWITCH: &[(&str, Col)] = &[
    ("provider", Col::Text),
    ("on", Col::Bool),
    ("since", Col::Text),
    ("due", Col::Bool),
    ("final", Col::Bool),
    ("consecutive_failures", Col::Int),
    ("retry_after", Col::Text),
    ("pending_links", Col::Json),
    ("cataloged_by", Col::Json),
];

/// A segment's metadata, as columns of `segments`; its text is `text`.
const SEGMENT: &[(&str, Col)] = &[
    ("n", Col::Int),
    ("heading", Col::Text),
    ("first_message", Col::Text),
    ("last_message", Col::Text),
    ("from", Col::Text),
    ("to", Col::Text),
    ("state", Col::Text),
    ("windows", Col::Int),
    ("origin", Col::Text),
    ("topics", Col::Json),
    ("related", Col::Json),
];

/// The body sections, after the [`RECORD`] columns.
const BODY: [&str; 3] = ["summary", "goals", "next"];

pub(super) fn quoted(name: &str) -> String {
    format!("\"{name}\"")
}

pub(super) fn to_sql(
    field: &str,
    value: Option<&Value>,
    col: Col,
) -> std::result::Result<Sql, String> {
    Ok(match (value.unwrap_or(&Value::Null), col) {
        (Value::Null, _) => Sql::Null,
        (Value::String(s), Col::Text) => Sql::Text(s.clone()),
        (Value::Bool(b), Col::Bool) => Sql::Integer(i64::from(*b)),
        (Value::Number(n), Col::Int) => Sql::Integer(
            n.as_i64()
                .ok_or_else(|| format!("{field} is out of range"))?,
        ),
        (v, Col::Json) => Sql::Text(v.to_string()),
        (v, _) => return Err(format!("{field} has an unexpected shape: {v}")),
    })
}

pub(super) fn from_sql(field: &str, value: Sql, col: Col) -> std::result::Result<Value, String> {
    Ok(match (value, col) {
        (Sql::Null, _) => Value::Null,
        (Sql::Text(s), Col::Text) => Value::String(s),
        (Sql::Integer(i), Col::Bool) => Value::Bool(i != 0),
        (Sql::Integer(i), Col::Int) => Value::Number(i.into()),
        (Sql::Text(s), Col::Json) => {
            serde_json::from_str(&s).map_err(|e| format!("{field} is not JSON: {e}"))?
        }
        (_, _) => return Err(format!("{field} has the wrong column type")),
    })
}

/// The JSON object a value serializes to.
pub(super) fn object<T: serde::Serialize>(
    value: &T,
) -> std::result::Result<Map<String, Value>, String> {
    match serde_json::to_value(value).map_err(|e| e.to_string())? {
        Value::Object(map) => Ok(map),
        other @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Array(_)) => Err(format!("expected an object, found {other}")),
    }
}

pub(super) fn columns(
    obj: &Map<String, Value>,
    spec: &[(&str, Col)],
) -> std::result::Result<Vec<Sql>, String> {
    spec.iter()
        .map(|(name, col)| to_sql(name, obj.get(*name), *col))
        .collect()
}

/// Where a row lives, for an error: `<root>/scribe.db#s-<n>`.
fn row_path(db: &Path, n: i64) -> PathBuf {
    PathBuf::from(format!("{}#s-{n}", db.display()))
}

/// A session row that did not decode.
#[derive(Debug)]
pub(crate) struct BadRow {
    /// Its provider session id, when that much reads.
    pub id: Option<SessionId>,
    /// `<root>/scribe.db#s-<n>`.
    pub path: PathBuf,
    /// Always [`Error::Corrupt`], naming the row.
    pub error: Error,
}

/// What the session tables hold, as the store's types.
#[derive(Debug, Default)]
pub(crate) struct Sessions {
    /// Every record, with its row number, in row order; a row that does
    /// not decode is an error naming it.
    pub records: Vec<std::result::Result<(NonZeroU64, SessionRecord), BadRow>>,
    /// Every session's row number, a header-only row's too.
    pub numbers: BTreeMap<SessionId, NonZeroU64>,
    /// Every switch and Codex cursor.
    pub control: ControlState,
    /// When each forgotten session was forgotten.
    pub forgotten: BTreeMap<SessionId, OffsetDateTime>,
}

/// Whether the sessions live in the database: schema 3 or newer and a
/// `record_kinds` row for them.
pub(crate) fn moved(conn: &Connection) -> rusqlite::Result<bool> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 3 {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM record_kinds WHERE kind = 'sessions')",
        [],
        |row| row.get(0),
    )
}

/// Record that the sessions moved in, and where their files went.
pub(crate) fn mark_moved(
    conn: &Connection,
    now: OffsetDateTime,
    moved_to: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO record_kinds (kind, moved_at, moved_to) VALUES ('sessions', ?1, ?2)
         ON CONFLICT (kind) DO NOTHING",
        (stamp(now), moved_to),
    )?;
    Ok(())
}

/// The sessions, read-only, when they live in the database; `None` when
/// they are still files (or there is no database).
pub(crate) fn read_sessions(root: &Path) -> Result<Option<Sessions>> {
    let (found, _) = read(root, |conn, path| {
        super::refuse_newer(conn, path)?;
        if !moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))? {
            return Ok(None);
        }
        load(conn, path).map(Some).map_err(Fallible::Failed)
    })?;
    Ok(found)
}

/// A forgotten row's short id and forget time, for archive cleanup.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tombstone {
    pub number: NonZeroU64,
    pub at: OffsetDateTime,
}

/// Forgotten session ids, without loading session payloads.
/// `None` means sessions have not moved into the database yet.
pub(crate) fn read_forgotten(root: &Path) -> Result<Option<BTreeMap<SessionId, Tombstone>>> {
    let (found, _) = read(root, |conn, path| {
        super::refuse_newer(conn, path)?;
        if !moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))? {
            return Ok(None);
        }
        forgotten(conn, path).map(Some).map_err(Fallible::Failed)
    })?;
    Ok(found)
}

/// Check the tombstone without decoding any session record or switch.
pub(crate) fn read_is_forgotten(root: &Path, id: &SessionId) -> Result<bool> {
    let (found, _) = read(root, |conn, path| {
        if super::refuse_newer(conn, path)? < 4 {
            return Ok(false);
        }
        is_forgotten(conn, id).map_err(|e| Fallible::from_sqlite("read", path, e))
    })?;
    Ok(found)
}

/// The switches and cursors, read-only, when the sessions live in the
/// database; `None` when they are still files.
pub(crate) fn read_control(root: &Path) -> Result<Option<ControlState>> {
    let (found, _) = read(root, |conn, path| {
        super::refuse_newer(conn, path)?;
        if !moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))? {
            return Ok(None);
        }
        control(conn, path).map(Some).map_err(Fallible::Failed)
    })?;
    Ok(found)
}

/// The rebuild checkpoint of `id`, read-only; `None` when the sessions are
/// still files or there is none.
pub(crate) fn read_checkpoint(root: &Path, id: &SessionId) -> Result<Option<String>> {
    let (found, _) = read(root, |conn, path| {
        super::refuse_newer(conn, path)?;
        if !moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))? {
            return Ok(None);
        }
        checkpoint(conn, id).map_err(|e| Fallible::from_sqlite("read", path, e))
    })?;
    Ok(found)
}

/// Whether the sessions live in the database, read-only.
pub(crate) fn read_moved(root: &Path) -> Result<bool> {
    let (found, _) = read(root, |conn, path| {
        moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))
    })?;
    Ok(found)
}

/// Every session, switch and cursor.
pub(crate) fn load(conn: &Connection, db: &Path) -> Result<Sessions> {
    let err = |e| db_error("read sessions from", db, e);
    let mut out = Sessions::default();
    {
        let mut select = conn
            .prepare("SELECT id, provider_session FROM sessions ORDER BY id")
            .map_err(err)?;
        let rows = select
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(err)?;
        for row in rows {
            let (n, id) = row.map_err(err)?;
            let id: SessionId = id.parse().map_err(|e| Error::corrupt(row_path(db, n), e))?;
            let n = NonZeroU64::new(u64::try_from(n).unwrap_or(0))
                .ok_or_else(|| Error::corrupt(row_path(db, n), "row id is not positive"))?;
            out.numbers.insert(id, n);
        }
    }
    let mut logs = segments(conn, db)?;
    let mut links = lists(
        conn,
        db,
        "SELECT session, type, target FROM session_links ORDER BY session, position",
        |row| {
            let mut link = Map::new();
            link.insert("type".into(), Value::String(row.get(1)?));
            link.insert("session".into(), Value::String(row.get(2)?));
            Ok(Value::Object(link))
        },
    )?;
    let mut terms = lists(
        conn,
        db,
        "SELECT session, term, said, message FROM session_terms ORDER BY session, position",
        |row| {
            let mut term = Map::new();
            term.insert("term".into(), Value::String(row.get(1)?));
            term.insert("said".into(), Value::String(row.get(2)?));
            term.insert("message".into(), Value::String(row.get(3)?));
            Ok(Value::Object(term))
        },
    )?;
    let names: Vec<String> = RECORD.iter().map(|(n, _)| quoted(n)).collect();
    let sql = format!(
        "SELECT id, provider_session, {}, summary, goals, next FROM sessions
         WHERE has_record = 1 ORDER BY id",
        names.join(", ")
    );
    let mut select = conn.prepare(&sql).map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    while let Some(row) = rows.next().map_err(err)? {
        let n: i64 = row.get(0).map_err(err)?;
        let decoded = (|| -> std::result::Result<_, String> {
            let mut obj = Map::new();
            obj.insert(
                "id".into(),
                Value::String(row.get(1).map_err(|e| e.to_string())?),
            );
            for (i, (name, col)) in RECORD.iter().enumerate() {
                let value: Sql = row.get(i + 2).map_err(|e| e.to_string())?;
                obj.insert((*name).into(), from_sql(name, value, *col)?);
            }
            let (log, texts): (Vec<Value>, Vec<String>) =
                logs.remove(&n).unwrap_or_default().into_iter().unzip();
            obj.insert("log".into(), Value::Array(log));
            obj.insert(
                "links".into(),
                Value::Array(links.remove(&n).unwrap_or_default()),
            );
            obj.insert(
                "terms".into(),
                Value::Array(terms.remove(&n).unwrap_or_default()),
            );
            let mut record: SessionRecord =
                serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())?;
            for (segment, text) in record.log.iter_mut().zip(texts) {
                segment.text = text;
            }
            let body = RECORD.len() + 2;
            record.summary = row.get(body).map_err(|e| e.to_string())?;
            record.goals = row.get(body + 1).map_err(|e| e.to_string())?;
            record.next = row.get(body + 2).map_err(|e| e.to_string())?;
            check_log(&record).map_err(|e| e.to_string())?;
            Ok(record)
        })();
        let number = NonZeroU64::new(u64::try_from(n).unwrap_or(0));
        let bad = |e: String| BadRow {
            id: row.get::<_, String>(1).ok().and_then(|id| id.parse().ok()),
            path: row_path(db, n),
            error: Error::corrupt(row_path(db, n), e),
        };
        out.records.push(match (decoded, number) {
            (Ok(record), Some(number)) => Ok((number, record)),
            (Err(e), _) => Err(bad(e)),
            (Ok(_), None) => Err(bad(String::from("row id is not positive"))),
        });
    }
    out.control = control(conn, db)?;
    out.forgotten = forgotten(conn, db)?
        .into_iter()
        .map(|(id, tombstone)| (id, tombstone.at))
        .collect();
    Ok(out)
}

/// When each forgotten session was forgotten; none before schema 4, which
/// a reader of a database no writer has migrated yet still sees.
pub(crate) fn forgotten(conn: &Connection, db: &Path) -> Result<BTreeMap<SessionId, Tombstone>> {
    let err = |e| db_error("read sessions from", db, e);
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(err)?;
    let mut out = BTreeMap::new();
    if version < 4 {
        return Ok(out);
    }
    let mut select = conn
        .prepare(
            "SELECT id, provider_session, forgotten FROM sessions
             WHERE forgotten IS NOT NULL ORDER BY id",
        )
        .map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    while let Some(row) = rows.next().map_err(err)? {
        let n: i64 = row.get(0).map_err(err)?;
        let id: String = row.get(1).map_err(err)?;
        let at: String = row.get(2).map_err(err)?;
        let id: SessionId = id.parse().map_err(|e| Error::corrupt(row_path(db, n), e))?;
        let at = OffsetDateTime::parse(&at, &time::format_description::well_known::Rfc3339)
            .map_err(|e| Error::corrupt(row_path(db, n), format!("forgotten: {e}")))?;
        let number = NonZeroU64::new(u64::try_from(n).unwrap_or(0))
            .ok_or_else(|| Error::corrupt(row_path(db, n), "row id is not positive"))?;
        out.insert(id, Tombstone { number, at });
    }
    Ok(out)
}

/// Whether `id` has a forgotten row.
pub(super) fn is_forgotten(conn: &Connection, id: &SessionId) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sessions
         WHERE provider_session = ?1 AND forgotten IS NOT NULL)",
        [id.as_str()],
        |row| row.get(0),
    )
}

pub(crate) use crate::checks::forgotten_refusal;

/// What [`forget`] erased.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Erased {
    /// Its `messages` rows deleted.
    pub messages: usize,
    /// Its segment rows deleted.
    pub segments: usize,
    /// Whether it was switched on, and is now off.
    pub was_on: bool,
}

/// A joined session can be known before it writes its first captured turn.
pub(crate) fn has_group_membership(conn: &Connection, path: &Path, id: &SessionId) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM group_members WHERE session = ?1)",
        [id.as_str()],
        |row| row.get(0),
    )
    .map_err(|e| db_error("read group membership in", path, e))
}

/// Allocate an id for a session previously known only by a handoff or membership.
pub(crate) fn register(conn: &Connection, db: &Path, id: &SessionId) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions(provider_session, has_record, has_switch) VALUES (?1, 0, 0)
         ON CONFLICT(provider_session) DO NOTHING",
        [id.as_str()],
    )
    .map_err(|e| db_error("register a session in", db, e))?;
    Ok(())
}

/// Erase the session and its directly owned rows. A tombstone keeps both
/// ids and the first forget timestamp. Reapplying also cleans residue from
/// older versions and reports whether that cleanup changed anything.
pub(crate) fn forget(
    conn: &Connection,
    db: &Path,
    id: &SessionId,
    now: OffsetDateTime,
) -> Result<Option<(Erased, bool, bool)>> {
    let err = |e| db_error("forget a session in", db, e);
    let row: Option<(i64, bool, bool)> = conn
        .query_row(
            "SELECT id, forgotten IS NOT NULL, coalesce(sw_on, 0) FROM sessions
             WHERE provider_session = ?1",
            [id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(err)?;
    let Some((n, already, was_on)) = row else {
        return Ok(None);
    };
    let messages = conn
        .execute("DELETE FROM messages WHERE session = ?1", [id.as_str()])
        .map_err(err)?;
    let segments = conn
        .execute("DELETE FROM segments WHERE session = ?1", [n])
        .map_err(err)?;
    let mut changed = messages + segments;
    for table in ["session_terms", "rebuild_checkpoints", "session_links"] {
        changed += conn
            .execute(&format!("DELETE FROM {table} WHERE session = ?1"), [n])
            .map_err(err)?;
    }
    for (table, key) in [
        ("now_pending", "session"),
        ("pass_signals", "session"),
        ("codex_cursors", "session"),
        ("marks", "session"),
    ] {
        changed += conn
            .execute(
                &format!("DELETE FROM {table} WHERE {key} = ?1"),
                [id.as_str()],
            )
            .map_err(err)?;
    }
    changed += super::groups::forget_session(conn, db, id, now)?;
    changed += conn
        .execute(
            "DELETE FROM session_links WHERE type = 'continues' AND target = ?1",
            [id.as_str()],
        )
        .map_err(err)?;
    changed += conn
        .execute(
            "UPDATE sessions SET sw_pending_links = (SELECT json_group_array(json(link.value))
             FROM json_each(sw_pending_links) AS link
             WHERE NOT (link.value ->> 'type' = 'continues' AND link.value ->> 'session' = ?1))
         WHERE EXISTS (SELECT 1 FROM json_each(sw_pending_links) AS link
             WHERE link.value ->> 'type' = 'continues' AND link.value ->> 'session' = ?1)",
            [id.as_str()],
        )
        .map_err(err)?;
    let clear = RECORD
        .iter()
        .map(|(name, _)| format!("{} = NULL", quoted(name)))
        .chain(SWITCH.iter().map(|(name, _)| format!("sw_{name} = NULL")))
        .collect::<Vec<_>>()
        .join(", ");
    changed += conn
        .execute(
            &format!(
                "UPDATE sessions SET forgotten = coalesce(forgotten, ?2),
             has_record = 0, has_switch = 0, summary = NULL, goals = NULL, next = NULL, {clear}
             WHERE id = ?1 AND (forgotten IS NULL OR has_record = 1 OR has_switch = 1)"
            ),
            (n, stamp(now)),
        )
        .map_err(err)?;
    if messages > 0 {
        optimize_fts(conn).map_err(err)?;
    }
    Ok(Some((
        Erased {
            messages,
            segments,
            was_on,
        },
        already,
        changed > 0,
    )))
}

/// Every segment, by row: its metadata object and its text, in order.
fn segments(conn: &Connection, db: &Path) -> Result<BTreeMap<i64, Vec<(Value, String)>>> {
    let err = |e| db_error("read sessions from", db, e);
    let names: Vec<String> = SEGMENT.iter().map(|(n, _)| quoted(n)).collect();
    let sql = format!(
        "SELECT session, {}, text FROM segments ORDER BY session, n",
        names.join(", ")
    );
    let mut select = conn.prepare(&sql).map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    let mut out: BTreeMap<i64, Vec<(Value, String)>> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(err)? {
        let session: i64 = row.get(0).map_err(err)?;
        let mut obj = Map::new();
        for (i, (name, col)) in SEGMENT.iter().enumerate() {
            let value: Sql = row.get(i + 1).map_err(err)?;
            let value = from_sql(name, value, *col)
                .map_err(|e| Error::corrupt(row_path(db, session), e))?;
            obj.insert((*name).into(), value);
        }
        let text: String = row.get(SEGMENT.len() + 1).map_err(err)?;
        out.entry(session)
            .or_default()
            .push((Value::Object(obj), text));
    }
    Ok(out)
}

/// The rows of `sql`, whose first column is the session row, grouped by it.
fn lists(
    conn: &Connection,
    db: &Path,
    sql: &str,
    item: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<Value>,
) -> Result<BTreeMap<i64, Vec<Value>>> {
    let err = |e| db_error("read sessions from", db, e);
    let mut select = conn.prepare(sql).map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    let mut out: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(err)? {
        let session: i64 = row.get(0).map_err(err)?;
        out.entry(session)
            .or_default()
            .push(item(row).map_err(err)?);
    }
    Ok(out)
}

/// Every switch and Codex cursor.
pub(crate) fn control(conn: &Connection, db: &Path) -> Result<ControlState> {
    let err = |e| db_error("read sessions from", db, e);
    let names: Vec<String> = SWITCH.iter().map(|(n, _)| format!("sw_{n}")).collect();
    let sql = format!(
        "SELECT id, provider_session, {} FROM sessions WHERE has_switch = 1 ORDER BY id",
        names.join(", ")
    );
    let mut state = ControlState::default();
    let mut select = conn.prepare(&sql).map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    while let Some(row) = rows.next().map_err(err)? {
        let n: i64 = row.get(0).map_err(err)?;
        let id: String = row.get(1).map_err(err)?;
        let decoded = (|| -> std::result::Result<_, String> {
            let mut obj = Map::new();
            for (i, (name, col)) in SWITCH.iter().enumerate() {
                let value: Sql = row.get(i + 2).map_err(|e| e.to_string())?;
                obj.insert((*name).into(), from_sql(name, value, *col)?);
            }
            let id: SessionId = id.parse().map_err(|e: crate::SessionError| e.to_string())?;
            let switch: SessionControl =
                serde_json::from_value(Value::Object(obj)).map_err(|e| e.to_string())?;
            Ok((id, switch))
        })()
        .map_err(|e| Error::corrupt(row_path(db, n), e))?;
        state.sessions.insert(decoded.0, decoded.1);
    }
    let mut select = conn
        .prepare("SELECT session, message FROM codex_cursors ORDER BY session")
        .map_err(err)?;
    let rows = select
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(err)?;
    for row in rows {
        let (session, message) = row.map_err(err)?;
        let session: SessionId = session
            .parse()
            .map_err(|e| Error::corrupt(db, format!("codex_cursors: {e}")))?;
        state.codex_cursors.insert(session, message);
    }
    Ok(state)
}

/// Write `record` whole: its row's record columns (the row created when
/// missing, its switch kept), then its segments, links and terms replaced.
/// Returns its row number.
pub(crate) fn put_record(conn: &Connection, db: &Path, record: &SessionRecord) -> Result<i64> {
    let corrupt = |e: String| Error::corrupt(db, format!("session {}: {e}", record.id));
    let err = |e| db_error("write a session to", db, e);
    if is_forgotten(conn, &record.id).map_err(err)? {
        return Err(forgotten_refusal(&record.id, "nothing is written to it"));
    }
    let obj = object(record).map_err(corrupt)?;
    let mut values = vec![Sql::Text(record.id.as_str().to_owned())];
    values.extend(columns(&obj, RECORD).map_err(corrupt)?);
    values.extend([
        Sql::Text(record.summary.clone()),
        Sql::Text(record.goals.clone()),
        Sql::Text(record.next.clone()),
    ]);
    let names: Vec<String> = RECORD
        .iter()
        .map(|(n, _)| quoted(n))
        .chain(BODY.iter().map(|n| quoted(n)))
        .collect();
    let n = upsert_session(conn, "has_record", &names, values).map_err(err)?;
    for table in ["segments", "session_links", "session_terms"] {
        conn.execute(&format!("DELETE FROM {table} WHERE session = ?1"), [n])
            .map_err(err)?;
    }
    let seg_names: Vec<String> = SEGMENT.iter().map(|(n, _)| quoted(n)).collect();
    let seg_marks: Vec<String> = (2..=SEGMENT.len() + 2).map(|i| format!("?{i}")).collect();
    let seg_sql = format!(
        "INSERT INTO segments (session, {}, text) VALUES (?1, {})",
        seg_names.join(", "),
        seg_marks.join(", ")
    );
    for segment in &record.log {
        let obj = object(segment).map_err(corrupt)?;
        let mut values = vec![Sql::Integer(n)];
        values.extend(columns(&obj, SEGMENT).map_err(corrupt)?);
        values.push(Sql::Text(segment.text.clone()));
        conn.execute(&seg_sql, params_from_iter(values))
            .map_err(err)?;
    }
    for (i, link) in record.links.iter().enumerate() {
        if link.kind == crate::LinkType::Continues
            && is_forgotten(conn, &link.session).map_err(err)?
        {
            continue;
        }
        conn.execute(
            "INSERT INTO session_links (session, position, type, target) VALUES (?1, ?2, ?3, ?4)",
            (
                n,
                i64::try_from(i).unwrap_or(i64::MAX),
                link.kind.as_str(),
                link.session.as_str(),
            ),
        )
        .map_err(err)?;
    }
    for (i, term) in record.terms.iter().enumerate() {
        conn.execute(
            "INSERT INTO session_terms (session, position, term, said, message)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                n,
                i64::try_from(i).unwrap_or(i64::MAX),
                &term.term,
                &term.said,
                &term.message,
            ),
        )
        .map_err(err)?;
    }
    Ok(n)
}

/// Write what changed from `before` to `after`: each changed switch (its
/// row created header-only when missing), each switch removed, and each
/// Codex cursor added, changed or removed.
/// Write `names` (quoted columns) of the session `values[0]` names, with
/// `flag` (`has_record` or `has_switch`) set, and return its row id. An
/// UPDATE first, then an INSERT for a new session: an UPSERT would spend an
/// AUTOINCREMENT id on every update, leaving gaps in the `s-<n>` numbers.
fn upsert_session(
    conn: &Connection,
    flag: &str,
    names: &[String],
    values: Vec<Sql>,
) -> rusqlite::Result<i64> {
    let marks: Vec<String> = (2..=names.len() + 1).map(|i| format!("?{i}")).collect();
    let sets: Vec<String> = names
        .iter()
        .zip(&marks)
        .map(|(n, m)| format!("{n} = {m}"))
        .collect();
    let update = format!(
        "UPDATE sessions SET {flag} = 1, {} WHERE provider_session = ?1 RETURNING id",
        sets.join(", ")
    );
    if let Some(n) = conn
        .query_row(&update, params_from_iter(values.iter()), |row| row.get(0))
        .optional()?
    {
        return Ok(n);
    }
    let (has_record, has_switch) = if flag == "has_record" { (1, 0) } else { (0, 1) };
    let insert = format!(
        "INSERT INTO sessions (provider_session, has_record, has_switch, {})
         VALUES (?1, {has_record}, {has_switch}, {}) RETURNING id",
        names.join(", "),
        marks.join(", ")
    );
    conn.query_row(&insert, params_from_iter(values), |row| row.get(0))
}

pub(crate) fn put_control(
    conn: &Connection,
    db: &Path,
    before: &ControlState,
    after: &ControlState,
) -> Result<()> {
    let err = |e| db_error("write a switch to", db, e);
    let names: Vec<String> = SWITCH.iter().map(|(n, _)| format!("sw_{n}")).collect();
    for (id, switch) in &after.sessions {
        if before.sessions.get(id) == Some(switch) {
            continue;
        }
        let corrupt = |e: String| Error::corrupt(db, format!("switch {id}: {e}"));
        if is_forgotten(conn, id).map_err(err)? {
            return Err(forgotten_refusal(id, "it cannot be switched on again"));
        }
        let obj = object(switch).map_err(corrupt)?;
        let mut values = vec![Sql::Text(id.as_str().to_owned())];
        values.extend(columns(&obj, SWITCH).map_err(corrupt)?);
        upsert_session(conn, "has_switch", &names, values).map_err(err)?;
    }
    let clear: Vec<String> = names.iter().map(|n| format!("{n} = NULL")).collect();
    for id in before.sessions.keys() {
        if !after.sessions.contains_key(id) {
            conn.execute(
                &format!(
                    "UPDATE sessions SET has_switch = 0, {} WHERE provider_session = ?1",
                    clear.join(", ")
                ),
                [id.as_str()],
            )
            .map_err(err)?;
        }
    }
    for (id, message) in &after.codex_cursors {
        if before.codex_cursors.get(id) != Some(message) && !is_forgotten(conn, id).map_err(err)? {
            conn.execute(
                "INSERT INTO codex_cursors (session, message) VALUES (?1, ?2)
                 ON CONFLICT (session) DO UPDATE SET message = excluded.message",
                (id.as_str(), message),
            )
            .map_err(err)?;
        }
    }
    for id in before.codex_cursors.keys() {
        if !after.codex_cursors.contains_key(id) {
            conn.execute(
                "DELETE FROM codex_cursors WHERE session = ?1",
                [id.as_str()],
            )
            .map_err(err)?;
        }
    }
    Ok(())
}

/// The rebuild checkpoint of `id`: a session record's file text.
pub(crate) fn checkpoint(conn: &Connection, id: &SessionId) -> rusqlite::Result<Option<String>> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    // Schema 4 is the first with `sessions.forgotten`. A forgotten session
    // keeps no checkpoint, including one an older build left before the
    // version 8 repair deletes the row.
    let sql = if version >= 4 {
        "SELECT c.record FROM rebuild_checkpoints c JOIN sessions s ON s.id = c.session
         WHERE s.provider_session = ?1 AND s.forgotten IS NULL"
    } else {
        "SELECT c.record FROM rebuild_checkpoints c JOIN sessions s ON s.id = c.session
         WHERE s.provider_session = ?1"
    };
    conn.query_row(sql, [id.as_str()], |row| row.get(0))
        .optional()
}

/// Keep `text` as the rebuild checkpoint of `id`, which has a row.
pub(crate) fn put_checkpoint(
    conn: &Connection,
    id: &SessionId,
    text: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO rebuild_checkpoints (session, record, created)
         SELECT id, ?2, ?3 FROM sessions WHERE provider_session = ?1
         ON CONFLICT (session) DO UPDATE SET record = excluded.record, created = excluded.created",
        (id.as_str(), text, stamp(now)),
    )
}

/// Drop the rebuild checkpoint of `id`, if any.
pub(crate) fn drop_checkpoint(conn: &Connection, id: &SessionId) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM rebuild_checkpoints WHERE session =
             (SELECT id FROM sessions WHERE provider_session = ?1)",
        [id.as_str()],
    )?;
    Ok(())
}

/// The database file under `root`, for errors.
pub(crate) fn db_path(root: &Path) -> PathBuf {
    root.join(DB_FILE)
}

/// Convert one SQLite column value into the JSON value `export_rows` writes.
pub(super) fn export_sql_value(sql: Sql) -> Value {
    match sql {
        Sql::Null => Value::Null,
        Sql::Integer(n) => Value::Number(n.into()),
        Sql::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        Sql::Text(s) => Value::String(s),
        Sql::Blob(b) => Value::String(String::from_utf8_lossy(&b).into_owned()),
    }
}

/// Every table's rows as JSON objects, for `scribe export --as jsonl`:
/// `(table, row)` in table order, then row order. Reads only.
pub(crate) fn export_rows(
    conn: &Connection,
    db: &Path,
    tables: &[(&'static str, &str)],
) -> Result<Vec<(&'static str, Value)>> {
    let err = |e| db_error("export", db, e);
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(err)?;
    let mut out = Vec::new();
    for &(table, order) in tables {
        let mut select = conn
            .prepare(&export_select(table, order, version))
            .map_err(err)?;
        let names: Vec<String> = select.column_names().map(str::to_owned).collect();
        let mut rows = select.query([]).map_err(err)?;
        while let Some(row) = rows.next().map_err(err)? {
            let mut obj = Map::new();
            for (i, name) in names.iter().enumerate() {
                let value = export_sql_value(row.get::<_, Sql>(i).map_err(err)?);
                obj.insert(name.clone(), value);
            }
            // A database last written by an older build can still carry
            // the original header on its tombstone.
            if table == "sessions" && obj.get("forgotten").is_some_and(|v| !v.is_null()) {
                for (name, value) in &mut obj {
                    match name.as_str() {
                        "id" | "provider_session" | "forgotten" => {}
                        "has_record" | "has_switch" => *value = Value::from(0),
                        _ => *value = Value::Null,
                    }
                }
            }
            out.push((table, Value::Object(obj)));
        }
    }
    Ok(out)
}

/// The `SELECT` [`export_rows`] runs for one table. Deletion residue from
/// older builds is omitted even on a read-only database.
fn export_select(table: &str, order: &str, version: u32) -> String {
    if version >= 4
        && matches!(
            table,
            "rebuild_checkpoints" | "segments" | "session_terms" | "session_links"
        )
    {
        return format!(
            "SELECT c.* FROM {table} AS c
             JOIN sessions AS s ON s.id = c.session
             WHERE s.forgotten IS NULL
             ORDER BY {order}"
        );
    }
    format!("SELECT * FROM {table} ORDER BY {order}")
}

/// One write transaction on the session tables (`BEGIN IMMEDIATE`), so the
/// store's write paths need no SQLite types. Dropped without
/// [`SessionTx::commit`], it rolls back.
pub(crate) struct SessionTx<'c> {
    tx: super::scope::Tx<'c>,
    path: PathBuf,
    started: std::time::Instant,
}

impl<'c> SessionTx<'c> {
    /// Begin on a writer connection for the database under `root`: inside a
    /// store write, a savepoint of its transaction.
    pub(crate) fn begin(conn: &'c Connection, root: &Path) -> Result<Self> {
        let path = db_path(root);
        let tx = super::scope::Tx::begin(conn, &path, "write sessions to")?;
        let started = tx.started();
        Ok(Self { tx, path, started })
    }

    fn fail(&self, e: rusqlite::Error) -> Error {
        super::write_error("write sessions to", &self.path, self.started, e)
    }

    /// Every switch and Codex cursor.
    pub(crate) fn control(&self) -> Result<ControlState> {
        control(&self.tx, &self.path)
    }

    /// Write a record whole; returns its row number.
    pub(crate) fn put_record(&self, record: &SessionRecord) -> Result<i64> {
        put_record(&self.tx, &self.path, record)
    }

    /// Write the switches and cursors that changed.
    pub(crate) fn put_control(&self, before: &ControlState, after: &ControlState) -> Result<()> {
        put_control(&self.tx, &self.path, before, after)
    }

    /// Keep a rebuild checkpoint; `false` when `id` has no row. A forgotten
    /// session keeps none: its words were erased, so nothing re-persists them.
    pub(crate) fn put_checkpoint(
        &self,
        id: &SessionId,
        text: &str,
        now: OffsetDateTime,
    ) -> Result<bool> {
        if is_forgotten(&self.tx, id).map_err(|e| self.fail(e))? {
            return Err(forgotten_refusal(id, "nothing is written to it"));
        }
        Ok(put_checkpoint(&self.tx, id, text, now).map_err(|e| self.fail(e))? > 0)
    }

    /// Drop a rebuild checkpoint.
    pub(crate) fn drop_checkpoint(&self, id: &SessionId) -> Result<()> {
        drop_checkpoint(&self.tx, id).map_err(|e| self.fail(e))
    }

    /// Upsert a window's messages, each redacted first, in this
    /// transaction. Returns how many rows changed.
    pub(crate) fn upsert_messages(
        &self,
        messages: &[crate::message::NewMessage],
        redactor: &crate::Redactor,
    ) -> Result<usize> {
        super::upsert_in(&self.tx, &self.path, self.started, messages, redactor)
    }

    /// Stage a memory commit's rows and journal in this transaction.
    pub(crate) fn stage_memory(
        &self,
        commit: &super::MemoryCommit,
        now: OffsetDateTime,
    ) -> Result<(Option<super::Journal>, usize)> {
        super::memory::stage(&self.tx, &self.path, self.started, commit, now)
    }

    /// Commit.
    pub(crate) fn commit(self) -> Result<()> {
        self.tx.commit()
    }
}

impl crate::Store {
    /// Set one session record field, `title` or `project`, to `value`, in
    /// the write under way; returns how many rows changed (0 when the session
    /// has no record or was forgotten).
    pub(crate) fn set_session_field(
        &self,
        id: &SessionId,
        field: crate::session::SessionField,
        value: &str,
    ) -> Result<usize> {
        self.write_step(|| {
            let conn = super::open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let sql = match field {
                crate::session::SessionField::Project => "UPDATE sessions SET project=?2 WHERE provider_session=?1 AND has_record=1 AND forgotten IS NULL",
                crate::session::SessionField::Title => "UPDATE sessions SET title=?2 WHERE provider_session=?1 AND has_record=1 AND forgotten IS NULL",
            };
            conn.execute(sql, (id.as_str(), value))
                .map_err(|e| db_error("update session in", &path, e))
        })
    }
}
