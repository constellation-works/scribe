//! `scribe.db`: the SQLite database beside the files
//! (docs/design/scribe/2_design.md §15.7). Phase 4 keeps the session pass's
//! messages here, with an FTS5 index for BM25 search, and creates the
//! memory tables phase 5 fills. The only module that uses `rusqlite`
//! (scripts/check-dependency-direction.sh).
//!
//! - **Privacy.** The file and its `-wal` and `-shm` files are owner-only:
//!   created `0600` before SQLite opens it (SQLite gives the `-wal` and `-shm`
//!   files the database's mode), and refused on every open, read or write,
//!   when one is a link, another user's or open to any group or other bit
//!   (STD-05 R7–R9).
//! - **Writers** open read-write in WAL mode with `busy_timeout` equal to the
//!   store's lock wait and take `BEGIN IMMEDIATE`; a timeout is
//!   [`Error::Locked`]. Each sets `SQLITE_FCNTL_PERSIST_WAL`, so the `-wal`
//!   and `-shm` files outlive the last connection and a read-only reader
//!   finds them.
//! - **Readers** take no lock and write nothing (STD-01 R31): they open with
//!   `SQLITE_OPEN_READ_ONLY`. On a read-only root without the `-wal` and
//!   `-shm` files (a copied or mounted store) SQLite cannot open the WAL, so
//!   the reader retries with `immutable=1`, which is safe there because no
//!   writer can exist, and says so ([`MessageSearch::read_only_copy`]).
//! - **Schema.** [`MIGRATIONS`] is a numbered list of SQL keyed on `PRAGMA
//!   user_version`; each step runs in its own `BEGIN IMMEDIATE` transaction.
//!   A database newer than this build is refused (STD-03 R10).
//!   `specs/store.md` gives every table's types and constraints.

pub(super) mod docs;

/// A `scribe.db` connection, as the store's writers hold it.
pub(in crate::store) type Conn = Connection;
mod groups;
mod memory;
pub(super) mod scope;
pub(super) mod sessions;

pub(super) use memory::{
    FileOp, HistoryRow, Journal, MemoryCommit, PendingRow, Writer, memory_view, pending_read,
    revision_at,
};

use super::fsio;
use super::redact::Redactor;
use crate::entry::Speaker;
use crate::error::{Error, Result};
use crate::message::{MessageMatch, MessageSearch, NewMessage};
use crate::session::SessionId;
use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior};
use scope::Tx;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

pub use groups::{ClaudeCapturePosition, ClaudeGroupCapture};

/// The database's file name under the store root.
pub const DB_FILE: &str = "scribe.db";

/// The schema, one step per `user_version`: step `n` (1-based) moves the
/// database from version `n - 1` to `n`. Append only; never edit a shipped
/// step.
const MIGRATIONS: &[&str] = &[
    V1,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    V8,
    groups::V9,
    V10,
    groups::V11,
    groups::V12,
    V13,
    docs::V14,
    groups::V15,
];

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
/// Refuse, for a reader as for a writer, a database a newer build migrated
/// past this build's schema: `store_too_new`, never a read of tables whose
/// shape this build does not know.
fn refuse_newer(conn: &Connection, path: &Path) -> Result<u32, Fallible> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| Fallible::from_sqlite("read", path, e))?;
    if version > schema() {
        return Err(Fallible::Failed(Error::NewerFormat {
            path: path.to_path_buf(),
            found: version,
            supported: schema(),
        }));
    }
    Ok(version)
}

fn schema() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

/// The database and its WAL-mode companions under `root`.
fn paths(root: &Path) -> [PathBuf; 3] {
    let db = root.join(DB_FILE);
    let side = |suffix: &str| {
        let mut name = db.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    [side(""), side("-wal"), side("-shm")]
}

/// Refuse the database or a companion that is a link, another user's or
/// open to others; `Ok(false)` when the database does not exist.
fn inspect(root: &Path) -> Result<bool> {
    let [db, wal, shm] = paths(root);
    fsio::inspect_file(&wal)?;
    fsio::inspect_file(&shm)?;
    fsio::inspect_file(&db)
}

/// A read-write connection: the database created owner-only if missing,
/// WAL with a persistent WAL, `busy_timeout = wait`, migrated to
/// [`schema`]. The caller has prepared the root. Inside a store write on
/// this thread ([`scope::Scope`]) it is that write's connection, so the
/// caller joins its transaction.
pub(super) fn open_writer(root: &Path, wait: Duration) -> Result<Rc<Connection>> {
    if let Some(conn) = scope::active(root) {
        return Ok(conn);
    }
    open_new_writer(root, wait).map(Rc::new)
}

/// [`open_writer`] on a connection of its own.
pub(super) fn open_new_writer(root: &Path, wait: Duration) -> Result<Connection> {
    let path = root.join(DB_FILE);
    if !inspect(root)? {
        fsio::create_private_file(&path)?;
    }
    let mut conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| db_error("open", &path, e))?;
    conn.busy_timeout(wait)
        .map_err(|e| db_error("configure", &path, e))?;
    let mut persist = 1;
    conn.file_control(
        rusqlite::DEFAULT_NAME,
        rusqlite::FileControl::PersistWal(&mut persist),
    )
    .map_err(|e| db_error("configure", &path, e))?;
    let mode = switch_to_wal(&conn, &path, wait)?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::Database {
            action: "switch to WAL",
            path,
            source: format!("journal_mode stayed {mode}").into(),
        });
    }
    // FULL: a committed window survives power loss, not only a crash.
    // secure_delete: a deleted message's text is overwritten, not left in
    // a free page.
    conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA secure_delete = ON;")
        .map_err(|e| db_error("configure", &path, e))?;
    migrate(&mut conn, &path, wait)?;
    Ok(conn)
}

/// Put the database in WAL mode, which stays set in the file. SQLite does
/// not call the busy handler for a journal-mode change, so while another
/// writer holds a new database the switch is retried every
/// [`WAL_RETRY`], an in-process sleep (STD-03 R17), until `wait` runs out.
fn switch_to_wal(conn: &Connection, path: &Path, wait: Duration) -> Result<String> {
    let started = Instant::now();
    loop {
        let current = conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0));
        let mode = match current {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => Ok(mode),
            Ok(_) => conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0)),
            Err(e) => Err(e),
        };
        match mode {
            Ok(mode) => return Ok(mode),
            Err(e) if is_busy(&e) && started.elapsed() < wait => std::thread::sleep(WAL_RETRY),
            Err(e) => return Err(write_error("switch to WAL", path, started, e)),
        }
    }
}

/// How often [`switch_to_wal`] retries a busy database.
const WAL_RETRY: Duration = Duration::from_millis(20);

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Run every step past the database's `user_version`, each in its own
/// immediate transaction that re-reads the version, so two writers racing
/// on a new database run each step once.
fn migrate(conn: &mut Connection, path: &Path, wait: Duration) -> Result<()> {
    let target = schema();
    loop {
        let started = Instant::now();
        // Versions 8 and 9 may have committed the deletion without finishing
        // its physical cleanup. Version 10 is the durable completion marker.
        // Checkpoint outside the transaction; never let a busy checkpoint
        // erase the obligation. Recheck the version under the write lock below.
        let observed: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| db_error("read the schema version of", path, e))?;
        let cleaned = (8..=9).contains(&observed);
        if cleaned {
            let checkpoint_started = Instant::now();
            loop {
                let busy: i64 = conn
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
                    .map_err(|e| write_error("checkpoint", path, started, e))?;
                if busy == 0 {
                    break;
                }

                let waited = checkpoint_started.elapsed();
                if waited >= wait {
                    return Err(Error::Locked {
                        path: path.to_path_buf(),
                        holder: String::from("another scribe.db connection"),
                        waited_ms: waited.as_millis(),
                    });
                }

                // Another opener may have advanced the schema while this
                // checkpoint was busy. Version 10 records completed cleanup,
                // so it removes the obligation to checkpoint here.
                let current: u32 = conn
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .map_err(|e| db_error("read the schema version of", path, e))?;
                if !(8..=9).contains(&current) {
                    break;
                }

                std::thread::sleep(WAL_RETRY.min(wait.saturating_sub(waited)));
            }
        }
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| write_error("migrate", path, started, e))?;
        let version: u32 = tx
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| db_error("read the schema version of", path, e))?;
        if version > target {
            return Err(Error::NewerFormat {
                path: path.to_path_buf(),
                found: version,
                supported: target,
            });
        }
        if (8..=9).contains(&version) && !cleaned {
            tx.commit()
                .map_err(|e| write_error("migrate", path, started, e))?;
            continue;
        }
        if version == target {
            return tx
                .commit()
                .map_err(|e| write_error("migrate", path, started, e));
        }
        let index = usize::try_from(version).unwrap_or(usize::MAX);
        let Some(step) = MIGRATIONS.get(index) else {
            return Ok(());
        };
        tx.execute_batch(step)
            .map_err(|e| db_error("migrate", path, e))?;
        if version == 0 {
            let created = now();
            tx.execute(
                "INSERT INTO meta (id, store_id, created) VALUES (1, ?1, ?2)",
                (store_id(path, &created), &created),
            )
            .map_err(|e| db_error("migrate", path, e))?;
        }
        tx.pragma_update(None, "user_version", version + 1)
            .map_err(|e| db_error("migrate", path, e))?;
        tx.commit()
            .map_err(|e| write_error("migrate", path, started, e))?;
    }
}

/// Upsert `messages` on (session, message) in one immediate transaction,
/// each text redacted first. Returns how many rows were inserted or
/// changed: a window written again with the same text changes none.
pub(super) fn upsert_messages(
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
pub(super) fn upsert_in(
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
pub(super) fn insert_group_only_in<'a>(
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
        let sha256 = crate::capture::prompt_sha256(&text);
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

/// Empty the WAL after a forget committed (`TRUNCATE`), so the deleted
/// pages leave the store. The checkpoint waits out readers through
/// `busy_timeout`; one still reading keeps its frames until the next
/// checkpoint, so a busy result is reported, not failed.
pub(super) fn truncate_wal(conn: &Connection, path: &Path, started: Instant) -> Result<()> {
    if let Some(root) = path.parent()
        && scope::Scope::truncate_later(root)
    {
        return Ok(());
    }
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|e| write_error("checkpoint", path, started, e))?;
    if busy != 0 {
        tracing::warn!(path = %path.display(), "a reader kept forgotten messages' old pages in the WAL until the next checkpoint");
    }
    Ok(())
}

/// [`truncate_wal`] on a new writer connection, after a forget committed
/// in a transaction whose connection is gone.
pub(super) fn truncate_wal_of(root: &Path, wait: Duration) -> Result<()> {
    let path = root.join(DB_FILE);
    let conn = open_writer(root, wait)?;
    truncate_wal(&conn, &path, Instant::now())
}

/// Delete the messages keyed by `keys` (session, message id) for a forget,
/// and leave no trace of their text: `secure_delete` overwrites the freed
/// pages, an FTS5 `optimize` rewrites the index without their terms, and a
/// `TRUNCATE` checkpoint empties the WAL that held the old pages. A missing
/// database deletes nothing. Returns how many rows were deleted.
pub(super) fn delete_messages(
    root: &Path,
    wait: Duration,
    keys: &[(SessionId, String)],
) -> Result<usize> {
    let path = root.join(DB_FILE);
    if keys.is_empty() || !inspect(root)? {
        return Ok(0);
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
        tx.execute(
            "INSERT INTO messages_fts (messages_fts) VALUES ('optimize')",
            [],
        )
        .map_err(|e| write_error("forget messages in", &path, started, e))?;
    }
    tx.commit()?;
    if deleted > 0 {
        truncate_wal(&conn, &path, started)?;
    }
    Ok(deleted)
}

/// Every message whose text matches one of `tokens` as a word prefix, best
/// BM25 score first. Read-only: no lock, no write; a missing database
/// matches nothing.
pub(super) fn search_messages(
    root: &Path,
    tokens: &[String],
    group: Option<i64>,
) -> Result<MessageSearch> {
    let path = root.join(DB_FILE);
    if tokens.is_empty() || !inspect(root)? {
        return Ok(MessageSearch::default());
    }
    let query = fts_query(tokens);
    match open_reader(&path, false).and_then(|conn| read_matches(&conn, &path, &query, group)) {
        Ok(matches) => Ok(MessageSearch {
            matches,
            read_only_copy: false,
        }),
        Err(Fallible::NeedsImmutable(first)) => {
            tracing::debug!(path = %path.display(), error = %first, "retrying scribe.db as a read-only copy");
            let conn = open_reader(&path, true).map_err(Fallible::into_error)?;
            let matches =
                read_matches(&conn, &path, &query, group).map_err(Fallible::into_error)?;
            Ok(MessageSearch {
                matches,
                read_only_copy: true,
            })
        }
        Err(Fallible::Failed(e)) => Err(e),
    }
}

/// A read failure, split by whether a read-only copy could succeed.
enum Fallible {
    /// SQLite could not open or recover the WAL read-only: the `-wal` or
    /// `-shm` file is missing and the directory is not writable.
    NeedsImmutable(rusqlite::Error),
    /// Anything else.
    Failed(Error),
}

impl Fallible {
    fn from_sqlite(action: &'static str, path: &Path, e: rusqlite::Error) -> Self {
        let wal_unreadable = matches!(
            e.sqlite_error_code(),
            Some(ErrorCode::CannotOpen | ErrorCode::ReadOnly)
        );
        if wal_unreadable {
            Self::NeedsImmutable(e)
        } else {
            Self::Failed(db_error(action, path, e))
        }
    }

    fn into_error(self) -> Error {
        match self {
            Self::NeedsImmutable(e) => db_error("read", Path::new(DB_FILE), e),
            Self::Failed(e) => e,
        }
    }
}

fn open_reader(path: &Path, immutable: bool) -> Result<Connection, Fallible> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let target = if immutable {
        format!("file:{}?immutable=1", uri_path(path))
    } else {
        format!("file:{}", uri_path(path))
    };
    Connection::open_with_flags(target, flags).map_err(|e| Fallible::from_sqlite("open", path, e))
}

fn read_matches(
    conn: &Connection,
    path: &Path,
    query: &str,
    group: Option<i64>,
) -> Result<Vec<MessageMatch>, Fallible> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| Fallible::from_sqlite("read", path, e))?;
    if version > schema() {
        return Err(Fallible::Failed(Error::NewerFormat {
            path: path.to_path_buf(),
            found: version,
            supported: schema(),
        }));
    }
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
        .map_err(|e| Fallible::from_sqlite("read", path, e))?;
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
        .map_err(|e| Fallible::from_sqlite("read", path, e))?;
    let mut matches = Vec::new();
    for row in rows {
        let (session, message, role, at, text, bm25, project) =
            row.map_err(|e| Fallible::from_sqlite("read", path, e))?;
        let corrupt = |what: &str| {
            Fallible::Failed(Error::corrupt(path, format!("a message row has {what}")))
        };
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

/// What `scribe export --as jsonl` read from `scribe.db`, with which record
/// kinds already live there.
#[derive(Default)]
pub(in crate::store) struct DbExport {
    /// Whether the session tables live in `scribe.db`.
    pub sessions_moved: bool,
    /// Whether the document tables live in `scribe.db`.
    pub docs_moved: bool,
    /// Rows from whichever kinds live in `scribe.db`, in table then row order.
    pub rows: Vec<(&'static str, serde_json::Value)>,
}

/// Read every moved table's rows for `scribe export --as jsonl`, read-only.
pub(in crate::store) fn read_export(
    root: &Path,
    session_tables: &[(&'static str, &str)],
    doc_tables: &[(&'static str, &str)],
) -> Result<DbExport> {
    let (found, _) = memory::read(root, |conn, path| {
        let version = refuse_newer(conn, path)?;
        let sessions_moved =
            sessions::moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))?;
        let docs_moved = docs::moved(conn).map_err(|e| Fallible::from_sqlite("read", path, e))?;
        let mut rows = Vec::new();
        if sessions_moved {
            rows.extend(
                sessions::export_rows(conn, path, session_tables).map_err(Fallible::Failed)?,
            );
        }
        if docs_moved {
            rows.extend(sessions::export_rows(conn, path, doc_tables).map_err(Fallible::Failed)?);
        }
        if version >= 9 {
            let tables = [
                ("groups", "id"),
                ("group_seats", "group_id, name"),
                ("group_members", "position"),
            ];
            rows.extend(sessions::export_rows(conn, path, &tables).map_err(Fallible::Failed)?);
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
    crate::capture::prompt_sha256(&seed)
        .chars()
        .take(32)
        .collect()
}

/// An FTS5 query: each token as a quoted prefix, any of them.
fn fts_query(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// `path` for a `file:` URI: `%`, `?` and `#` escaped, as SQLite requires.
fn uri_path(path: &Path) -> String {
    let mut out = String::new();
    for c in path.to_string_lossy().chars() {
        match c {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            _ => out.push(c),
        }
    }
    out
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

/// A write failure: `SQLITE_BUSY` past `busy_timeout` is [`Error::Locked`].
pub(super) fn write_error(
    action: &'static str,
    path: &Path,
    started: Instant,
    e: rusqlite::Error,
) -> Error {
    if is_busy(&e) {
        Error::Locked {
            path: path.to_path_buf(),
            holder: String::from("another scribe.db writer"),
            waited_ms: started.elapsed().as_millis(),
        }
    } else {
        db_error(action, path, e)
    }
}

pub(super) fn db_error(action: &'static str, path: &Path, e: rusqlite::Error) -> Error {
    match e.sqlite_error_code() {
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt) => Error::corrupt(path, e),
        Some(_) | None => Error::Database {
            action,
            path: path.to_path_buf(),
            source: Box::new(e),
        },
    }
}

/// Restore the schema-10 shape before exercising migrations from an older version.
#[cfg(test)]
pub(super) const REMOVE_GROUP_CAPTURE_SCHEMA: &str = "
DROP TABLE group_claude_seen;
DROP TABLE group_claude_progress;
DROP TABLE deferred_ancestry;
DROP INDEX group_open_slug;
ALTER TABLE groups DROP COLUMN slug;
DROP TRIGGER handoffs_clear_group_summary;
DROP TRIGGER messages_clear_group_summary;
ALTER TABLE messages DROP COLUMN group_only;
ALTER TABLE group_members DROP COLUMN turn_cursor;
ALTER TABLE group_members DROP COLUMN joined;
ALTER TABLE groups DROP COLUMN summary;
";

/// The schema version and store id of the database under `root`, for
/// tests; `None` when there is no database.
#[cfg(test)]
pub(super) fn meta(root: &Path) -> Result<Option<(u32, String)>> {
    let path = root.join(DB_FILE);
    if !inspect(root)? {
        return Ok(None);
    }
    let conn = open_reader(&path, false).map_err(Fallible::into_error)?;
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| db_error("read", &path, e))?;
    let id: Option<String> = conn
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
#[cfg(test)]
pub(super) fn execute(root: &Path, sql: &str) -> Result<()> {
    let conn = open_writer(root, Duration::from_secs(5))?;
    conn.execute_batch(sql)
        .map_err(|e| db_error("run", &root.join(DB_FILE), e))
}
