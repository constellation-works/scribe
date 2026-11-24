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

use crate::db::archive::{self as rows, ArchiveRow};
use crate::db::{self, Conn as Connection, DB_FILE};
use crate::documents::is_moved_variant;
use crate::entry::{Entry, EntryId};
use crate::error::{Error, Result};
use crate::proposal::Proposal;
use crate::session::SessionId;
use crate::{
    AUDIT_FILE, CONTROL_FILE, ENTRIES_DIR, MARKS_FILE, PICKUPS_FILE, PROPOSALS_FILE, RECORD_EXT,
    SESSIONS_DIR, Store, fsio,
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

/// The tag converted issue records carry.
const ISSUE_TAG: &str = "issue";

/// What an archive import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Imported {
    /// Archive directories imported.
    pub archives: usize,
    /// Records imported.
    pub records: usize,
}

/// The keys of an audit line's structural fields (`AuditEvent`): timestamps,
/// command paths, statuses, record ids, codes, agent names, counts and the
/// `issue` record's ids, config trigger and hash. They never hold free text,
/// so a forget keeps them, and no entry's words are matched under them.
const AUDIT_STANDARD_KEYS: &[&str] = &[
    "format",
    "at",
    "command",
    "status",
    "target",
    "error_code",
    "author",
    "instruction_session",
    "instruction_message",
    "issue",
    "duration_ms",
];

impl Store {
    /// The archive directories under the root, by name.
    pub(crate) fn archive_dirs(&self) -> Result<Vec<String>> {
        if !self.root_exists()? {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for name in fsio::list_dir(&self.root)? {
            if (name.starts_with(MOVED_PREFIX) || name.starts_with(ISSUE_PREFIX))
                && fsio::inspect_subdir(&self.root.join(&name))?
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
            None => fsio::list_dir(dir)?,
        };
        for item in top {
            collect(&dir.join(&item), &item, &mut files)?;
        }
        files.sort();
        let issue = name.starts_with(ISSUE_PREFIX);
        let mut expected: BTreeMap<String, usize> = BTreeMap::new();
        for rel in &files {
            let path = dir.join(rel);
            let bytes = fsio::read(&path)?;
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
        while rows::imported(conn, &db, &name)? || fsio::inspect_subdir(&self.root.join(&name))? {
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
        for (id, tombstone) in db::sessions::forgotten(conn, db)? {
            scrub_session(conn, db, &id, Some(tombstone.number))?;
        }
        let pending = db::docs::pending_archive_forgets(conn, db)?;
        for item in &pending {
            let matcher = TextMatcher::digests(&item.salt, &item.digests, db)?;
            scrub_entry_with(conn, db, &item.entry, None, matcher)?;
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
            return if fsio::inspect_file(path)? {
                fsio::remove_private(path)
            } else {
                Ok(())
            };
        }
        if !fsio::inspect_subdir(path)? {
            return Ok(());
        }
        for name in fsio::list_dir(path)? {
            self.remove_archive(&path.join(name))?;
        }
        std::fs::remove_dir(path).map_err(|e| Error::io("remove", path, e))
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
        if fsio::inspect_subdir(path)? {
            for name in fsio::list_dir(path)? {
                collect(&path.join(&name), &format!("{rel}/{name}"), out)?;
            }
        }
    } else if fsio::inspect_file(path)? {
        out.push(rel.to_owned());
    }
    Ok(())
}

/// Whether `path` is a directory itself (a link is not followed, and is
/// refused by the inspection that follows).
fn is_dir(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.is_dir()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io("inspect", path, e)),
    }
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
        (crate::documents::TAGS_FILE, "tags"),
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

/// Whether a value is the string `s`.
fn is_str(value: Option<&Value>, s: impl Fn(&str) -> bool) -> bool {
    value.and_then(Value::as_str).is_some_and(s)
}

/// Whether any string anywhere in `value` satisfies `f`.
fn any_string(value: &Value, f: &dyn Fn(&str) -> bool) -> bool {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter().any(|v| any_string(v, f)),
        Value::Object(map) => map.values().any(|v| any_string(v, f)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// The marker a cite to an erased session carries (`Cite::mark_deleted`).
fn deleted_cite(cite: &mut Map<String, Value>) {
    cite.insert("session".into(), json!("deleted"));
    cite.insert("message".into(), Value::Null);
    cite.insert("at".into(), json!("1970-01-01T00:00:00Z"));
    cite.insert("prompt_sha256".into(), json!(""));
    cite.insert("line".into(), json!(0));
    cite.insert("cwd".into(), json!(""));
}

/// Erase session `id` (`s-<number>` too) from the archive tables. Returns
/// how many of the session's own records (its record and checkpoints) were
/// deleted, and whether anything changed.
pub(crate) fn scrub_session(
    conn: &Connection,
    db: &Path,
    id: &SessionId,
    number: Option<std::num::NonZeroU64>,
) -> Result<(usize, bool)> {
    let number = number.map(|n| format!("s-{n}"));
    let matches = |value: &str| value == id.as_str() || number.as_deref() == Some(value);
    let named = |value: &str| matches(value.strip_prefix("session:").unwrap_or(value));
    let mut own = 0;
    let mut changed = false;
    let mut removed: Vec<String> = Vec::new();
    // Entries first: the claims and pickups about an entry this removes
    // go with it.
    for row in rows::records(conn, db, &["entry"])? {
        let mut record = row.record.clone();
        let remove = match record.get_mut("front").and_then(Value::as_object_mut) {
            Some(front) => detach_entry(front, &matches),
            None => any_string(&record, &|s| s.contains(id.as_str())),
        };
        if remove {
            if let Some(entry) = record.pointer("/front/id").and_then(Value::as_str) {
                removed.push(entry.to_owned());
            }
            rows::delete(conn, db, row.id)?;
            changed = true;
        } else if record != row.record {
            rows::update(conn, db, row.id, &record)?;
            changed = true;
        }
    }
    let kinds = [
        "session",
        "checkpoint",
        "claims",
        "control",
        "marks",
        "pickups",
        "audit",
        "other",
    ];
    for row in rows::records(conn, db, &kinds)? {
        let mut record = row.record.clone();
        let mut delete = false;
        match row.kind.as_str() {
            "session" | "checkpoint" => {
                let of_session = match record.pointer("/front/id") {
                    Some(value) => is_str(Some(value), matches),
                    None => {
                        any_string(&record, &|s| s.contains(id.as_str()))
                            || row.path.contains(id.as_str())
                    }
                };
                if of_session {
                    own += 1;
                    delete = true;
                }
            }
            "other" => {
                delete = row.path.contains(id.as_str())
                    || any_string(&record, &|s| s.contains(id.as_str()));
            }
            "claims" => match record.get_mut("proposals").and_then(Value::as_array_mut) {
                Some(claims) => detach_claims(claims, &matches, &removed),
                None => delete = any_string(&record, &|s| s.contains(id.as_str())),
            },
            "control" => {
                for key in ["sessions", "codex_cursors"] {
                    if let Some(map) = record.get_mut(key).and_then(Value::as_object_mut) {
                        map.retain(|sid, _| !matches(sid));
                    }
                }
                if let Some(map) = record.get_mut("sessions").and_then(Value::as_object_mut) {
                    for switch in map.values_mut() {
                        if let Some(links) = switch
                            .get_mut("pending_links")
                            .and_then(Value::as_array_mut)
                        {
                            links.retain(|link| {
                                !(is_str(link.get("kind"), |k| k == "continues")
                                    && is_str(link.get("session"), matches))
                            });
                        }
                    }
                }
            }
            "marks" => {
                if let Some(marks) = record.get_mut("marks").and_then(Value::as_array_mut) {
                    marks.retain(|mark| !is_str(mark.get("session"), matches));
                }
            }
            "pickups" => {
                if let Some(pickups) = record.get_mut("pickups").and_then(Value::as_array_mut) {
                    pickups.retain(|p| !is_str(p.get("entry"), |e| removed.iter().any(|r| r == e)));
                }
            }
            "audit" => match record.as_object_mut() {
                Some(line) => {
                    let related = is_str(line.get("target"), named)
                        || is_str(line.get("instruction_session"), matches)
                        || is_str(
                            line.get("issue").and_then(|issue| issue.get("session")),
                            matches,
                        );
                    if related {
                        line.retain(|key, _| AUDIT_STANDARD_KEYS.contains(&key.as_str()));
                    }
                }
                None => delete = any_string(&record, &|s| s.contains(id.as_str())),
            },
            _ => {}
        }
        if delete {
            rows::delete(conn, db, row.id)?;
            changed = true;
        } else if record != row.record {
            rows::update(conn, db, row.id, &record)?;
            changed = true;
        }
    }
    Ok((own, changed))
}

/// Detach an archived entry's frontmatter from the erased session, as the
/// live store detaches an entry. `true` when the entry goes: a handoff
/// naming the session, or scribe's own record of it.
fn detach_entry(front: &mut Map<String, Value>, matches: &dyn Fn(&str) -> bool) -> bool {
    let names_session = |r: &Value| {
        r.as_str()
            .and_then(|r| r.strip_prefix("session:"))
            .is_some_and(matches)
    };
    let related_names = front
        .get("related")
        .and_then(Value::as_array)
        .is_some_and(|related| related.iter().any(names_session));
    if is_str(front.get("tag"), |t| t == crate::handoff::HANDOFF_TAG) && related_names {
        return true;
    }
    let cited = front
        .get("cite")
        .and_then(|c| c.get("session"))
        .and_then(Value::as_str)
        .is_some_and(|s| s != "deleted" && matches(s));
    if cited {
        if is_str(front.get("author").and_then(|a| a.get("kind")), |k| {
            k == "scribe"
        }) {
            return true;
        }
        if let Some(cite) = front.get_mut("cite").and_then(Value::as_object_mut) {
            deleted_cite(cite);
        }
        if let Some(from) = front.get_mut("content_from").and_then(Value::as_object_mut)
            && is_str(from.get("kind"), |k| k == "message")
        {
            from.insert("message".into(), json!("deleted"));
            from.insert("at".into(), json!("1970-01-01T00:00:00Z"));
        }
        if front.contains_key("window") {
            front.insert("window".into(), Value::Null);
        }
    }
    if is_str(front.get("evidence"), |e| {
        e.strip_prefix("session:").is_some_and(matches)
    }) {
        front.insert("evidence".into(), json!("session:deleted"));
    }
    if related_names {
        if let Some(related) = front.get_mut("related").and_then(Value::as_array_mut) {
            related.retain(|r| !names_session(r));
            if !related.iter().any(|r| r == "session:deleted") {
                related.push(json!("session:deleted"));
            }
        }
        if front.get("decision").is_some_and(|d| !d.is_null()) {
            front.insert("quote".into(), Value::Null);
            if let Some(decision) = front.get_mut("decision").and_then(Value::as_object_mut) {
                decision.insert("reason_quote".into(), Value::Null);
            }
        }
    }
    if let Some(item) = front.get_mut("item").and_then(Value::as_object_mut)
        && is_str(item.get("answer").and_then(|a| a.get("session")), matches)
    {
        item.insert("answer".into(), Value::Null);
    }
    for key in ["instruction", "removal_instruction"] {
        if let Some(instruction) = front.get_mut(key).and_then(Value::as_object_mut)
            && is_str(instruction.get("session"), matches)
        {
            instruction.insert("quote".into(), Value::Null);
        }
    }
    false
}

/// Detach archived claims from the erased session: a claim sourced from it,
/// or about an entry the forget removed, goes; a settlement or instruction
/// from it loses its quote and cite.
fn detach_claims(claims: &mut Vec<Value>, matches: &dyn Fn(&str) -> bool, removed: &[String]) {
    let removed_entry = |v: Option<&Value>| is_str(v, |e| removed.iter().any(|r| r == e));
    claims.retain(|claim| {
        let source = is_str(claim.pointer("/candidate/cite/session"), |s| {
            s != "deleted" && matches(s)
        });
        !(source || removed_entry(claim.get("older")) || removed_entry(claim.get("newer")))
    });
    for claim in claims.iter_mut() {
        let Some(claim) = claim.as_object_mut() else {
            continue;
        };
        let settled = is_str(
            claim.get("settled_by").and_then(|c| c.get("session")),
            |s| s != "deleted" && matches(s),
        );
        let instructed = is_str(
            claim.get("instruction").and_then(|i| i.get("session")),
            matches,
        );
        if settled || instructed {
            if let Some(instruction) = claim.get_mut("instruction").and_then(Value::as_object_mut) {
                instruction.insert("quote".into(), Value::Null);
            }
            if settled
                && let Some(cite) = claim.get_mut("settled_by").and_then(Value::as_object_mut)
            {
                deleted_cite(cite);
            }
        }
    }
}

/// Erase entry `entry` from the archive tables: its archived files, the
/// issue record it was converted from, the claims about it, and every
/// audit line whose free-text fields carry its words (`proposals` names
/// the live claims whose texts are erased with it). Returns whether
/// anything changed.
pub(crate) fn scrub_entry(
    conn: &Connection,
    db: &Path,
    entry: &Entry,
    proposals: &[Proposal],
) -> Result<bool> {
    let texts = crate::documents::erased_texts(entry, proposals);
    scrub_entry_with(conn, db, &entry.id, Some(entry), TextMatcher::texts(texts))
}

/// [`scrub_entry`] for the entry `id`, matching erased texts with `matcher`.
fn scrub_entry_with(
    conn: &Connection,
    db: &Path,
    id: &EntryId,
    entry: Option<&Entry>,
    mut matcher: TextMatcher,
) -> Result<bool> {
    let id_str = id.to_string();
    let mut changed = false;
    // Gather the archived copies' words first, so an audit line that holds
    // a pre-edit text is found too.
    let mut doomed = Vec::new();
    for row in rows::records(conn, db, &["entry", "issue", "claims"])? {
        match row.kind.as_str() {
            "entry" if is_str(row.record.pointer("/front/id"), |s| s == id_str) => {
                archived_entry_texts(&row.record, &mut matcher.texts);
                doomed.push(row.id);
            }
            "issue" if is_issue_source(&row, id, entry, &matcher) => {
                push_text(&mut matcher.texts, row.record.get("body"));
                push_text(&mut matcher.texts, row.record.get("text"));
                doomed.push(row.id);
            }
            "claims" => {
                if let Some(claims) = row.record.get("proposals").and_then(Value::as_array) {
                    for claim in claims {
                        claim_texts(claim, &id_str, &mut matcher.texts);
                    }
                }
            }
            _ => {}
        }
    }
    matcher.texts.retain(|t| !t.trim().is_empty());
    matcher.texts.sort();
    matcher.texts.dedup();
    for row in rows::records(conn, db, &["audit"])? {
        let carries = match &row.record {
            Value::Object(map) => map.iter().any(|(k, v)| {
                !AUDIT_STANDARD_KEYS.contains(&k.as_str()) && any_string(v, &|s| matcher.matches(s))
            }),
            other @ (Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)) => any_string(other, &|s| matcher.matches(s)),
        };
        if carries {
            rows::delete(conn, db, row.id)?;
            changed = true;
        }
    }
    for row in rows::records(conn, db, &["claims"])? {
        let mut record = row.record.clone();
        if let Some(claims) = record.get_mut("proposals").and_then(Value::as_array_mut) {
            forget_claims(claims, &id_str);
        }
        if record != row.record {
            rows::update(conn, db, row.id, &record)?;
            changed = true;
        }
    }
    for row in rows::records(conn, db, &["pickups", "marks"])? {
        let mut record = row.record.clone();
        for key in ["pickups", "marks"] {
            if let Some(list) = record.get_mut(key).and_then(Value::as_array_mut) {
                list.retain(|item| !is_str(item.get("entry"), |e| e == id_str));
            }
        }
        if record != row.record {
            rows::update(conn, db, row.id, &record)?;
            changed = true;
        }
    }
    for id in doomed {
        rows::delete(conn, db, id)?;
        changed = true;
    }
    Ok(changed)
}

/// Whether an archived issue record is the source of the converted entry
/// `id`: its body holds the entry's verified quote, or (for a replayed
/// forget) its text matches the erased digests.
fn is_issue_source(row: &ArchiveRow, id: &EntryId, entry: Option<&Entry>, m: &TextMatcher) -> bool {
    if id.tag().as_str() != ISSUE_TAG {
        return false;
    }
    let body = row
        .record
        .get("body")
        .or_else(|| row.record.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match entry {
        Some(entry) => entry
            .quote
            .as_deref()
            .is_some_and(|q| !q.trim().is_empty() && body.contains(q)),
        None => m.matches(body),
    }
}

/// Trimmed: a Markdown body ends in a newline the same words elsewhere (an
/// audit line's free text, a claim's quote) do not carry.
fn push_text(out: &mut Vec<String>, value: Option<&Value>) {
    if let Some(text) = value.and_then(Value::as_str).map(str::trim)
        && !text.is_empty()
    {
        out.push(text.to_owned());
    }
}

/// The free-text fields of an archived entry, as `erased_texts` reads a
/// live one: the body, what was said and quoted, and every string of its
/// decision or item.
fn archived_entry_texts(record: &Value, out: &mut Vec<String>) {
    push_text(out, record.get("body"));
    push_text(out, record.get("text"));
    let Some(front) = record.get("front") else {
        return;
    };
    for pointer in [
        "/said",
        "/quote",
        "/evidence",
        "/retirement_reason",
        "/instruction/quote",
        "/removal_instruction/quote",
        "/outcome/reason",
    ] {
        push_text(out, front.pointer(pointer));
    }
    if let Some(turns) = front.pointer("/window/turns").and_then(Value::as_array) {
        for turn in turns {
            push_text(out, turn.get("text"));
        }
    }
    for key in ["decision", "item"] {
        if let Some(value) = front.get(key) {
            strings(value, out);
        }
    }
}

fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(_) => push_text(out, Some(value)),
        Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings(v, out)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// The texts of an archived claim about entry `id`.
fn claim_texts(claim: &Value, id: &str, out: &mut Vec<String>) {
    let about = is_str(claim.get("older"), |s| s == id) || is_str(claim.get("newer"), |s| s == id);
    if about {
        push_text(out, claim.get("reason"));
        if let Some(candidate) = claim.get("candidate") {
            for key in ["quote", "context", "statement", "subject", "evidence"] {
                push_text(out, candidate.get(key));
            }
            push_text(out, candidate.pointer("/admitted/matched"));
        }
        push_text(out, claim.pointer("/instruction/quote"));
    }
    if let Some(quotes) = claim.get("quotes").and_then(Value::as_array) {
        for quote in quotes {
            if is_str(quote.get("entry"), |s| s == id) {
                push_text(out, quote.get("text"));
            }
        }
    }
}

/// Archived claims after entry `id` is erased: a claim about it goes, and
/// a quote of it in any other claim loses its text.
fn forget_claims(claims: &mut Vec<Value>, id: &str) {
    claims.retain(|claim| {
        !(is_str(claim.get("older"), |s| s == id) || is_str(claim.get("newer"), |s| s == id))
    });
    for claim in claims {
        if let Some(quotes) = claim.get_mut("quotes").and_then(Value::as_array_mut) {
            for quote in quotes {
                if is_str(quote.get("entry"), |s| s == id)
                    && let Some(quote) = quote.as_object_mut()
                {
                    quote.insert("text".into(), json!(""));
                }
            }
        }
    }
}

/// The words a forget erases: cleartext, plus the salted digests a forget
/// recorded before phase 8 for archives it had not cleaned yet.
pub(crate) struct TextMatcher {
    texts: Vec<String>,
    digests: Option<crate::documents::DigestMatcher>,
}

impl TextMatcher {
    fn texts(texts: Vec<String>) -> Self {
        Self {
            texts,
            digests: None,
        }
    }

    fn digests(salt: &str, digests: &[db::docs::TextDigest], db: &Path) -> Result<Self> {
        Ok(Self {
            texts: Vec::new(),
            digests: Some(crate::documents::DigestMatcher::new(salt, digests, db)?),
        })
    }

    fn matches(&self, haystack: &str) -> bool {
        self.texts.iter().any(|t| haystack.contains(t.as_str()))
            || self
                .digests
                .as_ref()
                .is_some_and(|d| d.matches_str(haystack))
    }
}
