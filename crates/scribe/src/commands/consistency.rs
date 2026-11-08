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
//!    proposal per entry due for review, in one locked store call.
//!
//! A scan never changes an entry except in step 1: Daniel settles every
//! proposal with `#scribe approve` or `#scribe reject` in a typed turn. A
//! failed batch is reported and its pairs are offered again next scan.
//! Each reapplied approval, opened proposal and failed batch is one audit
//! line; a scan that changes nothing and fails nothing writes none.

use super::RunLimits;
use crate::app::Context;
use crate::audit_middleware::AuditMeta;
use crate::error::CliError;
use crate::output::{BatchFailure, Output, ProposalPayload, ReappliedPayload, ScanReport};
use clap::{Args, Subcommand};
use scribe_core::{Config, EntryFilter, Models, consistency};

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
prompt. --json reports what the scan did.

Examples:
  scribe consistency scan
  scribe consistency scan --json | jq '.opened[] | {id, kind, older, newer}'
  scribe proposal list";

/// Verbs on the `consistency` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum ConsistencyCommand {
    /// Propose superseding entries a later one replaced, and retiring entries due for review, with the consistency model
    #[command(after_help = SCAN_EXAMPLES)]
    Scan(ScanArgs),
}

/// Arguments to `consistency scan`: none.
#[derive(Debug, Args)]
pub(crate) struct ScanArgs {}

impl ConsistencyCommand {
    /// The audit entry for this verb: `scan` writes proposals and entries.
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

/// Scan with the `Models` adapter `models` builds for the config (a test
/// answers with a fake), judging batches within `limits`.
pub(in crate::commands) fn scan<M: Models>(
    ctx: &Context,
    models: impl FnOnce(&Config) -> M,
    limits: RunLimits,
) -> Result<Output, CliError> {
    let store = ctx.store();
    let now = ctx.now();
    // Fail closed before any write (the reapply below writes entries) when
    // a configured [redact] env name has no value in this environment.
    let _checked = ctx.redactor(&store.config()?)?;
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
        notices: Vec::new(),
    };
    let mut drafts = Vec::new();
    if !found.pairs.is_empty() {
        let config = store.config()?;
        let adapter = models(&config);
        for (n, batch) in found.pairs.chunks(consistency::BATCH).enumerate() {
            if !limits.may_start(n) {
                report.pairs_left = report.pairs_left.saturating_add(batch.len());
                continue;
            }
            let request = consistency::request(&config.models.consistency, batch);
            match adapter.consistency(&request) {
                Ok(answer) => {
                    let judged = consistency::judge(answer, batch);
                    report.judged_pairs = report.judged_pairs.saturating_add(batch.len());
                    report.unrelated_pairs =
                        report.unrelated_pairs.saturating_add(judged.unrelated);
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
    }
    drafts.extend(found.stale.iter().map(consistency::stale));
    if !drafts.is_empty() {
        let config = store.config()?;
        let redactor = ctx.redactor(&config)?;
        report.opened = store
            .open_proposals(drafts, now, &redactor)?
            .iter()
            .map(ProposalPayload::from)
            .collect();
    }
    if report.reapplied.is_empty() && report.opened.is_empty() && report.failed_batches.is_empty() {
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
    Ok(Output::Scan(Box::new(report)))
}
