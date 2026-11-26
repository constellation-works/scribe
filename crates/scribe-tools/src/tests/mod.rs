//! Unit tests for the top-level modules, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "unit tests stage temporary files and synchronization channels directly"
)]

mod claim;
mod control;
mod markers;
mod project;
mod transcript;
mod trigger;
mod verbatim;

pub(crate) use scribe_store::fixtures::{T0, entry};
