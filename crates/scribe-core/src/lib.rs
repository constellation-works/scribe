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
//!   ([`Config`]) in `config`; and the quote check in [`verbatim`];
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

mod config;
mod entry;
mod error;
mod proposal;
mod record;
mod session;
mod store;
mod tag;
pub mod verbatim;

pub use config::{Config, ConfigError, Models};
pub use entry::{
    Cite, ContentFrom, Entry, EntryError, EntryFilter, EntryId, EntryStatus, Speaker, TagName,
};
pub use error::{Error, Result};
pub use proposal::{Proposal, ProposalError, ProposalId, ProposalKind, ProposalState, Quote};
pub use record::{Label, Listing, Project, RecordError, Related, RelatedKind, Topic, Topics};
pub use session::{
    Cadence, ControlState, SECTIONS, SessionControl, SessionError, SessionFilter, SessionId,
    SessionRecord, SessionStatus, Term,
};
pub use store::{
    AUDIT_FILE, AuditEvent, AuditStatus, CONFIG_FILE, CONTROL_FILE, DEFAULT_LOCK_WAIT, ENTRIES_DIR,
    EntryList, MIN_SECRET_CHARS, PROPOSALS_FILE, REDACTED_FIELDS, Redactor, SESSIONS_DIR,
    SessionList, SessionView, Skipped, Store, TagList,
};
pub use tag::{TagDef, TagError, TagSummary, TagTable};

#[cfg(test)]
mod tests;
