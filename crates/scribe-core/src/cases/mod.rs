//! The use cases (ORB-27707 step 2): the verbs that read and write the store,
//! as extension traits implemented for [`Store`](crate::Store). Each impl
//! holds its use case's flow. It reads through [`Store::read`](crate::Store::read)
//! and writes through [`Store::write`](crate::Store::write), and the checks that
//! must be atomic with a write ([`crate::checks`]) run inside that write's
//! closure. The store holds no rule.
//!
//! These traits are not a storage-backend trait: the store stays one concrete
//! type. Step 3 adds each use case's `run(ctx, input)` entry point here, the
//! public API; at step 4 this module becomes scribe-core's use-case modules.
//!
//! This is the one core module besides `store/` that may name the store, and
//! neither the business modules nor `store/` may name it
//! (scripts/check-dependency-direction.sh).

pub mod annotation;
pub mod audit;
pub mod brief;
pub mod catalog;
pub mod codex;
pub mod consistency_scan;
pub mod continuation;
mod ctx;
pub mod entry;
pub mod entry_cmd;
pub mod entry_verify;
pub mod group;
pub mod group_add;
pub mod group_backfill;
pub mod group_close;
mod group_cmd;
pub mod group_join;
pub mod group_leave;
pub mod group_open;
pub mod group_turns;
pub mod hook;
pub mod instruction_check;
pub mod mark;
pub mod mark_retry;
pub mod memory;
pub mod memory_block;
pub mod memory_cmd;
pub mod pass;
pub mod pickup;
pub mod proposal;
pub mod proposal_cmd;
pub mod proposal_command;
pub mod read_error;
pub mod rebuild;
pub mod resolve;
pub mod scribe_command;
pub mod search;
pub mod session;
pub mod session_activity;
pub mod session_end;
pub mod session_start;
pub mod stop;
pub mod tag;
pub mod tag_cmd;
pub mod transcript;
pub mod user_prompt_submit;
pub mod write_error;

pub use ctx::{
    AuditNotes, Clock, Ctx, Discovered, PortError, Redaction, RunLimits, TranscriptState,
    Transcripts, Unreadable,
};
#[cfg(unix)]
pub use ctx::{ClaudeRead, ClaudeWindow};
pub use group_cmd::GroupError;
