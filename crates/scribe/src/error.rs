//! The surface error type: every failure a command can end with.
//!
//! Domain errors arrive through the one translator, `#[from]` on
//! [`CliError::Core`], applied by `?` (STD-02 R12); nothing matches core
//! variants ad hoc. Usage errors are clap's, which reports them and exits 2
//! before a command runs, except [`CliError::Usage`]: arguments that each
//! parse but contradict each other.

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
    /// `session pass --session` named a session that is not switched on.
    #[error(
        "session {0} is not switched on, so it is never passed; /scribe on in that session switches it on"
    )]
    SessionNotOn(scribe_core::SessionId),
    /// `session show --segment <n>` (or `session:<id>#<n>`) named a segment
    /// the record's log does not have.
    #[error("session {session} has no segment {n}: its log has {count} segment(s)")]
    SegmentNotFound {
        /// The session.
        session: scribe_core::SessionId,
        /// The segment asked for.
        n: u32,
        /// How many segments the log has.
        count: usize,
    },
    /// Rebuilding replaces the visible record and requires explicit confirmation.
    #[error(
        "session {id} has {segments} segment(s) and {passes} pass(es) to replace; pass --confirm to rebuild it"
    )]
    ConfirmRequired {
        id: scribe_core::SessionId,
        segments: usize,
        passes: u32,
    },
    /// No transcript is available for rebuilding this record.
    #[error("no transcript for session {0}; its record is unchanged")]
    TranscriptMissing(scribe_core::SessionId),
    /// A rebuild's transcript failed the same provenance check as a pass.
    #[error("transcript for session {id} refused: {reason}")]
    TranscriptRefused {
        id: scribe_core::SessionId,
        reason: String,
    },
    /// A session pass ran and at least one pass failed. The report is printed
    /// first; its redacted reasons are included here for the routine's root
    /// cause.
    #[error("{0}")]
    PassFailed(String),
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
            Self::Usage(_) => "usage_error",
            Self::NoDataDir => "no_data_dir",
            Self::RedactEnvUnset(_) => "redact_env_unset",
            Self::HookPayload(_) => "hook_payload_invalid",
            Self::Read { .. } => "io_error",
            Self::SessionNotOn(_) => "session_not_on",
            Self::SegmentNotFound { .. } => "segment_not_found",
            Self::ConfirmRequired { .. } => "confirm_required",
            Self::TranscriptMissing(_) => "transcript_missing",
            Self::TranscriptRefused { .. } => "provenance_failed",
            Self::PassFailed(_) => "session_pass_failed",
            Self::Model(e) => e.code(),
            Self::Output(_) => "output_error",
        }
    }

    /// The process exit code. Only usage errors exit 2.
    pub(crate) fn exit_code(&self) -> u8 {
        if matches!(self, Self::Usage(_)) { 2 } else { 1 }
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
            Self::SegmentNotFound { .. } => Some(
                "run 'scribe session show' on the same id, without --segment, to see its segments",
            ),
            // The core enum is #[non_exhaustive], so it needs this arm.
            Self::Core(_)
            | Self::Usage(_)
            | Self::RedactEnvUnset(_)
            | Self::NoDataDir
            | Self::HookPayload(_)
            | Self::Read { .. }
            | Self::SessionNotOn(_)
            | Self::ConfirmRequired { .. }
            | Self::TranscriptMissing(_)
            | Self::TranscriptRefused { .. }
            | Self::PassFailed(_)
            | Self::Model(_)
            | Self::Output(_) => None,
        }
    }
}
