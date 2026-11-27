//! The surface error type: every failure a command can end with.
//!
//! Domain errors arrive through the one translator, `#[from]` on
//! [`CliError::Core`], applied by `?` (STD-02 R12); nothing matches core
//! variants ad hoc. Usage errors are clap's, which reports them and exits 2
//! before a command runs, except [`CliError::Usage`]: arguments that each
//! parse but contradict each other.

use scribe_core::CommandError;
use std::io;

/// Why a command failed. Every variant but [`CliError::Usage`] exits 1
/// (STD-01 R20).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// The store refused or failed.
    #[error(transparent)]
    Core(#[from] scribe_core::Error),
    /// Arguments that each parse but together make no sense; exits 2.
    #[error("{0}")]
    Usage(String),
    /// An app command failed.
    #[error(transparent)]
    Command(CommandError),
    /// Reading stdin or a file the command was pointed at failed.
    #[error("cannot read {what}")]
    Read {
        /// What was being read: `stdin`, or the transcript's path.
        what: String,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// A session pass ran and at least one pass failed. The report is printed
    /// first; its redacted reasons are included here for the routine's root
    /// cause.
    #[error("{0}")]
    PassFailed(String),
    /// A model call failed.
    #[error(transparent)]
    Model(#[from] scribe_core::models::ModelError),
    /// Writing the result to stdout failed for a reason other than a closed pipe.
    #[error("cannot write output")]
    Output(#[source] io::Error),
}

impl CliError {
    /// Stable `snake_case` code for the JSON error object (STD-01 R19).
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Core(e) => e.code(),
            Self::Usage(_) => "usage_error",
            Self::Command(e) => e.code(),
            Self::Read { .. } => "io_error",
            Self::PassFailed(_) => "session_pass_failed",
            Self::Model(e) => e.code(),
            Self::Output(_) => "output_error",
        }
    }

    /// The process exit code. Only usage errors exit 2.
    pub(crate) fn exit_code(&self) -> u8 {
        if matches!(self, Self::Usage(_)) || matches!(self.code(), "id_ambiguous" | "usage_error") {
            2
        } else {
            1
        }
    }

    /// The fix, where it is known and not already in the message (STD-01
    /// R21). A remedy must be runnable as written (STD-02 R26), so this one
    /// reminds the reader to keep the same data directory.
    pub(crate) fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Core(scribe_core::Error::EntryNotFound { .. }) => {
                Some("run 'scribe entry list' with the same --root to see existing ids")
            }
            Self::Core(scribe_core::Error::SessionNotFound { .. }) => {
                Some("run 'scribe session list' with the same --root to see existing ids")
            }
            Self::Command(CommandError::SegmentNotFound { .. }) => Some(
                "run 'scribe session show' on the same id, without --segment, to see its segments",
            ),
            // The core enum is #[non_exhaustive], so it needs this arm.
            Self::Core(_)
            | Self::Usage(_)
            | Self::Command(_)
            | Self::Read { .. }
            | Self::PassFailed(_)
            | Self::Model(_)
            | Self::Output(_) => None,
        }
    }
}

/// Preserve directly handled errors and wrap other command errors.
impl From<CommandError> for CliError {
    fn from(err: CommandError) -> Self {
        match err {
            CommandError::Core(e) => Self::Core(e),
            CommandError::Usage(message) => Self::Usage(message),
            CommandError::Read { what, source } => Self::Read { what, source },
            CommandError::Model(e) => Self::Model(e),
            other @ (CommandError::NoDataDir
            | CommandError::RedactEnvUnset(_)
            | CommandError::HookPayload(_)
            | CommandError::SessionPassStep { .. }
            | CommandError::SessionNotOn(_)
            | CommandError::SegmentNotFound { .. }
            | CommandError::ConfirmRequired { .. }
            | CommandError::ForgetConfirmRequired(_)
            | CommandError::TranscriptMissing(_)
            | CommandError::TranscriptRefused { .. }
            | CommandError::SessionCataloged(_)
            | CommandError::TranscriptNotFound { .. }) => Self::Command(other),
        }
    }
}
