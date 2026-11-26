//! Read bounded group evidence and atomically apply a supplied summary.

use crate::group::{CloseEvidence, GroupCloser};
use crate::repository::ReadGroupRows;
use crate::store::refusal;
use crate::{AuditEvent, Redactor, Related, Result, Store, WriteRefusal};
use time::OffsetDateTime;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Snapshot {
    /// Canonical group id binding the evidence to its source.
    pub id: String,
    /// Bounded handoffs and typed turns.
    pub evidence: CloseEvidence,
    /// Related references.
    pub related: Vec<Related>,
}

impl Snapshot {
    /// Digest of the group id, exact evidence and related references used by a close.
    pub fn digest(&self) -> String {
        crate::text::bytes_sha256(serde_json::json!(self).to_string().as_bytes())
    }
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

pub(in crate::apps::group) fn evidence(
    store: &Store,
    id: &str,
    closer: GroupCloser<'_>,
) -> Result<Option<Snapshot>> {
    let groups = store.read().groups();
    let group = groups.get(id)?;
    let n = groups.number(&group.id)?;
    groups.read_rows_in_tx("read group evidence from", |rows| {
        Ok(snapshot(rows, n, closer)?)
    })
}

/// Whether `closer` may close group `id` and it is still open: the check a
/// no-quote close takes before its transcript accident guard, so an already
/// closed group still returns unchanged and an unauthorized caller is still
/// refused as one.
pub(in crate::apps::group) fn authorized_open(
    store: &Store,
    id: &str,
    closer: GroupCloser<'_>,
) -> Result<bool> {
    let groups = store.read().groups();
    let n = groups.number(id)?;
    groups.read_rows(|rows| Ok(open_for(rows, n, closer)?))
}

pub(in crate::apps::group) fn apply(
    store: &Store,
    id: &str,
    closer: GroupCloser<'_>,
    now: OffsetDateTime,
    redactor: &Redactor,
    answer: Option<&str>,
    expected_and_audit: (Option<&str>, impl FnOnce(&str) -> AuditEvent),
) -> Result<CloseGroupOutcome> {
    let (expected, audit) = expected_and_audit;
    let groups = store.read().groups();
    let group = groups.get(id)?;
    let n = groups.number(&group.id)?;
    let (changed, cleanup) = store.write_with_cleanup(|tx| {
        tx.groups().rows("close group in", |rows| {
            let Some(current) = snapshot(rows, n, closer)? else {
                return Ok(false);
            };
            if expected.is_some_and(|digest| digest != current.digest()) {
                return Err(refusal(
                    WriteRefusal::GroupEvidenceChanged,
                    "group evidence changed during close; read it and draft the summary again",
                ));
            }
            let mut summary = match answer {
                Some(answer) => {
                    let cleaned = redactor.group_summary(answer.trim());
                    if cleaned.is_empty()
                        || answer.trim().chars().count() > 4000
                        || cleaned.chars().count() > 4000
                    {
                        return Err(refusal(
                            WriteRefusal::InvalidText,
                            "the group summary must be 1–4000 characters before and after redaction",
                        ));
                    }
                    Some(cleaned)
                }
                None => None,
            };
            if !current.related.is_empty() {
                let refs = redactor.group_summary(
                    &current
                        .related
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                let note = format!("For work missing from the handoffs, see {refs}.");
                match &mut summary {
                    Some(text) => {
                        text.push_str("\n\n");
                        text.push_str(&note);
                    }
                    None => summary = Some(note),
                }
            }
            rows.set_closed(n, now, summary.as_deref())?;
            tx.audit().record(&audit(&group.id))?;
            Ok(true)
        })
    })?;
    if changed {
        Ok(CloseGroupOutcome::Closed {
            id: group.id,
            cleanup,
        })
    } else {
        cleanup?;
        Ok(CloseGroupOutcome::AlreadyClosed { id: group.id })
    }
}

/// The opener, or the session that now holds the seat whose chain contains the
/// opener, may close. A seat replacement already took Daniel's word or a
/// verified clear, so the seat chain is consent-checked. An opener that never
/// joined a seat, or left it, has no chain: only that exact session closes.
/// A quoted closer needs no seat, but its consent must have been verified for
/// closing exactly this group: consent for another group or for opening one
/// never authorizes a close.
fn check_closer(
    rows: &ReadGroupRows<'_>,
    n: i64,
    opener: &str,
    closer: GroupCloser<'_>,
) -> Result<()> {
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

fn open_for(rows: &ReadGroupRows<'_>, n: i64, closer: GroupCloser<'_>) -> Result<bool> {
    let header = rows
        .header(n)?
        .ok_or_else(|| refusal(WriteRefusal::GroupNotFound, format!("no group g-{n}")))?;
    check_closer(rows, n, &header.opener, closer)?;
    Ok(header.open)
}

fn snapshot(rows: &ReadGroupRows<'_>, n: i64, closer: GroupCloser<'_>) -> Result<Option<Snapshot>> {
    let header = rows
        .header(n)?
        .ok_or_else(|| refusal(WriteRefusal::GroupNotFound, format!("no group g-{n}")))?;
    check_closer(rows, n, &header.opener, closer)?;
    if !header.open {
        return Ok(None);
    }
    let related = rows.related(&header.related)?;
    let handoffs = rows.member_handoffs(n)?;
    let turns = rows.member_turns(n)?;
    Ok(Some(Snapshot {
        id: format!("g-{n}"),
        evidence: CloseEvidence {
            title: header.title,
            handoffs,
            turns,
        },
        related,
    }))
}
