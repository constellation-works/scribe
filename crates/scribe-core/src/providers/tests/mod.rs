//! Unit tests for the model adapters, one file per subject (STD-02 R19).

#![expect(
    clippy::disallowed_methods,
    reason = "provider unit tests stage fake CLI scripts and directories on disk"
)]

mod codex_models;
mod models;
mod schema;
