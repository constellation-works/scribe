//! Unit tests for the runtime's modules, one file per source file (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "audit unit tests inspect spooled files directly on disk"
)]

mod audit;
