//! The rows a group close reads and writes: its evidence, the opener's seat
//! chain and the close itself. The close's flow and its refusals are the
//! caller's (`scribe-core`'s group app).

use super::{GroupRows, HOLDS_SEAT, ReadGroupRows, RowError, Rows, db_error, stamp};
use crate::{Error, Related};
use cw_sqlite::rusqlite::{self, OptionalExtension, params};
use time::OffsetDateTime;

const MEMBERS: &str = "SELECT gm.session FROM group_members gm WHERE gm.group_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM sessions s
                       WHERE s.provider_session = gm.session AND s.forgotten IS NOT NULL)";

/// A group's row as a close reads it ([`ReadGroupRows::header`]).
#[derive(Debug)]
pub struct CloseHeader {
    /// The session that opened the group.
    pub opener: String,
    /// Whether the group is open.
    pub open: bool,
    /// The first 500 characters of its title.
    pub title: String,
    /// Its related references, as stored.
    pub related: String,
}

impl ReadGroupRows<'_> {
    fn evidence_fail(&self, e: rusqlite::Error) -> RowError {
        RowError(db_error("read evidence for group close from", self.path, e))
    }

    /// Group `n`'s row as a close reads it.
    pub fn header(&self, n: i64) -> Rows<Option<CloseHeader>> {
        self.conn
            .query_row(
                "SELECT opener, closed IS NULL, substr(title, 1, 500), related FROM groups WHERE id = ?1",
                [n],
                |r| {
                    Ok(CloseHeader {
                        opener: r.get(0)?,
                        open: r.get(1)?,
                        title: r.get(2)?,
                        related: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|e| self.evidence_fail(e))
    }

    /// Whether `caller` holds the seat whose chain in group `n` contains
    /// `opener`.
    pub fn holds_opener_seat(&self, n: i64, caller: &str, opener: &str) -> Rows<bool> {
        self.conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM group_members m
                 JOIN group_members o ON o.group_id = m.group_id AND o.seat = m.seat
                 WHERE m.group_id = ?1 AND m.session = ?2 AND o.session = ?3 AND {HOLDS_SEAT})"
                ),
                params![n, caller, opener],
                |r| r.get(0),
            )
            .map_err(|e| {
                RowError(db_error(
                    "read seat chain for group close from",
                    self.path,
                    e,
                ))
            })
    }

    /// Decode a group's stored related references.
    pub fn related(&self, stored: &str) -> Rows<Vec<Related>> {
        Ok(serde_json::from_str(stored).map_err(|e| Error::corrupt(self.path, e))?)
    }

    /// The newest active handoffs of group `n`'s unforgotten members: id and
    /// the first 1200 characters, at most 16.
    pub fn member_handoffs(&self, n: i64) -> Rows<Vec<(String, String)>> {
        let mut handoffs = self
            .conn
            .prepare(&format!(
                "SELECT e.id, substr(e.content, 1, 1200) FROM entries e
         WHERE e.tag = 'handoff' AND e.status = 'active' AND EXISTS (
             SELECT 1 FROM json_each(e.related)
             WHERE substr(value, 1, 8) = 'session:' AND substr(value, 9) IN ({MEMBERS})
         )
         ORDER BY julianday(e.created) DESC, e.n DESC LIMIT 16",
            ))
            .map_err(|e| self.evidence_fail(e))?;
        handoffs
            .query_map([n], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| self.evidence_fail(e))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| self.evidence_fail(e))
    }

    /// The newest typed turns of group `n`'s unforgotten members: session,
    /// message id and text, bounded, at most 32.
    pub fn member_turns(&self, n: i64) -> Rows<Vec<(String, String, String)>> {
        let mut turns = self
            .conn
            .prepare(&format!(
                "SELECT session, substr(message, 1, 128), substr(text, 1, 500) FROM messages
         WHERE role = 'user' AND session IN ({MEMBERS})
         ORDER BY julianday(at) DESC, id DESC LIMIT 32",
            ))
            .map_err(|e| self.evidence_fail(e))?;
        turns
            .query_map([n], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| self.evidence_fail(e))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| self.evidence_fail(e))
    }
}

impl GroupRows<'_> {
    /// Close group `n` at `now` with `summary`.
    pub fn set_closed(&self, n: i64, now: OffsetDateTime, summary: Option<&str>) -> Rows<()> {
        self.conn
            .execute(
                "UPDATE groups SET closed = ?2, summary = ?3 WHERE id = ?1",
                params![n, stamp(now), summary],
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }
}
