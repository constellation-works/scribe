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
//! 3. opens one `stale` proposal per active entry past `review_after`
//!    with no stale proposal in any state, in one locked store call;
//! 4. (S10) deactivates every active, unpinned session whose last message
//!    is older than `[sessions] inactive_after_days` (route `age`).
//!
//! It calls no model: ORB-27911 step 2a removed the `consistency` model's
//! supersede labels and the removed-feature deactivations.
//!
//! A scan never changes an entry except in step 1: the user settles every
//! proposal with `#scribe approve` or `#scribe reject` in a typed turn.
//! A scan deactivates sessions directly rather than proposing: an inactive
//! session is hidden, not changed, and `session activate` reverses it.
//! Pinned sessions (`keep_active`) are never deactivated by a scan.
//! Each reapplied approval, closed retired-tag candidate, covered-candidate
//! close, opened proposal and deactivated session is one audit line; a scan that changes nothing and fails nothing writes none.
//! A scan runs in one store transaction with its audit lines, so a failure
//! rolls back every change and writes only its failure line.

use crate::error::CliError;
use crate::output::{
    CoveredPayload, DeactivatedPayload, DeactivationFailure, MemoryScanPayload, Output,
    ProposalPayload, ReappliedPayload, ScanReport,
};
use clap::{Args, Subcommand};
use scribe_core::apps::consistency::{self as app, Command, Outcome, ScanInput, exec};
use scribe_core::composition::Context;

/// The command path in the audit log.
pub(in crate::commands) const COMMAND: &str = app::COMMAND;

pub(super) const CONSISTENCY_EXAMPLES: &str = "\
Examples:
  scribe consistency scan
  scribe consistency scan --json | jq '.opened[].id'";

const SCAN_EXAMPLES: &str = "\
Runs from the scribe-consistency routine once a week, with no model call.
Opens a proposal for each entry due for review. It changes an entry only to
finish an approval that was cut short; settle a proposal with #scribe
approve or #scribe reject in a prompt. It also deactivates sessions with no
messages for [sessions] inactive_after_days (120); pinned sessions are left
alone, and scribe session activate reverses it. --json reports what the
scan did.

Examples:
  scribe consistency scan
  scribe consistency scan --json | jq '.opened[] | {id, kind, older, newer}'
  scribe consistency scan --json | jq '.deactivated[] | {session, route, reason}'
  scribe proposal list";

/// Verbs on the `consistency` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ConsistencyCommand {
    /// Propose retiring entries due for review and deactivate stale sessions
    ///
    /// Proposes retiring entries due for review and deactivates sessions past the age limit, with no model call.
    #[command(after_help = SCAN_EXAMPLES)]
    Scan(ScanArgs),
}

/// Arguments to `consistency scan`: none.
#[derive(Debug, Args)]
pub(crate) struct ScanArgs {}

/// Run one `consistency` verb.
pub(super) fn run(command: ConsistencyCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        ConsistencyCommand::Scan(ScanArgs {}) => scan(ctx),
    }
}

/// Scan (the consistency app's `scan`).
pub(in crate::commands) fn scan(ctx: &Context) -> Result<Output, CliError> {
    let input = ScanInput {
        provider: ctx.provider(),
    };
    let Outcome::Scanned(outcome) = exec(ctx, Command::Scan(input))?;
    let outcome = *outcome;
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
        stale_entries: outcome.stale_entries,
        deactivated: outcome
            .deactivated
            .into_iter()
            .map(|d| DeactivatedPayload {
                session: d.session,
                route: d.route,
                reason: d.reason,
                cite: None,
                quote: None,
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
        // No pair is judged since ORB-27911 step 2a; these stay neutral.
        judged_pairs: 0,
        pairs_left: 0,
        unrelated_pairs: 0,
        dropped_labels: 0,
        failed_batches: Vec::new(),
        removal_pairs_judged: 0,
        removal_pairs_left: 0,
        dropped_removal_labels: 0,
        failed_removal_batches: Vec::new(),
        memory: MemoryScanPayload::default(),
        notices: outcome.notices,
    };
    Ok(Output::Scan(Box::new(report)))
}
