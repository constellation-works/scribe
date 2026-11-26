//! Unit tests for the host ports, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "host port unit tests stage temporary transcript directories and permissions on disk"
)]

mod transcripts;
