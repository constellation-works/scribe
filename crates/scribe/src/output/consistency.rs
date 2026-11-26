//! Payloads and rendering for `scribe consistency`.

use super::common::console::{Sink, notice};
use super::common::table::{Cell, Result, left, shown_id, table};
use super::proposal::ProposalPayload;
use cw_cli::{Column, Role};
use scribe_core::ShortIds;
use serde::Serialize;
use std::io::Write;

/// An approved candidate a scan closed because an active entry already
/// covers its statement. The proposal is `rejected`; no entry was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CoveredPayload {
    /// The candidate proposal id.
    pub(crate) proposal: String,
    /// The active entry that covers the statement.
    pub(crate) by: String,
}

/// An approved proposal a scan applied again, because its older entry was
/// still `active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReappliedPayload {
    /// The proposal id.
    pub(crate) proposal: String,
    /// The older entry's id.
    pub(crate) entry: String,
    /// Its status now: `superseded` or `retired`.
    pub(crate) status: &'static str,
}

/// A batch of pairs the consistency model did not label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BatchFailure {
    /// The batch's number in this scan, from 1.
    pub(crate) batch: usize,
    /// The model failure's stable code.
    pub(crate) error_code: &'static str,
}

/// A session whose automatic deactivation failed after the scan found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeactivationFailure {
    /// The session id.
    pub(crate) session: String,
    /// `age` or `removed`.
    pub(crate) route: &'static str,
    /// The stable storage error code.
    pub(crate) error_code: &'static str,
}

/// An item a scan merged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MergedPayload {
    /// The newer item, now `merged`.
    pub(crate) item: String,
    /// The older item that holds its occurrences.
    pub(crate) into: String,
}

/// An item a scan superseded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SupersededPayload {
    /// The older item, now `superseded`.
    pub(crate) item: String,
    /// The newer item.
    pub(crate) by: String,
}

/// A promotion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PromotedPayload {
    /// The `now` item, now `promoted`.
    pub(crate) item: String,
    /// The milestone.
    pub(crate) to: String,
    /// `profile_change`, `recurs_in_sessions` or `user_says`.
    pub(crate) rule: &'static str,
}

/// What a scan's memory steps did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryScanPayload {
    /// Memory pairs the model labelled.
    pub(crate) pairs_judged: usize,
    /// Memory pairs past the cap of 40 or not reached in this call's time.
    pub(crate) pairs_left: usize,
    /// Pairs labelled `unrelated`, never offered again.
    pub(crate) unrelated: usize,
    /// Labels refused: a quote not verbatim in its item's quotes, an
    /// unknown or repeated pair, an unknown label.
    pub(crate) dropped_labels: usize,
    /// Batches the model failed; their pairs are offered again next scan.
    pub(crate) failed_batches: Vec<BatchFailure>,
    /// Items merged into an older one (`same`).
    pub(crate) merged: Vec<MergedPayload>,
    /// Items a newer one superseded (`contradicts`).
    pub(crate) superseded: Vec<SupersededPayload>,
    /// `now` items promoted to `milestones` (rule (b) after the merges).
    pub(crate) promoted: Vec<PromotedPayload>,
    /// Items changed to complete a promotion a crash cut short.
    pub(crate) completed: Vec<String>,
    /// Every item whose file the scan changed, `memory:<id>`: one audit
    /// line each.
    #[serde(skip)]
    pub(crate) targets: Vec<String>,
}

/// A session a scan deactivated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeactivatedPayload {
    /// The session id.
    pub(crate) session: String,
    /// `age`, the one route a scan takes since ORB-27911 step 2a removed
    /// `removed`.
    pub(crate) route: &'static str,
    /// Why: scribe's sentence.
    pub(crate) reason: String,
    /// Always `null`: the later record a `removed` deactivation rested on.
    /// Kept because `--json` field names are a contract.
    pub(crate) cite: Option<String>,
    /// Always `null`, as `cite`.
    pub(crate) quote: Option<String>,
}

/// What `consistency scan` did; `--json` and the `scribe.consistency-scan`
/// tool print it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ScanReport {
    /// Approvals applied again, by proposal id.
    pub(crate) reapplied: Vec<ReappliedPayload>,
    /// Proposals this scan opened, by id.
    pub(crate) opened: Vec<ProposalPayload>,
    /// Open candidate proposals this scan closed automatically (`tag_retired`).
    pub(crate) closed: Vec<ProposalPayload>,
    /// Approved candidates this scan closed because an active entry already
    /// covers the statement. Not stored on the proposal.
    pub(crate) covered: Vec<CoveredPayload>,
    /// Always 0 since ORB-27911 step 2a: no pair is judged. Kept because
    /// `--json` field names are a contract (STD-01@2 §R10).
    pub(crate) judged_pairs: usize,
    /// Always 0, as `judged_pairs`.
    pub(crate) pairs_left: usize,
    /// Always 0, as `judged_pairs`.
    pub(crate) unrelated_pairs: usize,
    /// Always 0, as `judged_pairs`.
    pub(crate) dropped_labels: usize,
    /// Stale entries found (each opens a proposal unless one exists).
    pub(crate) stale_entries: usize,
    /// Always empty, as `judged_pairs`.
    pub(crate) failed_batches: Vec<BatchFailure>,
    /// Sessions this scan deactivated (S10), by route.
    pub(crate) deactivated: Vec<DeactivatedPayload>,
    /// Sessions the scan could not deactivate; other sessions are still
    /// processed.
    pub(crate) failed_deactivations: Vec<DeactivationFailure>,
    /// Always 0, as `judged_pairs`: no removed-feature pair is judged.
    pub(crate) removal_pairs_judged: usize,
    /// Always 0, as `judged_pairs`.
    pub(crate) removal_pairs_left: usize,
    /// Always 0, as `judged_pairs`.
    pub(crate) dropped_removal_labels: usize,
    /// Always empty, as `judged_pairs`.
    pub(crate) failed_removal_batches: Vec<BatchFailure>,
    /// Always empty since the fixed memory shape (2_design.md §15.5): the
    /// scan no longer consolidates memory. Kept because `--json` field names
    /// are a contract.
    pub(crate) memory: MemoryScanPayload,
    /// Lines for stderr, printed in every mode.
    #[serde(skip)]
    pub(crate) notices: Vec<String>,
}

/// A consistency scan's report: a table or tab-separated lines, one per
/// approval re-applied, proposal opened, retired-tag candidate closed
/// (`closed`, the proposal id, `closed_reason` or kind, older, newer),
/// covered candidate, session deactivated or deactivation
/// failed, or the JSON object. A scan that changed nothing prints nothing
/// but the JSON object.
pub(crate) fn emit_scan<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    report: &ScanReport,
) -> Result {
    for text in &report.notices {
        notice(sink, text);
    }
    let mode = sink.terminal().mode();
    sink.emit(report, |report| {
        table(mode, scan_columns(), scan_rows(shorts, report))
    })
}

fn scan_columns() -> Vec<Column> {
    vec![
        left("ACTION"),
        left("ID"),
        left("WHAT"),
        left("OLDER"),
        left("NEWER"),
    ]
}

fn scan_rows(shorts: &ShortIds, report: &ScanReport) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for r in &report.reapplied {
        rows.push(vec![
            Cell::plain("reapplied"),
            Cell::plain(r.proposal.clone()),
            Cell::plain(r.status),
            Cell::plain(r.entry.clone()),
            Cell::plain("-"),
        ]);
    }
    for p in &report.opened {
        rows.push(vec![
            Cell {
                text: String::from("opened"),
                role: Role::Warn,
            },
            Cell::plain(p.id.clone()),
            Cell::plain(p.kind),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for p in &report.closed {
        rows.push(vec![
            Cell::plain("closed"),
            Cell::plain(p.id.clone()),
            Cell::plain(p.closed_reason.unwrap_or(p.kind)),
            Cell::plain(p.older.clone()),
            Cell::plain(p.newer.clone().unwrap_or_else(|| String::from("-"))),
        ]);
    }
    for item in &report.covered {
        rows.push(vec![
            Cell::plain("covered"),
            Cell::plain(item.proposal.clone()),
            Cell::plain(item.by.clone()),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for d in &report.deactivated {
        rows.push(vec![
            Cell {
                text: String::from("deactivated"),
                role: Role::Warn,
            },
            Cell::plain(shown_id(&d.session, Some(shorts))),
            Cell::plain(d.route),
            Cell::plain(d.cite.clone().unwrap_or_else(|| String::from("-"))),
            Cell::plain("-"),
        ]);
    }
    for f in &report.failed_deactivations {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(shown_id(&f.session, Some(shorts))),
            Cell::plain(f.route),
            Cell::plain(f.error_code),
            Cell::plain("-"),
        ]);
    }
    let memory = &report.memory;
    for m in &memory.merged {
        rows.push(vec![
            Cell::plain("merged"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.into.clone()),
            Cell::plain(m.item.clone()),
        ]);
    }
    for m in &memory.superseded {
        rows.push(vec![
            Cell::plain("superseded"),
            Cell::plain(m.item.clone()),
            Cell::plain("memory"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.by.clone()),
        ]);
    }
    for m in &memory.promoted {
        rows.push(vec![
            Cell::plain("promoted"),
            Cell::plain(m.item.clone()),
            Cell::plain(m.rule),
            Cell::plain(m.item.clone()),
            Cell::plain(m.to.clone()),
        ]);
    }
    for id in &memory.completed {
        rows.push(vec![
            Cell::plain("completed"),
            Cell::plain(id.clone()),
            Cell::plain("memory"),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    for f in &memory.failed_batches {
        rows.push(vec![
            Cell {
                text: String::from("failed"),
                role: Role::Warn,
            },
            Cell::plain(format!("memory-batch-{}", f.batch)),
            Cell::plain(f.error_code),
            Cell::plain("-"),
            Cell::plain("-"),
        ]);
    }
    rows
}
