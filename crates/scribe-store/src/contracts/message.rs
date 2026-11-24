//! The messages the store keeps in `scribe.db` (docs/design/scribe/2_design.md
//! §15.7) and hands back from a search: a message to keep, before redaction,
//! and a search's matches. The entry and session apps decide which turns are
//! kept.

use crate::entry::Speaker;
use crate::session::SessionId;
use time::OffsetDateTime;

/// One message to keep, before redaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMessage {
    /// The session it belongs to.
    pub session: SessionId,
    /// The provider's message id (the transcript line's `uuid`).
    pub message: String,
    /// Whose words they are.
    pub role: Speaker,
    /// When the provider wrote it.
    pub at: OffsetDateTime,
    /// Its text: a user turn's own typed words, or a cited assistant turn.
    pub text: String,
}

/// One message a full-text search matched, as the store holds it (redacted).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageMatch {
    /// The session it belongs to.
    pub session: SessionId,
    /// The provider's message id.
    pub message: String,
    /// Whose words they are.
    pub role: Speaker,
    /// When the provider wrote it.
    pub at: OffsetDateTime,
    /// Its stored text.
    pub text: String,
    /// FTS5's `bm25()` score, negated so that higher is better.
    pub score: f64,
    /// Project of the group that authorized a turn without a session pass.
    pub group_project: Option<crate::Project>,
}

/// What a message search read: the matches, best first, and whether they
/// came from a read-only copy of the database opened `immutable=1`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageSearch {
    /// Every match, best first.
    pub matches: Vec<MessageMatch>,
    /// The database was opened as a read-only copy (§15.7): a read-only
    /// root without its `-wal` and `-shm` files.
    pub read_only_copy: bool,
}
