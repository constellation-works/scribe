//! Group records and exclusive seat membership. Every decision and its write share
//! the store transaction. Provider ids have no foreign key to cataloged sessions.

use super::{DB_FILE, Fallible, db_error, memory::read, open_writer, refuse_newer, stamp};
use crate::group::{Group, GroupConsent, JoinGroup, NewGroup, Seat, SeatHold, SeatStart};
use crate::store::agent::refusal;
use crate::{Error, Redactor, RelatedKind, Result, SessionId, Store, WriteRefusal};
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

/// The handoff lookup follows the session reference and ranks active handoffs across
/// the whole seat. A single SELECT keeps metadata, chains and handoffs in one snapshot.
const ROSTER: &str = "
WITH handoffs AS (
    SELECT substr(r.value, 9) AS session, e.id, e.created, e.n
    FROM entries e JOIN json_each(e.related) r
    WHERE e.tag = 'handoff' AND e.status = 'active' AND substr(r.value, 1, 8) = 'session:'
)
SELECT g.id, g.title, g.kind, g.project, g.opened, g.closed, g.opener, g.related,
       s.name, s.role, m.session, h.id, h.created, h.n
FROM groups g LEFT JOIN group_seats s ON s.group_id = g.id
LEFT JOIN group_members m ON m.group_id = s.group_id AND m.seat = s.name
LEFT JOIN handoffs h ON h.session = m.session
WHERE ?1 IS NULL OR g.id = ?1
ORDER BY g.id DESC, s.name, m.position";

fn roster(conn: &Connection, path: &Path, number: Option<i64>) -> Result<Vec<Group>, Fallible> {
    let fail = |e| Fallible::from_sqlite("read groups from", path, e);
    let mut query = conn.prepare(ROSTER).map_err(fail)?;
    let mut rows = query.query([number]).map_err(fail)?;
    let mut groups: Vec<Group> = Vec::new();
    let mut newest = None;
    while let Some(row) = rows.next().map_err(fail)? {
        let id = format!("g-{}", row.get::<_, i64>(0).map_err(fail)?);
        if groups.last().is_none_or(|g| g.id != id) {
            let related: String = row.get(7).map_err(fail)?;
            groups.push(Group {
                id,
                title: row.get(1).map_err(fail)?,
                kind: row.get(2).map_err(fail)?,
                project: row.get(3).map_err(fail)?,
                opened: row.get(4).map_err(fail)?,
                closed: row.get(5).map_err(fail)?,
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

    fn missing_group(&self, id: &str) -> Error {
        refusal(
            WriteRefusal::GroupNotFound,
            format!("no group {id} under {}", self.root().display()),
        )
    }

    fn group_number(&self, input: &str) -> Result<i64> {
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

    /// Open a group on verified consent. The opener joins a seat separately.
    pub fn open_group(
        &self,
        mut draft: NewGroup,
        consent: &GroupConsent,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<String> {
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
                .map_err(|e| db_error("open group in", &path, e))?;
            Ok(format!("g-{n}"))
        })
    }

    /// Append a member, or return false on retry. An ended transcript is named
    /// explicitly so a stale liveness check cannot authorize a different replacement.
    pub fn join_group(&self, join: JoinGroup<'_>, redactor: &Redactor) -> Result<bool> {
        crate::check_agent_author(join.author)?;
        if let Some(role) = join.role {
            crate::check_content(role)?;
        }
        self.write(|| {
            let n = self.group_number(join.group)?;
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("join group in", &path, e);
            let seat = join.seat.as_str();
            let session = join.session.as_str();
            let held: Option<(i64, String)> = conn
                .query_row(
                    "SELECT group_id, seat FROM group_members WHERE session = ?1",
                    [session],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(fail)?;
            if held.as_ref().is_some_and(|(group, name)| *group != n || name != seat) {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    "the calling session already belongs to another seat; leave it before joining",
                ));
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
                "INSERT INTO group_members(group_id, seat, session, author) VALUES (?1, ?2, ?3, ?4)",
                params![n, seat, session, join.author.as_str()],
            )
            .map_err(fail)?;
            Ok(true)
        })
    }

    /// Remove only the caller from its chain and remove a seat left empty.
    pub fn leave_group(&self, session: &SessionId) -> Result<String> {
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("leave group in", &path, e);
            let (n, seat): (i64, String) = conn
                .query_row(
                    "DELETE FROM group_members WHERE session=?1 RETURNING group_id,seat",
                    [session.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(fail)?
                .ok_or_else(|| {
                    refusal(
                        WriteRefusal::GroupMembership,
                        "the calling session has no group membership",
                    )
                })?;
            conn.execute(
                "DELETE FROM group_seats WHERE group_id=?1 AND name=?2 AND NOT EXISTS
                (SELECT 1 FROM group_members WHERE group_id=?1 AND seat=?2)",
                params![n, seat],
            )
            .map_err(fail)?;
            Ok(format!("g-{n}"))
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
                "INSERT INTO group_members(group_id, seat, session, author) VALUES (?1, ?2, ?3, ?4)",
                params![group, seat, child.as_str(), author],
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
}

/// The seat the session holds now. Its row must be the seat's newest position:
/// a session that a later member replaced keeps its row as history but holds nothing.
fn held_seat(conn: &Connection, session: &SessionId) -> rusqlite::Result<Option<SeatHold>> {
    conn.query_row(
        "SELECT m.group_id, m.seat, s.role
         FROM group_members m
         JOIN group_seats s ON s.group_id = m.group_id AND s.name = m.seat
         WHERE m.session = ?1
           AND m.position = (
               SELECT MAX(m2.position)
               FROM group_members m2
               WHERE m2.group_id = m.group_id AND m2.seat = m.seat
           )",
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
