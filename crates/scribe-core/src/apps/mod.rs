//! The apps (docs/design/scribe/specs/architecture.md, §Apps): one subject a
//! user names on the command line each, with `Command`, `Outcome`, `Error`,
//! `run` and `exec`. ORB-27921 step 2 moves the use cases here from
//! `cases/`, one app family per pull request, lowest in the app order first.
//!
//! An app imports only apps earlier in the order and only through what their
//! `mod.rs` exports (scripts/check-dependency-direction.sh).

pub mod entry;
pub mod group;
pub mod mark;
pub mod memory;
pub mod proposal;
pub mod session;
pub mod tag;
