//! A group close reads bounded stored evidence, calls the model outside the
//! write transaction, then compares the evidence before publishing its
//! summary.

use crate::checks::refusal;
use crate::group::{CloseEvidence, GroupCloser};
use crate::repo::GroupRows;
use crate::{AuditEvent, Error, Redactor, Related, Result, Store, WriteRefusal};
use time::OffsetDateTime;

#[derive(PartialEq, Eq)]
struct Snapshot {
    evidence: CloseEvidence,
    related: Vec<Related>,
}

pub(super) fn close_group_with_outcome<E: From<Error>>(
    store: &Store,
    id: &str,
    closer: GroupCloser<'_>,
    now: OffsetDateTime,
    redactor: &Redactor,
    summarize: impl FnOnce(&CloseEvidence) -> Result<String, E>,
    audit: impl FnOnce(&str) -> AuditEvent,
) -> Result<CloseGroupOutcome, E> {
    if store.in_write() {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group close must run outside a store write transaction",
        )
        .into());
    }
    let groups = store.read().groups();
    let group = groups.get(id)?;
    let n = groups.number(&group.id)?;
    groups.read_rows(|rows| Ok(check_closer(rows, n, &group.opener, closer)?))?;
    if group.closed.is_some() {
        return Ok(CloseGroupOutcome::AlreadyClosed { id: group.id });
    }
    let _lock = store.session_lock(&format!("group-close-{n}.lock"), "close group")?;
    let Some(before) = groups.read_rows_in_tx("read group for close", |rows| {
        Ok(snapshot(rows, n, closer)?)
    })?
    else {
        return Ok(CloseGroupOutcome::AlreadyClosed { id: group.id });
    };
    let answer = summarize(&before.evidence)?;
    let mut summary = redactor.group_summary(answer.trim());
    if summary.is_empty() || answer.trim().chars().count() > 4000 || summary.chars().count() > 4000
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
    let ((), cleanup) = store.write_with_cleanup(|tx| {
        tx.groups().rows("close group in", |rows| {
            if snapshot(rows, n, closer)?.as_ref() != Some(&before) {
                return Err(refusal(
                    WriteRefusal::GroupMembership,
                    "group evidence changed during close; run group close again",
                ));
            }
            rows.close(n, now, &summary)?;
            tx.audit().record(&audit(&group.id))?;
            Ok(())
        })
    })?;
    Ok(CloseGroupOutcome::Closed {
        id: group.id,
        cleanup,
    })
}

/// The opener, or the session that now holds the seat whose chain contains the
/// opener, may close. A seat replacement already took Daniel's word or a
/// verified clear, so the seat chain is consent-checked. An opener that never
/// joined a seat, or left it, has no chain: only that exact session closes.
/// A quoted closer needs no seat, but its consent must have been verified for
/// closing exactly this group: consent for another group or for opening one
/// never authorizes a close.
fn check_closer(rows: &GroupRows<'_>, n: i64, opener: &str, closer: GroupCloser<'_>) -> Result<()> {
    let caller = match closer {
        GroupCloser::Session(caller) => caller,
        GroupCloser::Quoted(consent) => {
            return if consent.authorizes_close(&format!("g-{n}")) {
                Ok(())
            } else {
                Err(refusal(
                    WriteRefusal::GroupMembership,
                    format!("the quoted consent was not given to close g-{n}"),
                ))
            };
        }
    };
    if opener == caller.as_str() {
        return Ok(());
    }
    if rows.holds_opener_seat(n, caller.as_str(), opener)? {
        Ok(())
    } else {
        Err(refusal(
            WriteRefusal::GroupMembership,
            format!(
                "only the opening session or the current holder of its seat can close g-{n}; \
                 any other session can close it with --quote '<span>', where <span> is a \
                 verbatim span of a user turn typed in that session that contains the whole \
                 canonical group id g-{n} and is at least eight characters long after normalization"
            ),
        ))
    }
}

fn snapshot(rows: &GroupRows<'_>, n: i64, closer: GroupCloser<'_>) -> Result<Option<Snapshot>> {
    let header = rows
        .close_header(n)?
        .ok_or_else(|| refusal(WriteRefusal::GroupNotFound, format!("no group g-{n}")))?;
    check_closer(rows, n, &header.opener, closer)?;
    if !header.open {
        return Ok(None);
    }
    let related = rows.related(&header.related)?;
    let handoffs = rows.close_handoffs(n)?;
    let turns = rows.close_turns(n)?;
    Ok(Some(Snapshot {
        evidence: CloseEvidence {
            title: header.title,
            handoffs,
            turns,
        },
        related,
    }))
}

/// Whether a group close committed and how its post-commit cleanup ended.
#[derive(Debug)]
pub enum CloseGroupOutcome {
    /// The group was already closed when this attempt ran.
    AlreadyClosed {
        /// The resolved group id.
        id: String,
    },
    /// The close and its success audit committed.
    Closed {
        /// The resolved group id.
        id: String,
        /// The result of store cleanup after the commit.
        cleanup: Result<()>,
    },
}
impl CloseGroupOutcome {
    /// Return the group id and whether it changed, propagating any cleanup error.
    pub fn into_result(self) -> Result<(String, bool)> {
        match self {
            Self::AlreadyClosed { id } => Ok((id, false)),
            Self::Closed { id, cleanup } => {
                cleanup?;
                Ok((id, true))
            }
        }
    }
}
