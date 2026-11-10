//! The scribe store as a library.
//!
//! This crate is the domain, split by responsibility so it can grow:
//!
//! - business logic with no I/O: entries ([`Entry`], [`EntryId`],
//!   [`TagName`], [`Cite`]) in `entry`; the tag table ([`TagTable`]) in
//!   `tag`; session records, the on/off state and the due rule
//!   ([`SessionRecord`], [`ControlState`], [`Cadence`]) in `session`;
//!   proposals ([`Proposal`]) in `proposal`; the shared metadata
//!   ([`Project`], [`Topic`], [`Related`]) in `record`; `config.toml`
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
mod session;
mod store;
mod tag;
pub mod transcript;
pub mod verbatim;

pub use config::{Config, ConfigError, ModelIds};
pub use entry::{
    Cite, ContentFrom, Entry, EntryError, EntryFilter, EntryId, EntryStatus, NewEntry, Speaker,
    TagName,
};
pub use error::{Error, Result};
pub use models::Models;
pub use proposal::{Proposal, ProposalError, ProposalId, ProposalKind, ProposalState, Quote};
pub use record::{Label, Listing, Project, RecordError, Related, RelatedKind, Topic, Topics};
pub use session::{
    Cadence, Child, ControlState, LinkError, LinkType, SECTIONS, SessionControl, SessionError,
    SessionFilter, SessionId, SessionLink, SessionRecord, SessionStatus, Term, check_link,
    children, tree_order, tree_parent,
};
pub use store::{
    AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE, Captured, DEFAULT_LOCK_WAIT,
    ENTRIES_DIR, EntryList, MIN_SECRET_CHARS, PROPOSALS_FILE, REDACTED_FIELDS, Redactor,
    SESSIONS_DIR, SessionList, SessionView, Skipped, Store, TagList, unresolved_redact_env,
};
pub use tag::{TagDef, TagError, TagSummary, TagTable};

#[cfg(test)]
mod tests;
