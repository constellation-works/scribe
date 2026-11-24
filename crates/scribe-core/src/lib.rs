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
//!   in [`instruction`]; and the [`Models`] port, every model call
//!   as a typed request, in [`models`];
//! - persistence in `store`: the file-backed [`Store`], its formats, the
//!   audit log ([`AuditEvent`]), the [`Redactor`] and the filesystem
//!   mechanisms, and nothing else;
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

pub mod action;
pub mod activity;
pub mod annotation;
pub mod ask;
pub mod brief;
pub mod capture;
mod cases;
mod checks;
mod claim;
pub use transcript::codex;
mod config;
pub mod consistency;
pub mod control;
mod entry;
mod error;
pub mod group;
pub mod handoff;
pub mod instruction;
pub mod memory;
pub mod message;
pub mod models;
pub mod origin;
pub mod pass;
mod proposal;
mod provider;
pub use provider::Provider;
mod record;
pub mod search;
mod session;
mod store;
mod tag;
pub mod text;
pub mod transcript;
pub mod trigger;
pub mod verbatim;
pub mod voice;

pub use cases::entry::EntryVerbs;
pub use cases::group::GroupVerbs;
pub use cases::memory::MemoryVerbs;
pub use cases::pass::{ClaimsWritten, PassVerbs};
pub use cases::proposal::ProposalVerbs;
pub use cases::rebuild::RebuildVerbs;
pub use cases::session::{HandoffSelection, SessionVerbs};
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
    AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE, COVERED, ClaudeCapturePosition,
    ClaudeGroupCapture, CloseGroupOutcome, DB_FILE, DEFAULT_LOCK_WAIT, ENTRIES_DIR, EntryList,
    ExportFormat, Exported, InitFile, InitReport, IssueAudit, LOCKS_DIR, MARKS_FILE, MEMORY_DIR,
    MILESTONES_DIR, MIN_SECRET_CHARS, MODEL_FAILED, MarkChange, MarkKey, MarkResult, MemoryShown,
    MemoryView, MemoryWritten, Moved, NEEDS_REDACTION, NOW_DIR, PROFILE_FILE, PROPOSALS_FILE,
    PassMemory, PassWrite, PassWritten, ProfileInputs, ProfileRevision, REDACTED_FIELDS,
    RebuildGuard, Redactor, RefreshWritten, SESSIONS_DIR, SessionForget, SessionList, SessionView,
    Skipped, Snapshot, Store, TagList, Tx, check_agent_author, check_content, check_description,
    check_handoff_related, check_reason, check_unsettled_item, init_project,
    init_project_with_skill_files, integration_hooks, resolve_prefix, unresolved_redact_env,
};
pub use tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, FINDING_TAG,
    MAX_FINDINGS_PER_SESSION, Rule, RuleOrigin, StoreTag, TagDef, TagError, TagPatch, TagSummary,
    TagTable,
};

#[cfg(test)]
mod tests;
