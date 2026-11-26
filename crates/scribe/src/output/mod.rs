//! The output layer: the only code that writes to stdout or stderr.
//!
//! Each top-level command owns its payloads and renderers in its own module
//! directly under `output/` (STD-02 R48); shared console, sink, envelope,
//! table, detail, color, printable and usage helpers live under `output/common/`.
//! This file wires those modules and re-exports the output surface.
//!
//! The process environment is captured by composition and passed in; this
//! module reads none of it (STD-02 R3, R7).

mod brief;
mod common;
mod consistency;
mod entry;
mod export;
mod group;
mod hook;
mod init;
mod mark;
mod memory;
mod plugin;
mod proposal;
mod search;
mod session;
mod store;
mod tag;
mod transcript;

use crate::error::CliError;
use common::console::{RenderOutput, SinkStyle, emit_list_diagnostics, notice};
use scribe_core::ShortIds;
use session::ActiveColumn;
use std::io::Write;

pub(crate) use brief::BriefPayload;
pub(crate) use common::console::{
    Console, Sink, finish, finish_parse_error, help_width, init_logging,
};
pub(crate) use common::record::WarningPayload;
pub(crate) use consistency::{
    CoveredPayload, DeactivatedPayload, DeactivationFailure, MemoryScanPayload, ReappliedPayload,
    ScanReport,
};
pub(crate) use entry::EntryPayload;
pub(crate) use export::ExportPayload;
pub(crate) use group::{GroupBackfill, GroupBackfillMember};
pub(crate) use hook::HookReply;
pub(crate) use memory::{
    MemoryBlockPayload, MemoryPayload, MemoryRulesPayload, RefreshPayload, RefreshRetryPayload,
    RevisionPayload,
};
pub(crate) use plugin::{PluginError, PluginReply};
pub(crate) use proposal::ProposalPayload;
pub(crate) use search::HitPayload;
#[cfg(test)]
pub(crate) use session::PassPayload;
pub(crate) use session::{
    Kin, PassFailure, PassReport, SessionHandoffsPayload, SessionPayload, TreeRow,
};
pub(crate) use store::MigratePayload;
pub(crate) use tag::{TagPayload, TagShowPayload};
pub(crate) use transcript::TranscriptPayload;

/// The records a list returned, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Records {
    Groups(Vec<scribe_core::group::Group>),
    Entries(Vec<EntryPayload>),
    /// `session list`; `active_column` with `--include-inactive`, which adds
    /// an `active` column to the human forms.
    Sessions {
        items: Vec<SessionPayload>,
        active_column: bool,
    },
    /// `session list --tree`: sessions in tree order, each with its depth.
    SessionTree {
        rows: Vec<TreeRow>,
        active_column: bool,
    },
    Proposals(Vec<ProposalPayload>),
    Tags(Vec<TagPayload>),
    /// `scribe search`, best first.
    Hits(Vec<HitPayload>),
    /// `transcript list`.
    Transcripts(Vec<TranscriptPayload>),
    /// `mark list` and `mark retry`: `#scribe` markers as `marks.json`
    /// stores them.
    Marks(Vec<scribe_core::annotation::MarkRecord>),
}

impl Records {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Groups(v) => v.len(),
            Self::Entries(v) => v.len(),
            Self::Sessions { items, .. } => items.len(),
            Self::SessionTree { rows, .. } => rows.len(),
            Self::Proposals(v) => v.len(),
            Self::Tags(v) => v.len(),
            Self::Hits(v) => v.len(),
            Self::Transcripts(v) => v.len(),
            Self::Marks(v) => v.len(),
        }
    }
}

/// One record a `show` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Group(Box<scribe_core::group::Group>),
    Tag(Box<scribe_core::StoreTag>),
    TagRules(Box<TagShowPayload>),
    Proposal(Box<ProposalPayload>),
    Entry(Box<EntryPayload>),
    Session(Box<SessionPayload>),
    /// `session show <id>` for a session with handoff(s) and no session record.
    SessionHandoffs(Box<SessionHandoffsPayload>),
    Transcript(Box<TranscriptPayload>),
    /// One memory file.
    Memory(Box<MemoryPayload>),
    /// `memory show` with no id: the memory block.
    MemoryBlock(Box<MemoryBlockPayload>),
    /// `memory show --revision`.
    MemoryRevision(Box<RevisionPayload>),
    /// `memory refresh` or `memory apply` (written).
    MemoryRefresh(Box<RefreshPayload>),
    /// `memory prepare`.
    MemoryPrepare(Box<scribe_core::apps::memory::PreparedRefresh>),
    /// `memory apply` returning a conditional retry refusal.
    MemoryRefreshRetry(Box<RefreshRetryPayload>),
    /// `store migrate`: the archives imported into `scribe.db`.
    StoreMigrate(Box<MigratePayload>),
    /// `export`: what was written where.
    Export(Box<ExportPayload>),
    /// `memory rules`.
    MemoryRules(Box<MemoryRulesPayload>),
    /// `brief`.
    Brief(Box<BriefPayload>),
}

/// What a command produced, before rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// Provider integration scaffold result.
    Init(scribe_core::apps::init::InitReport),
    /// A list result.
    List {
        /// The records returned, in order.
        records: Records,
        /// How many matched before the limit; more than the records when the
        /// list was cut.
        total: usize,
        /// Files left out because they did not parse; each is also a stderr
        /// line.
        warnings: Vec<WarningPayload>,
        /// Lines for stderr, printed in every mode (STD-01 R12): what was
        /// searched when nothing matched (R16), or that the list was cut
        /// (R34).
        notices: Vec<String>,
        /// The stored messages a default `search` found when no entry or
        /// session matched (`message_matches` in `--json`); `None` otherwise,
        /// and then the key is absent.
        message_matches: Option<usize>,
    },
    /// A single record (`show`).
    Show {
        /// The record.
        record: Record,
    },
    /// The record a direct write verb wrote, in the `show` shape (`entry`,
    /// `tag` and `proposal` writes; `session activate`, `session deactivate`).
    Written {
        /// The record as it now stands.
        record: Record,
        /// Whether the write changed it; an unchanged write is not audited.
        changed: bool,
        /// Lines for stderr, printed in every mode: `nothing changed`, or
        /// that a deactivation unpinned the session.
        notices: Vec<String>,
    },
    /// Bounded evidence for a group close.
    GroupEvidence(serde_json::Value),
    /// Group backfill counts.
    GroupBackfill(GroupBackfill),
    /// A hook's reply.
    Hook(HookReply),
    /// A session pass's report.
    Pass(PassReport),
    /// A session pass prepare or apply result.
    PassStep(serde_json::Value),
    /// A consistency scan's report.
    Scan(Box<ScanReport>),
    /// A plugin tool call's envelope.
    Plugin(PluginReply),
}

impl Output {
    /// A direct write's outcome. An unchanged write says `nothing changed`
    /// on stderr and is not audited.
    pub(crate) fn written(record: Record, changed: bool) -> Self {
        let notices = if changed {
            Vec::new()
        } else {
            vec![String::from("nothing changed")]
        };
        Self::Written {
            record,
            changed,
            notices,
        }
    }
}

impl RenderOutput for Output {
    fn sink_style(&self) -> SinkStyle {
        match self {
            Self::Hook(_) | Self::Plugin(_) => SinkStyle::MachineCompactJson,
            Self::GroupEvidence(_) | Self::PassStep(_) => SinkStyle::MachinePrettyJson,
            Self::GroupBackfill(_) => SinkStyle::Compact,
            Self::Init(_)
            | Self::List { .. }
            | Self::Show { .. }
            | Self::Written { .. }
            | Self::Pass(_)
            | Self::Scan(_) => SinkStyle::Pretty,
        }
    }

    fn emit<O: Write, E: Write>(
        &self,
        sink: &mut Sink<O, E>,
        shorts: &ShortIds,
    ) -> cw_cli::Result<()> {
        match self {
            Self::Init(report) => init::emit(sink, report),
            Self::List {
                records,
                total,
                warnings,
                notices,
                message_matches,
            } => {
                emit_list_diagnostics(sink, warnings, notices);
                list(sink, shorts, records, *total, warnings, *message_matches)
            }
            Self::Show { record } => show(sink, shorts, record),
            Self::Written {
                record, notices, ..
            } => {
                for text in notices {
                    notice(sink, text);
                }
                show(sink, shorts, record)
            }
            Self::GroupEvidence(value) => group::emit_evidence(sink, value),
            Self::GroupBackfill(report) => group::emit_backfill(sink, shorts, report),
            Self::Hook(reply) => hook::emit(sink, reply),
            Self::Pass(report) => session::emit_pass(sink, shorts, report),
            Self::PassStep(value) => session::emit_pass_step(sink, value),
            Self::Scan(report) => consistency::emit_scan(sink, shorts, report),
            Self::Plugin(reply) => plugin::emit(sink, reply),
        }
    }

    fn after_emit(&self) -> Result<(), CliError> {
        match self {
            Self::Pass(report) => report.failure().map_or(Ok(()), Err),
            Self::Init(_)
            | Self::List { .. }
            | Self::Show { .. }
            | Self::Written { .. }
            | Self::PassStep(_)
            | Self::GroupEvidence(_)
            | Self::GroupBackfill(_)
            | Self::Hook(_)
            | Self::Scan(_)
            | Self::Plugin(_) => Ok(()),
        }
    }
}

fn list<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    records: &Records,
    total: usize,
    warnings: &[WarningPayload],
    message_matches: Option<usize>,
) -> cw_cli::Result<()> {
    match records {
        Records::Groups(items) => group::list(sink, shorts, items, total, warnings),
        Records::Entries(items) => entry::list(sink, items, total, warnings),
        Records::Sessions {
            items,
            active_column,
        } => session::list(
            sink,
            shorts,
            items,
            ActiveColumn::from_flag(*active_column),
            total,
            warnings,
        ),
        Records::SessionTree {
            rows,
            active_column,
        } => session::list_tree(
            sink,
            shorts,
            rows,
            ActiveColumn::from_flag(*active_column),
            total,
            warnings,
        ),
        Records::Proposals(items) => proposal::list(sink, shorts, items, total, warnings),
        Records::Tags(items) => tag::list(sink, items, total, warnings),
        Records::Hits(items) => search::list(sink, shorts, items, total, warnings, message_matches),
        Records::Transcripts(items) => transcript::list(sink, shorts, items, total, warnings),
        Records::Marks(items) => mark::list(sink, shorts, items, total, warnings),
    }
}

fn show<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    record: &Record,
) -> cw_cli::Result<()> {
    match record {
        Record::Group(group) => group::show(sink, shorts, group),
        Record::Tag(tag) => tag::show(sink, tag),
        Record::TagRules(tag) => tag::show_rules(sink, tag),
        Record::Proposal(proposal) => proposal::show(sink, shorts, proposal),
        Record::Entry(entry) => entry::show(sink, shorts, entry),
        Record::Session(session) => session::show(sink, shorts, session),
        Record::SessionHandoffs(handoffs) => session::show_handoffs(sink, shorts, handoffs),
        Record::Transcript(transcript) => transcript::show(sink, shorts, transcript),
        Record::Memory(item) => memory::show(sink, item),
        Record::MemoryBlock(block) => memory::show_block(sink, block),
        Record::MemoryRevision(revision) => memory::show_revision(sink, revision),
        Record::MemoryRefresh(refresh) => memory::show_refresh(sink, refresh),
        Record::MemoryPrepare(prepared) => memory::show_prepare(sink, prepared),
        Record::MemoryRefreshRetry(retry) => memory::show_refresh_retry(sink, retry),
        Record::StoreMigrate(report) => store::show(sink, report),
        Record::Export(report) => export::show(sink, report),
        Record::MemoryRules(rules) => memory::show_rules(sink, rules),
        Record::Brief(brief) => brief::show(sink, brief),
    }
}

#[cfg(test)]
mod tests;
