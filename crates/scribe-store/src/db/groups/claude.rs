//! Incremental Claude group capture, scoped to one membership position.

use super::{CaptureState, GroupRows, ReadGroupRows, RowError, Rows, capture_state};
use crate::ClaudeSeen;
use crate::SessionId;
use crate::db::{Fallible, refuse_newer};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub(in crate::db) const V15: &str = "
CREATE TABLE group_claude_progress (
    position INTEGER PRIMARY KEY REFERENCES group_members(position) ON DELETE CASCADE,
    file_id TEXT NOT NULL,
    offset INTEGER NOT NULL CHECK (offset > 0),
    modified TEXT NOT NULL,
    boundary TEXT NOT NULL
) STRICT;
CREATE TABLE group_claude_seen (
    position INTEGER NOT NULL REFERENCES group_claude_progress(position) ON DELETE CASCADE,
    id_hash TEXT NOT NULL,
    permitted INTEGER CHECK (permitted IN (0, 1)),
    message INTEGER NOT NULL CHECK (message IN (0, 1)),
    PRIMARY KEY (position, id_hash)
) STRICT, WITHOUT ROWID;
";

/// Byte progress in a checked Claude transcript. Local file identities
/// and boundary hashes are rebuildable and are excluded from exports.
#[derive(PartialEq, Eq)]
pub struct ClaudeCapturePosition {
    /// Device and inode of the checked file.
    pub file_id: String,
    /// Next byte after the last processed JSONL record.
    pub offset: i64,
    /// Modification time observed while reading the file.
    pub modified: String,
    /// Hash of bounded samples at the start and processed end.
    pub boundary: String,
}

/// The membership and progress a hook observed before reading its transcript.
#[derive(PartialEq, Eq)]
pub struct ClaudeGroupCapture {
    member: CaptureState,
    progress: Option<ClaudeCapturePosition>,
}

impl ClaudeGroupCapture {
    /// Where the hook can resume after checking the open file.
    #[must_use]
    pub fn progress(&self) -> Option<&ClaudeCapturePosition> {
        self.progress.as_ref()
    }

    /// The membership the hook observed.
    #[must_use]
    pub fn member(&self) -> &CaptureState {
        &self.member
    }
}

impl ReadGroupRows<'_> {
    /// `session`'s capture row and its Claude transcript progress.
    pub fn claude_snapshot(&self, session: &SessionId) -> Rows<Option<ClaudeGroupCapture>> {
        snapshot(self.conn, self.path, session).map_err(RowError)
    }
}

impl GroupRows<'_> {
    /// Keep membership row `position`'s byte progress in its transcript.
    pub fn save_progress(&self, position: i64, at: &ClaudeCapturePosition) -> Rows<()> {
        self.conn
            .execute(
                "INSERT INTO group_claude_progress (position, file_id, offset, modified, boundary)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(position) DO UPDATE SET
                 file_id = excluded.file_id, offset = excluded.offset,
                 modified = excluded.modified, boundary = excluded.boundary",
                (position, &at.file_id, at.offset, &at.modified, &at.boundary),
            )
            .map(drop)
            .map_err(|e| self.fail(e))
    }

    /// Keep the hashed ids membership row `position` has now seen.
    pub fn save_seen(&self, position: i64, seen: Vec<(String, ClaudeSeen)>) -> Rows<()> {
        let mut save = self
            .conn
            .prepare(
                "INSERT INTO group_claude_seen (position, id_hash, permitted, message)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT(position, id_hash) DO UPDATE SET
                 permitted = excluded.permitted, message = excluded.message",
            )
            .map_err(|e| self.fail(e))?;
        for (id, seen) in seen {
            save.execute((position, id, seen.permitted, seen.message))
                .map_err(|e| self.fail(e))?;
        }
        Ok(())
    }
}

impl<'c> ReadGroupRows<'c> {
    /// The ids membership row `position` has seen, read one at a time
    /// ([`ClaudeSeenIds::get`]).
    pub fn seen(&self, position: i64) -> Rows<ClaudeSeenIds<'_, 'c>> {
        let lookup = self
            .conn
            .prepare(
                "SELECT permitted, message FROM group_claude_seen
                 WHERE position = ?1 AND id_hash = ?2",
            )
            .map_err(|e| self.fail(e))?;
        Ok(ClaudeSeenIds {
            rows: self,
            position,
            lookup,
        })
    }
}

/// The message ids one membership row has seen, read through one prepared
/// statement.
pub struct ClaudeSeenIds<'r, 'c> {
    rows: &'r ReadGroupRows<'c>,
    position: i64,
    lookup: rusqlite::Statement<'c>,
}

impl ClaudeSeenIds<'_, '_> {
    /// What the row has seen of the message whose id hashes to `id`; the
    /// default when it has seen nothing of it.
    pub fn get(&mut self, id: &str) -> Rows<ClaudeSeen> {
        self.lookup
            .query_row((self.position, id), |row| {
                Ok(ClaudeSeen {
                    permitted: row.get(0)?,
                    message: row.get(1)?,
                })
            })
            .optional()
            .map(|seen| seen.unwrap_or_default())
            .map_err(|e| self.rows.fail(e))
    }
}

fn snapshot(
    conn: &Connection,
    path: &Path,
    session: &SessionId,
) -> std::result::Result<Option<ClaudeGroupCapture>, Fallible> {
    let Some(member) = capture_state(conn, path, session)? else {
        return Ok(None);
    };
    let progress = if refuse_newer(conn, path)? >= 15 {
        conn.query_row(
            "SELECT file_id, offset, modified, boundary FROM group_claude_progress WHERE position = ?1",
            [member.position],
            |row| Ok(ClaudeCapturePosition {
                file_id: row.get(0)?,
                offset: row.get(1)?,
                modified: row.get(2)?,
                boundary: row.get(3)?,
            }),
        ).optional().map_err(|e| Fallible::from_sqlite("read Claude group progress from", path, e))?
    } else {
        None
    };
    Ok(Some(ClaudeGroupCapture { member, progress }))
}

pub(super) fn clear_progress(conn: &Connection, member: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM group_claude_progress WHERE position = ?1",
        [member],
    )?;
    Ok(())
}
