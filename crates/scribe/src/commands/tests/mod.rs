//! Unit tests for the command modules, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "command unit tests stage files, inspect scribe.db directly and run helper commands"
)]

mod commands;
mod entry;
mod search;
mod transcript;
