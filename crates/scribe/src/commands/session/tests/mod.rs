//! Unit tests for the `session` verbs, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "session command unit tests stage transcript files and synchronization channels directly"
)]

mod catalog;
mod pass;
