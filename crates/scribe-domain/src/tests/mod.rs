//! Unit tests for the business-logic modules, one file per source file
//! (STD-02 R19). Being siblings rather than children of the modules they test,
//! they reach only `pub(crate)` items. The store's tests live in
//! `store/tests/`.

mod annotation;
mod capture;
mod claim;
mod config;
mod control;
mod entry;
mod group;
mod handoff;
mod instruction;
mod memory;
mod message;
mod proposal;
mod record;
mod session;
mod tag;
mod transcript;
mod trigger;
mod verbatim;
mod voice;

pub(crate) use crate::fixtures::{T0, entry, session, tags_with_idea};
