//! A close reads bounded stored evidence, calls the model outside the write
//! transaction, then compares the evidence before publishing its summary.

use super::{DB_FILE, Fallible, db_error, open_writer, refusal, stamp};
use crate::group::CloseEvidence;
use crate::store::fsio::LockGuard;
use crate::{
    AuditEvent, CloseGroupOutcome, Error, Redactor, Related, Result, SessionId, Store, WriteRefusal,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use time::OffsetDateTime;

#[derive(PartialEq, Eq)]
struct Snapshot {
    evidence: CloseEvidence,
    related: Vec<Related>,
}

impl Store {
    /// Summarize and close a group from its opening session. Return false on
    /// retry after a successful close, without calling the model again.
    ///
    /// The model closure runs outside any SQLite write transaction. The final
    /// write commits `closed`, `summary`, and the caller-built success audit
    /// row in one `scribe.db` transaction.
    pub fn close_group<E: From<Error>>(
        &self,
        id: &str,
        caller: &SessionId,
        now: OffsetDateTime,
        redactor: &Redactor,
        summarize: impl FnOnce(&CloseEvidence) -> std::result::Result<String, E>,
        audit: impl FnOnce(&str) -> AuditEvent,
    ) -> std::result::Result<(String, bool), E> {
        self.close_group_with_outcome(id, caller, now, redactor, summarize, audit)?
            .into_result()
            .map_err(E::from)
    }

    /// Close a group and report a committed success audit even if cleanup
    /// failed. An outer error means this attempt did not commit a close.
    pub fn close_group_with_outcome<E: From<Error>>(
        &self,
        id: &str,
        caller: &SessionId,
        now: OffsetDateTime,
        redactor: &Redactor,
        summarize: impl FnOnce(&CloseEvidence) -> Result<String, E>,
        audit: impl FnOnce(&str) -> AuditEvent,
    ) -> Result<CloseGroupOutcome, E> {
        if self.in_write() {
            return Err(refusal(
                WriteRefusal::GroupMembership,
                "group close must run outside a store write transaction",
            )
            .into());
        }
        let group = self.group(id)?;
        check_opener(&group.opener, caller)?;
        if group.closed.is_some() {
            return Ok(CloseGroupOutcome::AlreadyClosed { id: group.id });
        }
        let n = self.group_number(&group.id)?;
        let lock_path = self.lock_path(&format!("group-close-{n}.lock"))?;
        let _lock = LockGuard::acquire(&lock_path, "close group", self.lock_wait)?;
        let Some(before) = self.read_groups(|conn, path| {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| Fallible::from_sqlite("read group for close", path, e))?;
            snapshot(&tx, path, n, caller).map_err(Fallible::Failed)
        })?
        else {
            return Ok(CloseGroupOutcome::AlreadyClosed { id: group.id });
        };
        let answer = summarize(&before.evidence)?;
        let mut summary = redactor.group_summary(answer.trim());
        if summary.is_empty()
            || answer.trim().chars().count() > 4000
            || summary.chars().count() > 4000
        {
            return Err(refusal(
                WriteRefusal::InvalidText,
                "the group summary must be 1–4000 characters before and after redaction",
            )
            .into());
        }
        if !before.related.is_empty() {
            summary.push_str("\n\nFor work missing from the handoffs, see ");
            summary.push_str(
                &redactor.group_summary(
                    &before
                        .related
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            );
            summary.push('.');
        }
        let ((), cleanup) = self.write_with_cleanup(|| {
            let conn = open_writer(self.root(), self.lock_wait)?;
            let path = self.root().join(DB_FILE);
            if snapshot(&conn, &path, n, caller)?.as_ref() != Some(&before) {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    "group evidence changed during close; run group close again",
                ));
            }
            conn.execute(
                "UPDATE groups SET closed = ?2, summary = ?3 WHERE id = ?1",
                params![n, stamp(now), summary],
            )
            .map_err(|e| db_error("close group in", &path, e))?;
            self.record_audit(&audit(&group.id))?;
            Ok(())
        })?;
        Ok(CloseGroupOutcome::Closed {
            id: group.id,
            cleanup,
        })
    }
}

fn check_opener(opener: &str, caller: &SessionId) -> Result<()> {
    if opener != caller.as_str() {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "only the opening session can close this group",
        ));
    }
    Ok(())
}

fn snapshot(
    conn: &Connection,
    path: &Path,
    n: i64,
    caller: &SessionId,
) -> Result<Option<Snapshot>> {
    let fail = |e| db_error("read evidence for group close from", path, e);
    let (opener, open, title, related): (String, bool, String, String) = conn.query_row(
        "SELECT opener, closed IS NULL, substr(title, 1, 500), related FROM groups WHERE id = ?1",
        [n],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).optional().map_err(fail)?.ok_or_else(|| {
        refusal(WriteRefusal::GroupNotFound, format!("no group g-{n}"))
    })?;
    check_opener(&opener, caller)?;
    if !open {
        return Ok(None);
    }
    let related = serde_json::from_str(&related).map_err(|e| Error::corrupt(path, e))?;
    let members = "SELECT gm.session FROM group_members gm WHERE gm.group_id = ?1
                   AND NOT EXISTS (SELECT 1 FROM sessions s
                       WHERE s.provider_session = gm.session AND s.forgotten IS NOT NULL)";
    let mut handoffs = conn
        .prepare(&format!(
            "SELECT e.id, substr(e.content, 1, 1200) FROM entries e
         WHERE e.tag = 'handoff' AND e.status = 'active' AND EXISTS (
             SELECT 1 FROM json_each(e.related)
             WHERE substr(value, 1, 8) = 'session:' AND substr(value, 9) IN ({members})
         )
         ORDER BY julianday(e.created) DESC, e.n DESC LIMIT 16",
        ))
        .map_err(fail)?;
    let handoffs = handoffs
        .query_map([n], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(fail)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(fail)?;
    let mut turns = conn
        .prepare(&format!(
            "SELECT session, substr(message, 1, 128), substr(text, 1, 500) FROM messages
         WHERE role = 'user' AND session IN ({members})
         ORDER BY julianday(at) DESC, id DESC LIMIT 32",
        ))
        .map_err(fail)?;
    let turns = turns
        .query_map([n], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(fail)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(fail)?;
    Ok(Some(Snapshot {
        evidence: CloseEvidence {
            title,
            handoffs,
            turns,
        },
        related,
    }))
}
