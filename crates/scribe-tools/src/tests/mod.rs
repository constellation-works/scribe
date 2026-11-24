//! Unit tests for the top-level modules, one file per source file (STD-02 R19).

mod claim;
mod control;
mod markers;
mod project;
mod transcript;
mod trigger;
mod verbatim;

pub(crate) use scribe_domain::fixtures::{T0, entry};
