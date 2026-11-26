//! `scribe.db`: the SQLite database beside the files
//! (docs/design/scribe/2_design.md §15.7). Phase 4 keeps the session pass's
//! messages here, with an FTS5 index for BM25 search, and creates the
//! memory tables phase 5 fills. The only module that uses `cw-sqlite` and
//! its re-exported `rusqlite` (scripts/check-dependency-direction.sh).
//!
//! Opening, owner-only file checks, WAL configuration (`synchronous = FULL`,
//! `foreign_keys = ON`, `PersistWal`, `secure_delete = ON`), clock-bounded
//! lock waits, read-only / immutable-copy opens, schema migrations keyed on
//! `PRAGMA user_version`, newer-schema refusal (STD-03 R10), and WAL
//! `TRUNCATE` checkpoints all go through `cw-sqlite`. Scribe keeps its
//! thread-local write scope and savepoints ([`scope`]) and its queries.
//! `specs/store.md` gives every table's types and constraints.

pub(super) mod archive;
pub(crate) mod codec;
pub(super) mod docs;
pub(crate) mod documents;
pub(crate) mod files;
pub(crate) mod import;
pub(crate) mod locks;
pub(crate) mod log;
pub(crate) mod write;

/// A `scribe.db` connection, as the store's writers hold it.
pub(crate) type Conn = Connection;
mod groups;
mod memory;
mod messages;
pub(super) mod scope;
pub(super) mod sessions;

pub use memory::{FileOp, HistoryRow, MemoryCommit, PendingRow, Revision};
pub(super) use memory::{Journal, Writer, revision_at};
pub use memory::{memory_view, pending_read};
pub(super) use messages::{
    delete_messages, insert_group_only_in, optimize_fts, search_messages, upsert_in,
    upsert_messages,
};

use crate::error::{Error, Result};
use cw_sqlite::rusqlite::{self, Connection, Transaction};
use cw_sqlite::{Db, Step, Writer as SqliteWriter};
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub use groups::{
    CaptureState, ClaudeCapturePosition, ClaudeGroupCapture, ClaudeSeenIds, CloseHeader, GroupRows,
    MemberWindow, ReadGroupRows, RowError,
};

/// The database's file name under the store root.
pub const DB_FILE: &str = "scribe.db";

/// The schema, one step per `user_version`: step `n` (1-based) moves the
/// database from version `n - 1` to `n`. Append only; never edit a shipped
/// step.
const MIGRATIONS: &[Step] = &[
    Step::function(migrate_v1),
    Step::sql(V2),
    Step::sql(V3),
    Step::sql(V4),
    Step::sql(V5),
    Step::sql(V6),
    Step::sql(V7),
    Step::sql(V8),
    // Versions 8 and 9 may have committed the deletion in V8 without
    // finishing its physical WAL cleanup. Checkpoint outside the transaction
    // before advancing past either version; V10 is the durable completion marker.
    Step::sql(groups::V9).checkpoint_truncate_before(),
    Step::sql(V10).checkpoint_truncate_before(),
    Step::sql(groups::V11),
    Step::sql(groups::V12),
    Step::sql(V13),
    Step::sql(docs::V14),
    Step::sql(groups::V15),
    Step::sql(V16),
    Step::sql(V17),
    Step::sql(groups::V18),
    Step::sql(groups::V19),
    Step::sql(archive::V20),
];

/// Version 1: create the initial tables and record the database's store id.
fn migrate_v1(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(V1)?;
    let created = now();
    let path = tx.path().map_or_else(|| Path::new(DB_FILE), Path::new);
    tx.execute(
        "INSERT INTO meta (id, store_id, created) VALUES (1, ?1, ?2)",
        (store_id(path, &created), &created),
    )?;
    Ok(())
}

/// Version 17 ships typed records and replaces the trial's tag registrations.
const V17: &str = "
ALTER TABLE entries ADD COLUMN decision TEXT CHECK (decision IS NULL OR json_valid(decision));
ALTER TABLE entries ADD COLUMN item TEXT CHECK (item IS NULL OR json_valid(item));
DELETE FROM agent_tags WHERE name IN ('decision', 'open-item');
";

/// Protect legacy issue erasure obligations from older writers. They can
/// read the digest rows but would discard them without erasing pre-issue
/// sources. No table shape changes.
const V16: &str = "SELECT 1;";

/// Pending first-prompt ancestry checks for starts before transcript creation.
const V13: &str = "CREATE TABLE deferred_ancestry (session TEXT PRIMARY KEY NOT NULL) STRICT;";

/// Version 1 (phase 4): `meta`, `messages` and its FTS5 index, and the
/// memory tables phase 5 uses.
const V1: &str = "
CREATE TABLE meta (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    store_id TEXT    NOT NULL CHECK (length(store_id) = 32),
    created  TEXT    NOT NULL
) STRICT;

CREATE TABLE messages (
    id      INTEGER PRIMARY KEY,
    session TEXT    NOT NULL CHECK (length(session) BETWEEN 1 AND 128),
    message TEXT    NOT NULL CHECK (length(message) >= 1),
    role    TEXT    NOT NULL CHECK (role IN ('user', 'assistant')),
    at      TEXT    NOT NULL,
    text    TEXT    NOT NULL,
    sha256  TEXT    NOT NULL CHECK (length(sha256) = 64),
    UNIQUE (session, message)
) STRICT;

CREATE VIRTUAL TABLE messages_fts USING fts5(
    text,
    content = 'messages',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER messages_au AFTER UPDATE OF text ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO messages_fts (rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE now_pending (
    id      INTEGER PRIMARY KEY,
    session TEXT    NOT NULL CHECK (length(session) BETWEEN 1 AND 128),
    message TEXT    NOT NULL CHECK (length(message) >= 1),
    subject TEXT    NOT NULL CHECK (length(subject) >= 1),
    text    TEXT    NOT NULL,
    created TEXT    NOT NULL,
    UNIQUE (session, subject)
) STRICT;
CREATE TRIGGER now_pending_cap BEFORE INSERT ON now_pending
WHEN (SELECT count(*) FROM now_pending WHERE session = new.session) >= 5
BEGIN
    SELECT RAISE(ABORT, 'a session holds at most 5 pending now candidates');
END;

CREATE TABLE memory_history (
    id         INTEGER PRIMARY KEY,
    item       TEXT    NOT NULL CHECK (length(item) >= 1),
    kind       TEXT    NOT NULL CHECK (length(kind) >= 1),
    text       TEXT    NOT NULL,
    sources    TEXT    NOT NULL CHECK (json_valid(sources) AND json_type(sources) = 'array'),
    removed_at TEXT    NOT NULL
) STRICT;

CREATE TABLE profile_revisions (
    n           INTEGER PRIMARY KEY AUTOINCREMENT,
    created     TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    file_sha256 TEXT    CHECK (file_sha256 IS NULL OR length(file_sha256) = 64)
) STRICT;

CREATE TABLE profile_cursor (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    through_at      TEXT    NOT NULL,
    through_session TEXT    NOT NULL CHECK (length(through_session) BETWEEN 1 AND 128)
) STRICT;
";

/// Version 2 (phase 5): the memory journal of §15.6, the messages each
/// memory item cites (so a forget deletes them, §15.7), and the pass's
/// signal per window.
const V2: &str = "
CREATE TABLE memory_journal (
    id       INTEGER PRIMARY KEY,
    created  TEXT    NOT NULL,
    ops      TEXT    NOT NULL CHECK (json_valid(ops) AND json_type(ops) = 'array'),
    done     INTEGER NOT NULL DEFAULT 0 CHECK (done IN (0, 1)),
    revision INTEGER REFERENCES profile_revisions (n)
) STRICT;
CREATE INDEX memory_journal_open ON memory_journal (done) WHERE done = 0;

CREATE TABLE memory_messages (
    item    TEXT NOT NULL CHECK (length(item) >= 1),
    session TEXT NOT NULL CHECK (length(session) BETWEEN 1 AND 128),
    message TEXT NOT NULL CHECK (length(message) >= 1),
    PRIMARY KEY (item, session, message)
) STRICT;

CREATE TABLE pass_signals (
    session TEXT    NOT NULL CHECK (length(session) BETWEEN 1 AND 128),
    through TEXT    NOT NULL CHECK (length(through) >= 1),
    signal  INTEGER NOT NULL CHECK (signal BETWEEN 0 AND 3),
    PRIMARY KEY (session, through)
) STRICT;
";

/// Version 3 (phase 6): sessions, their segments, links and terms, the
/// switch fields `control.json` held, Codex cursors, rebuild checkpoints,
/// and which record kinds have moved in from files.
const V3: &str = "
CREATE TABLE sessions (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    provider_session        TEXT    NOT NULL UNIQUE
                                    CHECK (length(provider_session) BETWEEN 1 AND 128),
    has_record              INTEGER NOT NULL CHECK (has_record IN (0, 1)),
    status                  TEXT    CHECK (status IN ('open', 'final')),
    active                  INTEGER CHECK (active IN (0, 1)),
    keep_active             INTEGER CHECK (keep_active IN (0, 1)),
    active_change           TEXT    CHECK (active_change IS NULL OR json_valid(active_change)),
    cataloged_by            TEXT    CHECK (cataloged_by IS NULL OR json_valid(cataloged_by)),
    rebuilt_by              TEXT    CHECK (rebuilt_by IS NULL OR json_valid(rebuilt_by)),
    project                 TEXT,
    topics                  TEXT    CHECK (topics IS NULL OR json_valid(topics)),
    related                 TEXT    CHECK (related IS NULL OR json_valid(related)),
    provider                TEXT,
    host                    TEXT,
    created                 TEXT,
    updated                 TEXT,
    title                   TEXT,
    cwd                     TEXT,
    started                 TEXT,
    on_since                TEXT,
    scribed_through         TEXT,
    passes                  INTEGER CHECK (passes >= 0),
    next_open               INTEGER CHECK (next_open >= 0),
    summary                 TEXT,
    goals                   TEXT,
    next                    TEXT,
    has_switch              INTEGER NOT NULL CHECK (has_switch IN (0, 1)),
    sw_provider             TEXT,
    sw_on                   INTEGER CHECK (sw_on IN (0, 1)),
    sw_since                TEXT,
    sw_due                  INTEGER CHECK (sw_due IN (0, 1)),
    sw_final                INTEGER CHECK (sw_final IN (0, 1)),
    sw_consecutive_failures INTEGER CHECK (sw_consecutive_failures >= 0),
    sw_retry_after          TEXT,
    sw_pending_links        TEXT    CHECK (sw_pending_links IS NULL OR json_valid(sw_pending_links)),
    sw_cataloged_by         TEXT    CHECK (sw_cataloged_by IS NULL OR json_valid(sw_cataloged_by)),
    CHECK (has_record = 0 OR (status IS NOT NULL AND active IS NOT NULL
        AND keep_active IS NOT NULL AND project IS NOT NULL AND topics IS NOT NULL
        AND related IS NOT NULL AND provider IS NOT NULL AND host IS NOT NULL
        AND created IS NOT NULL AND updated IS NOT NULL AND title IS NOT NULL
        AND cwd IS NOT NULL AND started IS NOT NULL AND on_since IS NOT NULL
        AND passes IS NOT NULL AND next_open IS NOT NULL AND summary IS NOT NULL
        AND goals IS NOT NULL AND next IS NOT NULL)),
    CHECK (has_switch = 0 OR (sw_provider IS NOT NULL AND sw_on IS NOT NULL
        AND sw_due IS NOT NULL AND sw_final IS NOT NULL
        AND sw_consecutive_failures IS NOT NULL AND sw_pending_links IS NOT NULL))
) STRICT;

CREATE TABLE segments (
    session       INTEGER NOT NULL,
    n             INTEGER NOT NULL CHECK (n >= 1),
    heading       TEXT    NOT NULL CHECK (length(heading) >= 1),
    first_message TEXT,
    last_message  TEXT,
    \"from\"        TEXT    NOT NULL,
    \"to\"          TEXT    NOT NULL,
    state         TEXT    NOT NULL CHECK (state IN ('open', 'sealed')),
    windows       INTEGER NOT NULL CHECK (windows >= 1),
    origin        TEXT    NOT NULL CHECK (origin IN ('pass', 'rebuild', 'catalog', 'upgrade')),
    topics        TEXT    NOT NULL CHECK (json_valid(topics) AND json_type(topics) = 'array'),
    related       TEXT    NOT NULL CHECK (json_valid(related) AND json_type(related) = 'array'),
    text          TEXT    NOT NULL,
    PRIMARY KEY (session, n)
) STRICT;

CREATE TABLE session_links (
    session  INTEGER NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    type     TEXT    NOT NULL CHECK (type IN ('spawned_from', 'continues')),
    target   TEXT    NOT NULL CHECK (length(target) BETWEEN 1 AND 128),
    PRIMARY KEY (session, position)
) STRICT;

CREATE TABLE session_terms (
    session  INTEGER NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    term     TEXT    NOT NULL,
    said     TEXT    NOT NULL,
    message  TEXT    NOT NULL,
    PRIMARY KEY (session, position)
) STRICT;

CREATE TABLE codex_cursors (
    session TEXT NOT NULL PRIMARY KEY CHECK (length(session) BETWEEN 1 AND 128),
    message TEXT NOT NULL CHECK (length(message) >= 1)
) STRICT;

CREATE TABLE rebuild_checkpoints (
    session INTEGER NOT NULL PRIMARY KEY,
    record  TEXT    NOT NULL,
    created TEXT    NOT NULL
) STRICT;

CREATE TABLE record_kinds (
    kind     TEXT NOT NULL PRIMARY KEY CHECK (kind IN ('sessions')),
    moved_at TEXT NOT NULL,
    moved_to TEXT
) STRICT;
";

/// Version 4: when `#scribe forget s-<n>` erased a session (§15.7). The
/// row stays, so its id is never reused; NULL for every other session.
const V4: &str = "
ALTER TABLE sessions ADD COLUMN forgotten TEXT;
";

/// Version 5 (phase 7): entries, claims, agent tags, marks, pickups and
/// the audit log, which leave their files; `record_kinds` takes their
/// kinds. A list field is its JSON text; a time its RFC 3339 text.
const V5: &str = "
CREATE TABLE entries (
    id                  TEXT    NOT NULL PRIMARY KEY CHECK (length(id) >= 3),
    tag                 TEXT    NOT NULL CHECK (length(tag) >= 1),
    n                   INTEGER NOT NULL CHECK (n >= 1),
    status              TEXT    NOT NULL
                                CHECK (status IN ('active', 'superseded', 'retired', 'forgotten')),
    project             TEXT    NOT NULL,
    topics              TEXT    NOT NULL CHECK (json_valid(topics) AND json_type(topics) = 'array'),
    related             TEXT    NOT NULL CHECK (json_valid(related) AND json_type(related) = 'array'),
    provider            TEXT    NOT NULL,
    host                TEXT    NOT NULL,
    author              TEXT    NOT NULL CHECK (json_valid(author)),
    updated_by          TEXT    CHECK (updated_by IS NULL OR json_valid(updated_by)),
    created             TEXT    NOT NULL,
    updated             TEXT    NOT NULL,
    inactive_since      TEXT,
    said                TEXT    NOT NULL,
    content_from        TEXT    NOT NULL CHECK (json_valid(content_from)),
    cite                TEXT    CHECK (cite IS NULL OR json_valid(cite)),
    retirement_reason   TEXT,
    superseded_by       TEXT,
    review_after        TEXT,
    quote               TEXT,
    \"window\"            TEXT    CHECK (\"window\" IS NULL OR json_valid(\"window\")),
    evidence            TEXT,
    as_of               TEXT,
    approved_by         TEXT    CHECK (approved_by IS NULL OR json_valid(approved_by)),
    instruction         TEXT    CHECK (instruction IS NULL OR json_valid(instruction)),
    removal_instruction TEXT    CHECK (removal_instruction IS NULL OR json_valid(removal_instruction)),
    stated_by           TEXT    CHECK (stated_by IS NULL OR json_valid(stated_by)),
    outcome             TEXT    CHECK (outcome IS NULL OR json_valid(outcome)),
    content             TEXT    NOT NULL,
    UNIQUE (tag, n)
) STRICT;

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
                                  OR closed_reason IN ('tag_retired', 'instructed', 'covered')),
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

CREATE TABLE agent_tags (
    name             TEXT    NOT NULL PRIMARY KEY CHECK (length(name) >= 1),
    aliases          TEXT    NOT NULL CHECK (json_valid(aliases) AND json_type(aliases) = 'array'),
    description      TEXT    NOT NULL,
    stale_after_days INTEGER CHECK (stale_after_days IS NULL OR stale_after_days >= 1),
    author           TEXT    NOT NULL CHECK (json_valid(author)),
    updated_by       TEXT    CHECK (updated_by IS NULL OR json_valid(updated_by)),
    created          TEXT    NOT NULL,
    updated          TEXT    NOT NULL
) STRICT;

CREATE TABLE marks (
    position      INTEGER NOT NULL PRIMARY KEY CHECK (position >= 0),
    session       TEXT    NOT NULL CHECK (length(session) BETWEEN 1 AND 128),
    message       TEXT    NOT NULL,
    prompt_sha256 TEXT    NOT NULL,
    marker        INTEGER NOT NULL CHECK (marker >= 0),
    outcome       TEXT    NOT NULL CHECK (outcome IN ('entry', 'none', 'refused', 'pending')),
    entry         TEXT,
    code          TEXT,
    attempts      INTEGER NOT NULL CHECK (attempts >= 0),
    at            TEXT    NOT NULL
) STRICT;

CREATE TABLE pickups (
    position INTEGER NOT NULL PRIMARY KEY CHECK (position >= 0),
    entry    TEXT    NOT NULL,
    author   TEXT    NOT NULL CHECK (json_valid(author)),
    at       TEXT    NOT NULL,
    result   TEXT,
    rejected INTEGER NOT NULL CHECK (rejected IN (0, 1))
) STRICT;

CREATE TABLE audit (
    id                  INTEGER PRIMARY KEY,
    format              INTEGER NOT NULL CHECK (format >= 1),
    at                  TEXT    NOT NULL,
    command             TEXT    NOT NULL,
    status              TEXT    NOT NULL CHECK (status IN ('success', 'failure')),
    target              TEXT,
    error_code          TEXT,
    author              TEXT,
    instruction_session TEXT,
    instruction_message TEXT,
    issue               TEXT    CHECK (issue IS NULL OR json_valid(issue)),
    duration_ms         INTEGER NOT NULL CHECK (duration_ms >= 0),
    spool               TEXT    UNIQUE
) STRICT;

CREATE TABLE record_kinds_v5 (
    kind     TEXT NOT NULL PRIMARY KEY
             CHECK (kind IN ('sessions', 'entries', 'claims', 'agent_tags', 'marks',
                             'pickups', 'audit')),
    moved_at TEXT NOT NULL,
    moved_to TEXT
) STRICT;
INSERT INTO record_kinds_v5 (kind, moved_at, moved_to)
    SELECT kind, moved_at, moved_to FROM record_kinds;
DROP TABLE record_kinds;
ALTER TABLE record_kinds_v5 RENAME TO record_kinds;
";

/// Version 6: durable post-commit archive-erasure obligations for forgotten
/// entries (STD-03 R8–R9). Holds only the entry id, a per-obligation salt
/// and salted SHA-256 digests (`{len, sha256}`) of erased texts — never the
/// entry's cleartext — so a restarted write after a crash finishes cleaning
/// `pre-sqlite-*` archives without keeping the forgotten text in cleartext.
const V6: &str = "
CREATE TABLE pending_archive_forgets (
    id      INTEGER PRIMARY KEY,
    entry   TEXT    NOT NULL CHECK (length(entry) >= 3),
    salt    TEXT    NOT NULL CHECK (length(salt) = 64),
    digests TEXT    NOT NULL CHECK (json_valid(digests) AND json_type(digests) = 'array'),
    created TEXT    NOT NULL
) STRICT;
";

/// Version 7: optional handoff refresh mode.
const V7: &str = "ALTER TABLE entries ADD COLUMN handoff_mode TEXT CHECK (handoff_mode IS NULL OR handoff_mode IN ('continue', 'new'));";

/// Version 8: drop rebuild checkpoints an older build could write after
/// `#scribe forget` (STD-03 R26). No table changes. A forgotten session's
/// checkpoint is plaintext the forget was supposed to delete; checkpoints
/// of sessions that were not forgotten stay. JSONL export omits the same
/// rows even before this step runs.
const V8: &str = "
DELETE FROM rebuild_checkpoints
WHERE session IN (SELECT id FROM sessions WHERE forgotten IS NOT NULL);
";

/// Version 10 marks completion of the schema-8 WAL cleanup barrier.
const V10: &str = "SELECT 1;";

/// The newest schema version this build reads and writes.
pub(super) fn schema() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

/// The `cw-sqlite` handle for `scribe.db` under `root`, with `secure_delete`
/// enabled so forgotten or redacted text is overwritten on freed pages.
pub(super) fn database(root: &Path, wait: Duration) -> Db {
    Db::new(root.join(DB_FILE), MIGRATIONS)
        .lock_wait(wait)
        .secure_delete(true)
}

/// Convert a `cw-sqlite` error into the store's typed [`Error`], keeping its
/// stable code, sentence, and elapsed lock wait.
pub(super) fn from_sqlite_error(error: cw_sqlite::Error, started: Instant) -> Error {
    match error {
        cw_sqlite::Error::File(e) => Error::from(e),
        cw_sqlite::Error::Missing { path } => Error::io(
            "open",
            path,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ),
        cw_sqlite::Error::Locked { path, .. } => Error::Locked {
            path,
            holder: String::from("another scribe.db connection"),
            waited_ms: started.elapsed().as_millis(),
        },
        cw_sqlite::Error::Corrupt { path, source } => Error::corrupt(path, source),
        cw_sqlite::Error::CheckFailed { path, problems } => {
            Error::corrupt(path, problems.join("; "))
        }
        cw_sqlite::Error::Incompatible {
            path,
            found,
            supported,
        } => Error::NewerFormat {
            path,
            found,
            supported,
        },
        cw_sqlite::Error::NotWal { path, mode } => Error::Database {
            action: "switch to WAL",
            path,
            source: format!("journal_mode stayed {mode}").into(),
        },
        cw_sqlite::Error::Migration { path, source, .. } => Error::Database {
            action: "migrate",
            path,
            source: Box::new(source),
        },
        cw_sqlite::Error::Sqlite {
            action,
            path,
            source,
        } => Error::Database {
            action,
            path,
            source: Box::new(source),
        },
    }
}

/// A read-write connection opened and migrated through `cw-sqlite`. Inside a
/// store write on this thread ([`scope::Scope`]) it is that write's
/// connection, so the caller joins its transaction.
pub fn open_writer(root: &Path, wait: Duration) -> Result<Rc<SqliteWriter>> {
    if let Some(conn) = scope::active(root) {
        return Ok(conn);
    }
    open_new_writer(root, wait).map(Rc::new)
}

/// [`open_writer`] on a connection of its own.
pub fn open_new_writer(root: &Path, wait: Duration) -> Result<SqliteWriter> {
    let started = Instant::now();
    database(root, wait)
        .open_writer()
        .map_err(|e| from_sqlite_error(e, started))
}

/// Empty the WAL after a forget committed (`TRUNCATE`), so the deleted
/// pages leave the store. A reader still holding an older snapshot keeps its
/// frames until the next checkpoint, so a lock timeout is logged, not failed.
pub(super) fn truncate_wal(writer: &SqliteWriter, path: &Path) -> Result<()> {
    if let Some(root) = path.parent()
        && scope::Scope::truncate_later(root)
    {
        return Ok(());
    }
    let started = Instant::now();
    match writer.checkpoint_truncate() {
        Ok(()) => Ok(()),
        Err(cw_sqlite::Error::Locked { .. }) => {
            tracing::warn!(path = %path.display(), "a reader kept forgotten messages' old pages in the WAL until the next checkpoint");
            Ok(())
        }
        Err(e) => Err(from_sqlite_error(e, started)),
    }
}

/// [`truncate_wal`] on a new writer connection, after a forget committed
/// in a transaction whose connection is gone.
pub(super) fn truncate_wal_of(root: &Path, wait: Duration) -> Result<()> {
    if scope::Scope::truncate_later(root) {
        return Ok(());
    }
    let path = root.join(DB_FILE);
    let started = Instant::now();
    match database(root, wait).checkpoint_truncate() {
        Ok(()) | Err(cw_sqlite::Error::Missing { .. }) => Ok(()),
        Err(cw_sqlite::Error::Locked { .. }) => {
            tracing::warn!(path = %path.display(), "a reader kept forgotten messages' old pages in the WAL until the next checkpoint");
            Ok(())
        }
        Err(e) => Err(from_sqlite_error(e, started)),
    }
}

/// What `scribe export --as jsonl` read from `scribe.db`, with which record
/// kinds already live there.
#[derive(Default)]
pub(crate) struct DbExport {
    /// Whether the session tables live in `scribe.db`.
    pub sessions_moved: bool,
    /// Whether the document tables live in `scribe.db`.
    pub docs_moved: bool,
    /// Rows from whichever kinds live in `scribe.db`, in table then row order.
    pub rows: Vec<(&'static str, serde_json::Value)>,
}

/// Read every moved table's rows for `scribe export --as jsonl`, read-only.
pub(crate) fn read_export(
    root: &Path,
    session_tables: &[(&'static str, &str)],
    doc_tables: &[(&'static str, &str)],
) -> Result<DbExport> {
    let (found, _) = memory::read(root, |conn, version, path| {
        let sessions_moved =
            sessions::moved(conn, version).map_err(|e| db_error("read", path, e))?;
        let docs_moved = docs::moved(conn, version).map_err(|e| db_error("read", path, e))?;
        let mut rows = Vec::new();
        if sessions_moved {
            rows.extend(sessions::export_rows(conn, version, path, session_tables)?);
        }
        if docs_moved {
            rows.extend(sessions::export_rows(conn, version, path, doc_tables)?);
        }
        if version >= 9 {
            let tables = [
                ("groups", "id"),
                ("group_seats", "group_id, name"),
                ("group_members", "position"),
            ];
            rows.extend(sessions::export_rows(conn, version, path, &tables)?);
        }
        Ok(DbExport {
            sessions_moved,
            docs_moved,
            rows,
        })
    })?;
    Ok(found)
}

/// The store id `meta` records: 32 hex characters from the creation time,
/// the process and the path, unique per database without a random source.
fn store_id(path: &Path, created: &str) -> String {
    let seed = format!(
        "{created}|{}|{}|{}",
        std::process::id(),
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
        path.display()
    );
    crate::digest::prompt_sha256(&seed)
        .chars()
        .take(32)
        .collect()
}

/// RFC 3339 UTC, as every store time is written.
fn stamp(at: OffsetDateTime) -> String {
    at.to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

fn now() -> String {
    stamp(OffsetDateTime::now_utc())
}

/// A write failure: `SQLITE_BUSY` past the lock wait is [`Error::Locked`].
pub(super) fn write_error(
    action: &'static str,
    path: &Path,
    started: Instant,
    e: rusqlite::Error,
) -> Error {
    from_sqlite_error(cw_sqlite::Error::sqlite(action, path, e), started)
}

/// A database failure `action` on `path`, as the store reports it: busy or
/// locked is [`Error::Locked`], not-a-database or damaged is
/// [`Error::Corrupt`], anything else a database error.
pub fn db_error(action: &'static str, path: &Path, e: rusqlite::Error) -> Error {
    from_sqlite_error(cw_sqlite::Error::sqlite(action, path, e), Instant::now())
}

/// Restore the schema-10 shape before exercising migrations from an older version.
#[cfg(any(test, feature = "test-support"))]
pub const REMOVE_GROUP_CAPTURE_SCHEMA: &str = "
DROP TABLE group_claude_seen;
DROP TABLE group_claude_progress;
DROP TABLE deferred_ancestry;
DROP INDEX group_open_slug;
ALTER TABLE groups DROP COLUMN slug;
DROP TRIGGER handoffs_clear_group_summary;
DROP TRIGGER messages_clear_group_summary;
ALTER TABLE messages DROP COLUMN group_only;
ALTER TABLE group_members DROP COLUMN restarted;
ALTER TABLE group_members DROP COLUMN turn_cursor;
ALTER TABLE group_members DROP COLUMN joined;
ALTER TABLE groups DROP COLUMN summary;
";

/// The schema version and store id of the database under `root`, for
/// tests; `None` when there is no database.
#[cfg(any(test, feature = "test-support"))]
pub fn meta(root: &Path) -> Result<Option<(u32, String)>> {
    let path = root.join(DB_FILE);
    let started = Instant::now();
    let reader = match database(root, crate::DEFAULT_LOCK_WAIT).read() {
        Ok(reader) => reader,
        Err(cw_sqlite::Error::Missing { .. }) => return Ok(None),
        Err(e) => return Err(from_sqlite_error(e, started)),
    };
    let version = reader.version();
    let id: Option<String> = reader
        .query_row("SELECT store_id FROM meta WHERE id = 1", [], |row| {
            row.get(0)
        })
        .map(Some)
        .or_else(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                Ok(None)
            } else {
                Err(e)
            }
        })
        .map_err(|e| db_error("read", &path, e))?;
    Ok(Some((version, id.unwrap_or_default())))
}

/// Run `sql` against the database under `root` read-write, for tests that
/// check a constraint.
#[cfg(any(test, feature = "test-support"))]
pub fn execute(root: &Path, sql: &str) -> Result<()> {
    let conn = open_writer(root, Duration::from_secs(5))?;
    conn.execute_batch(sql)
        .map_err(|e| db_error("run", &root.join(DB_FILE), e))
}
