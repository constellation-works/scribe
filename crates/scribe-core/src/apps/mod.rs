//! The apps (docs/design/scribe/specs/architecture.md, §Apps): one subject a
//! user names on the command line each, with `Command`, `Outcome`, `Error`,
//! `run` and `exec`. ORB-27921 step 2 moved every use case here from
//! `cases/`, which is gone.
//!
//! An app imports only apps earlier in the order and only through what their
//! `mod.rs` exports (scripts/check-dependency-direction.sh).

pub mod admin;
pub mod brief;
pub mod consistency;
pub mod entry;
pub mod group;
pub mod hook;
pub mod mark;
pub mod memory;
pub mod proposal;
pub mod search;
pub mod session;
pub mod tag;
