//! Group records and exclusive seat membership. Every decision and its write share
//! the store transaction. Provider ids have no foreign key to cataloged sessions.

mod claude;
mod close;

pub(super) use claude::V15;
pub use claude::{ClaudeCapturePosition, ClaudeGroupCapture};

use super::{
    DB_FILE, Fallible, db_error, memory::read, open_writer, optimize_fts, refuse_newer, stamp,
};
use crate::checks::refusal;
use crate::group::{
    Added, Group, GroupAdded, GroupAddition, GroupConsent, JoinGroup, NewGroup, OpenedGroup, Seat,
    SeatHold, SeatStart,
};
use crate::message::NewMessage;
use crate::{Error, Project, Redactor, RelatedKind, Result, SessionId, Store, WriteRefusal};
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
pub(super) fn forget_session(
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

fn number(id: &str) -> Option<i64> {
    let n = id.strip_prefix("g-")?.parse::<i64>().ok()?;
    (n > 0 && format!("g-{n}") == id).then_some(n)
}

/// A project group slug: lowercase letters, digits and interior hyphens, not a
/// group id, and free of configured sensitive text.
fn check_project_slug(slug: &str, redactor: &Redactor) -> Result<()> {
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || slug.starts_with('-')
        || slug.ends_with('-')
        || number(slug).is_some()
    {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug must be lowercase letters, digits and interior hyphens, and not a group id",
        ));
    }
    if redactor.mask(slug) != slug {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug contains sensitive text",
        ));
    }
    Ok(())
}

/// Seat `session` in group `n` under an automatic name that no member of the
/// group holds, so a join never displaces anyone.
fn seat_member(
    conn: &Connection,
    n: i64,
    session: &SessionId,
    now: OffsetDateTime,
) -> rusqlite::Result<()> {
    let mut query =
        conn.prepare("SELECT EXISTS(SELECT 1 FROM group_seats WHERE group_id=?1 AND name=?2)")?;
    let seat = crate::group::automatic_seat(session, |name| {
        query.query_row(params![n, name], |r| r.get(0))
    })?;
    drop(query);
    let seat = seat.as_str();
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
    pub fn groups(&self) -> Result<Vec<Group>> {
        self.read_groups(|conn, path| roster(conn, path, None))
    }

    /// The most recently opened group in the project.
    pub fn last_group(&self, project: &Project) -> Result<Option<crate::brief::LastGroup>> {
        self.read_groups(|conn, path| {
            let summary = if refuse_newer(conn, path)? >= 11 { "summary" } else { "NULL" };
            conn.query_row(
                &format!("SELECT id, title, closed, {summary} FROM groups WHERE project = ?1 ORDER BY id DESC LIMIT 1"),
                [project.as_str()],
                |row| Ok(crate::brief::LastGroup {
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

    fn group_number(&self, input: &str) -> Result<i64> {
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
    pub fn group(&self, id: &str) -> Result<Group> {
        let n = self.group_number(id)?;
        self.read_groups(|conn, path| roster(conn, path, Some(n)))?
            .pop()
            .ok_or_else(|| self.missing_group(id))
    }

    /// Search only stored messages whose session belongs to this group.
    /// The database checks membership in the same read as the messages.
    pub fn search_group_messages(
        &self,
        group: &str,
        tokens: &[String],
    ) -> Result<crate::message::MessageSearch> {
        let n = self.group_number(group)?;
        super::search_messages(self.root(), tokens, Some(n))
    }

    /// Open a group on verified consent. The opener joins a seat separately.
    /// `earlier_turns` are the opener's typed turns from the start of its own
    /// transcript. They are stored with the group in the same transaction, so
    /// the line that started the work is kept even though it was typed before
    /// the group existed. An opener that is already a member of a group, or has
    /// been forgotten, stores none: its own join decided what it keeps.
    pub fn open_group(
        &self,
        mut draft: NewGroup,
        consent: &GroupConsent,
        earlier_turns: &[NewMessage],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<OpenedGroup> {
        if !consent.authorizes_open() {
            return Err(refusal(
                WriteRefusal::GroupMembership,
                "the quoted consent was given for another operation, not opening a group",
            ));
        }
        check_own_user_turns(consent.session(), earlier_turns)?;
        crate::check_content(&draft.title)?;
        crate::check_content(&draft.kind)?;
        if draft
            .related
            .iter()
            .any(|r| r.kind() == RelatedKind::Session)
        {
            return Err(refusal(
                WriteRefusal::GroupMembership,
                "group references accept task:, pr: and repo:; use group join for a session",
            ));
        }
        redactor.group(&mut draft);
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("open group in", &path, e);
            let mut query = conn
                .prepare("SELECT 'g-' || id FROM groups WHERE project = ?1 AND closed IS NULL ORDER BY id DESC")
                .map_err(fail)?;
            let still_open = query
                .query_map([draft.project.as_str()], |row| row.get(0))
                .map_err(fail)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(fail)?;
            let related =
                serde_json::to_string(&draft.related).map_err(|e| Error::corrupt(&path, e))?;
            let n: i64 = conn
                .query_row(
                    "INSERT INTO groups(title,kind,project,opened,opener,related)
                VALUES (?1,?2,?3,?4,?5,?6) RETURNING id",
                    params![
                        draft.title,
                        draft.kind,
                        draft.project.as_str(),
                        stamp(now),
                        consent.session().as_str(),
                        related
                    ],
                    |r| r.get(0),
                )
                .map_err(fail)?;
            let opener = consent.session();
            if !earlier_turns.is_empty()
                && !has_membership(&conn, opener).map_err(fail)?
                && !super::sessions::is_forgotten(&conn, opener).map_err(fail)?
            {
                write_captured_turns(
                    &conn,
                    &path,
                    opener,
                    earlier_turns.iter().collect(),
                    redactor,
                )?;
            }
            Ok(OpenedGroup { id: format!("g-{n}"), still_open })
        })
    }

    /// Create or join an open project group on a user-typed hook command.
    pub fn join_project_group(
        &self,
        slug: &str,
        project: &Project,
        session: &SessionId,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String> {
        check_project_slug(slug, redactor)?;
        self.write(|| {
            self.refuse_forgotten(session)?;
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("join project group in", &path, e);
            let held = project_group_membership(&conn, session).map_err(fail)?;
            if let Some((_, name)) = held {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    format!(
                        "session already belongs to group {}; leave it before joining",
                        redactor.mask(&name)
                    ),
                ));
            }
            let existing = open_project_group(&conn, slug).map_err(fail)?;
            let n = match existing {
                Some(n) => n,
                None => insert_project_group(&conn, slug, project, now, session).map_err(fail)?,
            };
            seat_member(&conn, n, session, now).map_err(fail)?;
            Ok(format!("g-{n}"))
        })
    }

    /// Add named past sessions to an open project group on a user-typed hook
    /// command, opening the group when absent. A session already in another
    /// group refuses the whole line, naming each such session and its group, so
    /// nothing changes. A session whose transcript was read has its typed turns
    /// stored, whatever their age, and its capture cursor set to the last of
    /// them; one without a transcript gets membership only. Backfilled turns
    /// must all be user turns belonging to their named session; otherwise the
    /// whole call is refused before it writes anything.
    pub fn add_project_group_sessions(
        &self,
        slug: &str,
        project: &Project,
        opener: &SessionId,
        additions: &[GroupAddition],
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<GroupAdded> {
        check_project_slug(slug, redactor)?;
        if additions.iter().any(|addition| {
            addition.turns.as_ref().is_some_and(|turns| {
                turns.iter().any(|turn| {
                    turn.session != addition.session || turn.role != crate::Speaker::User
                })
            })
        }) {
            return Err(refusal(
                WriteRefusal::GroupMembership,
                "project group backfill takes only the named session's user turns",
            ));
        }
        self.write(|| {
            for addition in additions {
                self.refuse_forgotten(&addition.session)?;
            }
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("add to project group in", &path, e);
            let existing = open_project_group(&conn, slug).map_err(fail)?;
            let mut elsewhere = Vec::new();
            let mut already = Vec::new();
            for addition in additions {
                let held = project_group_membership(&conn, &addition.session).map_err(fail)?;
                match held {
                    Some((group, _)) if Some(group) == existing => {
                        already.push(addition.session.as_str());
                    }
                    Some((_, name)) => elsewhere.push(format!(
                        "session {} already belongs to group {}",
                        addition.session,
                        redactor.mask(&name)
                    )),
                    None => {}
                }
            }
            if !elsewhere.is_empty() {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    format!("{}; nothing changed", elsewhere.join("; ")),
                ));
            }
            let (n, created) = match existing {
                Some(n) => (n, false),
                None => (
                    insert_project_group(&conn, slug, project, now, opener).map_err(fail)?,
                    true,
                ),
            };
            let mut sessions = Vec::new();
            for addition in additions {
                if already.contains(&addition.session.as_str()) {
                    sessions.push((addition.session.clone(), Added::AlreadyMember));
                    continue;
                }
                seat_member(&conn, n, &addition.session, now).map_err(fail)?;
                let stored = match &addition.turns {
                    None => None,
                    Some(turns) => {
                        let inserted = write_captured_turns(
                            &conn,
                            &path,
                            &addition.session,
                            turns.iter().collect(),
                            redactor,
                        )?;
                        if let Some(last) = turns.last() {
                            conn.execute(
                                "UPDATE group_members SET turn_cursor = ?2 WHERE session = ?1",
                                params![addition.session.as_str(), last.message],
                            )
                            .map_err(fail)?;
                        }
                        Some(inserted)
                    }
                };
                sessions.push((addition.session.clone(), Added::Joined { stored }));
            }
            Ok(GroupAdded {
                id: format!("g-{n}"),
                created,
                sessions,
            })
        })
    }

    /// Append a member, or return false on retry. An ended transcript is named
    /// explicitly so a stale liveness check cannot authorize a different replacement.
    pub fn join_group(&self, join: JoinGroup<'_>, redactor: &Redactor) -> Result<bool> {
        crate::check_agent_author(join.author)?;
        if let Some(role) = join.role {
            crate::check_content(role)?;
        }
        check_own_user_turns(join.session, join.earlier_turns)?;
        self.write(|| {
            self.refuse_forgotten(join.session)?;
            let n = self.group_number(join.group)?;
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("join group in", &path, e);
            let seat = join.seat.as_str();
            let session = join.session.as_str();
            let closed: bool = conn
                .query_row("SELECT closed IS NOT NULL FROM groups WHERE id = ?1", [n], |r| r.get(0))
                .map_err(fail)?;
            let held: Option<(i64, String)> = conn
                .query_row(
                    "SELECT group_id, seat FROM group_members WHERE session = ?1",
                    [session],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(fail)?;
            if let Some((group, name)) = &held
                && (*group != n || name != seat)
            {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    format!("the calling session already belongs to group g-{group}; leave it before joining"),
                ));
            }
            if closed && held.is_none() {
                return Err(refusal(WriteRefusal::GroupMembership, "the group is already closed"));
            }

            let role = join.role.map(|role| redactor.mask(role));
            let current: Option<(String, String)> = conn
                .query_row(
                    "SELECT s.role, m.session FROM group_seats s
                     JOIN group_members m ON m.group_id = s.group_id AND m.seat = s.name
                     WHERE s.group_id = ?1 AND s.name = ?2 ORDER BY m.position DESC LIMIT 1",
                    params![n, seat],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(fail)?;
            if let Some((stored_role, latest)) = current {
                if role.as_ref().is_some_and(|r| r != &stored_role) {
                    return Err(refusal(
                        WriteRefusal::GroupMembership,
                        "the seat already has a different role; omit --role to keep it",
                    ));
                }
                if held.is_some() {
                    return Ok(false);
                }
                if !join.replace
                    && join.ended.map(SessionId::as_str) != Some(latest.as_str())
                    && join.predecessor.map(SessionId::as_str) != Some(latest.as_str())
                {
                    return Err(refusal(
                        WriteRefusal::SeatOccupied,
                        format!("seat {seat} is held by session {latest}; pass --replace to take it over"),
                    ));
                }
            } else {
                let role = role.unwrap_or_else(|| redactor.mask(seat));
                conn.execute(
                    "INSERT INTO group_seats(group_id, name, role) VALUES (?1, ?2, ?3)",
                    params![n, seat, role],
                )
                .map_err(fail)?;
            }
            conn.execute(
                "INSERT INTO group_members(group_id, seat, session, author, joined) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![n, seat, session, join.author.as_str(), stamp(join.snapshot_at)],
            )
            .map_err(fail)?;
            // The capture starts at the session's own first line, not at the
            // join time, so the cursor takes over from its last stored turn.
            // With no stored turn the join time is the snapshot's, taken before
            // the transcript was read, so a turn typed since stays in capture.
            if let Some(last) = join.earlier_turns.last() {
                write_captured_turns(
                    &conn,
                    &path,
                    join.session,
                    join.earlier_turns.iter().collect(),
                    redactor,
                )?;
                conn.execute(
                    "UPDATE group_members SET turn_cursor = ?2 WHERE session = ?1",
                    params![session, last.message],
                )
                .map_err(fail)?;
            }
            Ok(true)
        })
    }

    /// Remove only the caller from its chain and remove a seat left empty.
    pub fn leave_group(&self, session: &SessionId) -> Result<String> {
        self.leave_group_inner(session, false)
    }

    fn leave_group_inner(&self, session: &SessionId, keep_turns: bool) -> Result<String> {
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("leave group in", &path, e);
            let n = remove_seat_member(
                &conn,
                &path,
                "leave group in",
                session,
                OffsetDateTime::now_utc(),
            )?
            .ok_or_else(|| {
                refusal(
                    WriteRefusal::GroupMembership,
                    "the calling session has no group membership",
                )
            })?
            .group;
            let erased = if keep_turns {
                0
            } else {
                conn.execute(
                    "DELETE FROM messages WHERE session = ?1 AND group_only = 1",
                    [session.as_str()],
                )
                .map_err(fail)?
            };
            let cleared = conn
                .execute(
                    "UPDATE groups SET summary = NULL WHERE id = ?1 AND summary IS NOT NULL",
                    [n],
                )
                .map_err(fail)?;
            if erased > 0 {
                optimize_fts(&conn).map_err(fail)?;
            }
            if erased > 0 || cleared > 0 {
                super::truncate_wal(&conn, &path, std::time::Instant::now())?;
            }
            Ok(format!("g-{n}"))
        })
    }

    /// Move a member atomically, retaining its stored turns and capture cursor.
    pub fn move_project_group(
        &self,
        session: &SessionId,
        slug: &str,
        project: &Project,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String> {
        check_project_slug(slug, redactor)?;
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("move group member in", &path, e);
            let (old, joined, cursor, held): (i64, String, Option<String>, bool) = conn
                .query_row(
                    &format!("SELECT m.group_id, m.joined, m.turn_cursor, {HOLDS_SEAT} FROM group_members m WHERE m.session=?1"),
                    [session.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()
                .map_err(fail)?
                .ok_or_else(|| {
                    refusal(
                        WriteRefusal::GroupMembership,
                        "session has no group membership",
                    )
                })?;
            let target = open_project_group(&conn, slug).map_err(fail)?;
            if target == Some(old) {
                return Ok(format!("g-{old}"));
            }
            self.leave_group_inner(session, true)?;
            let id = self.join_project_group(slug, project, session, now, redactor)?;
            let (joined, cursor) = if held { (joined, cursor) } else { (stamp(now), None) };
            conn.execute(
                "UPDATE group_members SET joined=?2, turn_cursor=?3 WHERE session=?1",
                params![session.as_str(), joined, cursor],
            )
            .map_err(fail)?;
            if let Some(n) = target {
                conn.execute("UPDATE groups SET summary=NULL WHERE id=?1", [n])
                    .map_err(fail)?;
            }
            Ok(id)
        })
    }

    /// Record that a start could not inspect its transcript yet.
    pub fn defer_ancestry(&self, session: &SessionId) -> Result<bool> {
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            conn.execute(
                "INSERT OR IGNORE INTO deferred_ancestry (session) VALUES (?1)",
                [session.as_str()],
            )
            .map(|n| n > 0)
            .map_err(|e| db_error("defer ancestry in", &path, e))
        })
    }

    /// Whether a deferred ancestry check is pending for `session`.
    pub fn has_deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
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

    /// Consume a deferred check once, without writing on ordinary prompts.
    pub fn take_deferred_ancestry(&self, session: &SessionId) -> Result<bool> {
        if !self.has_deferred_ancestry(session)? {
            return Ok(false);
        }
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            conn.execute(
                "DELETE FROM deferred_ancestry WHERE session = ?1",
                [session.as_str()],
            )
            .map(|n| n > 0)
            .map_err(|e| db_error("consume deferred ancestry in", &path, e))
        })
    }

    /// Inherit the predecessor's open seat for a newly cleared session.
    /// An ordinary start takes no write lock; any candidate write rechecks
    /// seat ownership inside the transaction.
    pub fn inherit_seat(
        &self,
        child: &SessionId,
        predecessor: Option<&SessionId>,
    ) -> Result<SeatStart> {
        #[derive(Default)]
        enum Snapshot {
            #[default]
            NoOpenGroups,
            AlreadySeated(SeatHold),
            Replaced,
            Inherit,
            Unjoined,
        }

        let predecessor = predecessor.filter(|parent| *parent != child);
        let snap = self.read_groups(|conn, path| {
            let fail = |e| Fallible::from_sqlite("read groups from", path, e);
            if let Some(hold) = held_seat(conn, child).map_err(fail)? {
                return Ok(Snapshot::AlreadySeated(hold));
            }
            if !has_open_groups(conn).map_err(fail)? {
                return Ok(Snapshot::NoOpenGroups);
            }
            if has_membership(conn, child).map_err(fail)? {
                return Ok(Snapshot::Replaced);
            }
            let Some(parent) = predecessor else {
                return Ok(Snapshot::Unjoined);
            };
            if newest_open_seat(conn, parent).map_err(fail)?.is_none() {
                return Ok(Snapshot::Unjoined);
            }
            Ok(Snapshot::Inherit)
        })?;
        let Snapshot::Inherit = snap else {
            return Ok(match snap {
                Snapshot::NoOpenGroups => SeatStart::NoOpenGroups,
                Snapshot::AlreadySeated(hold) => SeatStart::AlreadySeated(hold),
                Snapshot::Replaced => SeatStart::Replaced,
                Snapshot::Unjoined => SeatStart::Unjoined,
                Snapshot::Inherit => unreachable!(),
            });
        };
        let Some(parent) = predecessor else {
            return Ok(SeatStart::Unjoined);
        };
        self.write(|| {
            self.refuse_forgotten(child)?;
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("inherit group seat in", &path, e);
            if let Some(hold) = held_seat(&conn, child).map_err(fail)? {
                return Ok(SeatStart::AlreadySeated(hold));
            }
            if !has_open_groups(&conn).map_err(fail)? {
                return Ok(SeatStart::NoOpenGroups);
            }
            if has_membership(&conn, child).map_err(fail)? {
                return Ok(SeatStart::Replaced);
            }
            let Some((group, seat, role, author)) =
                newest_open_seat(&conn, parent).map_err(fail)?
            else {
                return Ok(SeatStart::Unjoined);
            };
            conn.execute(
                "INSERT INTO group_members(group_id, seat, session, author, joined) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![group, seat, child.as_str(), author, stamp(OffsetDateTime::now_utc())],
            )
            .map_err(fail)?;
            Ok(SeatStart::Inherited {
                hold: SeatHold {
                    group: format!("g-{group}"),
                    seat,
                    role,
                },
                from: parent.clone(),
            })
        })
    }

    /// Backfill verified turns for the current holders without changing capture cursors.
    /// Membership and the open state are checked again in the write transaction.
    pub fn backfill_group_turns(
        &self,
        id: &str,
        members: &[(SessionId, Vec<NewMessage>)],
        redactor: &Redactor,
    ) -> Result<Vec<usize>> {
        for (session, turns) in members {
            check_own_user_turns(session, turns)?;
        }
        let n = self.group_number(id)?;
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("backfill group turns in", &path, e);
            let open: bool = conn.query_row(
                "SELECT closed IS NULL FROM groups WHERE id = ?1", [n], |r| r.get(0),
            ).map_err(fail)?;
            if !open {
                return Err(refusal(WriteRefusal::GroupMembership, "the group is already closed"));
            }
            let holders: i64 = conn.query_row(
                &format!("SELECT count(*) FROM group_members m WHERE m.group_id = ?1 AND {HOLDS_SEAT}"),
                [n], |r| r.get(0),
            ).map_err(fail)?;
            if usize::try_from(holders).ok() != Some(members.len()) {
                return Err(refusal(WriteRefusal::GroupMembership, "group membership changed during backfill; retry"));
            }
            let mut counts = Vec::with_capacity(members.len());
            for (session, turns) in members {
                let window: Option<(String, bool)> = conn.query_row(
                    &format!("SELECT m.joined, m.restarted FROM group_members m WHERE m.group_id = ?1 AND m.session = ?2 AND {HOLDS_SEAT}"),
                    params![n, session.as_str()], |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional().map_err(fail)?;
                let Some((joined, restarted)) = window else {
                    return Err(refusal(WriteRefusal::GroupMembership, "group membership changed during backfill; retry"));
                };
                // A restored displaced holder restarted its consent window at
                // its new join time: turns typed before it, during the
                // displacement, were never authorized, so backfill keeps the
                // same cutoff as capture.
                let since = if restarted {
                    Some(OffsetDateTime::parse(&joined, &Rfc3339).map_err(|e| Error::corrupt(&path, e))?)
                } else {
                    None
                };
                let eligible = turns.iter().filter(|m| since.is_none_or(|s| m.at >= s)).collect();
                counts.push(write_captured_turns(&conn, &path, session, eligible, redactor)?);
            }
            Ok(counts)
        })
    }

    /// Whether an open group authorizes capture for this session. A missing,
    /// closed or forgotten membership takes no write lock in the hook.
    pub fn captures_group_turns(&self, session: &SessionId) -> Result<bool> {
        self.read_groups(|conn, path| Ok(capture_state(conn, path, session)?.is_some()))
    }

    /// Store previously unprocessed turns from a verified transcript, in its
    /// order. The membership and cursor are rechecked inside the write.
    /// Returns how many new message rows were persisted.
    pub fn capture_group_turns(
        &self,
        session: &SessionId,
        messages: &[NewMessage],
        redactor: &Redactor,
    ) -> Result<usize> {
        if messages
            .iter()
            .any(|m| &m.session != session || m.role != crate::Speaker::User)
        {
            return Err(refusal(
                WriteRefusal::GroupMembership,
                "group capture takes only the member's verified user turns",
            ));
        }
        let Some(before) = self.read_groups(|conn, path| capture_state(conn, path, session))?
        else {
            return Ok(0);
        };
        if !before.holds {
            return self.skip_displaced_turns(session, &before, messages);
        }
        if next_turns(&before, messages).is_empty() {
            return Ok(0);
        }
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("store group turns in", &path, e);
            let Some(state) = capture_state(&conn, &path, session).map_err(Fallible::into_error)?
            else {
                return Ok(0);
            };
            if state.position != before.position || !state.holds {
                return Ok(0);
            }
            let turns = next_turns(&state, messages);
            let Some(last) = turns.last().copied() else {
                return Ok(0);
            };
            let inserted = write_captured_turns(&conn, &path, session, turns, redactor)?;
            conn.execute(
                "UPDATE group_members SET turn_cursor = ?2 WHERE position = ?1",
                params![state.position, last.message],
            )
            .map_err(fail)?;
            claude::clear_progress(&conn, state.position).map_err(fail)?;
            Ok(inserted)
        })
    }

    /// Move a displaced member's cursor past the turns it typed while a later
    /// member held its seat, storing none of them, so they stay out of the
    /// group if the seat comes back to it.
    fn skip_displaced_turns(
        &self,
        session: &SessionId,
        before: &CaptureState,
        messages: &[NewMessage],
    ) -> Result<usize> {
        let Some(last) = messages.last() else {
            return Ok(0);
        };
        if before.cursor.as_deref() == Some(last.message.as_str()) {
            return Ok(0);
        }
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let Some(state) = capture_state(&conn, &path, session).map_err(Fallible::into_error)?
            else {
                return Ok(0);
            };
            if state.position == before.position && !state.holds {
                conn.execute(
                    "UPDATE group_members SET turn_cursor = ?2 WHERE position = ?1",
                    params![state.position, last.message],
                )
                .map_err(|e| db_error("skip displaced group turns in", &path, e))?;
                claude::clear_progress(&conn, state.position)
                    .map_err(|e| db_error("skip displaced group turns in", &path, e))?;
            }
            Ok(0)
        })
    }
}

/// A backfill takes only the named session's own user turns.
fn check_own_user_turns(session: &SessionId, turns: &[NewMessage]) -> Result<()> {
    if turns
        .iter()
        .any(|m| &m.session != session || m.role != crate::Speaker::User)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group backfill takes only the member's own user turns",
        ));
    }
    Ok(())
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

#[derive(PartialEq, Eq)]
struct CaptureState {
    position: i64,
    /// Whether the member holds its seat now. A replaced member keeps its row
    /// but stores nothing.
    holds: bool,
    since: OffsetDateTime,
    cursor: Option<String>,
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

fn next_turns<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Vec<&'a NewMessage> {
    turns_since(state, after_cursor(state, messages).unwrap_or_default())
}

fn after_cursor<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Option<&'a [NewMessage]> {
    match &state.cursor {
        Some(cursor) => messages
            .iter()
            .position(|m| &m.message == cursor)
            .map(|i| &messages[i + 1..]),
        None => Some(messages),
    }
}

/// The turns a member's capture may store. A member with no cursor holds its
/// join time as a cutoff, so older turns stay out. Once a cursor is set (by a
/// backfill or an earlier capture) it is the continuity point: every turn after
/// it is new whatever its timestamp, since a turn appended while the cursor's
/// snapshot was being read can be dated before the membership was committed.
/// Restoring a displaced holder's seat clears its cursor (`leave_group`,
/// `forget_session`), so the new join time cuts off the turns typed during the
/// displacement.
fn turns_since<'a>(state: &CaptureState, messages: &'a [NewMessage]) -> Vec<&'a NewMessage> {
    if state.cursor.is_some() {
        return messages.iter().collect();
    }
    messages.iter().filter(|m| m.at >= state.since).collect()
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
