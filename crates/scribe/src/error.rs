//! The surface error type: every failure a command can end with.
//!
//! Domain errors arrive through the one translator, `#[from]` on
//! [`CliError::Core`], applied by `?` (STD-02 R12); nothing matches core
//! variants ad hoc. Usage errors never reach this type: clap reports them
//! and exits 2 before a command runs.

use std::io;

/// Why a command failed. Every variant exits 1 (STD-01 R20).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// The store refused or failed.
    #[error(transparent)]
    Core(#[from] scribe_core::Error),
    /// No data directory could be resolved.
    #[error("no data directory: pass --root <dir> or set SCRIBE_ROOT (HOME is not set either)")]
    NoDataDir,
    /// A `[redact] env` name has no value in this process's environment, so
    /// text it was configured to mask could be stored unmasked.
    #[error(
        "configured [redact] env name(s) have no value in this environment: {}; refusing to write unmasked text (add the name(s) to the caller's env_pass or remove them from [redact] env)",
        .0.join(", ")
    )]
    RedactEnvUnset(Vec<String>),
    /// A hook's stdin was not the JSON Claude Code sends for that event.
    #[error("the hook input is not a Claude Code hook payload: {0}")]
    HookPayload(String),
    /// Reading stdin or a file the command was pointed at failed.
    #[error("cannot read {what}")]
    Read {
        /// What was being read: `stdin`, or the transcript's path.
        what: String,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// A model call failed (`question ask`).
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
            Self::NoDataDir => "no_data_dir",
            Self::RedactEnvUnset(_) => "redact_env_unset",
            Self::HookPayload(_) => "hook_payload_invalid",
            Self::Read { .. } => "io_error",
            Self::Model(e) => e.code(),
            Self::Output(_) => "output_error",
        }
    }

    /// The process exit code. Only usage errors (clap's) exit 2.
    pub(crate) fn exit_code(&self) -> u8 {
        1
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
            // The core enum is #[non_exhaustive], so it needs this arm.
            Self::Core(_)
            | Self::RedactEnvUnset(_)
            | Self::NoDataDir
            | Self::HookPayload(_)
            | Self::Read { .. }
            | Self::Model(_)
            | Self::Output(_) => None,
        }
    }
}
