//! The record shapes the store hands its callers (architecture.md,
//! `scribe-store`): type definitions, a constructor or parser that enforces
//! the type's own invariant, serde and `Display`, and conversions between
//! two contract types. Nothing here reads the database or a repository, and
//! nothing decides between two records.
//!
//! Empty until step 5 moves the record shapes in.
