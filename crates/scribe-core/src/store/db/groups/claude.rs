//! Incremental Claude group capture, scoped to one membership position.

use super::{CaptureState, after_cursor, capture_state, turns_since, write_captured_turns};
use crate::instruction::{ClaudeGroupTurns, ClaudeSeen};
use crate::store::db::{DB_FILE, Fallible, db_error, open_writer, refuse_newer};
use crate::{Redactor, Result, SessionId, Store};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub(in crate::store::db) const V15: &str = "
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
}

impl Store {
    /// Read the capture snapshot without locking or migrating the database.
    pub fn claude_group_capture(&self, session: &SessionId) -> Result<Option<ClaudeGroupCapture>> {
        self.read_groups(|conn, path| snapshot(conn, path, session))
    }

    /// Commit the verified batch, first-seen ids and byte position together.
    /// A stale snapshot returns `None` so the hook can retry from fresh progress.
    pub fn capture_claude_group_turns(
        &self,
        before: &ClaudeGroupCapture,
        resumed: bool,
        position: &ClaudeCapturePosition,
        batch: ClaudeGroupTurns,
        redactor: &Redactor,
    ) -> Result<Option<usize>> {
        self.write(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            let fail = |e| db_error("store Claude group progress in", &path, e);
            let session = batch.session().clone();
            if snapshot(&conn, &path, &session)
                .map_err(Fallible::into_error)?
                .as_ref()
                != Some(before)
            {
                return Ok(None);
            }
            let member = &before.member;
            let mut lookup = conn
                .prepare(
                    "SELECT permitted, message FROM group_claude_seen
                 WHERE position = ?1 AND id_hash = ?2",
                )
                .map_err(fail)?;
            let resolved = batch
                .resolve(|id| {
                    if !resumed {
                        return Ok(ClaudeSeen::default());
                    }
                    lookup
                        .query_row((member.position, id), |row| {
                            Ok(ClaudeSeen {
                                permitted: row.get(0)?,
                                message: row.get(1)?,
                            })
                        })
                        .optional()
                        .map(|seen| seen.unwrap_or_default())
                })
                .map_err(fail)?;

            let after = if resumed {
                Some(resolved.turns.as_slice())
            } else {
                after_cursor(member, &resolved.turns)
            };
            let skipped = (!member.holds).then(|| resolved.turns.last()).flatten();
            if after.is_none() && skipped.is_none() {
                // A replacement transcript without the old cursor does not
                // establish continuity. Wait for a replay that contains it.
                return Ok(Some(0));
            }
            let turns = if member.holds {
                turns_since(member, after.unwrap_or_default())
            } else {
                Vec::new()
            };
            let last = skipped.or_else(|| turns.last().copied());
            let inserted = write_captured_turns(&conn, &path, &session, turns, redactor)?;
            if let Some(last) = last {
                conn.execute(
                    "UPDATE group_members SET turn_cursor = ?2 WHERE position = ?1",
                    (member.position, &last.message),
                )
                .map_err(fail)?;
            }
            if !resumed {
                clear_progress(&conn, member.position).map_err(fail)?;
            }
            conn.execute(
                "INSERT INTO group_claude_progress (position, file_id, offset, modified, boundary)
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(position) DO UPDATE SET
                 file_id = excluded.file_id, offset = excluded.offset,
                 modified = excluded.modified, boundary = excluded.boundary",
                (
                    member.position,
                    &position.file_id,
                    position.offset,
                    &position.modified,
                    &position.boundary,
                ),
            )
            .map_err(fail)?;
            let mut save = conn
                .prepare(
                    "INSERT INTO group_claude_seen (position, id_hash, permitted, message)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT(position, id_hash) DO UPDATE SET
                 permitted = excluded.permitted, message = excluded.message",
                )
                .map_err(fail)?;
            for (id, seen) in resolved.seen {
                save.execute((member.position, id, seen.permitted, seen.message))
                    .map_err(fail)?;
            }
            Ok(Some(inserted))
        })
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
