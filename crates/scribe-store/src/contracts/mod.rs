//! The record shapes the store hands its callers (architecture.md,
//! `scribe-store`): type definitions, a constructor or parser that enforces
//! the type's own invariant, serde and `Display`, and conversions between
//! two contract types. Nothing here reads the database or a repository, and
//! nothing decides between two records.
//!
//! Step 5 moves the record shapes in, family by family.

pub(crate) mod action;
pub mod entry;
pub(crate) mod group;
pub mod handoff;
pub(crate) mod mark;
pub(crate) mod memory;
pub(crate) mod message;
pub(crate) mod proposal;
pub mod provider;
pub mod record;
pub(crate) mod session;
pub(crate) mod tag;
#[cfg(test)]
mod tests;
