//! Unit tests for the business-logic modules, one file per source file
//! (STD-02 R19). Being siblings rather than children of the modules they test,
//! they reach only `pub(crate)` items. The store's tests live in
//! `store/tests/`.

mod claim;
mod entry;
mod handoff;
mod proposal;
mod record;
mod session;
mod tag;
mod text;
mod trigger;
mod verbatim;

pub(crate) use crate::fixtures::{T0, entry, session};
