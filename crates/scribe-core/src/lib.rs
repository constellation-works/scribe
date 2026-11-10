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
//!   proposals ([`Proposal`]) in `proposal`; the shared metadata
//!   ([`Project`], [`Topic`], [`Related`], [`Author`]) in `record`; `config.toml`
//!   ([`Config`]) in `config`; parsing a typed turn into `#scribe`
//!   instructions in [`capture`]; `/scribe on|off|status` and the switch
//!   transitions in [`control`]; a session pass, from transcript to record,
//!   in [`pass`]; the consistency routine's candidates and checks and the
//!   settling verbs (`approve`, `reject`, `retire`, `forget`) in
//!   [`consistency`]; reading a Claude Code transcript in [`transcript`]; the
//!   quote check in [`verbatim`]; and the [`Models`] port, every model call
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

pub mod activity;
pub mod ask;
pub mod capture;
mod config;
pub mod consistency;
pub mod control;
mod entry;
mod error;
pub mod models;
pub mod pass;
mod proposal;
mod record;
pub mod search;
mod session;
mod store;
mod tag;
pub mod transcript;
pub mod verbatim;

pub use config::{Config, ConfigError, ModelIds, SessionsConfig};
pub use entry::{
    AgentEntry, Cite, ContentFrom, Entry, EntryError, EntryFilter, EntryId, EntryPatch,
    EntryStatus, NewEntry, Speaker, TagName,
};
pub use error::{Error, Result, WriteRefusal};
pub use models::Models;
pub use proposal::{
    Proposal, ProposalChange, ProposalError, ProposalFilter, ProposalId, ProposalKind,
    ProposalState, Quote,
};
pub use record::{
    Author, AuthorKind, AuthorName, DateRange, Label, Listing, Project, RecordError, Related,
    RelatedKind, Topic, Topics, parse_day,
};
pub use session::{
    Cadence, Child, ControlState, HEADER_SECTIONS, LOG_SECTION, LinkError, LinkType, LogError,
    SEGMENT_HEADING_CHARS, Segment, SegmentOrigin, SegmentState, SessionControl, SessionError,
    SessionFilter, SessionId, SessionLink, SessionRecord, SessionStatus, Term, check_link,
    check_log, children, tree_order, tree_parent,
};
pub use store::{
    AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE, Captured, DEFAULT_LOCK_WAIT,
    ENTRIES_DIR, EntryList, MIN_SECRET_CHARS, PROPOSALS_FILE, REDACTED_FIELDS, RebuildGuard,
    Redactor, SESSIONS_DIR, SessionList, SessionView, Skipped, Store, TagList,
    unresolved_redact_env,
};
pub use tag::{StoreTag, TagDef, TagError, TagPatch, TagSummary, TagTable};

#[cfg(test)]
mod tests;
