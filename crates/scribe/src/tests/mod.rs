//! Unit tests for the top-level modules, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "CLI unit tests read repository files for parser checks"
)]

mod cli;
mod help;
