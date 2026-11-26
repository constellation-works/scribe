//! Payloads and rendering for `scribe brief`.

use super::common::console::{Sink, notice};
use super::common::detail::emit_detail;
use super::common::record::Timestamp;
use super::common::table::Result;
use super::memory::MemoryBlockPayload;
use cw_cli::Mode;
use serde::Serialize;
use std::fmt::Write as _;
use std::io::Write;

/// One session in the brief.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefSessionPayload {
    pub(crate) id: String,
    pub(crate) project: String,
    pub(crate) title: String,
    /// Its last recorded activity.
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    /// Whether scribe holds a session record; `false` for a handoff-only session.
    pub(crate) has_record: bool,
    /// The newest active handoff entry id, or `null` when none.
    pub(crate) handoff: Option<String>,
    /// How many active handoff entries this session has.
    pub(crate) handoffs: usize,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
}

/// One open claim: an open proposal, settled by its `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefClaimPayload {
    /// `p-<n>`.
    pub(crate) id: String,
    /// `candidate`, `supersedes` or `stale`.
    pub(crate) kind: &'static str,
    /// The words its line quotes: a candidate's verbatim words, or a
    /// consistency proposal's first quoted span (its reason when it has none).
    pub(crate) quote: String,
    /// A candidate's statement (its quote when it has none), or a
    /// consistency proposal's reason.
    pub(crate) statement: String,
    pub(crate) subject: Option<String>,
    /// The entry it would supersede or retire; `null` for a candidate.
    pub(crate) older: Option<String>,
    pub(crate) newer: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) opened_at: Timestamp,
}

/// The open claims under one tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefClaimGroupPayload {
    pub(crate) tag: String,
    pub(crate) claims: Vec<BriefClaimPayload>,
}

/// `scribe brief`: the last sessions and the open claims
/// (docs/design/scribe/specs/output-contract.md). Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefPayload {
    /// The memory block: `{profile, now, milestones}`.
    pub(crate) memory: MemoryBlockPayload,
    /// Warnings about the memory block (`memory_pending`, a read-only
    /// copy), each also a stderr line.
    pub(crate) warnings: Vec<String>,
    /// The project whose sessions come first; `null` for none.
    pub(crate) project: Option<String>,
    /// The project's most recently opened group, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_group: Option<scribe_core::brief::LastGroup>,
    /// The last sessions: the project's first, each part newest first.
    pub(crate) sessions: Vec<BriefSessionPayload>,
    /// How many open claims `claims` holds.
    pub(crate) claim_count: usize,
    /// Open and answered items, in display order.
    pub(crate) open_items: Vec<scribe_core::brief::OpenItem>,
    /// The open claims, grouped by tag.
    pub(crate) claims: Vec<BriefClaimGroupPayload>,
    /// The brief as text, exactly as it prints and as the hook injects it.
    #[serde(skip)]
    pub(crate) text: String,
}

impl BriefPayload {
    /// The payload of `b`, whose memory block is `memory`.
    pub(crate) fn new(b: &scribe_core::brief::Brief, memory: MemoryBlockPayload) -> Self {
        let warnings = memory.warnings.clone();
        Self {
            memory,
            warnings,
            project: b.project.as_ref().map(ToString::to_string),
            last_group: b.last_group.clone(),
            sessions: b
                .sessions
                .iter()
                .map(|s| BriefSessionPayload {
                    id: s.id.to_string(),
                    project: s.project.to_string(),
                    title: s.title.clone(),
                    at: s.at,
                    has_record: s.has_record,
                    handoff: s.handoff.as_ref().map(ToString::to_string),
                    handoffs: s.handoffs,
                    handoff_mode: s.handoff_mode,
                })
                .collect(),
            claim_count: b.claim_count(),
            open_items: b.open_items.iter().take(10).cloned().collect(),
            claims: b
                .claims
                .iter()
                .map(|g| BriefClaimGroupPayload {
                    tag: g.tag.to_string(),
                    claims: g
                        .claims
                        .iter()
                        .map(|c| BriefClaimPayload {
                            id: c.id.to_string(),
                            kind: c.kind.as_str(),
                            quote: c.quote.clone(),
                            statement: c.statement.clone(),
                            subject: c.subject.clone(),
                            older: c.older.as_ref().map(ToString::to_string),
                            newer: c.newer.as_ref().map(ToString::to_string),
                            opened_at: c.opened_at,
                        })
                        .collect(),
                })
                .collect(),
            text: b.text(),
        }
    }
}

/// Render `scribe brief`, emitting its memory-block warnings to stderr first.
pub(crate) fn show<O: Write, E: Write>(sink: &mut Sink<O, E>, brief: &BriefPayload) -> Result {
    for warning in &brief.warnings {
        notice(sink, warning);
    }
    emit_detail(sink, brief, |out, mode| match mode {
        Mode::Json => Ok(()),
        Mode::Plain | Mode::Table => write!(out, "{}", brief.text),
    })
}
