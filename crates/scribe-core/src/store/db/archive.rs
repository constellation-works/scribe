//! The archive tables (docs/design/scribe/2_design.md §15.7, phase 8): the
//! records of the `pre-sqlite-<date>/` and `pre-issue-<date>/` directories
//! the file store left, imported once and kept as JSON so `forget` can
//! erase from them without a file decoder.

use super::{db_error, stamp};
use crate::error::{Error, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use time::OffsetDateTime;

/// Version 20 (phase 8): `archives`, one row per imported directory, and
/// `archive_records`, one row per archived file (one per line of an
/// `audit.jsonl`). The record is JSON: a Markdown file as
/// `{"front": {...}, "body": "..."}`, a JSON document or an audit line as
/// itself, and anything else as `{"text": "..."}` (or `{"hex": "..."}` for
/// bytes that are not UTF-8). `IF NOT EXISTS`, so a step replayed on a
/// database an older build set back finds its tables kept.
pub(super) const V20: &str = "
CREATE TABLE IF NOT EXISTS archives (
    name     TEXT    PRIMARY KEY NOT NULL
             CHECK (name GLOB 'pre-sqlite-*' OR name GLOB 'pre-issue-*'),
    imported TEXT    NOT NULL,
    files    INTEGER NOT NULL CHECK (files >= 0)
) STRICT;

CREATE TABLE IF NOT EXISTS archive_records (
    id      INTEGER PRIMARY KEY,
    archive TEXT    NOT NULL,
    path    TEXT    NOT NULL CHECK (length(path) >= 1),
    line    INTEGER NOT NULL CHECK (line >= 0),
    kind    TEXT    NOT NULL CHECK (kind IN ('session', 'checkpoint', 'entry', 'issue',
                'claims', 'control', 'tags', 'marks', 'pickups', 'audit', 'other')),
    record  TEXT    NOT NULL CHECK (json_valid(record)),
    UNIQUE (archive, path, line)
) STRICT;

CREATE INDEX IF NOT EXISTS archive_records_kind ON archive_records (kind);
";

/// One archived record.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::store) struct ArchiveRow {
    pub id: i64,
    pub archive: String,
    pub path: String,
    pub line: u32,
    pub kind: String,
    pub record: Value,
}

/// Whether the archive directory `name` was imported.
pub(in crate::store) fn imported(conn: &Connection, db: &Path, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM archives WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )
    .map_err(|e| db_error("read archives from", db, e))
}

/// Record that the directory `name` and its `files` were imported.
pub(in crate::store) fn put_archive(
    conn: &Connection,
    db: &Path,
    name: &str,
    files: usize,
    now: OffsetDateTime,
) -> Result<()> {
    conn.execute(
        "INSERT INTO archives (name, imported, files) VALUES (?1, ?2, ?3)",
        (name, stamp(now), i64::try_from(files).unwrap_or(i64::MAX)),
    )
    .map(drop)
    .map_err(|e| db_error("import archives into", db, e))
}

/// Insert one archived record.
pub(in crate::store) fn put_record(
    conn: &Connection,
    db: &Path,
    archive: &str,
    path: &str,
    line: u32,
    kind: &str,
    record: &Value,
) -> Result<()> {
    conn.execute(
        "INSERT INTO archive_records (archive, path, line, kind, record)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (archive, path, line, kind, record.to_string()),
    )
    .map(drop)
    .map_err(|e| db_error("import archives into", db, e))
}

/// How many records of each kind the archive `name` holds.
pub(in crate::store) fn counts(
    conn: &Connection,
    db: &Path,
    name: &str,
) -> Result<BTreeMap<String, usize>> {
    let err = |e| db_error("read archives from", db, e);
    let mut select = conn
        .prepare("SELECT kind, count(*) FROM archive_records WHERE archive = ?1 GROUP BY kind")
        .map_err(err)?;
    let rows = select
        .query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(err)?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (kind, n) = row.map_err(err)?;
        out.insert(kind, usize::try_from(n).unwrap_or(0));
    }
    Ok(out)
}

/// Every archived record of the `kinds`, in archive, path and line order.
pub(in crate::store) fn records(
    conn: &Connection,
    db: &Path,
    kinds: &[&str],
) -> Result<Vec<ArchiveRow>> {
    let err = |e| db_error("read archives from", db, e);
    let mut select = conn
        .prepare(
            "SELECT id, archive, path, line, kind, record FROM archive_records
             ORDER BY archive, path, line",
        )
        .map_err(err)?;
    let rows = select
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(err)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, archive, path, line, kind, record) = row.map_err(err)?;
        if !kinds.contains(&kind.as_str()) {
            continue;
        }
        let record = serde_json::from_str(&record).map_err(|e| {
            Error::corrupt(
                db.with_file_name(format!("scribe.db#archive_records/{id}")),
                e,
            )
        })?;
        out.push(ArchiveRow {
            id,
            archive,
            path,
            line,
            kind,
            record,
        });
    }
    Ok(out)
}

/// Replace an archived record's JSON.
pub(in crate::store) fn update(
    conn: &Connection,
    db: &Path,
    id: i64,
    record: &Value,
) -> Result<()> {
    conn.execute(
        "UPDATE archive_records SET record = ?2 WHERE id = ?1",
        (id, record.to_string()),
    )
    .map(drop)
    .map_err(|e| db_error("erase from the archives of", db, e))
}

/// Delete an archived record.
pub(in crate::store) fn delete(conn: &Connection, db: &Path, id: i64) -> Result<()> {
    conn.execute("DELETE FROM archive_records WHERE id = ?1", [id])
        .map(drop)
        .map_err(|e| db_error("erase from the archives of", db, e))
}
