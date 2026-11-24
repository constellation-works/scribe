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

/// Action entries at the path they had in the domain: the pickup shape
/// (the store's contracts), the cancel window, state, stuck count and
/// quote check (`apps/entry`), and the per-tag counts (`apps/tag`).
pub mod action {
    pub use crate::apps::entry::rules::{
        ActionState, CANCEL_WINDOW, STUCK_AFTER, quote_found, state, stuck,
    };
    pub use crate::apps::tag::rules::tag_counts;
    pub use crate::store::Pickup;
}
pub use apps::session::rules::activity;
/// `annotation` at its old path: the marker scanning (the domain's
/// `capture`, now `scribe_tools::markers`), the mark shapes (the store's contracts) and the refusal of a
/// marker (`apps/mark`).
pub mod annotation {
    pub use crate::apps::mark::rules::{CheckRefusal, record_tag_refusal};
    pub use scribe_store::{AGENT_HANDOFF, MAX_ATTEMPTS, MODEL_FAILED, MarkRecord, MarkState};
    pub use scribe_tools::markers::{MAX_MARKS, Mark, MarkProblem, marks, turn_marks};
}
pub mod apps;
pub mod composition;
pub use apps::brief::rules as brief;
pub use provider::Provider;
/// `capture` at its old path: the marker scanning and the settling-line
/// parser (`scribe_tools::markers`), a typed turn and its prompt hashes
/// (`apps/hook`), and the origin guard and hashes it re-exported.
pub mod capture {
    pub use crate::apps::hook::rules::{Turn, prompt_hashes};
    pub use scribe_domain::origin::{
        OriginClaims, PromptClaims, agent_originated, present_value, prompt_agent_originated,
    };
    pub use scribe_domain::text::{bytes_sha256, prompt_sha256};
    pub use scribe_tools::markers::*;
}
pub(crate) use scribe_domain::claim;
pub(crate) use scribe_domain::config;
/// The consistency rules and the settling verbs, at the path they had in
/// the domain: the stale step (`apps/consistency`), settling and approval
/// (`apps/proposal`) and `forget` with the outcome every verb reports
/// (`apps/entry`).
pub mod consistency {
    pub use crate::apps::consistency::rules::*;
    pub use crate::apps::entry::rules::{SettleRefusal, Settled, forget};
    pub use crate::apps::proposal::rules::*;
}
/// `control` at its old path: the `/scribe` line's parser (the domain's
/// `control`, until the domain's `transcript` stops reading it) and the
/// switch transitions (`apps/session`).
pub mod control {
    pub use crate::apps::session::rules::control::*;
    pub use scribe_domain::control::*;
}
pub(crate) use scribe_domain::entry;
pub(crate) use scribe_domain::error;
/// `group` at its old path: the group shapes (the store's contracts), the
/// group app's requests, outcomes and consent (`apps/group`), and the
/// membership of an entry (`apps/entry`).
pub mod group {
    pub use crate::apps::entry::rules::entry_belongs;
    pub use crate::apps::group::rules::{
        Added, GroupAdded, GroupAddition, GroupCloser, GroupConsent, JoinGroup, OpenedGroup,
        SeatStart, automatic_seat,
    };
    pub use crate::models::CloseEvidence;
    pub use scribe_store::{Group, NewGroup, Seat, SeatHold};
}
pub use scribe_domain::handoff;
/// `instruction` at its old path: the instruction check (`apps/entry`) and
/// what a member's Claude capture has seen (the store's contracts).
pub mod instruction {
    pub use crate::apps::entry::rules::instruction::*;
    pub use scribe_store::ClaudeSeen;
}
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
/// `pass` at its old path: the session pass (`apps/session`), its claim
/// checks (`apps/proposal`, re-exported by the pass) and its window
/// (`scribe_tools::window`).
pub mod pass {
    pub use crate::apps::session::rules::pass::*;
    pub use scribe_tools::window::*;
}
pub(crate) use scribe_domain::proposal;
pub(crate) use scribe_domain::provider;
pub(crate) use scribe_domain::record;
pub mod providers;
pub(crate) mod runtime;
pub use apps::search::rules as search;
pub use runtime::RunLimits;
pub use runtime::audit::{Audit, AuditMeta, AuditSubject};
pub use runtime::errors::CommandError;
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
    // Use-case types that left the store (ORB-27921 step 4), at the paths
    // the store tests name them by.
    pub(crate) use crate::apps::mark::{MarkChange, MarkKey};
    pub(crate) use crate::apps::session::PassWrite;
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
pub use apps::entry::rules::{
    check_content, check_handoff_related, check_reason, check_unsettled_item,
};
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
pub use apps::session::{SessionList, SessionView};
pub use apps::tag::TagList;
#[cfg(any(test, feature = "test-support"))]
pub use apps::tag::TagVerbs;
pub use apps::tag::rules::{check_agent_author, check_description};
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
pub use store::repository;
pub use store::{
    AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE, ClaudeCapturePosition,
    ClaudeGroupCapture, DB_FILE, DEFAULT_LOCK_WAIT, ENTRIES_DIR, EntryList, ExportCounts,
    ExportFormat, ImportFacts, Imported, IssueAudit, LOCKS_DIR, MARKS_FILE, MEMORY_DIR,
    MILESTONES_DIR, MIN_SECRET_CHARS, MemoryShown, MemoryStep, MemoryView, MemoryWritten, NOW_DIR,
    PROFILE_FILE, PROPOSALS_FILE, ProfileRevision, REDACTED_FIELDS, Redactor, SESSIONS_DIR,
    SessionForget, SessionLock, SessionRow, Skipped, Snapshot, Store, Tx, resolve_prefix,
    unresolved_redact_env,
};
pub use tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, FINDING_TAG,
    MAX_FINDINGS_PER_SESSION, Rule, RuleOrigin, StoreTag, TagDef, TagError, TagPatch, TagSummary,
    TagTable,
};

/// Test support: the store's ([`scribe_store::testing`]), and a shim for
/// the reads whose view an app derives (ORB-27921 step 5), so the tests'
/// call syntax (`store.session(id)`, `store.list_tags()`, …) stays as it
/// was. It is not API. Compiled for tests and the `test-support` feature
/// only.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    pub use scribe_store::testing::*;

    use crate::{Result, SessionFilter, SessionId, SessionList, SessionView, Store, TagList};

    /// The reads the tests call by their store names whose view an app now
    /// derives. Each is one call to that app's read.
    pub trait ViewTestReads {
        /// [`crate::apps::session::session_view`].
        fn session(&self, id: &SessionId) -> Result<SessionView>;
        /// [`crate::apps::session::list_sessions`], unscoped.
        fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList>;
        /// [`crate::apps::tag::list_tags`].
        fn list_tags(&self) -> Result<TagList>;
    }

    impl ViewTestReads for Store {
        fn session(&self, id: &SessionId) -> Result<SessionView> {
            crate::apps::session::session_view(&self.read(), id)
        }
        fn list_sessions(&self, filter: &SessionFilter) -> Result<SessionList> {
            crate::apps::session::list_sessions(&self.read(), filter, None)
        }
        fn list_tags(&self) -> Result<TagList> {
            crate::apps::tag::list_tags(&self.read())
        }
    }
}
#[cfg(test)]
mod tests;
