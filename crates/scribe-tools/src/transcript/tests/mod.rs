//! Deterministic read accounting for the Claude capture window.

#![expect(
    clippy::disallowed_types,
    reason = "transcript unit tests open temporary files directly to count read syscalls"
)]

mod claude;
