//! scribe's domain: the records and the pure rules over them, with no I/O.
//!
//! Entries ([`Entry`], [`EntryId`], [`TagName`], [`Cite`]) are in `entry`;
//! the tag table ([`TagTable`]) in `tag`; session records, the on/off state
//! and the due rule ([`SessionRecord`], its log of [`Segment`]s,
//! [`ControlState`], [`Cadence`]) in `session`; proposals ([`Proposal`]) in
//! `proposal`; the shared metadata ([`Project`], [`Topic`], [`Related`],
//! [`Author`]) in `record`; `config.toml` ([`Config`]) in `config`; the
//! model request and answer shapes in [`models`]; the rules
//! that select and judge records (capture, claims, the pass, instructions)
//! in their modules; and the typed [`Error`].
//!
//! The modules sit in four rings, each importing only its own and the ones
//! before it: base, records, transcript, rules (ARCHITECTURE.md;
//! `scripts/check-dependency-direction.sh`). It never touches the file
//! system, the network, a process or the environment: `scribe-store` holds
//! the records and `scribe-core` runs the use cases over both.

#![deny(clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod annotation;
pub mod capture;
pub mod claim;
pub mod config;
pub mod control;
pub mod entry;
pub mod error;
pub mod group;
pub mod handoff;
pub mod instruction;
pub mod memory;
pub mod message;
pub mod models;
pub mod origin;
pub mod proposal;
pub mod provider;
pub mod record;
pub mod session;
pub mod tag;
pub mod text;
pub mod transcript;
pub mod trigger;
pub mod verbatim;
pub mod voice;
pub use provider::Provider;
pub use transcript::codex;

pub use config::{Config, ConfigError, ModelIds, SessionsConfig};
pub use entry::{
    AgentEntry, AnswerIn, Cite, ContentFrom, Decision, Entry, EntryError, EntryFilter, EntryId,
    EntryPatch, EntryStatus, Instruction, InvalidAnswerIn, InvalidItemState, ItemAnswer,
    ItemClosure, ItemState, NewEntry, OpenItem, Outcome, OutcomeState, Speaker, TagName, Window,
    WindowTurn,
};
pub use error::{Error, Result, WriteRefusal};
pub use handoff::{HANDOFF_TAG, HandoffError, HandoffSessionError};
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
pub use tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, FINDING_TAG,
    MAX_FINDINGS_PER_SESSION, Rule, RuleOrigin, StoreTag, TagDef, TagError, TagPatch, TagSummary,
    TagTable,
};

#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
#[cfg(test)]
mod tests;
