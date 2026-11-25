//! scribe's domain: the records and the pure rules over them, with no I/O.
//!
//! Session records, the on/off state
//! and the due rule ([`SessionRecord`], its log of [`Segment`]s,
//! [`ControlState`], [`Cadence`]) are in `session`; the shared metadata ([`Project`], [`Topic`], [`Related`],
//! [`Author`]) in `record`; the rules
//! that select and judge records in their modules. The typed `Error` is the store's (`scribe-store`'s
//! `error`), and so are entries and handoffs, proposals and the tag table
//! (`scribe-store`'s `contracts::{entry, handoff, proposal, tag}`).
//!
//! The modules sit in two rings, each importing only its own and the ones
//! before it: base, records (ARCHITECTURE.md;
//! `scripts/check-dependency-direction.sh`). It never touches the file
//! system, the network, a process or the environment: `scribe-store` holds
//! the records and `scribe-core` runs the use cases over both.

#![deny(clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod provider;
pub mod record;
pub mod session;
pub use provider::Provider;

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

#[cfg(any(test, feature = "test-support"))]
pub mod fixtures;
#[cfg(test)]
mod tests;
