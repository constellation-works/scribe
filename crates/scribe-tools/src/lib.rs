//! scribe's tools: functions of host files, bytes and text that decide
//! nothing about a record (docs/design/scribe/specs/architecture.md).
//!
//! - [`transcript`]: provider transcripts on this host, found under the
//!   transcript roots and read under the provenance checks;
//! - [`project`]: the project a working directory belongs to, from git's own
//!   files;
//! - [`host`]: this host's name;
//! - [`markers`]: the `#scribe` markers and settling lines of a typed turn,
//!   and the `/scribe` control line ([`markers::control`]);
//! - [`origin`]: the origin guard, whether a turn was plainly typed;
//! - [`window`]: the messages a session pass reads and the window it sends;
//! - [`fs`]: the crate's file-system access, the only module naming
//!   `std::fs`.
//!
//! The crate names no `scribe_core`, never calls `Store::write`, and reads
//! no environment variable and spawns no process: the caller passes the
//! resolved roots (scripts/check-dependency-direction.sh).

// Library code never prints; diagnostics go through `tracing` (STD-02 R15).
#![deny(clippy::print_stderr, clippy::print_stdout)]
// Unit tests use unwrap/expect for fixture setup; production call sites remain linted.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod fs;
pub mod host;
pub mod markers;
pub mod origin;
pub mod payload;
pub mod project;
pub mod text;
pub mod transcript;
pub mod window;

#[cfg(test)]
mod tests;

// The domain's modules and names at the paths the transcript parser,
// `codex`, `markers` and `control` name them by, and the control parser and
// Codex rollout at the root paths they had in the domain.
use markers::control;
use scribe_domain::{SessionId, Speaker, entry, proposal, record, session};
use transcript::codex;
