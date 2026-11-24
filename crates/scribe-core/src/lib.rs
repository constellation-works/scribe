//! The scribe store as a library.
//!
//! This crate is the domain, split by responsibility so it can grow:
//!
//! - business logic with no I/O: entries ([`Entry`], [`EntryId`],
//!   [`TagName`], [`Cite`]) in `entry`; the tag table ([`TagTable`]) in
//!   `tag`; session records, the on/off state and the due rule
//!   ([`SessionRecord`], its log of [`Segment`]s, [`ControlState`],
//!   [`Cadence`]) in `session`; whether a session is still current and who
//!   cataloged it in [`activity`];
//!   proposals ([`Proposal`]) in `proposal`; the session brief, the last
//!   sessions and the open claims, in [`brief`]; the shared metadata
//!   ([`Project`], [`Topic`], [`Related`], [`Author`]) in `record`; `config.toml`
//!   ([`Config`]) in `config`; parsing a typed turn into `#scribe`
//!   instructions and an action entry's transcript window in [`capture`];
//!   action entries' cancel window, pickups and stuck count in [`action`];
//!   `/scribe on|off|status` and the switch
//!   transitions in [`control`]; a session pass, from transcript to record,
//!   in [`pass`]; the consistency routine's candidates and checks and the
//!   settling verbs (`approve`, `reject`, `retire`, `forget`) in
//!   [`consistency`]; reading a Claude Code transcript in [`transcript`]; the
//!   quote check in [`verbatim`]; the check of an instruction Daniel typed
//!   for an agent's write (word lists, freshness, interactive entry points)
//!   in [`instruction`]; and every model call as a typed request, in
//!   [`models`], carried by the [`Models`] port in [`providers`];
//! - persistence in `store`, the `scribe-store` crate: the file-backed
//!   [`Store`], its formats, the audit log ([`AuditEvent`]) and the
//!   filesystem mechanisms, and nothing else;
//! - the crate's typed [`Error`].
//!
//! It never parses arguments, never touches the terminal and never decides
//! where data lives: the caller resolves the data directory and hands it to
//! [`Store::open`] (STD-02 R3). Anything another surface (the hooks, the
//! Orbit backend) needs to see the same records belongs here, not in the CLI
//! crate.

// Library code never prints; diagnostics go through `tracing` (STD-02 R15).
#![deny(clippy::print_stderr, clippy::print_stdout)]
// Unit tests use unwrap/expect for fixture setup; production call sites remain linted.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

// Tests moved from the binary name the crate `scribe_core`, as they did there.
#[cfg(test)]
extern crate self as scribe_core;

pub use scribe_domain::action;
pub use scribe_domain::activity;
pub use scribe_domain::annotation;
pub mod apps;
pub mod composition;
pub use provider::Provider;
pub use scribe_domain::brief;
pub use scribe_domain::capture;
pub(crate) use scribe_domain::checks;
pub(crate) use scribe_domain::config;
pub use scribe_domain::consistency;
pub use scribe_domain::control;
pub(crate) use scribe_domain::entry;
pub(crate) use scribe_domain::error;
pub use scribe_domain::group;
pub use scribe_domain::handoff;
pub use scribe_domain::instruction;
pub use scribe_domain::memory;
pub use scribe_domain::message;
/// The model request and answer shapes (`scribe-domain`), with the
/// [`Models`] port and its [`ModelError`](providers::ModelError) from
/// [`providers`] at their old paths.
pub mod models {
    pub use crate::providers::{ModelError, Models};
    pub use scribe_domain::models::*;
}
pub use scribe_domain::origin;
pub use scribe_domain::pass;
pub(crate) use scribe_domain::proposal;
pub(crate) use scribe_domain::provider;
pub(crate) use scribe_domain::record;
pub mod providers;
pub(crate) mod runtime;
pub use runtime::RunLimits;
pub use runtime::audit::{Audit, AuditMeta, AuditSubject};
pub use runtime::errors::CommandError;
pub use scribe_domain::search;
pub(crate) use scribe_domain::session;
#[cfg(not(test))]
pub(crate) use scribe_store as store;
pub use transcript::codex;
// The store tests that call the use cases live here (`tests/store/`), and
// name the store internals they reach by the paths they had inside it; the
// internals come from `scribe_store::testing`.
#[cfg(test)]
mod store {
    pub(crate) use crate::tests::store as tests;
    pub(crate) use scribe_store::testing::{audit, codec, db, memory, write};
    pub use scribe_store::*;
}
pub(crate) use scribe_domain::tag;
pub use scribe_domain::text;
pub use scribe_domain::transcript;
pub use scribe_domain::trigger;
pub use scribe_domain::verbatim;
pub use scribe_domain::voice;

#[cfg(any(test, feature = "test-support"))]
pub use apps::entry::EntryVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::entry::PickupVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::group::GroupVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::mark::MarkVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::memory::MemoryVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::proposal::ProposalVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::session::RebuildVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::session::SessionVerbs;
#[cfg(any(test, feature = "test-support"))]
pub use apps::session::{ClaimsWritten, PassVerbs};
#[cfg(any(test, feature = "test-support"))]
pub use apps::tag::TagVerbs;
pub use checks::entry::check_handoff_related;
pub use checks::records::check_unsettled_item;
pub use checks::tag::check_description;
pub use checks::{check_agent_author, check_content, check_reason};
pub use config::{Config, ConfigError, ModelIds, SessionsConfig};
pub use entry::{
    AgentEntry, AnswerIn, Cite, ContentFrom, Decision, Entry, EntryError, EntryFilter, EntryId,
    EntryPatch, EntryStatus, Instruction, InvalidAnswerIn, InvalidItemState, ItemAnswer,
    ItemClosure, ItemState, NewEntry, OpenItem, Outcome, OutcomeState, Speaker, TagName, Window,
    WindowTurn,
};
pub use error::{Error, Result, WriteRefusal};
pub use handoff::{HANDOFF_TAG, HandoffError, HandoffSessionError};
pub use models::Models;
pub use proposal::{
    Admitted, CONTEXT_CHARS, CONTEXT_SENTENCES, ClosedReason, EntryCandidate, Proposal,
    ProposalChange, ProposalError, ProposalFilter, ProposalId, ProposalKind, ProposalState, Quote,
    SUBJECT_CHARS, close_retired_tag_candidates, context_stands_alone,
};
pub use record::{
    Author, AuthorKind, AuthorName, DateRange, Label, Listing, Project, RecordError, Related,
    RelatedKind, Topic, Topics, parse_day,
};
pub use session::{
    Cadence, Child, ControlState, HEADER_SECTIONS, LOG_SECTION, LinkError, LinkType, LogError,
    SEGMENT_HEADING_CHARS, SHORT_ID_MIN, Segment, SegmentOrigin, SegmentState, SessionControl,
    SessionError, SessionFilter, SessionId, SessionLink, SessionOrder, SessionRecord,
    SessionStatus, ShortIds, Term, check_link, check_log, children, tree_order, tree_parent,
};
pub use store::repo;
pub use store::{
    AGENT_HANDOFF, AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE,
    ClaudeCapturePosition, ClaudeGroupCapture, CloseGroupOutcome, DB_FILE, DEFAULT_LOCK_WAIT,
    ENTRIES_DIR, EntryList, ExportFormat, Exported, InitFile, InitReport, IssueAudit, LOCKS_DIR,
    MARKS_FILE, MEMORY_DIR, MILESTONES_DIR, MIN_SECRET_CHARS, MODEL_FAILED, MarkChange, MarkKey,
    MarkResult, MemoryShown, MemoryView, MemoryWritten, Moved, NOW_DIR, PROFILE_FILE,
    PROPOSALS_FILE, PassMemory, PassWrite, PassWritten, ProfileInputs, ProfileRevision,
    REDACTED_FIELDS, RebuildGuard, Redactor, RefreshWritten, SESSIONS_DIR, SessionForget,
    SessionList, SessionView, Skipped, Snapshot, Store, TagList, Tx, init_project,
    init_project_with_skill_files, integration_hooks, resolve_prefix, unresolved_redact_env,
};
pub use tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, FINDING_TAG,
    MAX_FINDINGS_PER_SESSION, Rule, RuleOrigin, StoreTag, TagDef, TagError, TagPatch, TagSummary,
    TagTable,
};

#[cfg(any(test, feature = "test-support"))]
pub use scribe_store::testing;
#[cfg(test)]
mod tests;
