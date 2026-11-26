//! Group records and exclusive seat membership. Every decision and its write share
//! the store transaction. Provider ids have no foreign key to cataloged sessions.

mod claude;
mod rows;

pub(super) use claude::V15;
pub use claude::{ClaudeCapturePosition, ClaudeGroupCapture, ClaudeSeenIds};
pub use rows::CloseHeader;

use super::{
    DB_FILE, Fallible, db_error, memory::read, open_writer, optimize_fts, refuse_newer, stamp,
};
use crate::error::refusal;
use crate::group::{Group, NewGroup, Seat, SeatHold};
use crate::message::NewMessage;
use crate::repository::ids::number;
use crate::{Error, Label, Project, Redactor, Result, SessionId, Store, WriteRefusal};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(super) const V9: &str = "
CREATE TABLE groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    kind TEXT NOT NULL CHECK (length(trim(kind)) > 0),
    project TEXT NOT NULL,
    opened TEXT NOT NULL,
    closed TEXT,
    opener TEXT NOT NULL CHECK (length(opener) BETWEEN 1 AND 128),
    related TEXT NOT NULL CHECK (json_valid(related) AND json_type(related) = 'array')
) STRICT;
CREATE TABLE group_seats (
    group_id INTEGER NOT NULL REFERENCES groups(id),
    name TEXT NOT NULL,
    role TEXT NOT NULL,
    PRIMARY KEY (group_id, name)
) STRICT;
CREATE TABLE group_members (
    position INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id INTEGER NOT NULL,
    seat TEXT NOT NULL,
    session TEXT NOT NULL UNIQUE CHECK (length(session) BETWEEN 1 AND 128),
    author TEXT NOT NULL,
    FOREIGN KEY (group_id, seat) REFERENCES group_seats(group_id, name)
) STRICT;
CREATE INDEX group_chain ON group_members(group_id, seat, position);
";

pub(super) const V11: &str = "
ALTER TABLE groups ADD COLUMN summary TEXT;
ALTER TABLE group_members ADD COLUMN joined TEXT;
ALTER TABLE group_members ADD COLUMN turn_cursor TEXT;
UPDATE group_members SET joined = strftime('%Y-%m-%dT%H:%M:%fZ', 'now');
ALTER TABLE messages ADD COLUMN group_only INTEGER NOT NULL DEFAULT 0 CHECK (group_only IN (0, 1));
CREATE TRIGGER messages_clear_group_summary AFTER DELETE ON messages
BEGIN
    UPDATE groups SET summary = NULL
    WHERE summary IS NOT NULL AND id = (SELECT group_id FROM group_members WHERE session = old.session);
END;
CREATE TRIGGER handoffs_clear_group_summary AFTER UPDATE OF status ON entries
WHEN old.tag = 'handoff' AND old.status <> 'forgotten' AND new.status = 'forgotten'
BEGIN
    UPDATE groups SET summary = NULL WHERE summary IS NOT NULL AND id IN (
        SELECT gm.group_id FROM group_members gm JOIN json_each(old.related) r
        ON r.value = 'session:' || gm.session
    );
END;
";

pub(super) const V12: &str = "
ALTER TABLE groups ADD COLUMN slug TEXT;
CREATE UNIQUE INDEX group_open_slug ON groups(slug) WHERE slug IS NOT NULL AND closed IS NULL;
";

/// Version 18 marks a holder whose seat was restored after a displacement, so
/// an explicit backfill keeps its restarted consent window.
pub(super) const V18: &str = "
ALTER TABLE group_members ADD COLUMN restarted INTEGER NOT NULL DEFAULT 0 CHECK (restarted IN (0, 1));
";

/// Version 19 repairs the version 18 default. A holder restored before its
/// store reached version 18 has a reset `joined` but `restarted = 0`, and the
/// rows cannot be told apart from a holder that never was displaced. Mark
/// conservatively: a seat holder with a vanished `position` after its own was
/// possibly restored, since `AUTOINCREMENT` never reuses a position and the
/// departed displacer's row is gone. A holder with no such gap cannot have
/// been restored, because its displacer's row would still hold a later
/// position. A false positive only narrows an explicit backfill to turns
/// dated since `joined`; a miss would store a turn typed during displacement.
pub(super) const V19: &str = "
UPDATE group_members SET restarted = 1
WHERE position IN (
    SELECT ranked.position
    FROM (
        SELECT position, COUNT(*) OVER (
            ORDER BY position ROWS BETWEEN 1 FOLLOWING AND UNBOUNDED FOLLOWING
        ) AS later_count
        FROM group_members
    ) ranked
    JOIN group_members holder ON holder.position = ranked.position
    WHERE holder.position = (
        SELECT MAX(m2.position) FROM group_members m2
        WHERE m2.group_id = holder.group_id AND m2.seat = holder.seat
    )
    AND ranked.later_count
        < COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'group_members'), 0) - ranked.position
);
";

/// Drop a forgotten session's seat membership, clear derived summaries, and
/// restart any restored holder's capture window. A seat that still has members
/// keeps its role and chain.
pub(super) fn erase_session_rows(
    conn: &Connection,
    path: &Path,
    id: &SessionId,
    now: OffsetDateTime,
) -> Result<usize> {
    let Some(removed) = remove_seat_member(conn, path, "forget a group member in", id, now)? else {
        return Ok(0);
    };
    conn.execute(
        "UPDATE groups SET summary = NULL WHERE id = ?1",
        [removed.group],
    )
    .map_err(|e| db_error("forget a group member in", path, e))?;
    Ok(removed.removed)
}

struct RemovedMember {
    group: i64,
    removed: usize,
}

/// Remove `session` from its seat, prune an empty seat, and restart any
/// restored holder's capture window when `session` held the seat.
fn remove_seat_member(
    conn: &Connection,
    path: &Path,
    action: &'static str,
    session: &SessionId,
    now: OffsetDateTime,
) -> Result<Option<RemovedMember>> {
    let fail = |e| db_error(action, path, e);
    let member: Option<(i64, String, bool)> = conn
        .query_row(
            &format!(
                "SELECT m.group_id, m.seat, {HOLDS_SEAT}
                 FROM group_members m WHERE m.session = ?1"
            ),
            [session.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(fail)?;
    let Some((group, seat, held)) = member else {
        return Ok(None);
    };
    let removed = conn
        .execute(
            "DELETE FROM group_members WHERE session = ?1",
            [session.as_str()],
        )
        .map_err(fail)?;
    conn.execute(
        "DELETE FROM group_seats WHERE group_id = ?1 AND name = ?2
         AND NOT EXISTS (SELECT 1 FROM group_members WHERE group_id = ?1 AND seat = ?2)",
        params![group, seat],
    )
    .map_err(fail)?;
    if held {
        // The departing member held the seat, so the member it displaced holds
        // it again. Turns that member typed in between were never authorized:
        // restart its consent window now. Its cursor goes too, since a cursor
        // would make every later turn count as new whatever its timestamp, and
        // no hook may have moved it past the displacement interval. The mark
        // keeps an explicit backfill inside the restarted window too.
        conn.execute(
            "UPDATE group_members SET joined = ?3, turn_cursor = NULL, restarted = 1
             WHERE group_id = ?1 AND seat = ?2 AND position = (
                 SELECT MAX(position) FROM group_members WHERE group_id = ?1 AND seat = ?2
             )",
            params![group, seat, stamp(now)],
        )
        .map_err(fail)?;
    }
    Ok(Some(RemovedMember { group, removed }))
}

/// The handoff lookup follows the session reference and ranks active handoffs across
/// the whole seat. A single SELECT keeps metadata, chains and handoffs in one snapshot.
const ROSTER: &str = "
WITH handoffs AS (
    SELECT substr(r.value, 9) AS session, e.id, e.created, e.n
    FROM entries e JOIN json_each(e.related) r
    WHERE e.tag = 'handoff' AND e.status = 'active' AND substr(r.value, 1, 8) = 'session:'
)
SELECT g.id, g.title, g.kind, g.project, g.opened, g.closed, g.opener, g.related,
       s.name, s.role, m.session, h.id, h.created, h.n, {summary}, {slug}
FROM groups g LEFT JOIN group_seats s ON s.group_id = g.id
LEFT JOIN group_members m ON m.group_id = s.group_id AND m.seat = s.name
LEFT JOIN handoffs h ON h.session = m.session
WHERE ?1 IS NULL OR g.id = ?1
ORDER BY g.id DESC, s.name, m.position";

fn roster(conn: &Connection, path: &Path, number: Option<i64>) -> Result<Vec<Group>, Fallible> {
    let fail = |e| Fallible::from_sqlite("read groups from", path, e);
    let summary = if refuse_newer(conn, path)? >= 11 {
        "g.summary"
    } else {
        "NULL"
    };
    let slug = if refuse_newer(conn, path)? >= 12 {
        "g.slug"
    } else {
        "NULL"
    };
    let sql = ROSTER.replace("{summary}", summary).replace("{slug}", slug);
    let mut query = conn.prepare(&sql).map_err(fail)?;
    let mut rows = query.query([number]).map_err(fail)?;
    let mut groups: Vec<Group> = Vec::new();
    let mut newest = None;
    while let Some(row) = rows.next().map_err(fail)? {
        let id = format!("g-{}", row.get::<_, i64>(0).map_err(fail)?);
        if groups.last().is_none_or(|g| g.id != id) {
            let related: String = row.get(7).map_err(fail)?;
            groups.push(Group {
                id,
                slug: row.get(15).map_err(fail)?,
                title: row.get(1).map_err(fail)?,
                kind: row.get(2).map_err(fail)?,
                project: row.get(3).map_err(fail)?,
                opened: row.get(4).map_err(fail)?,
                closed: row.get(5).map_err(fail)?,
                summary: row.get(14).map_err(fail)?,
                opener: row.get(6).map_err(fail)?,
                related: serde_json::from_str(&related)
                    .map_err(|e| Fallible::Failed(Error::corrupt(path, e)))?,
                seats: Vec::new(),
            });
        }
        let Some(name) = row.get::<_, Option<String>>(8).map_err(fail)? else {
            continue;
        };
        if let Some(group) = groups.last_mut() {
            if group.seats.last().is_none_or(|s| s.name != name) {
                newest = None;
                group.seats.push(Seat {
                    name,
                    role: row.get(9).map_err(fail)?,
                    transcripts: Vec::new(),
                    newest_handoff: None,
                });
            }
            let session: String = row.get(10).map_err(fail)?;
            if let Some(seat) = group.seats.last_mut() {
                if seat.transcripts.last().map(SessionId::as_str) != Some(&session) {
                    seat.transcripts.push(
                        session
                            .parse()
                            .map_err(|e| Fallible::Failed(Error::corrupt(path, e)))?,
                    );
                }
                if let Some(created) = row.get::<_, Option<String>>(12).map_err(fail)? {
                    let at = OffsetDateTime::parse(&created, &Rfc3339)
                        .map_err(|e| Fallible::Failed(Error::corrupt(path, e)))?;
                    let key = (at, row.get::<_, i64>(13).map_err(fail)?);
                    if newest.is_none_or(|prior| key > prior) {
                        newest = Some(key);
                        seat.newest_handoff = row.get(11).map_err(fail)?;
                    }
                }
            }
        }
    }
    Ok(groups)
}

/// Seat `session` in group `n` under `seat`, a member seat the caller
/// chose so that a join never displaces anyone.
fn seat_member(
    conn: &Connection,
    n: i64,
    session: &SessionId,
    seat: &str,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO group_seats(group_id,name,role) VALUES (?1,?2,'member')",
        params![n, seat],
    )?;
    conn.execute(
        "INSERT INTO group_members(group_id,seat,session,author,joined) VALUES (?1,?2,?3,'user',?4)",
        params![n, seat, session.as_str(), stamp(now)],
    )?;
    Ok(())
}

impl Store {
    fn read_groups<T: Default>(
        &self,
        f: impl Fn(&Connection, &Path) -> Result<T, Fallible>,
    ) -> Result<T> {
        if !self.root_exists()? {
            return Ok(T::default());
        }
        self.check_stamp()?;
        let (result, _) = read(self.root(), |conn, path| {
            let version = refuse_newer(conn, path)?;
            if version < 9 {
                Ok(T::default())
            } else {
                f(conn, path)
            }
        })?;
        Ok(result)
    }

    /// Groups newest first, including seats and their active handoffs.
    pub(crate) fn groups(&self) -> Result<Vec<Group>> {
        self.read_groups(|conn, path| roster(conn, path, None))
    }

    /// The most recently opened group in the project.
    pub(crate) fn last_group(&self, project: &Project) -> Result<Option<crate::LastGroup>> {
        self.read_groups(|conn, path| {
            let summary = if refuse_newer(conn, path)? >= 11 { "summary" } else { "NULL" };
            conn.query_row(
                &format!("SELECT id, title, closed, {summary} FROM groups WHERE project = ?1 ORDER BY id DESC LIMIT 1"),
                [project.as_str()],
                |row| Ok(crate::LastGroup {
                    id: format!("g-{}", row.get::<_, i64>(0)?),
                    title: row.get(1)?,
                    closed: row.get(2)?,
                    summary: row.get(3)?,
                }),
            ).optional().map_err(|e| Fallible::from_sqlite("read last group from", path, e))
        })
    }

    fn missing_group(&self, id: &str) -> Error {
        refusal(
            WriteRefusal::GroupNotFound,
            format!("no group {id} under {}", self.root().display()),
        )
    }

    pub(crate) fn group_number(&self, input: &str) -> Result<i64> {
        // Open project slugs take precedence over id prefixes.
        let slug = self.read_groups(|conn, path| {
            if refuse_newer(conn, path)? < 12 {
                return Ok(None);
            }
            conn.query_row(
                "SELECT id FROM groups WHERE slug = ?1 ORDER BY closed IS NULL DESC, id DESC LIMIT 1",
                [input],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| Fallible::from_sqlite("read group slug from", path, e))
        })?;
        if let Some(n) = slug {
            return Ok(n);
        }
        // An exact id wins, including when a longer id starts with it.
        let ids = self.read_groups(|conn, path| {
            let fail = |e| Fallible::from_sqlite("read group ids from", path, e);
            let mut query = conn
                .prepare(
                    "SELECT 'g-' || id FROM groups WHERE instr('g-' || id, ?1) = 1 ORDER BY id",
                )
                .map_err(fail)?;
            query
                .query_map([input], |r| r.get::<_, String>(0))
                .map_err(fail)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(fail)
        })?;
        if ids.is_empty() && number(input).is_some() {
            return Err(self.missing_group(input));
        }
        crate::resolve_prefix(input, &ids)?
            .as_deref()
            .and_then(number)
            .ok_or_else(|| self.missing_group(input))
    }

    /// One group by exact id or unique prefix.
    pub(crate) fn group(&self, id: &str) -> Result<Group> {
        let n = self.group_number(id)?;
        self.read_groups(|conn, path| roster(conn, path, Some(n)))?
            .pop()
            .ok_or_else(|| self.missing_group(id))
    }

    /// Search only stored messages whose session belongs to this group.
    /// The database checks membership in the same read as the messages.
    pub(crate) fn search_group_messages(
        &self,
        group: &str,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch> {
        let n = self.group_number(group)?;
        super::search_messages(self.root(), tokens, Some(n))
    }

    /// Run `f` over the group rows of one write step: the store's lock and
    /// transaction (the write under way, when there is one), with `action`
    /// naming the write in a database error.
    pub(crate) fn group_rows<T>(
        &self,
        action: &'static str,
        f: impl FnOnce(&GroupRows<'_>) -> Result<T>,
    ) -> Result<T> {
        self.write_step(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            f(&GroupRows(ReadGroupRows {
                conn: &conn,
                path: &path,
                action,
                read: false,
            }))
        })
    }

    /// Run `f` over the group rows of one read; `T::default()` before the
    /// store has groups.
    pub(crate) fn read_group_rows<T: Default>(
        &self,
        f: impl Fn(&ReadGroupRows<'_>) -> Result<T, RowError>,
    ) -> Result<T> {
        self.read_groups(|conn, path| {
            f(&ReadGroupRows {
                conn,
                path,
                action: "read groups from",
                read: true,
            })
            .map_err(|e| e.0)
        })
    }

    /// [`Store::read_group_rows`] inside one read transaction, so that its
    /// reads see one state; `action` names the read in a database error.
    pub(crate) fn read_group_rows_in_tx<T: Default>(
        &self,
        action: &'static str,
        f: impl Fn(&ReadGroupRows<'_>) -> Result<T, RowError>,
    ) -> Result<T> {
        self.read_groups(|conn, path| {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| Fallible::from_sqlite(action, path, e))?;
            f(&ReadGroupRows {
                conn: &tx,
                path,
                action,
                read: true,
            })
            .map_err(|e| e.0)
        })
    }

    /// Whether a deferred ancestry check is pending for `session`.
    pub(crate) fn deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
        self.read_groups(|conn, path| {
            if refuse_newer(conn, path)? < 13 {
                return Ok(false);
            }
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM deferred_ancestry WHERE session = ?1)",
                [session.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|e| Fallible::from_sqlite("read deferred ancestry from", path, e))
        })
    }
}

fn write_captured_turns(
    conn: &Connection,
    path: &Path,
    session: &SessionId,
    turns: Vec<&NewMessage>,
    redactor: &Redactor,
) -> Result<usize> {
    if turns.is_empty() {
        return Ok(0);
    }
    let inserted = super::insert_group_only_in(
        conn,
        path,
        std::time::Instant::now(),
        turns.into_iter(),
        redactor,
    )?;
    conn.execute(
        "INSERT INTO sessions(provider_session, has_record, has_switch)
         VALUES (?1, 0, 0) ON CONFLICT(provider_session) DO NOTHING",
        [session.as_str()],
    )
    .map_err(|e| db_error("store group turns in", path, e))?;
    Ok(inserted)
}

/// A member's capture row in an open group ([`GroupRows::capture_state`]).
#[derive(Debug, PartialEq, Eq)]
pub struct CaptureState {
    /// The membership row's position.
    pub position: i64,
    /// Whether the member holds its seat now. A replaced member keeps its row
    /// but stores nothing.
    pub holds: bool,
    /// The start of its capture window.
    pub since: OffsetDateTime,
    /// Its capture cursor.
    pub cursor: Option<String>,
}

fn capture_state(
    conn: &Connection,
    path: &Path,
    session: &SessionId,
) -> Result<Option<CaptureState>, Fallible> {
    let fail = |e| Fallible::from_sqlite("read group capture state from", path, e);
    let joined = if refuse_newer(conn, path)? >= 11 {
        ("m.joined", "m.turn_cursor")
    } else {
        ("g.opened", "NULL")
    };
    // Capture belongs to the seat holder only: while a later member holds the
    // seat, a replaced member stores no new turns.
    let row: Option<(i64, String, Option<String>, bool)> = conn.query_row(
        &format!("SELECT m.position, {}, {}, {HOLDS_SEAT} FROM group_members m
         JOIN groups g ON g.id = m.group_id
         WHERE m.session = ?1 AND g.closed IS NULL AND NOT EXISTS (
             SELECT 1 FROM sessions s WHERE s.provider_session = m.session AND s.forgotten IS NOT NULL
         )", joined.0, joined.1),
        [session.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional().map_err(fail)?;
    row.map(|(position, joined, cursor, holds)| {
        let since = OffsetDateTime::parse(&joined, &Rfc3339)
            .map_err(|e| Fallible::Failed(Error::corrupt(path, e)))?;
        Ok(CaptureState {
            position,
            holds,
            since,
            cursor,
        })
    })
    .transpose()
}

/// A membership row (alias `m`) holds its seat only while it is the seat's newest
/// position. A session that a later member replaced keeps its row as history but
/// holds nothing. Seat lookup and turn capture both use this one rule.
const HOLDS_SEAT: &str = "m.position = (
               SELECT MAX(m2.position)
               FROM group_members m2
               WHERE m2.group_id = m.group_id AND m2.seat = m.seat
           )";

/// The seat the session holds now.
fn held_seat(conn: &Connection, session: &SessionId) -> rusqlite::Result<Option<SeatHold>> {
    conn.query_row(
        &format!(
            "SELECT m.group_id, m.seat, s.role
         FROM group_members m
         JOIN group_seats s ON s.group_id = m.group_id AND s.name = m.seat
         WHERE m.session = ?1 AND {HOLDS_SEAT}"
        ),
        [session.as_str()],
        |r| {
            Ok(SeatHold {
                group: format!("g-{}", r.get::<_, i64>(0)?),
                seat: r.get(1)?,
                role: r.get(2)?,
            })
        },
    )
    .optional()
}

/// Whether the session has ever been a member. `session` is unique across the
/// store, so a historical row blocks a second insert for that session.
fn has_membership(conn: &Connection, session: &SessionId) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM group_members WHERE session = ?1)",
        [session.as_str()],
        |r| r.get(0),
    )
}

fn open_project_group(conn: &Connection, slug: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM groups WHERE slug=?1 AND closed IS NULL",
        [slug],
        |r| r.get(0),
    )
    .optional()
}

fn project_group_membership(
    conn: &Connection,
    session: &SessionId,
) -> rusqlite::Result<Option<(i64, String)>> {
    conn.query_row(
        "SELECT m.group_id, COALESCE(g.slug, 'g-' || g.id) FROM group_members m JOIN groups g ON g.id=m.group_id WHERE m.session=?1",
        [session.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

fn insert_project_group(
    conn: &Connection,
    slug: &str,
    project: &Project,
    now: OffsetDateTime,
    opener: &SessionId,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "INSERT INTO groups(title,kind,project,opened,opener,related,slug) VALUES (?1,'project',?2,?3,?4,'[]',?1) RETURNING id",
        params![slug, project.as_str(), stamp(now), opener.as_str()],
        |r| r.get(0),
    )
}

fn has_open_groups(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM groups WHERE closed IS NULL)",
        [],
        |r| r.get(0),
    )
}

fn newest_open_seat(
    conn: &Connection,
    session: &SessionId,
) -> rusqlite::Result<Option<(i64, String, String, String)>> {
    conn.query_row(
        "SELECT g.id, s.name, s.role, m.author
         FROM groups g
         JOIN group_seats s ON s.group_id = g.id
         JOIN group_members m ON m.group_id = s.group_id AND m.seat = s.name
         WHERE g.closed IS NULL
           AND m.session = ?1
           AND m.position = (
               SELECT MAX(m2.position)
               FROM group_members m2
               WHERE m2.group_id = s.group_id AND m2.seat = s.name
           )",
        [session.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
}

/// Group rows in a write step ([`crate::repository::Groups::rows`]).
/// Reads and mutations report the rows as they are; their meaning and any
/// write refusal are the caller's responsibility.
pub struct GroupRows<'c>(ReadGroupRows<'c>);

impl<'c> std::ops::Deref for GroupRows<'c> {
    type Target = ReadGroupRows<'c>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Read-only access to group rows, including inside a write transaction.
pub struct ReadGroupRows<'c> {
    conn: &'c Connection,
    path: &'c Path,
    action: &'static str,
    read: bool,
}

/// A membership row's group and capture window ([`GroupRows::member_window`]).
#[derive(Debug)]
pub struct MemberWindow {
    /// The group's number.
    pub group: i64,
    /// When the window starts, as stored.
    pub joined: String,
    /// The capture cursor, as stored.
    pub cursor: Option<String>,
    /// Whether the member holds its seat.
    pub holds_seat: bool,
}

/// A [`GroupRows`] call's failure. It converts into [`Error`]; a read keeps
/// whether a read-only copy of the database could serve it.
pub struct RowError(Fallible);

impl std::fmt::Debug for RowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RowError")
    }
}

impl From<RowError> for Error {
    fn from(e: RowError) -> Self {
        e.0.into_error()
    }
}

impl From<Error> for RowError {
    fn from(e: Error) -> Self {
        Self(Fallible::Failed(e))
    }
}

type Rows<T> = std::result::Result<T, RowError>;

impl ReadGroupRows<'_> {
    fn fail(&self, e: rusqlite::Error) -> RowError {
        RowError(if self.read {
            Fallible::from_sqlite(self.action, self.path, e)
        } else {
            Fallible::Failed(db_error(self.action, self.path, e))
        })
    }

    /// The ids of the project's open groups, newest first.
    pub fn open_in_project(&self, project: &Project) -> Rows<Vec<String>> {
        let mut query = self
            .conn
            .prepare("SELECT 'g-' || id FROM groups WHERE project = ?1 AND closed IS NULL ORDER BY id DESC")
            .map_err(|e| self.fail(e))?;
        query
            .query_map([project.as_str()], |row| row.get(0))
            .map_err(|e| self.fail(e))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| self.fail(e))
    }

    /// Insert the group `draft` opened by `opener`; returns its number.
    pub fn insert_group(
        &self,
        draft: &NewGroup,
        opener: &SessionId,
        now: OffsetDateTime,
    ) -> Rows<i64> {
        let related =
            serde_json::to_string(&draft.related).map_err(|e| Error::corrupt(self.path, e))?;
        self.conn
            .query_row(
                "INSERT INTO groups(title,kind,project,opened,opener,related)
                VALUES (?1,?2,?3,?4,?5,?6) RETURNING id",
                params![
                    draft.title,
                    draft.kind,
                    draft.project.as_str(),
                    stamp(now),
                    opener.as_str(),
                    related
                ],
                |r| r.get(0),
            )
            .map_err(|e| self.fail(e))
    }

    /// Whether `session` has a membership row, current or historical.
    pub fn has_membership(&self, session: &SessionId) -> Rows<bool> {
        has_membership(self.conn, session).map_err(|e| self.fail(e))
    }

    /// Whether `session`'s catalog row is forgotten.
    pub fn session_forgotten(&self, session: &SessionId) -> Rows<bool> {
        super::sessions::is_forgotten(self.conn, session).map_err(|e| self.fail(e))
    }

    /// Store `turns` as `session`'s group turns, masked by `redactor`;
    /// returns how many rows were inserted.
    pub fn store_turns(
        &self,
        session: &SessionId,
        turns: &[NewMessage],
        redactor: &Redactor,
    ) -> Rows<usize> {
        Ok(write_captured_turns(
            self.conn,
            self.path,
            session,
            turns.iter().collect(),
            redactor,
        )?)
    }

    /// The group `session` is a member of, with its slug or else its id.
    pub fn project_membership(&self, session: &SessionId) -> Rows<Option<(i64, String)>> {
        project_group_membership(self.conn, session).map_err(|e| self.fail(e))
    }

    /// The open project group with `slug`.
    pub fn open_project(&self, slug: &str) -> Rows<Option<i64>> {
        open_project_group(self.conn, slug).map_err(|e| self.fail(e))
    }

    /// Insert a project group `slug` opened by `opener`; returns its number.
    pub fn insert_project(
        &self,
        slug: &str,
        project: &Project,
        now: OffsetDateTime,
        opener: &SessionId,
    ) -> Rows<i64> {
        insert_project_group(self.conn, slug, project, now, opener).map_err(|e| self.fail(e))
    }

    /// Whether group `n` has a seat named `name`.
    pub fn seat_exists(&self, n: i64, name: &str) -> Rows<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM group_seats WHERE group_id=?1 AND name=?2)",
                params![n, name],
                |r| r.get(0),
            )
            .map_err(|e| self.fail(e))
    }

    /// Seat `session` in group `n` under the new member seat `seat`.
    pub fn seat_member(
        &self,
        n: i64,
        session: &SessionId,
        seat: &Label,
        now: OffsetDateTime,
    ) -> Rows<()> {
        seat_member(self.conn, n, session, seat.as_str(), now).map_err(|e| self.fail(e))
    }

    /// Set `session`'s capture cursor to `message`.
    pub fn set_cursor(&self, session: &SessionId, message: &str) -> Rows<()> {
        self.conn
            .execute(
                "UPDATE group_members SET turn_cursor = ?2 WHERE session = ?1",
                params![session.as_str(), message],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Whether group `n` is closed.
    pub fn is_closed(&self, n: i64) -> Rows<bool> {
        self.conn
            .query_row(
                "SELECT closed IS NOT NULL FROM groups WHERE id = ?1",
                [n],
                |r| r.get(0),
            )
            .map_err(|e| self.fail(e))
    }

    /// The group and seat of `session`'s membership row.
    pub fn membership(&self, session: &SessionId) -> Rows<Option<(i64, String)>> {
        self.conn
            .query_row(
                "SELECT group_id, seat FROM group_members WHERE session = ?1",
                [session.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| self.fail(e))
    }

    /// Seat `seat` of group `n`: its role and its newest member.
    pub fn seat_holder(&self, n: i64, seat: &str) -> Rows<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT s.role, m.session FROM group_seats s
                     JOIN group_members m ON m.group_id = s.group_id AND m.seat = s.name
                     WHERE s.group_id = ?1 AND s.name = ?2 ORDER BY m.position DESC LIMIT 1",
                params![n, seat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| self.fail(e))
    }

    /// Insert seat `seat` of group `n` with `role`.
    pub fn insert_seat(&self, n: i64, seat: &str, role: &str) -> Rows<()> {
        self.conn
            .execute(
                "INSERT INTO group_seats(group_id, name, role) VALUES (?1, ?2, ?3)",
                params![n, seat, role],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Insert `session` as the newest member of seat `seat` in group `n`.
    pub fn insert_member(
        &self,
        n: i64,
        seat: &str,
        session: &SessionId,
        author: &str,
        joined: OffsetDateTime,
    ) -> Rows<()> {
        self.conn
            .execute(
                "INSERT INTO group_members(group_id, seat, session, author, joined) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![n, seat, session.as_str(), author, stamp(joined)],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Delete `session`'s membership row and a seat it leaves empty, and
    /// restart the consent window of a member it displaced; returns the
    /// group, or `None` when `session` had no row.
    pub fn remove_member(&self, session: &SessionId, now: OffsetDateTime) -> Rows<Option<i64>> {
        Ok(
            remove_seat_member(self.conn, self.path, self.action, session, now)?
                .map(|removed| removed.group),
        )
    }

    /// Delete `session`'s group-only stored turns; returns how many.
    pub fn erase_group_turns(&self, session: &SessionId) -> Rows<usize> {
        self.conn
            .execute(
                "DELETE FROM messages WHERE session = ?1 AND group_only = 1",
                [session.as_str()],
            )
            .map_err(|e| self.fail(e))
    }

    /// Clear group `n`'s summary when it has one; returns how many rows
    /// changed.
    pub fn clear_set_summary(&self, n: i64) -> Rows<usize> {
        self.conn
            .execute(
                "UPDATE groups SET summary = NULL WHERE id = ?1 AND summary IS NOT NULL",
                [n],
            )
            .map_err(|e| self.fail(e))
    }

    /// Clear group `n`'s summary.
    pub fn clear_summary(&self, n: i64) -> Rows<()> {
        self.conn
            .execute("UPDATE groups SET summary=NULL WHERE id=?1", [n])
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Optimize the message search index after rows were deleted.
    pub fn optimize_search(&self) -> Rows<()> {
        optimize_fts(self.conn).map_err(|e| self.fail(e))
    }

    /// Truncate the write-ahead log.
    pub fn truncate_wal(&self) -> Rows<()> {
        Ok(super::truncate_wal(
            self.conn,
            self.path,
            std::time::Instant::now(),
        )?)
    }

    /// `session`'s membership row and whether it holds its seat.
    pub fn member_window(&self, session: &SessionId) -> Rows<Option<MemberWindow>> {
        self.conn
            .query_row(
                &format!("SELECT m.group_id, m.joined, m.turn_cursor, {HOLDS_SEAT} FROM group_members m WHERE m.session=?1"),
                [session.as_str()],
                |r| {
                    Ok(MemberWindow {
                        group: r.get(0)?,
                        joined: r.get(1)?,
                        cursor: r.get(2)?,
                        holds_seat: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| self.fail(e))
    }

    /// Set `session`'s joined time and capture cursor as stored.
    pub fn set_window(&self, session: &SessionId, joined: &str, cursor: Option<&str>) -> Rows<()> {
        self.conn
            .execute(
                "UPDATE group_members SET joined=?2, turn_cursor=?3 WHERE session=?1",
                params![session.as_str(), joined, cursor],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Restart `session`'s capture window at `now`, with no cursor.
    pub fn restart_window(&self, session: &SessionId, now: OffsetDateTime) -> Rows<()> {
        self.set_window(session, &stamp(now), None)
    }

    /// Record a deferred ancestry check for `session`; returns whether a row
    /// was added.
    pub fn defer_ancestry(&self, session: &SessionId) -> Rows<bool> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO deferred_ancestry (session) VALUES (?1)",
                [session.as_str()],
            )
            .map(|n| n > 0)
            .map_err(|e| self.fail(e))
    }

    /// Delete `session`'s deferred ancestry check; returns whether there was
    /// one.
    pub fn take_deferred_ancestry(&self, session: &SessionId) -> Rows<bool> {
        self.conn
            .execute(
                "DELETE FROM deferred_ancestry WHERE session = ?1",
                [session.as_str()],
            )
            .map(|n| n > 0)
            .map_err(|e| self.fail(e))
    }

    /// Whether group `n` is open.
    pub fn is_open(&self, n: i64) -> Rows<bool> {
        self.conn
            .query_row(
                "SELECT closed IS NULL FROM groups WHERE id = ?1",
                [n],
                |r| r.get(0),
            )
            .map_err(|e| self.fail(e))
    }

    /// How many members of group `n` hold their seats.
    pub fn holder_count(&self, n: i64) -> Rows<i64> {
        self.conn
            .query_row(
                &format!(
                    "SELECT count(*) FROM group_members m WHERE m.group_id = ?1 AND {HOLDS_SEAT}"
                ),
                [n],
                |r| r.get(0),
            )
            .map_err(|e| self.fail(e))
    }

    /// Whether `session` holds a seat in group `n`, and if so the start of
    /// its restarted consent window: `Some(None)` for a holder whose window
    /// was never restarted.
    pub fn holder_restart(
        &self,
        n: i64,
        session: &SessionId,
    ) -> Rows<Option<Option<OffsetDateTime>>> {
        let window: Option<(String, bool)> = self
            .conn
            .query_row(
                &format!("SELECT m.joined, m.restarted FROM group_members m WHERE m.group_id = ?1 AND m.session = ?2 AND {HOLDS_SEAT}"),
                params![n, session.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| self.fail(e))?;
        let Some((joined, restarted)) = window else {
            return Ok(None);
        };
        if !restarted {
            return Ok(Some(None));
        }
        let at =
            OffsetDateTime::parse(&joined, &Rfc3339).map_err(|e| Error::corrupt(self.path, e))?;
        Ok(Some(Some(at)))
    }

    /// Store the selected `turns` as `session`'s group turns, masked by
    /// `redactor`; returns how many rows were inserted.
    pub fn store_selected(
        &self,
        session: &SessionId,
        turns: Vec<&NewMessage>,
        redactor: &Redactor,
    ) -> Rows<usize> {
        Ok(write_captured_turns(
            self.conn, self.path, session, turns, redactor,
        )?)
    }

    /// `session`'s capture row in an open group, unless it is forgotten.
    pub fn capture_state(&self, session: &SessionId) -> Rows<Option<CaptureState>> {
        capture_state(self.conn, self.path, session).map_err(RowError)
    }

    /// Set the capture cursor of membership row `position` to `message`.
    pub fn set_cursor_at(&self, position: i64, message: &str) -> Rows<()> {
        self.conn
            .execute(
                "UPDATE group_members SET turn_cursor = ?2 WHERE position = ?1",
                params![position, message],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Delete membership row `position`'s Claude transcript progress.
    pub fn clear_progress(&self, position: i64) -> Rows<()> {
        claude::clear_progress(self.conn, position).map_err(|e| self.fail(e))
    }

    /// The seat `session` holds now.
    pub fn held_seat(&self, session: &SessionId) -> Rows<Option<SeatHold>> {
        held_seat(self.conn, session).map_err(|e| self.fail(e))
    }

    /// Whether any group is open.
    pub fn has_open_groups(&self) -> Rows<bool> {
        has_open_groups(self.conn).map_err(|e| self.fail(e))
    }

    /// The open seat `session` holds now: group, seat, role and author.
    pub fn newest_open_seat(
        &self,
        session: &SessionId,
    ) -> Rows<Option<(i64, String, String, String)>> {
        newest_open_seat(self.conn, session).map_err(|e| self.fail(e))
    }
}

#[cfg(test)]
mod migration_tests {
    use super::{V9, V18, V19};
    use rusqlite::{Connection, StatementStatus, params};

    fn migration_steps(size: i64) -> i32 {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(V9).unwrap();
        conn.execute_batch(V18).unwrap();
        for position in 1..=size {
            conn.execute(
                "INSERT INTO group_members (group_id, seat, session, author) VALUES (1, ?1, ?1, 'agent')",
                params![format!("seat-{position}")],
            )
            .unwrap();
        }
        let mut migration = conn.prepare(V19).unwrap();
        assert_eq!(migration.execute([]).unwrap(), 0);
        migration.get_status(StatementStatus::VmStep)
    }

    #[test]
    fn v19_work_grows_linearly_for_distinct_contiguous_holders() {
        let small = migration_steps(256);
        let large = migration_steps(512);
        // A suffix count per holder approaches four times the VM steps on
        // doubling; leave room for fixed costs and SQLite implementation changes.
        assert!(large < small * 3, "256: {small}, 512: {large}");
    }
}
