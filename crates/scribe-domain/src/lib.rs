//! scribe's domain: the shared record metadata ([`Project`], [`Topic`],
//! [`Related`], [`Author`]) in `record`, and the transcript [`Provider`].
//! Every other record shape is the store's (`scribe-store`'s `contracts`,
//! with the typed `Error` in its `error`), and the rules are the core's
//! apps.
//!
//! The modules sit in one ring, base (ARCHITECTURE.md;
//! `scripts/check-dependency-direction.sh`). It never touches the file
//! system, the network, a process or the environment.

#![deny(clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod provider;
pub mod record;
pub use provider::Provider;

pub use record::{
    Author, AuthorKind, AuthorName, DateRange, Label, Listing, Project, RecordError, Related,
    RelatedKind, Topic, Topics, parse_day,
};

#[cfg(test)]
mod tests;
