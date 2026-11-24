//! The store's errors. `Error`, `Result` and `WriteRefusal` are the
//! domain's, re-exported here until they move into the store (ORB-27921
//! PR 5-10). The two refusals the store builds itself live beside them: an
//! agent-write refusal, and the refusal of a write to a forgotten session,
//! the deliberate exception ARCHITECTURE.md names.

pub use scribe_domain::error::*;

/// An agent-write refusal of `kind`, saying `message`.
pub fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

/// The refusal of a write to or on behalf of a forgotten session
/// (`session_forgotten`); `what` says what is refused.
pub fn forgotten_refusal(id: &crate::SessionId, what: &str) -> Error {
    refusal(
        WriteRefusal::SessionForgotten,
        format!("session {id} was forgotten (#scribe forget); {what}"),
    )
}
