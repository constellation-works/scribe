//! The `consistency` noun: `scan`, the consistency routine's job
//! (docs/design/scribe/2_design.md §7).
//!
//! The `scribe-consistency` routine runs it weekly through the plugin's
//! `scribe.consistency-scan` tool; the user can run it too. One scan:
//!
//! 1. applies again every approved proposal whose older entry is still
//!    `active` (an approval writes two files; a crash between them leaves
//!    the entry behind) — [`scribe_core::ProposalVerbs::reapply_approved`];
//! 2. closes open candidate proposals whose tag is no longer in the
//!    effective tag table, without a model call or a new entry
//!    ([`scribe_core::ProposalVerbs::close_retired_candidates`]);
//! 3. finds the candidates ([`scribe_core::consistency::candidates`]): same-tag active
//!    pairs that share a topic or whose words overlap, newest first and at
//!    most [`scribe_core::consistency::MAX_PAIRS`], skipping any pair with a proposal in
//!    any state; and the active entries past `review_after`;
//! 4. asks the `consistency` model to label the pairs, in batches of
//!    [`scribe_core::consistency::BATCH`], each within 120 s; a label of `contradicts`
//!    or `refines` survives only with a verbatim quote of each entry
//!    ([`scribe_core::consistency::judge`]);
//! 5. opens one `supersedes` proposal per surviving label and one `stale`
//!    proposal per entry due for review, in one locked store call;
//! 6. (S10) deactivates every active, unpinned session whose last message
//!    is older than `[sessions] inactive_after_days` (route `age`);
//! 7. (S10) asks the model about removed-feature pairs
//!    ([`scribe_core::consistency::removal_candidates`]: an active, unpinned session and
//!    a later entry the user wrote with a removal cue, at most
//!    [`scribe_core::consistency::MAX_REMOVAL_PAIRS`]) and deactivates each session a
//!    `removed` label with a verbatim quote names (route `removed`, citing
//!    the later entry). Model-written session text and agent-written
//!    entries are never offered as removal evidence.
//!
//! A scan never changes an entry except in step 1: the user settles every
//! proposal with `#scribe approve` or `#scribe reject` in a typed turn. A
//! failed batch is reported and its pairs are offered again next scan.
//! A scan deactivates sessions directly rather than proposing: an inactive
//! session is hidden, not changed, and `session activate` reverses it.
//! Pinned sessions (`keep_active`) are never deactivated by a scan.
//! Each reapplied approval, closed retired-tag candidate, covered-candidate
//! close, opened proposal, deactivated session and failed batch is one
//! audit line; a scan that changes nothing and fails nothing writes none.
//! When a later step fails after earlier mutations are already on disk, the
//! scan still writes one success line per persisted write, then its failure
//! line.

use super::RunLimits;
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{
    BatchFailure, CoveredPayload, DeactivatedPayload, DeactivationFailure, MemoryScanPayload,
    Output, ProposalPayload, ReappliedPayload, ScanReport,
};
use clap::{Args, Subcommand};
use scribe_core::cases::consistency_scan::{self as case, ScanInput};
use scribe_core::{Config, Models};

/// The command path in the audit log.
pub(in crate::commands) const COMMAND: &str = case::COMMAND;

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
pairs, sessions a later entry the user wrote says were removed or replaced; pinned
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
    /// Propose entry updates and deactivate stale sessions
    ///
    /// Uses the consistency model to propose superseding entries a later one replaced and retiring entries due for review, and deactivates stale or removed-feature sessions.
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
            Self::Scan(_) => Some(AuditMeta {
                command: COMMAND,
                stepwise: true,
            }),
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
    case::max_batches()
}

/// Scan with the `Models` adapter `models` builds for the config (a test
/// answers with a fake), judging batches within `limits`
/// ([`scribe_core::cases::consistency_scan::run`]).
pub(in crate::commands) fn scan<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
    limits: RunLimits,
) -> Result<Output, CliError> {
    let outcome = case::run(
        &ctx.core(),
        ScanInput {
            provider: ctx.provider(),
            models,
            limits,
        },
    )?;
    let failed = |batches: Vec<case::BatchFailed>| -> Vec<BatchFailure> {
        batches
            .into_iter()
            .map(|b| BatchFailure {
                batch: b.batch,
                error_code: b.error_code,
            })
            .collect()
    };
    let report = ScanReport {
        reapplied: outcome
            .reapplied
            .applied
            .iter()
            .map(|r| ReappliedPayload {
                proposal: r.proposal.to_string(),
                entry: r.entry.to_string(),
                status: r.status.as_str(),
            })
            .collect(),
        opened: outcome.opened.iter().map(ProposalPayload::from).collect(),
        closed: outcome.closed.iter().map(ProposalPayload::from).collect(),
        covered: outcome
            .reapplied
            .covered
            .iter()
            .map(|item| CoveredPayload {
                proposal: item.proposal.to_string(),
                by: item.by.to_string(),
            })
            .collect(),
        judged_pairs: outcome.judged_pairs,
        pairs_left: outcome.pairs_left,
        unrelated_pairs: outcome.unrelated_pairs,
        dropped_labels: outcome.dropped_labels,
        stale_entries: outcome.stale_entries,
        failed_batches: failed(outcome.failed_batches),
        deactivated: outcome
            .deactivated
            .into_iter()
            .map(|d| DeactivatedPayload {
                session: d.session,
                route: d.route,
                reason: d.reason,
                cite: d.cite,
                quote: d.quote,
            })
            .collect(),
        failed_deactivations: outcome
            .failed_deactivations
            .into_iter()
            .map(|f| DeactivationFailure {
                session: f.session,
                route: f.route,
                error_code: f.error_code,
            })
            .collect(),
        removal_pairs_judged: outcome.removal_pairs_judged,
        removal_pairs_left: outcome.removal_pairs_left,
        dropped_removal_labels: outcome.dropped_removal_labels,
        failed_removal_batches: failed(outcome.failed_removal_batches),
        memory: MemoryScanPayload::default(),
        notices: outcome.notices,
    };
    Ok(Output::Scan(Box::new(report)))
}
