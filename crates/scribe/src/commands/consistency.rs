//! The `consistency` noun: `scan`, the consistency routine's job
//! (docs/design/scribe/2_design.md §7).
//!
//! The `scribe-consistency` routine runs it weekly through the plugin's
//! `scribe.consistency-scan` tool; Daniel can run it too. One scan:
//!
//! 1. applies again every approved proposal whose older entry is still
//!    `active` (an approval writes two files; a crash between them leaves
//!    the entry behind) — [`scribe_core::Store::reapply_approved`];
//! 2. finds the candidates ([`consistency::candidates`]): same-tag active
//!    pairs that share a topic or whose words overlap, newest first and at
//!    most [`consistency::MAX_PAIRS`], skipping any pair with a proposal in
//!    any state; and the active entries past `review_after`;
//! 3. asks the `consistency` model to label the pairs, in batches of
//!    [`consistency::BATCH`], each within 120 s; a label of `contradicts`
//!    or `refines` survives only with a verbatim quote of each entry
//!    ([`consistency::judge`]);
//! 4. opens one `supersedes` proposal per surviving label and one `stale`
//!    proposal per entry due for review, in one locked store call;
//! 5. (S10) deactivates every active, unpinned session whose last message
//!    is older than `[sessions] inactive_after_days` (route `age`);
//! 6. (S10) asks the model about removed-feature pairs
//!    ([`consistency::removal_candidates`]: an active, unpinned session and
//!    a later entry Daniel wrote with a removal cue, at most
//!    [`consistency::MAX_REMOVAL_PAIRS`]) and deactivates each session a
//!    `removed` label with a verbatim quote names (route `removed`, citing
//!    the later entry). Model-written session text and agent-written
//!    entries are never offered as removal evidence.
//!
//! A scan never changes an entry except in step 1: Daniel settles every
//! proposal with `#scribe approve` or `#scribe reject` in a typed turn. A
//! failed batch is reported and its pairs are offered again next scan.
//! A scan deactivates sessions directly rather than proposing: an inactive
//! session is hidden, not changed, and `session activate` reverses it.
//! Pinned sessions (`keep_active`) are never deactivated by a scan.
//! Each reapplied approval, opened proposal, deactivated session and
//! failed batch is one audit line; a scan that changes nothing and fails
//! nothing writes none.

use super::RunLimits;
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{
    BatchFailure, DeactivatedPayload, DeactivationFailure, Output, ProposalPayload,
    ReappliedPayload, ScanReport,
};
use clap::{Args, Subcommand};
use scribe_core::activity::{self, ActiveRoute, SetActive};
use scribe_core::{
    Config, EntryFilter, Models, Redactor, SessionFilter, SessionId, Store, consistency,
};

/// The command path in the audit log.
pub(in crate::commands) const COMMAND: &str = "consistency scan";

pub(super) const CONSISTENCY_EXAMPLES: &str = "\
Examples:
  scribe consistency scan
  scribe consistency scan --json | jq '.opened[].id'";

const SCAN_EXAMPLES: &str = "\
Runs from the scribe-consistency routine once a week. Labels at most 40
candidate pairs per run, ten per call to the configured consistency model,
and opens a proposal for each pair one entry replaced and each entry due
for review. It changes an entry only to finish an approval that was cut
short; settle a proposal with #scribe approve or #scribe reject in a
prompt. It also deactivates sessions with no messages for [sessions]
inactive_after_days (120) and, after asking the model about at most 20
pairs, sessions a later entry Daniel wrote says were removed or replaced; pinned
sessions are left alone, and scribe session activate reverses either.
--json reports what the scan did.

Examples:
  scribe consistency scan
  scribe consistency scan --json | jq '.opened[] | {id, kind, older, newer}'
  scribe consistency scan --json | jq '.deactivated[] | {session, route, cite}'
  scribe proposal list";

/// Verbs on the `consistency` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ConsistencyCommand {
    /// Propose superseding entries a later one replaced and retiring entries due for review, and deactivate stale or removed-feature sessions, with the consistency model
    #[command(after_help = SCAN_EXAMPLES)]
    Scan(ScanArgs),
}

/// Arguments to `consistency scan`: none.
#[derive(Debug, Args)]
pub(crate) struct ScanArgs {}

impl ConsistencyCommand {
    /// The audit entry for this verb: `scan` writes proposals, entries and
    /// session records.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Scan(_) => Some(AuditMeta { command: COMMAND }),
        }
    }
}

/// Run one `consistency` verb.
pub(super) fn run(command: ConsistencyCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ConsistencyCommand::Scan(ScanArgs {}) => scan(
            ctx,
            |config| ctx.models(config),
            RunLimits::new(max_batches()),
        ),
    }
}

/// The most batches one scan judges: the pair cap, in batches.
pub(in crate::commands) fn max_batches() -> usize {
    consistency::MAX_PAIRS.div_ceil(consistency::BATCH)
}

/// The most removed-feature batches one scan judges.
fn max_removal_batches() -> usize {
    consistency::MAX_REMOVAL_PAIRS.div_ceil(consistency::BATCH)
}

/// Scan with the `Models` adapter `models` builds for the config (a test
/// answers with a fake), judging batches within `limits`.
pub(in crate::commands) fn scan<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
    limits: RunLimits,
) -> Result<Output, CliError> {
    let store = ctx.store();
    let now = ctx.now();
    let config = ctx.config()?;
    // Fail closed before any write (the reapply below writes entries) when
    // a configured [redact] env name has no value in this environment.
    let redactor = ctx.redactor(&config)?;
    let adapter = models(&config);
    let reapplied = store.reapply_approved(now)?;
    let entries = store.list_entries(&EntryFilter::default())?.listing.items;
    let proposals = store.proposals()?;
    let found = consistency::candidates(&entries, &proposals, now.date(), consistency::MAX_PAIRS);
    let mut report = ScanReport {
        reapplied: reapplied
            .iter()
            .map(|r| ReappliedPayload {
                proposal: r.proposal.to_string(),
                entry: r.entry.to_string(),
                status: r.status.as_str(),
            })
            .collect(),
        opened: Vec::new(),
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
        let request = consistency::request(&config.models.consistency, batch);
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
                report.failed_batches.push(BatchFailure {
                    batch: n.saturating_add(1),
                    error_code: err.code(),
                });
            }
        }
    }
    drafts.extend(found.stale.iter().map(consistency::stale));
    if !drafts.is_empty() {
        report.opened = store
            .open_proposals(drafts, now, &redactor)?
            .iter()
            .map(ProposalPayload::from)
            .collect();
    }
    let session_scan = SessionScan {
        store,
        now,
        config: &config,
        redactor: &redactor,
        adapter: &adapter,
        limits: &limits,
    };
    deactivate_sessions(&session_scan, calls, &mut report)?;
    let changed = !(report.reapplied.is_empty()
        && report.opened.is_empty()
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
    Ok(Output::Scan(Box::new(report)))
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
/// Daniel wrote says was removed or replaced, with a verbatim quote (route `removed`).
/// `calls` is how many model calls the scan has made so far: after the
/// first, a batch starts only within the call's time.
fn deactivate_sessions<M: Models>(
    scan: &SessionScan<'_, M>,
    mut calls: usize,
    report: &mut ScanReport,
) -> Result<(), CliError> {
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
    let mut sessions = store.list_sessions(&filter)?.listing.items;
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
                report.failed_deactivations.push(DeactivationFailure {
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
        if let Some(s) = sessions.iter_mut().find(|s| &s.id == id) {
            s.active = false;
        }
        if let SetActive::Deactivate { reason, .. } = change {
            report.deactivated.push(DeactivatedPayload {
                session: id.to_string(),
                route: ActiveRoute::Age.as_str(),
                reason: reason.clone(),
                cite: None,
                quote: None,
            });
        }
    }

    let entries = store.list_entries(&EntryFilter::default())?.listing.items;
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
        let request = consistency::removal_request(&config.models.consistency, batch);
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
                report.failed_removal_batches.push(BatchFailure {
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
                report.failed_deactivations.push(DeactivationFailure {
                    session: removal.session.to_string(),
                    route: ActiveRoute::Removed.as_str(),
                    error_code: error.code(),
                });
                continue;
            }
        };
        if outcome.changed() {
            report.deactivated.push(DeactivatedPayload {
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
