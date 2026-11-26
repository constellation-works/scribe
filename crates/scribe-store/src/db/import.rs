//! The file store's archives in `scribe.db` (docs/design/scribe/2_design.md
//! §15.7, phase 8).
//!
//! - **Import.** The first write (or `scribe store migrate`) imports each
//!   `pre-sqlite-<date>/` and `pre-issue-<date>/` directory into the
//!   `archive_records` table, in the write's own transaction, and checks the
//!   row count of each kind against the files. After the commit the
//!   directory is removed. A directory already imported is only removed,
//!   never imported twice, so a forget made since stays made.
//! - **No decoder.** The import reads no record format: a Markdown file is
//!   kept as its frontmatter (YAML read as JSON) and body, a JSON document
//!   and an audit line as JSON, anything else as text. A file that does not
//!   parse is kept as text, so nothing is lost.
//! - **Forget.** A session or entry forget erases from these rows in its
//!   own transaction, by field name: the session's records, switches,
//!   cursors and marks go; entries and claims that cite it lose the cite
//!   and its quotes; legacy audit lines that name it keep their audit
//!   fields and lose every other field. Unrelated records are kept.

mod forget;

pub(crate) use forget::{scrub_entry, scrub_session};

use crate::db::archive as rows;
use crate::db::files;
use crate::db::{self, Conn as Connection, DB_FILE};
use crate::error::{Error, Result};
use crate::{
    AUDIT_FILE, CONTROL_FILE, ENTRIES_DIR, MARKS_FILE, PICKUPS_FILE, PROPOSALS_FILE, RECORD_EXT,
    SESSIONS_DIR, Store,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use time::OffsetDateTime;

/// The directory the move put the file store's files in.
pub(crate) const MOVED_PREFIX: &str = "pre-sqlite-";
/// The directory issue records went to once converted to entries.
pub(crate) const ISSUE_PREFIX: &str = "pre-issue-";

/// What an archive import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Imported {
    /// Archive directories imported.
    pub archives: usize,
    /// Records imported.
    pub records: usize,
}

impl Store {
    /// The archive directories under the root, by name.
    pub(crate) fn archive_dirs(&self) -> Result<Vec<String>> {
        if !self.root_exists()? {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for name in files::list_root(&self.root)? {
            if (name.starts_with(MOVED_PREFIX) || name.starts_with(ISSUE_PREFIX))
                && cw_fs::inspect_dir(&self.root.join(&name))?
            {
                out.push(name);
            }
        }
        out.sort();
        Ok(out)
    }

    /// In the write's transaction: import every archive directory not yet
    /// imported, check its counts, erase what earlier forgets erased, and
    /// note every directory for removal after the commit.
    pub(crate) fn import_archives(
        &self,
        conn: &Connection,
        now: OffsetDateTime,
    ) -> Result<Imported> {
        let db = self.root.join(DB_FILE);
        let mut out = Imported::default();
        for name in self.archive_dirs()? {
            let dir = self.root.join(&name);
            if !rows::imported(conn, &db, &name)? {
                out.records += self.import_dir(conn, &db, &name, &dir, None, now)?;
                out.archives += 1;
            }
            self.after(|a| a.archive_removals.push(dir));
        }
        if out.archives > 0 {
            self.replay_forgets(conn, &db)?;
        }
        Ok(out)
    }

    /// Import the files under `dir` (only `only` of its entries, when
    /// given) as the archive `name`, and check the count of each kind.
    fn import_dir(
        &self,
        conn: &Connection,
        db: &Path,
        name: &str,
        dir: &Path,
        only: Option<&[String]>,
        now: OffsetDateTime,
    ) -> Result<usize> {
        let mut files = Vec::new();
        let top = match only {
            Some(only) => only.to_vec(),
            None => cw_fs::list(dir)?,
        };
        for item in top {
            collect(&dir.join(&item), &item, &mut files)?;
        }
        files.sort();
        let issue = name.starts_with(ISSUE_PREFIX);
        let mut expected: BTreeMap<String, usize> = BTreeMap::new();
        for rel in &files {
            let path = dir.join(rel);
            let bytes = files::read_archived(&path)?;
            let kind = classify(rel, issue);
            if kind == "audit" {
                for (n, line) in bytes.split(|b| *b == b'\n').enumerate() {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let line_no = u32::try_from(n + 1).unwrap_or(u32::MAX);
                    rows::put_record(conn, db, name, rel, line_no, kind, &json_or_text(line))?;
                    *expected.entry(kind.to_owned()).or_default() += 1;
                }
            } else {
                let record = match kind {
                    "session" | "checkpoint" | "entry" | "issue" => markdown(&bytes),
                    "other" => text(&bytes),
                    _ => json_or_text(&bytes),
                };
                rows::put_record(conn, db, name, rel, 0, kind, &record)?;
                *expected.entry(kind.to_owned()).or_default() += 1;
            }
        }
        rows::put_archive(conn, db, name, files.len(), now)?;
        let found = rows::counts(conn, db, name)?;
        if found != expected {
            return Err(Error::corrupt(
                db,
                format!(
                    "the archive import found {expected:?} records in {name} but scribe.db holds {found:?}"
                ),
            ));
        }
        Ok(expected.values().sum())
    }

    /// Import file-store files an interrupted move left in the root, after
    /// their records moved into `scribe.db`, as one more archive.
    pub(crate) fn import_leftovers(
        &self,
        conn: &Connection,
        items: &[String],
        now: OffsetDateTime,
    ) -> Result<Imported> {
        if items.is_empty() {
            return Ok(Imported::default());
        }
        let db = self.root.join(DB_FILE);
        let base = format!("{MOVED_PREFIX}{}", moved_date(now));
        let mut name = base.clone();
        let mut n = 2;
        while rows::imported(conn, &db, &name)? || cw_fs::inspect_dir(&self.root.join(&name))? {
            name = format!("{base}.{n}");
            n += 1;
        }
        let records = self.import_dir(conn, &db, &name, &self.root, Some(items), now)?;
        for item in items {
            self.after(|a| a.archive_removals.push(self.root.join(item)));
        }
        self.replay_forgets(conn, &db)?;
        Ok(Imported {
            archives: 1,
            records,
        })
    }

    /// Erase from freshly imported archives what forgets made while they
    /// were files erased: every forgotten session, and every entry forget
    /// whose archive cleanup had not finished.
    fn replay_forgets(&self, conn: &Connection, db: &Path) -> Result<()> {
        for (id, tombstone) in db::sessions::forgotten(conn, db::schema(), db)? {
            scrub_session(conn, db, &id, Some(tombstone.number))?;
        }
        let pending = db::docs::pending_archive_forgets(conn, db)?;
        for item in &pending {
            forget::scrub_pending_entry(conn, db, item)?;
        }
        let ids: Vec<i64> = pending.iter().map(|p| p.id).collect();
        db::docs::delete_pending_archive_forgets(conn, &ids, |e| {
            db::db_error("clear pending archive forgets in", db, e)
        })
    }

    /// Remove an archive directory (or a leftover file) whose records are
    /// in `scribe.db`: every file, then every directory, deepest first.
    pub(crate) fn remove_archive(&self, path: &Path) -> Result<()> {
        if !is_dir(path)? {
            return if cw_fs::inspect_file(path)? {
                files::remove(&self.root, path)
            } else {
                Ok(())
            };
        }
        if !cw_fs::inspect_dir(path)? {
            return Ok(());
        }
        for name in cw_fs::list(path)? {
            self.remove_archive(&path.join(name))?;
        }
        files::remove_empty_dir(path)
    }
}

/// Every regular file under `path` (itself when a file), as paths relative
/// to the archive, skipping Finder metadata (`.DS_Store`, `._*`).
fn collect(path: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if base == ".DS_Store" || base.starts_with("._") {
        // Never a record; removed with the archive.
        return Ok(());
    }
    if is_dir(path)? {
        if cw_fs::inspect_dir(path)? {
            for name in cw_fs::list(path)? {
                collect(&path.join(&name), &format!("{rel}/{name}"), out)?;
            }
        }
    } else if cw_fs::inspect_file(path)? {
        out.push(rel.to_owned());
    }
    Ok(())
}

/// Whether `path` is a directory itself (a link is not followed, and is
/// refused by the inspection that follows).
fn is_dir(path: &Path) -> Result<bool> {
    Ok(files::symlink_metadata(path)?.is_some_and(|meta| meta.is_dir()))
}

/// The kind of the archived file at `rel`.
fn classify(rel: &str, issue: bool) -> &'static str {
    let first = rel.split('/').next().unwrap_or(rel);
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let markdown = name.ends_with(RECORD_EXT);
    if issue {
        return if markdown { "issue" } else { "other" };
    }
    if is_moved_variant(first, SESSIONS_DIR) && rel.contains('/') {
        if name.starts_with('.') && name.ends_with(".rebuild.md") {
            return "checkpoint";
        }
        return if markdown { "session" } else { "other" };
    }
    if is_moved_variant(first, ENTRIES_DIR) && rel.contains('/') {
        return if markdown { "entry" } else { "other" };
    }
    for (base, kind) in [
        (PROPOSALS_FILE, "claims"),
        (CONTROL_FILE, "control"),
        (crate::TAGS_FILE, "tags"),
        (MARKS_FILE, "marks"),
        (PICKUPS_FILE, "pickups"),
        (AUDIT_FILE, "audit"),
    ] {
        if !rel.contains('/') && is_moved_variant(first, base) {
            return kind;
        }
    }
    "other"
}

/// A Markdown record as `{"front": {...}, "body": "..."}`; text when its
/// frontmatter does not parse.
fn markdown(bytes: &[u8]) -> Value {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return self::text(bytes);
    };
    let Some(rest) = text.strip_prefix("---\n") else {
        return json!({ "text": text });
    };
    let (front, body) = if let Some(body) = rest.strip_prefix("---\n") {
        ("", body)
    } else if let Some(at) = rest.find("\n---\n") {
        (&rest[..=at], &rest[at + 5..])
    } else if let Some(front) = rest.strip_suffix("\n---") {
        (front, "")
    } else {
        return json!({ "text": text });
    };
    let front = if front.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        match yaml_serde::from_str::<Value>(front) {
            Ok(value @ Value::Object(_)) => value,
            _ => return json!({ "text": text }),
        }
    };
    json!({ "front": front, "body": body })
}

/// JSON, or text when it does not parse.
fn json_or_text(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| text(bytes))
}

/// `{"text": ...}`, or `{"hex": ...}` for bytes that are not UTF-8.
fn text(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => json!({ "text": text }),
        Err(_) => {
            let mut hex = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                let _ = write!(&mut hex, "{b:02x}");
            }
            json!({ "hex": hex })
        }
    }
}

/// `YYYY-MM-DD` of `now`, as the move dated its directories.
fn moved_date(now: OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

pub(crate) fn is_moved_variant(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

pub(crate) fn is_empty(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Array(a) => a.is_empty(),
        serde_json::Value::Object(o) => o.is_empty(),
        serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}
