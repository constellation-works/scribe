//! A session forget's erasure from the entries and claims: its handoffs
//! deleted, its provenance detached from the entries it cites, and the
//! claims it raised withdrawn. The session app's forget runs it inside its
//! write, at the step [`crate::repository::Sessions::erase`] gives it.

use crate::{Entry, EntryId, Proposal, Result, SessionId, Tx};
use time::OffsetDateTime;

/// Erase session handoffs and withdraw claims in the same write.
///
/// # Errors
///
/// The first entry row that did not read, and the store's errors.
pub fn erase_session_documents(tx: &Tx<'_>, id: &SessionId, now: OffsetDateTime) -> Result<bool> {
    let (entries, skipped) = tx.entries().all()?;
    if let Some(bad) = skipped.into_iter().next() {
        return Err(bad.error);
    }
    let number = tx.sessions().numbers()?.get(id).map(|n| format!("s-{n}"));
    let matches = |value: &str| value == id.as_str() || number.as_deref() == Some(value);
    let mut changed = false;
    let mut removed = Vec::new();
    for before in entries {
        let mut entry = before.clone();
        if handoff_names(&entry, matches) {
            tx.entries().delete_handoff(&entry.id)?;
            removed.push(entry.id);
        } else {
            detach_entry(&mut entry, matches, now);
            if entry != before {
                tx.entries().put(&entry, false)?;
                if entry.author.kind == crate::AuthorKind::Scribe {
                    removed.push(entry.id);
                }
                changed = true;
            }
        }
    }
    let before = tx.proposals().all()?;
    let mut after = before.clone();
    detach_claims(&mut after, matches, &removed, now);
    if before != after {
        tx.proposals().put_all(&after)?;
        changed = true;
    }
    Ok(changed || !removed.is_empty())
}

fn handoff_names(entry: &Entry, matches: impl Fn(&str) -> bool) -> bool {
    entry.tag.as_str() == crate::handoff::HANDOFF_TAG
        && entry
            .related
            .iter()
            .any(|r| r.kind() == crate::RelatedKind::Session && matches(r.target()))
}

/// Preserve independently recorded entries and mark the removed provenance.
fn detach_entry(entry: &mut Entry, matches: impl Fn(&str) -> bool, now: OffsetDateTime) {
    if entry
        .cite
        .as_ref()
        .is_some_and(|c| !c.is_deleted() && matches(c.session.as_str()))
    {
        if entry.author.kind == crate::AuthorKind::Scribe {
            let id = entry.id.clone();
            crate::consistency::forget(&id, &mut [], std::slice::from_mut(entry), now);
        }
        if let Some(cite) = &mut entry.cite {
            cite.mark_deleted();
        }
        if let crate::ContentFrom::Message { message, at, .. } = &mut entry.content_from {
            *message = String::from("deleted");
            *at = OffsetDateTime::UNIX_EPOCH;
        }
        entry.window = None;
    }
    let named = |value: &str| value.strip_prefix("session:").is_some_and(&matches);
    if entry.evidence.as_deref().is_some_and(named) {
        entry.evidence = Some(String::from("session:deleted"));
    }
    let source_reference =
        |r: &crate::Related| r.kind() == crate::RelatedKind::Session && matches(r.target());
    if entry.related.iter().any(source_reference) {
        entry.related.retain(|r| !source_reference(r));
        if let Some(decision) = &mut entry.decision {
            entry.quote = None;
            decision.reason_quote = None;
        }
        let marker = crate::Related::deleted_session();
        if !entry.related.contains(&marker) {
            entry.related.push(marker);
        }
    }
    if let Some(item) = &mut entry.item
        && item
            .answer
            .as_ref()
            .is_some_and(|a| matches(a.session.as_str()))
    {
        // The answer is the user's verbatim words from the session; erase
        // them with the id. An unsettled item is open again.
        item.answer = None;
    }
    for instruction in [&mut entry.instruction, &mut entry.removal_instruction]
        .into_iter()
        .flatten()
    {
        if matches(instruction.session.as_str()) {
            instruction.quote = None;
        }
    }
}

fn detach_claims(
    claims: &mut [Proposal],
    matches: impl Fn(&str) -> bool,
    removed: &[EntryId],
    now: OffsetDateTime,
) {
    for claim in claims {
        let source = claim
            .candidate
            .as_ref()
            .is_some_and(|c| !c.cite.is_deleted() && matches(c.cite.session.as_str()));
        let about = removed.contains(&claim.older)
            || claim.newer.as_ref().is_some_and(|id| removed.contains(id));
        if source || about {
            claim.reason.clear();
            for quote in &mut claim.quotes {
                quote.text.clear();
            }
            if let Some(candidate) = &mut claim.candidate {
                candidate.cite.mark_deleted();
                candidate.quote.clear();
                candidate.context = None;
                candidate.statement = None;
                candidate.subject = None;
                candidate.evidence = None;
                candidate.admitted = None;
                candidate.project = crate::Project::sanitized("deleted");
                candidate.provider = crate::Label::sanitized("deleted", "deleted");
                candidate.host = crate::Label::sanitized("deleted", "deleted");
            }
            if claim.state == crate::ProposalState::Open {
                claim.state = crate::ProposalState::Rejected;
                claim.closed_reason = Some(crate::ClosedReason::Withdrawn);
                claim.settled_at = Some(now);
                claim.settled_author = Some(crate::Author::consistency());
            }
        }
        let settlement_deleted = claim
            .settled_by
            .as_ref()
            .is_some_and(|c| !c.is_deleted() && matches(c.session.as_str()));
        if let Some(instruction) = &mut claim.instruction {
            if settlement_deleted || source {
                instruction.quote = None;
            }
        } else if settlement_deleted && let Some(cite) = &mut claim.settled_by {
            cite.mark_deleted();
        }
    }
}
