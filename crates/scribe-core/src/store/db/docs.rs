//! The document tables of `scribe.db` (docs/design/scribe/2_design.md
//! §15.7, phase 7): `entries`, `claims`, `agent_tags`, `marks`, `pickups`
//! and `audit`, which replace `entries/`, `proposals.json`, `tags.json`,
//! `marks.json`, `pickups.json` and `audit.jsonl`.
//!
//! - **Columns.** A top-level field of a record's serde shape is its own
//!   column, named as the field, as the session tables store theirs; a
//!   claim's candidate fields are `c_<field>` beside `has_candidate`. A row
//!   is read back through the same serde shape and the same checks a file
//!   gets ([`super::super::format`]), so a hand-edited row is refused as a
//!   hand-edited file was.
//! - **Writes.** An entry or a claim is written by its id (UPDATE, then
//!   INSERT for a new one); tags, marks and pickups are small lists,
//!   rewritten whole when they change. Every write runs inside the store
//!   write's transaction ([`super::scope`]).
//! - **Audit.** One row per line, with the line's own `format`, so a line
//!   imported from `audit.jsonl` reads back as it was written. A line a
//!   command could not commit without waiting is spooled to a file and
//!   drained here by the next write, keyed by its file name (`spool`).

use super::sessions::{Col, columns, export_sql_value, from_sql, object, quoted};
use super::{Fallible, db_error, stamp};
use crate::action::Pickup;
use crate::annotation::MarkRecord;
use crate::entry::{Entry, EntryId};
use crate::error::{Error, Result};
use crate::proposal::{Proposal, ProposalId};
use crate::store::audit::{AUDIT_FORMAT, AuditEvent};
use crate::store::format;
use crate::tag::StoreTag;
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, params_from_iter};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Allow withdrawn claims and reserve the ids of erased handoff entries.
pub(super) const V14: &str = "
ALTER TABLE claims RENAME TO claims_v12;
CREATE TABLE claims (
    n              INTEGER NOT NULL PRIMARY KEY CHECK (n >= 1),
    kind           TEXT    NOT NULL CHECK (kind IN ('supersedes', 'stale', 'candidate')),
    older          TEXT    NOT NULL,
    newer          TEXT,
    reason         TEXT    NOT NULL,
    quotes         TEXT    NOT NULL CHECK (json_valid(quotes) AND json_type(quotes) = 'array'),
    state          TEXT    NOT NULL CHECK (state IN ('open', 'approved', 'rejected')),
    opened_at      TEXT    NOT NULL,
    settled_at     TEXT,
    settled_by     TEXT    CHECK (settled_by IS NULL OR json_valid(settled_by)),
    author         TEXT    NOT NULL CHECK (json_valid(author)),
    updated_by     TEXT    CHECK (updated_by IS NULL OR json_valid(updated_by)),
    settled_author TEXT    CHECK (settled_author IS NULL OR json_valid(settled_author)),
    closed_reason  TEXT    CHECK (closed_reason IS NULL
                                  OR closed_reason IN ('tag_retired', 'instructed', 'covered', 'withdrawn')),
    instruction    TEXT    CHECK (instruction IS NULL OR json_valid(instruction)),
    has_candidate  INTEGER NOT NULL CHECK (has_candidate IN (0, 1)),
    c_tag          TEXT,
    c_role         TEXT    CHECK (c_role IS NULL OR c_role IN ('user', 'assistant')),
    c_quote        TEXT,
    c_context      TEXT,
    c_statement    TEXT,
    c_subject      TEXT,
    c_evidence     TEXT,
    c_cite         TEXT    CHECK (c_cite IS NULL OR json_valid(c_cite)),
    c_project      TEXT,
    c_provider     TEXT,
    c_host         TEXT,
    c_admitted     TEXT    CHECK (c_admitted IS NULL OR json_valid(c_admitted)),
    CHECK (has_candidate = 0 OR (c_tag IS NOT NULL AND c_role IS NOT NULL
        AND c_quote IS NOT NULL AND c_cite IS NOT NULL AND c_project IS NOT NULL
        AND c_provider IS NOT NULL AND c_host IS NOT NULL))
) STRICT;
INSERT INTO claims SELECT * FROM claims_v12;
DROP TABLE claims_v12;

CREATE TABLE IF NOT EXISTS deleted_handoffs (
    id TEXT PRIMARY KEY NOT NULL
) STRICT;
";

/// The record kinds phase 7 moves, as `record_kinds` names them.
pub(in crate::store) const KINDS: [&str; 6] = [
    "entries",
    "claims",
    "agent_tags",
    "marks",
    "pickups",
    "audit",
];

/// An entry's fields, as columns of `entries`; `content` is the body.
const ENTRY: &[(&str, Col)] = &[
    ("id", Col::Text),
    ("tag", Col::Text),
    ("status", Col::Text),
    ("project", Col::Text),
    ("topics", Col::Json),
    ("related", Col::Json),
    ("provider", Col::Text),
    ("host", Col::Text),
    ("author", Col::Json),
    ("updated_by", Col::Json),
    ("created", Col::Text),
    ("updated", Col::Text),
    ("inactive_since", Col::Text),
    ("said", Col::Text),
    ("content_from", Col::Json),
    ("cite", Col::Json),
    ("retirement_reason", Col::Text),
    ("superseded_by", Col::Text),
    ("review_after", Col::Text),
    ("quote", Col::Text),
    ("window", Col::Json),
    ("evidence", Col::Text),
    ("as_of", Col::Text),
    ("approved_by", Col::Json),
    ("instruction", Col::Json),
    ("removal_instruction", Col::Json),
    ("stated_by", Col::Json),
    ("outcome", Col::Json),
    ("handoff_mode", Col::Text),
    ("decision", Col::Json),
    ("item", Col::Json),
];

/// A proposal's fields, as columns of `claims` (`id` is `n`, the
/// candidate is [`CANDIDATE`]).
const CLAIM: &[(&str, Col)] = &[
    ("kind", Col::Text),
    ("older", Col::Text),
    ("newer", Col::Text),
    ("reason", Col::Text),
    ("quotes", Col::Json),
    ("state", Col::Text),
    ("opened_at", Col::Text),
    ("settled_at", Col::Text),
    ("settled_by", Col::Json),
    ("author", Col::Json),
    ("updated_by", Col::Json),
    ("settled_author", Col::Json),
    ("closed_reason", Col::Text),
    ("instruction", Col::Json),
];

/// A candidate's fields, as `c_<field>` columns of `claims`.
const CANDIDATE: &[(&str, Col)] = &[
    ("tag", Col::Text),
    ("role", Col::Text),
    ("quote", Col::Text),
    ("context", Col::Text),
    ("statement", Col::Text),
    ("subject", Col::Text),
    ("evidence", Col::Text),
    ("cite", Col::Json),
    ("project", Col::Text),
    ("provider", Col::Text),
    ("host", Col::Text),
    ("admitted", Col::Json),
];

/// A store tag's fields, as columns of `agent_tags`.
const TAG: &[(&str, Col)] = &[
    ("name", Col::Text),
    ("aliases", Col::Json),
    ("description", Col::Text),
    ("stale_after_days", Col::Int),
    ("author", Col::Json),
    ("updated_by", Col::Json),
    ("created", Col::Text),
    ("updated", Col::Text),
];

/// A mark's fields, as columns of `marks` after `position`.
const MARK: &[(&str, Col)] = &[
    ("session", Col::Text),
    ("message", Col::Text),
    ("prompt_sha256", Col::Text),
    ("marker", Col::Int),
    ("outcome", Col::Text),
    ("entry", Col::Text),
    ("code", Col::Text),
    ("attempts", Col::Int),
    ("at", Col::Text),
];

/// A pickup's fields, as columns of `pickups` after `position`.
const PICKUP: &[(&str, Col)] = &[
    ("entry", Col::Text),
    ("author", Col::Json),
    ("at", Col::Text),
    ("result", Col::Text),
    ("rejected", Col::Bool),
];

/// An audit event's fields, as columns of `audit` after `format`.
const AUDIT: &[(&str, Col)] = &[
    ("at", Col::Text),
    ("command", Col::Text),
    ("status", Col::Text),
    ("target", Col::Text),
    ("error_code", Col::Text),
    ("author", Col::Text),
    ("instruction_session", Col::Text),
    ("instruction_message", Col::Text),
    ("issue", Col::Json),
    ("duration_ms", Col::Int),
];

/// Where a row lives, for an error: `<root>/scribe.db#<table>/<key>`.
fn row_path(db: &Path, table: &str, key: &str) -> PathBuf {
    PathBuf::from(format!("{}#{table}/{key}", db.display()))
}

/// The columns of `value`, which must have no field `spec` lacks: a field
/// added to a record without a column is refused here, never dropped.
fn row(
    db: &Path,
    what: &str,
    obj: &Map<String, Value>,
    spec: &[(&str, Col)],
    elsewhere: &[&str],
) -> Result<Vec<Sql>> {
    if let Some(extra) = obj
        .keys()
        .find(|k| !spec.iter().any(|(n, _)| n == k) && !elsewhere.contains(&k.as_str()))
    {
        return Err(Error::corrupt(
            db,
            format!("{what} has a field scribe.db has no column for: {extra}"),
        ));
    }
    columns(obj, spec).map_err(|e| Error::corrupt(db, format!("{what}: {e}")))
}

pub(in crate::store) fn serialized<T: serde::Serialize>(
    db: &Path,
    what: &str,
    value: &T,
) -> Result<Map<String, Value>> {
    object(value).map_err(|e| Error::corrupt(db, format!("{what}: {e}")))
}

/// Read the `spec` columns of `table` (with `extra` columns first), one
/// object per row in `order`.
fn objects(
    conn: &Connection,
    db: &Path,
    table: &str,
    extra: &[(&str, Col)],
    spec: &[(&str, Col)],
    order: &str,
) -> Result<Vec<Map<String, Value>>> {
    let err = |e| db_error("read", db, e);
    let all: Vec<&(&str, Col)> = extra.iter().chain(spec).collect();
    let names: Vec<String> = all.iter().map(|(n, _)| quoted(n)).collect();
    let mut select = conn
        .prepare(&format!(
            "SELECT {} FROM {table} ORDER BY {order}",
            names.join(", ")
        ))
        .map_err(err)?;
    let mut rows = select.query([]).map_err(err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(err)? {
        let mut obj = Map::new();
        for (i, (name, col)) in all.iter().enumerate() {
            let value = from_sql(name, r.get::<_, Sql>(i).map_err(err)?, *col)
                .map_err(|e| Error::corrupt(row_path(db, table, &i.to_string()), e))?;
            obj.insert((*name).to_owned(), value);
        }
        out.push(obj);
    }
    Ok(out)
}

fn insert(conn: &Connection, table: &str, spec: &[&str], values: Vec<Sql>) -> rusqlite::Result<()> {
    let names: Vec<String> = spec.iter().map(|n| quoted(n)).collect();
    let marks: Vec<String> = (1..=spec.len()).map(|i| format!("?{i}")).collect();
    conn.execute(
        &format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            names.join(", "),
            marks.join(", ")
        ),
        params_from_iter(values),
    )?;
    Ok(())
}

fn names<'a>(spec: &'a [(&'a str, Col)]) -> impl Iterator<Item = &'a str> {
    spec.iter().map(|(n, _)| *n)
}

/// Whether the documents live in the database: schema 5 or newer and a
/// `record_kinds` row for every kind of [`KINDS`].
pub(in crate::store) fn moved(conn: &Connection) -> rusqlite::Result<bool> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version < 5 {
        return Ok(false);
    }
    let n: i64 = conn.query_row(
        "SELECT count(*) FROM record_kinds WHERE kind IN
             ('entries', 'claims', 'agent_tags', 'marks', 'pickups', 'audit')",
        [],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(n).is_ok_and(|n| n == KINDS.len()))
}

/// Record that the documents moved in: a row for every kind, with the
/// `pre-sqlite-<date>` directory its files went to, or `None` when it had
/// none. A kind already marked keeps its row.
pub(in crate::store) fn mark_moved(
    conn: &Connection,
    now: OffsetDateTime,
    moved_to: impl Fn(&str) -> Option<String>,
) -> rusqlite::Result<()> {
    for kind in KINDS {
        conn.execute(
            "INSERT INTO record_kinds (kind, moved_at, moved_to) VALUES (?1, ?2, ?3)
             ON CONFLICT (kind) DO NOTHING",
            (kind, stamp(now), moved_to(kind)),
        )?;
    }
    Ok(())
}

/// Run `f` on a read-only connection when the documents live in the
/// database; `None` when they are still files (or there is no database).
pub(in crate::store) fn read<T>(
    root: &Path,
    f: impl Fn(&Connection, &Path) -> Result<T>,
) -> Result<Option<T>> {
    let (found, _) = super::memory::read(root, |conn, path| {
        super::refuse_newer(conn, path)?;
        if !moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))? {
            return Ok(None);
        }
        f(conn, path).map(Some).map_err(Fallible::Failed)
    })?;
    Ok(found)
}

/// An entry row that did not decode, by its id.
#[derive(Debug)]
pub(in crate::store) struct BadEntry {
    /// `<root>/scribe.db#entries/<id>`.
    pub path: PathBuf,
    /// Its id, when that much reads.
    pub id: Option<EntryId>,
    /// Always [`Error::Corrupt`], naming the row.
    pub error: Error,
}

/// Every entry, by id; a row that does not decode is a [`BadEntry`].
pub(in crate::store) fn entries(
    conn: &Connection,
    db: &Path,
) -> Result<Vec<std::result::Result<Entry, BadEntry>>> {
    let extra = [("content", Col::Text)];
    let mut out = Vec::new();
    // Reads project only fields present in the recorded schema.
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| db_error("read", db, e))?;
    let fields = match version {
        ..7 => &ENTRY[..ENTRY.len() - 3],
        7..17 => &ENTRY[..ENTRY.len() - 2],
        _ => ENTRY,
    };
    for mut obj in objects(conn, db, "entries", &extra, fields, "tag, n")? {
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let tag = obj
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let path = row_path(db, "entries", &id);
        let content = match obj.remove("content") {
            Some(Value::String(c)) => c,
            _ => String::new(),
        };
        let decoded = serde_json::from_value::<Entry>(Value::Object(obj))
            .map_err(|e| Error::corrupt(&path, e))
            .and_then(|entry| {
                format::checked_entry(&path, entry, format::FORMAT, &content, &id, &tag)
            });
        // A row that does not read is skipped and reported, as a file was;
        // any other failure fails the read.
        match decoded {
            Ok(entry) => out.push(Ok(entry)),
            Err(error @ Error::Corrupt { .. }) => out.push(Err(BadEntry {
                path,
                id: id.parse().ok(),
                error,
            })),
            Err(other) => return Err(other),
        }
    }
    Ok(out)
}

/// Write `entry` by its id; `new` refuses one whose id is taken, since ids
/// come from the listing and a taken one means the listing missed it.
pub(in crate::store) fn put_entry(
    conn: &Connection,
    db: &Path,
    entry: &Entry,
    new: bool,
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    let id = entry.id.to_string();
    format::validate_entry(
        &row_path(db, "entries", &id),
        entry,
        format::FORMAT,
        &entry.content,
        &id,
        entry.tag.as_str(),
    )?;
    let obj = serialized(db, "an entry", entry)?;
    let mut values = row(db, "an entry", &obj, ENTRY, &[])?;
    values.push(Sql::Text(entry.content.clone()));
    let n = i64::try_from(entry.id.number()).unwrap_or(i64::MAX);
    values.push(Sql::Integer(n));
    let mut cols: Vec<&str> = names(ENTRY).collect();
    cols.extend(["content", "n"]);
    let updated = if new {
        0
    } else {
        let set: Vec<String> = cols
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, c)| format!("{} = ?{}", quoted(c), i + 1))
            .collect();
        conn.execute(
            &format!("UPDATE entries SET {} WHERE id = ?1", set.join(", ")),
            params_from_iter(values.clone()),
        )
        .map_err(&fail)?
    };
    if updated > 0 {
        leave_no_old_page(db);
    }
    if updated == 0 {
        insert(conn, "entries", &cols, values).map_err(|e| {
            if matches!(
                e.sqlite_error_code(),
                Some(rusqlite::ErrorCode::ConstraintViolation)
            ) && new
            {
                Error::io(
                    "create",
                    row_path(db, "entries", &entry.id.to_string()),
                    std::io::Error::from(std::io::ErrorKind::AlreadyExists),
                )
            } else {
                fail(e)
            }
        })?;
    }
    Ok(())
}

/// Delete a handoff, retaining only its id so old references cannot name
/// a future handoff. Drop its pickups with it.
pub(in crate::store) fn delete_handoff(conn: &Connection, db: &Path, id: &EntryId) -> Result<()> {
    let fail = |e| db_error("delete a handoff in", db, e);
    let id = id.to_string();
    conn.execute("INSERT INTO deleted_handoffs (id) VALUES (?1)", [&id])
        .map_err(fail)?;
    conn.execute("DELETE FROM pickups WHERE entry = ?1", [&id])
        .map_err(fail)?;
    conn.execute("DELETE FROM entries WHERE id = ?1", [&id])
        .map_err(fail)?;
    Ok(())
}

/// Handoff ids that must not be allocated again.
pub(in crate::store) fn deleted_handoffs(conn: &Connection, db: &Path) -> Result<Vec<EntryId>> {
    let fail = |e| db_error("read deleted handoff ids from", db, e);
    if conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(fail)?
        < 14
    {
        return Ok(Vec::new());
    }
    let mut query = conn
        .prepare("SELECT id FROM deleted_handoffs")
        .map_err(fail)?;
    let rows = query
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(fail)?;
    rows.map(|row| {
        row.map_err(fail)?
            .parse()
            .map_err(|e| Error::corrupt(db, e))
    })
    .collect()
}

/// Every claim, by id, checked as `proposals.json` was.
pub(in crate::store) fn claims(conn: &Connection, db: &Path) -> Result<Vec<Proposal>> {
    let mut extra: Vec<(&str, Col)> = vec![("n", Col::Int), ("has_candidate", Col::Bool)];
    let cand: Vec<(String, Col)> = CANDIDATE
        .iter()
        .map(|(n, c)| (format!("c_{n}"), *c))
        .collect();
    extra.extend(cand.iter().map(|(n, c)| (n.as_str(), *c)));
    let mut out = Vec::new();
    for mut obj in objects(conn, db, "claims", &extra, CLAIM, "n")? {
        let n = obj.remove("n").and_then(|v| v.as_i64()).unwrap_or(0);
        let path = row_path(db, "claims", &format!("p-{n}"));
        obj.insert("id".into(), Value::String(format!("p-{n}")));
        let has = obj.remove("has_candidate").and_then(|v| v.as_bool()) == Some(true);
        let mut candidate = Map::new();
        for (name, _) in CANDIDATE {
            if let Some(v) = obj.remove(&format!("c_{name}")) {
                candidate.insert((*name).to_owned(), v);
            }
        }
        obj.insert(
            "candidate".into(),
            if has {
                Value::Object(candidate)
            } else {
                Value::Null
            },
        );
        let proposal: Proposal =
            serde_json::from_value(Value::Object(obj)).map_err(|e| Error::corrupt(&path, e))?;
        format::check_proposals(&path, std::slice::from_ref(&proposal))?;
        out.push(proposal);
    }
    Ok(out)
}

/// Write the claims that differ between `before` and `after` by id, and
/// delete those `after` lacks.
pub(in crate::store) fn put_claims(
    conn: &Connection,
    db: &Path,
    before: &[Proposal],
    after: &[Proposal],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    let n_of = |id: ProposalId| i64::try_from(id.number()).unwrap_or(i64::MAX);
    for claim in after {
        if before.iter().any(|b| b == claim) {
            continue;
        }
        let mut obj = serialized(db, "a claim", claim)?;
        let candidate = match obj.remove("candidate") {
            Some(Value::Object(c)) => Some(c),
            _ => None,
        };
        let mut values = row(db, "a claim", &obj, CLAIM, &["id"])?;
        let mut cols: Vec<String> = names(CLAIM).map(str::to_owned).collect();
        values.push(Sql::Integer(i64::from(candidate.is_some())));
        cols.push("has_candidate".into());
        let empty = Map::new();
        let c = candidate.as_ref().unwrap_or(&empty);
        values.extend(row(db, "a claim's candidate", c, CANDIDATE, &[])?);
        cols.extend(names(CANDIDATE).map(|n| format!("c_{n}")));
        values.push(Sql::Integer(n_of(claim.id)));
        cols.push("n".into());
        let set: Vec<String> = cols
            .iter()
            .enumerate()
            .take(cols.len() - 1)
            .map(|(i, c)| format!("{} = ?{}", quoted(c), i + 1))
            .collect();
        let updated = conn
            .execute(
                &format!(
                    "UPDATE claims SET {} WHERE n = ?{}",
                    set.join(", "),
                    cols.len()
                ),
                params_from_iter(values.clone()),
            )
            .map_err(&fail)?;
        if updated == 0 {
            let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
            insert(conn, "claims", &cols, values).map_err(&fail)?;
        } else {
            leave_no_old_page(db);
        }
    }
    for gone in before
        .iter()
        .filter(|b| !after.iter().any(|a| a.id == b.id))
    {
        conn.execute("DELETE FROM claims WHERE n = ?1", [n_of(gone.id)])
            .map_err(&fail)?;
        leave_no_old_page(db);
    }
    Ok(())
}

/// After a row's text changed: ask the write's commit to empty the WAL,
/// whose frames still hold the old page. `secure_delete` already zeroes
/// freed space in the database file, so a forgotten or redacted text
/// leaves no copy on disk, as a whole-file rewrite of an entry or of
/// `proposals.json` left none.
fn leave_no_old_page(db: &Path) {
    if let Some(root) = db.parent() {
        super::scope::Scope::truncate_later(root);
    }
}

/// Every store tag, by name.
pub(in crate::store) fn tags(conn: &Connection, db: &Path) -> Result<Vec<StoreTag>> {
    let mut out = Vec::new();
    for obj in objects(conn, db, "agent_tags", &[], TAG, "name")? {
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        out.push(
            serde_json::from_value(Value::Object(obj))
                .map_err(|e| Error::corrupt(row_path(db, "agent_tags", &name), e))?,
        );
    }
    format::check_tags(&row_path(db, "agent_tags", "*"), &out)?;
    Ok(out)
}

/// Replace every store tag.
pub(in crate::store) fn put_tags(
    conn: &Connection,
    db: &Path,
    tags: &[StoreTag],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    conn.execute("DELETE FROM agent_tags", []).map_err(&fail)?;
    let cols: Vec<&str> = names(TAG).collect();
    for tag in tags {
        let obj = serialized(db, "a store tag", tag)?;
        insert(
            conn,
            "agent_tags",
            &cols,
            row(db, "a store tag", &obj, TAG, &[])?,
        )
        .map_err(&fail)?;
    }
    Ok(())
}

/// A small list table read back as `T`, in `position` order.
fn list<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    db: &Path,
    table: &str,
    spec: &[(&str, Col)],
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for (i, obj) in objects(conn, db, table, &[], spec, "position")?
        .into_iter()
        .enumerate()
    {
        out.push(
            serde_json::from_value(Value::Object(obj))
                .map_err(|e| Error::corrupt(row_path(db, table, &i.to_string()), e))?,
        );
    }
    Ok(out)
}

/// Replace a small list table, in order.
fn put_list<T: serde::Serialize>(
    conn: &Connection,
    db: &Path,
    table: &str,
    spec: &[(&str, Col)],
    items: &[T],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    conn.execute(&format!("DELETE FROM {table}"), [])
        .map_err(&fail)?;
    let mut cols: Vec<&str> = names(spec).collect();
    cols.push("position");
    for (i, item) in items.iter().enumerate() {
        let obj = serialized(db, table, item)?;
        let mut values = row(db, table, &obj, spec, &[])?;
        values.push(Sql::Integer(i64::try_from(i).unwrap_or(i64::MAX)));
        insert(conn, table, &cols, values).map_err(&fail)?;
    }
    Ok(())
}

/// Every mark, in order.
pub(in crate::store) fn marks(conn: &Connection, db: &Path) -> Result<Vec<MarkRecord>> {
    let marks = list(conn, db, "marks", MARK)?;
    format::check_marks(&row_path(db, "marks", "*"), &marks)?;
    Ok(marks)
}

/// Replace every mark.
pub(in crate::store) fn put_marks(
    conn: &Connection,
    db: &Path,
    marks: &[MarkRecord],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    put_list(conn, db, "marks", MARK, marks, fail)
}

/// Every pickup, in order.
pub(in crate::store) fn pickups(conn: &Connection, db: &Path) -> Result<Vec<Pickup>> {
    let pickups = list(conn, db, "pickups", PICKUP)?;
    format::check_pickups(&row_path(db, "pickups", "*"), &pickups)?;
    Ok(pickups)
}

/// Replace every pickup.
pub(in crate::store) fn put_pickups(
    conn: &Connection,
    db: &Path,
    pickups: &[Pickup],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    put_list(conn, db, "pickups", PICKUP, pickups, fail)
}

/// Add one audit line at `format`; `spool` names the spooled file it came
/// from (or the `audit.jsonl:<offset>:<sha256>` key of an imported log
/// line), and a line already recorded under that key is not added again.
/// Returns whether it was added.
pub(in crate::store) fn put_audit(
    conn: &Connection,
    db: &Path,
    format: u32,
    event: &AuditEvent,
    spool: Option<&str>,
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<bool> {
    let obj = serialized(db, "an audit line", event)?;
    let mut values = row(db, "an audit line", &obj, AUDIT, &[])?;
    let mut cols: Vec<&str> = names(AUDIT).collect();
    values.push(Sql::Integer(i64::from(format)));
    cols.push("format");
    values.push(spool.map_or(Sql::Null, |s| Sql::Text(s.to_owned())));
    cols.push("spool");
    let quoted_cols: Vec<String> = cols.iter().map(|c| quoted(c)).collect();
    let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
    let added = conn
        .execute(
            &format!(
                "INSERT INTO audit ({}) VALUES ({}) ON CONFLICT (spool) DO NOTHING",
                quoted_cols.join(", "),
                marks.join(", ")
            ),
            params_from_iter(values),
        )
        .map_err(fail)?;
    Ok(added > 0)
}

/// Every audit line with its stored format, oldest first. A line of a newer
/// format refuses the read, as it does in `audit.jsonl`.
pub(in crate::store) fn audit(conn: &Connection, db: &Path) -> Result<Vec<(u32, AuditEvent)>> {
    let extra = [("id", Col::Int), ("format", Col::Int)];
    let mut out = Vec::new();
    for mut obj in objects(conn, db, "audit", &extra, AUDIT, "id")? {
        let id = obj.remove("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let found = obj
            .remove("format")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(u32::MAX);
        let path = row_path(db, "audit", &id.to_string());
        if found > AUDIT_FORMAT {
            return Err(Error::NewerFormat {
                path,
                found,
                supported: AUDIT_FORMAT,
            });
        }
        let event =
            serde_json::from_value(Value::Object(obj)).map_err(|e| Error::corrupt(&path, e))?;
        out.push((found, event));
    }
    Ok(out)
}

/// Format a pending spooled [`AuditEvent`] as one `audit.jsonl` row in the
/// `audit` table's JSONL schema (`id: null`, `format`, [`AUDIT`] columns,
/// `spool`).
pub(in crate::store) fn audit_export_row(
    path: &Path,
    format: u32,
    event: &AuditEvent,
    spool: &str,
) -> Result<Value> {
    let obj = serialized(path, "an audit line", event)?;
    let values = row(path, "an audit line", &obj, AUDIT, &[])?;
    let mut out = Map::new();
    out.insert(String::from("id"), Value::Null);
    out.insert(
        String::from("format"),
        export_sql_value(Sql::Integer(i64::from(format))),
    );
    for ((name, _), sql) in AUDIT.iter().zip(values) {
        out.insert((*name).to_owned(), export_sql_value(sql));
    }
    out.insert(
        String::from("spool"),
        export_sql_value(Sql::Text(spool.to_owned())),
    );
    Ok(Value::Object(out))
}

/// How many rows `table` holds, for a move's count check.
pub(in crate::store) fn count(conn: &Connection, table: &str) -> rusqlite::Result<usize> {
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
        row.get(0)
    })?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// The spool files already drained into `audit`, by name (excluding
/// `audit.jsonl:<offset>:<sha256>` idempotency keys).
pub(in crate::store) fn spooled_names(conn: &Connection, db: &Path) -> Result<Vec<String>> {
    let err = |e| db_error("read", db, e);
    let mut select = conn
        .prepare("SELECT spool FROM audit WHERE spool IS NOT NULL AND spool LIKE '%.json'")
        .map_err(err)?;
    let rows = select.query_map([], |row| row.get(0)).map_err(err)?;
    rows.collect::<rusqlite::Result<Vec<String>>>().map_err(err)
}

/// An erased text or legacy source token's UTF-8 byte length and SHA-256
/// digest (64 lowercase hex characters), keyed with its obligation's salt.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct TextDigest {
    pub len: usize,
    pub sha256: String,
}

/// One committed entry-forget archive erasure waiting for post-commit
/// completion across legacy archives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::store) struct PendingArchiveForget {
    pub id: i64,
    pub entry: EntryId,
    pub salt: String,
    pub digests: Vec<TextDigest>,
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether any committed entry forget still waits for post-commit archive
/// cleanup.
pub(in crate::store) fn has_pending_archive_forgets(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pending_archive_forgets)",
        [],
        |row| row.get(0),
    )
}

/// Record one durable archive-erasure obligation inside the forget's own
/// transaction (STD-03 R8). Stores only `entry`, `salt` and salted SHA-256
/// `digests` — never cleartext.
pub(in crate::store) fn put_pending_archive_forget(
    conn: &Connection,
    db: &Path,
    entry: &EntryId,
    salt: &str,
    digests: &[TextDigest],
    now: OffsetDateTime,
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    let encoded = serde_json::to_string(digests)
        .map_err(|e| Error::corrupt(db, format!("pending archive forget digests: {e}")))?;
    conn.execute(
        "INSERT INTO pending_archive_forgets (entry, salt, digests, created)
         VALUES (?1, ?2, ?3, ?4)",
        (entry.to_string(), salt, &encoded, stamp(now)),
    )
    .map_err(fail)?;
    Ok(())
}

/// Every pending archive-erasure obligation, oldest first.
pub(in crate::store) fn pending_archive_forgets(
    conn: &Connection,
    db: &Path,
) -> Result<Vec<PendingArchiveForget>> {
    let err = |e| db_error("read", db, e);
    let mut select = conn
        .prepare("SELECT id, entry, salt, digests FROM pending_archive_forgets ORDER BY id")
        .map_err(err)?;
    let rows = select
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(err)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, entry_str, salt, digests_json) = row.map_err(err)?;
        let path = row_path(db, "pending_archive_forgets", &id.to_string());
        let entry: EntryId = entry_str.parse().map_err(|e| Error::corrupt(&path, e))?;
        if !is_hex64(&salt) {
            return Err(Error::corrupt(&path, "salt must be 64 lowercase hex chars"));
        }
        let digests: Vec<TextDigest> =
            serde_json::from_str(&digests_json).map_err(|e| Error::corrupt(&path, e))?;
        for digest in &digests {
            if digest.len == 0 || !is_hex64(&digest.sha256) {
                return Err(Error::corrupt(
                    &path,
                    "digest must have positive length and 64 lowercase hex chars",
                ));
            }
        }
        out.push(PendingArchiveForget {
            id,
            entry,
            salt,
            digests,
        });
    }
    Ok(out)
}

/// Remove completed archive-erasure obligations by row id.
pub(in crate::store) fn delete_pending_archive_forgets(
    conn: &Connection,
    ids: &[i64],
    fail: impl Fn(rusqlite::Error) -> Error,
) -> Result<()> {
    for &id in ids {
        conn.execute("DELETE FROM pending_archive_forgets WHERE id = ?1", [id])
            .map_err(&fail)?;
    }
    Ok(())
}
