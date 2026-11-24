//! `scribe entry show`. Read-only: no lock, no audit, no write.

use super::verbs::EntryVerbs;

/// What `entry show` found.
pub struct Shown {
    /// The entry.
    pub entry: crate::Entry,
    /// Its action state.
    pub actions: super::written::Actions,
    /// For an entry approved from a pass claim, what admitted the claim.
    pub admitted: Option<crate::Admitted>,
}

/// `entry show`: the entry, its action state, and, for an entry approved
/// from a pass claim, what admitted the claim: the approved candidate
/// proposal that reserved its id.
///
/// # Errors
///
/// An unknown or ambiguous id, or a store error.
pub fn show<E>(ctx: &crate::runtime::Ctx<'_, E>, id: &str) -> crate::Result<Shown> {
    let entry = ctx.store.resolve_entry(id)?;
    let actions = super::written::Actions::load(ctx)?;
    let admitted = ctx
        .store
        .read()
        .proposals()
        .all()?
        .into_iter()
        .find(|p| {
            p.kind == crate::ProposalKind::Candidate
                && p.state == crate::ProposalState::Approved
                && p.older == entry.id
        })
        .and_then(|p| p.candidate.and_then(|c| c.admitted));
    Ok(Shown {
        entry,
        actions,
        admitted,
    })
}
