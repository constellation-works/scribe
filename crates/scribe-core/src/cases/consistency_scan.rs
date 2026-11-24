//! `scribe consistency scan` (ORB-27707 step 3d), the consistency routine's
//! job (docs/design/scribe/2_design.md §7). The CLI's `consistency.rs`
//! documents the steps; this is their order, and the audit lines a scan
//! owes for writes already on disk when a later step fails. It calls no
//! model (ORB-27911 step 2a).

use crate::activity::{self, ActiveRoute, SetActive};
use crate::cases::proposal::ProposalVerbs;
use crate::cases::session::SessionVerbs;
use crate::consistency::{self, ReapplyReport};
use crate::runtime::audit::PersistedAudit;
use crate::runtime::{Ctx, PortError};
use crate::{Config, EntryFilter, Proposal, Provider, Redactor, SessionFilter, SessionId, Store};

/// The command path in the audit log.
pub const COMMAND: &str = "consistency scan";

/// Why a scan stopped.
#[derive(Debug)]
pub enum ScanError<E> {
    /// A store error.
    Core(crate::Error),
    /// A port failed.
    Port(E),
}

impl<E> From<crate::Error> for ScanError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// A session the scan deactivated.
#[derive(Debug)]
pub struct Deactivated {
    /// The session.
    pub session: String,
    /// `age` (`removed` was the removed-feature route, before ORB-27911).
    pub route: &'static str,
    /// Why.
    pub reason: String,
}

/// A session whose automatic deactivation failed.
#[derive(Debug)]
pub struct DeactivationFailed {
    /// The session.
    pub session: String,
    /// `age` or `removed`.
    pub route: &'static str,
    /// The stable storage error code.
    pub error_code: &'static str,
}

/// What a scan did.
#[derive(Debug)]
pub struct ScanOutcome {
    /// Approvals applied again, and approved candidates closed as covered.
    pub reapplied: ReapplyReport,
    /// Proposals this scan opened.
    pub opened: Vec<Proposal>,
    /// Open candidates this scan closed (`tag_retired`).
    pub closed: Vec<Proposal>,
    /// Stale entries found.
    pub stale_entries: usize,
    /// Sessions deactivated.
    pub deactivated: Vec<Deactivated>,
    /// Sessions the scan could not deactivate.
    pub failed_deactivations: Vec<DeactivationFailed>,
    /// Lines for stderr.
    pub notices: Vec<String>,
}

/// What `consistency scan` runs.
pub struct ScanInput {
    /// The provider whose config the scan reads.
    pub provider: Provider,
}

/// Scan.
///
/// # Errors
///
/// [`ScanError`]: the config, the redactor, or a store error.
pub fn run<E: PortError>(ctx: &Ctx<'_, E>, input: ScanInput) -> Result<ScanOutcome, ScanError<E>> {
    let ScanInput { provider } = input;
    let store = ctx.store;
    let now = ctx.clock.now();
    let config = store.read().tags().config()?.for_provider(provider);
    // Fail closed before any write (the reapply below writes entries) when
    // a configured [redact] env name has no value in this environment.
    let redactor = ctx.redaction.redactor(&config).map_err(ScanError::Port)?;
    // Armed until the success return. Every durable write in the scan records
    // its audit target here right after persisting, so a later `?` drops this
    // and audits what is already on disk before the middleware writes the
    // scan's failure line. Success disarms it so the middleware writes those
    // lines once with the command's real duration.
    let mut persisted = PersistedAudit::new(store, COMMAND, now);
    let repaired = store.reapply_approved(now)?;
    persisted.extend(repaired.applied.iter().map(|r| r.entry.to_string()));
    persisted.extend(repaired.covered.iter().map(|c| c.proposal.to_string()));
    let closed = store.close_retired_candidates(now)?;
    persisted.extend(closed.iter().map(|proposal| proposal.id.to_string()));
    let entries = store
        .read()
        .entries()
        .list(&EntryFilter::default())?
        .listing
        .items;
    let proposals = store.read().proposals().all()?;
    let stale = consistency::stale_candidates(&entries, &proposals, now.date());
    let mut report = ScanOutcome {
        reapplied: repaired,
        opened: Vec::new(),
        closed,
        stale_entries: stale.len(),
        deactivated: Vec::new(),
        failed_deactivations: Vec::new(),
        notices: Vec::new(),
    };
    let drafts: Vec<_> = stale.iter().map(consistency::stale).collect();
    if !drafts.is_empty() {
        report.opened = store.open_proposals(drafts, now, &redactor)?;
        persisted.extend(report.opened.iter().map(|proposal| proposal.id.to_string()));
    }
    deactivate_aged(store, now, &config, &redactor, &mut report, &mut persisted)?;
    let changed = !(report.reapplied.applied.is_empty()
        && report.opened.is_empty()
        && report.closed.is_empty()
        && report.reapplied.covered.is_empty()
        && report.deactivated.is_empty()
        && report.failed_deactivations.is_empty());
    if !changed {
        report
            .notices
            .push(String::from("nothing to propose: no entry due for review"));
    }
    persisted.disarm();
    Ok(report)
}

/// The scan's session step (§7, S10), skipping pinned and inactive
/// sessions: deactivate every session whose last message is older than
/// `[sessions] inactive_after_days` (route `age`).
fn deactivate_aged(
    store: &Store,
    now: time::OffsetDateTime,
    config: &Config,
    redactor: &Redactor,
    report: &mut ScanOutcome,
    persisted: &mut PersistedAudit<'_>,
) -> crate::Result<()> {
    let filter = SessionFilter {
        include_inactive: true,
        ..SessionFilter::default()
    };
    let sessions = store.read().sessions().list(&filter)?.listing.items;
    let limit = config.sessions.inactive_after_days;
    let aged: Vec<(SessionId, SetActive)> = sessions
        .iter()
        .filter(|s| activity::past_age(s, limit, now))
        .map(|s| (s.id.clone(), activity::age_change(s, limit)))
        .collect();
    let outcomes = store.set_active_many(&aged, now, redactor)?;
    for ((id, change), outcome) in aged.iter().zip(outcomes) {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                report.failed_deactivations.push(DeactivationFailed {
                    session: id.to_string(),
                    route: ActiveRoute::Age.as_str(),
                    error_code: error.code(),
                });
                continue;
            }
        };
        if !outcome.changed() {
            continue;
        }
        persisted.record(format!("session:{id}"));
        if let SetActive::Deactivate { reason, .. } = change {
            report.deactivated.push(Deactivated {
                session: id.to_string(),
                route: ActiveRoute::Age.as_str(),
                reason: reason.clone(),
            });
        }
    }
    Ok(())
}
