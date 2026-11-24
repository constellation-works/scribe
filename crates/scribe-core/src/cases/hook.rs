//! What the hook use cases share (ORB-27707 step 3b): the reply a hook run
//! makes and its audit outcomes, the error the hook cases return, and the
//! text helpers their echoes use. The JSON a hook prints from a
//! [`HookReply`] (Claude Code's hook protocol) is rendered by the CLI.

use crate::cases::catalog::CatalogError;
use crate::runtime::PortError;
use crate::{Error, SessionId};

/// The code of a payload whose transcript failed the provenance check.
pub const PROVENANCE_FAILED: &str = "provenance_failed";

/// The longest quote an echo shows, in characters; `entry show` has it all.
const ECHO_CHARS: usize = 200;

/// The reply a Claude Code hook prints: text for the user
/// (`systemMessage`) and for the main agent (`additionalContext`), plus what
/// the audit log records for the run. Its JSON shape is Claude Code's hook
/// protocol, so it is the same in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookReply {
    /// The hook event, such as `UserPromptSubmit`.
    pub event: &'static str,
    /// Shown to the user; `None` prints no reply at all.
    pub message: Option<String>,
    /// Shown to the main agent beside the prompt.
    pub context: Option<String>,
    /// Block the prompt with this reason, shown to the user: a `/scribe`
    /// command is answered here and never reaches the model.
    pub block: Option<String>,
    /// One per record written or refused, and one per run that stored a group
    /// member's new turns, for the audit log; empty when the hook changed
    /// nothing.
    pub outcomes: Vec<HookOutcome>,
}

/// One write or refusal a hook run made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookOutcome {
    /// The record it acted on: an entry id, or `session:<id>` for a refusal.
    pub target: Option<String>,
    /// The stable code of a refusal; `None` for a write.
    pub error_code: Option<&'static str>,
}

/// Why a hook use case failed.
#[derive(Debug)]
pub enum HookError<E> {
    /// A store error.
    Core(Error),
    /// A port refused or failed.
    Port(E),
    /// The transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// The transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
    /// A usage error.
    Usage(String),
}

/// The port error of the prompt-side hooks: a reply that reports an error
/// in its text formats it as the port error it becomes, so the text, the
/// code and the cause chain are the CLI's own.
pub trait HookPortError:
    PortError
    + std::error::Error
    + From<Error>
    + From<HookError<Self>>
    + From<CatalogError<Self>>
    + Sized
{
}

impl<E> HookPortError for E where
    E: PortError
        + std::error::Error
        + From<Error>
        + From<HookError<E>>
        + From<CatalogError<E>>
        + Sized
{
}

impl<E> From<Error> for HookError<E> {
    fn from(err: Error) -> Self {
        Self::Core(err)
    }
}

/// `text` on one line, cut to [`ECHO_CHARS`].
#[must_use]
pub fn quote(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= ECHO_CHARS {
        flat
    } else {
        let mut cut: String = flat.chars().take(ECHO_CHARS).collect();
        cut.push('…');
        cut
    }
}

/// An error and its causes on one line.
#[must_use]
pub fn chain(err: &Error) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// A refusal outcome against `session`.
pub(crate) fn refusal(session: &SessionId, code: &'static str) -> HookOutcome {
    HookOutcome {
        target: Some(format!("session:{session}")),
        error_code: Some(code),
    }
}
