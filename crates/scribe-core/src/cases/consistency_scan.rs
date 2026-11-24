//! `scribe consistency scan` (ORB-27707 step 3d), the consistency routine's
//! job (docs/design/scribe/2_design.md §7). The CLI's `consistency.rs`
//! documents the steps; this is their order, and the audit lines a scan
//! owes for writes already on disk when a later step fails.

use crate::activity::{self, ActiveRoute, SetActive};
use crate::cases::audit::PersistedAudit;
use crate::cases::proposal::ProposalVerbs;
use crate::cases::session::SessionVerbs;
use crate::cases::{Ctx, PortError, RunLimits};
use crate::consistency::{self, ReapplyReport};
use crate::{
    Config, EntryFilter, Models, Proposal, Provider, Redactor, SessionFilter, SessionId, Store,
};

/// The command path in the audit log.
pub const COMMAND: &str = "consistency scan";

/// The most batches one scan judges: the pair cap, in batches.
#[must_use]
pub fn max_batches() -> usize {
    consistency::MAX_PAIRS.div_ceil(consistency::BATCH)
}

/// The most removed-feature batches one scan judges.
fn max_removal_batches() -> usize {
    consistency::MAX_REMOVAL_PAIRS.div_ceil(consistency::BATCH)
}

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

/// A batch the model failed.
#[derive(Debug)]
pub struct BatchFailed {
    /// The batch, from 1.
    pub batch: usize,
    /// The model error's code.
    pub error_code: &'static str,
}

/// A session the scan deactivated.
#[derive(Debug)]
pub struct Deactivated {
    /// The session.
    pub session: String,
    /// `age` or `removed`.
    pub route: &'static str,
    /// Why.
    pub reason: String,
    /// The later entry cited (route `removed`).
    pub cite: Option<String>,
    /// The verbatim quote (route `removed`).
    pub quote: Option<String>,
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
    /// Candidate pairs the model labelled.
    pub judged_pairs: usize,
    /// Candidate pairs left for a later scan.
    pub pairs_left: usize,
    /// Pairs labelled `unrelated`.
    pub unrelated_pairs: usize,
    /// Labels dropped.
    pub dropped_labels: usize,
    /// Stale entries found.
    pub stale_entries: usize,
    /// Batches the model failed.
    pub failed_batches: Vec<BatchFailed>,
    /// Sessions deactivated.
    pub deactivated: Vec<Deactivated>,
    /// Sessions the scan could not deactivate.
    pub failed_deactivations: Vec<DeactivationFailed>,
    /// Removed-feature pairs the model labelled.
    pub removal_pairs_judged: usize,
    /// Removed-feature pairs left.
    pub removal_pairs_left: usize,
    /// Removed-feature labels refused.
    pub dropped_removal_labels: usize,
    /// Removed-feature batches the model failed.
    pub failed_removal_batches: Vec<BatchFailed>,
    /// Lines for stderr.
    pub notices: Vec<String>,
}

/// What `consistency scan` runs.
pub struct ScanInput<F> {
    /// The provider whose config the scan reads.
    pub provider: Provider,
    /// Builds the `Models` adapter for the config.
    pub models: F,
    /// The batches it may judge, and by when.
    pub limits: RunLimits,
}

/// Scan, judging batches within `limits`.
///
/// # Errors
///
/// [`ScanError`]: the config, the redactor, or a store error. A model
/// failure is a failed batch, not an error.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: ScanInput<impl FnOnce(&Config) -> M>,
) -> Result<ScanOutcome, ScanError<E>> {
    let ScanInput {
        provider,
        models,
        limits,
    } = input;
    let store = ctx.store;
    let now = ctx.clock.now();
    let config = store.read().tags().config()?.for_provider(provider);
    // Fail closed before any write (the reapply below writes entries) when
    // a configured [redact] env name has no value in this environment.
    let redactor = ctx.redaction.redactor(&config).map_err(ScanError::Port)?;
    let adapter = models(&config);
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
    let found = consistency::candidates(&entries, &proposals, now.date(), consistency::MAX_PAIRS);
    let mut report = ScanOutcome {
        reapplied: repaired,
        opened: Vec::new(),
        closed,
        judged_pairs: 0,
        pairs_left: found.over_cap,
        unrelated_pairs: 0,
        dropped_labels: 0,
        stale_entries: found.stale.len(),
        failed_batches: Vec::new(),
        deactivated: Vec::new(),
        failed_deactivations: Vec::new(),
        removal_pairs_judged: 0,
        removal_pairs_left: 0,
        dropped_removal_labels: 0,
        failed_removal_batches: Vec::new(),
        notices: Vec::new(),
    };
    let mut calls = 0usize;
    let mut drafts = Vec::new();
    for (n, batch) in found.pairs.chunks(consistency::BATCH).enumerate() {
        if !limits.may_start(n) {
            report.pairs_left = report.pairs_left.saturating_add(batch.len());
            continue;
        }
        calls = calls.saturating_add(1);
        let request = consistency::request(
            &config.models.consistency,
            batch,
            config.voice(crate::voice::ModelUse::Consistency),
        );
        match adapter.consistency(&request) {
            Ok(answer) => {
                let judged = consistency::judge(answer, batch);
                report.judged_pairs = report.judged_pairs.saturating_add(batch.len());
                report.unrelated_pairs = report.unrelated_pairs.saturating_add(judged.unrelated);
                report.dropped_labels = report.dropped_labels.saturating_add(judged.dropped);
                drafts.extend(judged.drafts);
            }
            Err(err) => {
                tracing::warn!(batch = n.saturating_add(1), %err, "the consistency model failed");
                report.failed_batches.push(BatchFailed {
                    batch: n.saturating_add(1),
                    error_code: err.code(),
                });
            }
        }
    }
    drafts.extend(found.stale.iter().map(consistency::stale));
    if !drafts.is_empty() {
        report.opened = store.open_proposals(drafts, now, &redactor)?;
        persisted.extend(report.opened.iter().map(|proposal| proposal.id.to_string()));
    }
    let session_scan = SessionScan {
        store,
        now,
        config: &config,
        redactor: &redactor,
        adapter: &adapter,
        limits: &limits,
    };
    deactivate_sessions(&session_scan, calls, &mut report, &mut persisted)?;
    // The scan no longer consolidates memory (2_design.md §15.5), so its
    // memory steps change nothing and leave no pair.
    let changed = !(report.reapplied.applied.is_empty()
        && report.opened.is_empty()
        && report.closed.is_empty()
        && report.reapplied.covered.is_empty()
        && report.failed_batches.is_empty()
        && report.deactivated.is_empty()
        && report.failed_deactivations.is_empty()
        && report.failed_removal_batches.is_empty());
    if !changed {
        report
            .notices
            .push(if found.pairs.is_empty() && found.stale.is_empty() {
                String::from("nothing to propose: no candidate pair and no entry due for review")
            } else {
                String::from("nothing to propose: the model found no pair one entry replaced")
            });
    }
    if report.pairs_left > 0 {
        report.notices.push(format!(
            "{} candidate pairs left for the next scan",
            report.pairs_left
        ));
    }
    if report.removal_pairs_left > 0 {
        report.notices.push(format!(
            "{} removed-feature pairs left for the next scan",
            report.removal_pairs_left
        ));
    }
    persisted.disarm();
    Ok(report)
}

/// What the scan's session steps share with the rest of the scan.
struct SessionScan<'a, M> {
    store: &'a Store,
    now: time::OffsetDateTime,
    config: &'a Config,
    redactor: &'a Redactor,
    adapter: &'a M,
    limits: &'a RunLimits,
}

/// The scan's session steps (§7, S10), skipping pinned and inactive
/// sessions: deactivate every session whose last message is older than
/// `[sessions] inactive_after_days` (route `age`), then ask the model about
/// the removed-feature pairs and deactivate each session a later entry
/// the user wrote says was removed or replaced, with a verbatim quote (route `removed`).
/// `calls` is how many model calls the scan has made so far: after the
/// first, a batch starts only within the call's time.
fn deactivate_sessions<M: Models>(
    scan: &SessionScan<'_, M>,
    mut calls: usize,
    report: &mut ScanOutcome,
    persisted: &mut PersistedAudit<'_>,
) -> crate::Result<()> {
    let SessionScan {
        store,
        now,
        config,
        redactor,
        adapter,
        limits,
    } = *scan;
    let filter = SessionFilter {
        include_inactive: true,
        ..SessionFilter::default()
    };
    let mut sessions = store.read().sessions().list(&filter)?.listing.items;
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
        if let Some(s) = sessions.iter_mut().find(|s| &s.id == id) {
            s.active = false;
        }
        if let SetActive::Deactivate { reason, .. } = change {
            report.deactivated.push(Deactivated {
                session: id.to_string(),
                route: ActiveRoute::Age.as_str(),
                reason: reason.clone(),
                cite: None,
                quote: None,
            });
        }
    }

    let entries = store
        .read()
        .entries()
        .list(&EntryFilter::default())?
        .listing
        .items;
    let found =
        consistency::removal_candidates(&sessions, &entries, now, consistency::MAX_REMOVAL_PAIRS);
    report.removal_pairs_left = found.over_cap;
    let mut removals = Vec::new();
    for (n, batch) in found.pairs.chunks(consistency::BATCH).enumerate() {
        let may_start = n < max_removal_batches() && (calls == 0 || limits.may_continue());
        if !may_start {
            report.removal_pairs_left = report.removal_pairs_left.saturating_add(batch.len());
            continue;
        }
        calls = calls.saturating_add(1);
        let request = consistency::removal_request(
            &config.models.consistency,
            batch,
            config.voice(crate::voice::ModelUse::Removal),
        );
        match adapter.removal(&request) {
            Ok(answer) => {
                let judged = consistency::judge_removals(answer, batch);
                report.removal_pairs_judged =
                    report.removal_pairs_judged.saturating_add(batch.len());
                report.dropped_removal_labels =
                    report.dropped_removal_labels.saturating_add(judged.dropped);
                removals.extend(judged.removals);
            }
            Err(err) => {
                tracing::warn!(batch = n.saturating_add(1), %err, "the removed-feature model call failed");
                report.failed_removal_batches.push(BatchFailed {
                    batch: n.saturating_add(1),
                    error_code: err.code(),
                });
            }
        }
    }
    let mut seen_removals = Vec::new();
    let removals: Vec<_> = removals
        .into_iter()
        .filter(|removal| {
            if seen_removals.contains(&removal.session) {
                false
            } else {
                seen_removals.push(removal.session.clone());
                true
            }
        })
        .collect();
    let requests: Vec<(SessionId, SetActive)> = removals
        .iter()
        .map(|r| (r.session.clone(), r.change()))
        .collect();
    // A session may have several later records; keep its newest surviving
    // removal pair so one scan attempts to deactivate it only once.
    let outcomes = store.set_active_many(&requests, now, redactor)?;
    for (removal, outcome) in removals.iter().zip(outcomes) {
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                report.failed_deactivations.push(DeactivationFailed {
                    session: removal.session.to_string(),
                    route: ActiveRoute::Removed.as_str(),
                    error_code: error.code(),
                });
                continue;
            }
        };
        if outcome.changed() {
            persisted.record(format!("session:{}", removal.session));
            report.deactivated.push(Deactivated {
                session: removal.session.to_string(),
                route: ActiveRoute::Removed.as_str(),
                reason: redactor.mask(&removal.reason),
                cite: Some(removal.cite.clone()),
                quote: Some(redactor.mask(&removal.quote)),
            });
        }
    }
    Ok(())
}
