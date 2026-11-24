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
    /// `entry forget` cannot be undone and requires explicit confirmation.
    #[error(
        "forgetting {0} blanks its text for good and cannot be undone; pass --confirm to forget it"
    )]
    ForgetConfirmRequired(String),
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
    /// `session catalog` named a session that already has a record or a
    /// switch it did not write.
    #[error(
        "session {0} already has a record or was switched on; use 'scribe session pass --session {0}' or 'scribe session rebuild {0} --author <name> --confirm'"
    )]
    SessionCataloged(scribe_core::SessionId),
    /// No Claude Code transcript has this id.
    #[error("no transcript {id} under {}", dir.display())]
    TranscriptNotFound {
        /// The id that was looked up.
        id: scribe_core::SessionId,
        /// The transcripts directory that was searched.
        dir: std::path::PathBuf,
    },
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
            Self::NoDataDir => "no_data_dir",
            Self::RedactEnvUnset(_) => "redact_env_unset",
            Self::HookPayload(_) => "hook_payload_invalid",
            Self::Read { .. } => "io_error",
            Self::SessionNotOn(_) => "session_not_on",
            Self::SegmentNotFound { .. } => "segment_not_found",
            Self::ConfirmRequired { .. } | Self::ForgetConfirmRequired(_) => "confirm_required",
            Self::TranscriptMissing(_) => "transcript_missing",
            Self::TranscriptRefused { .. } => "provenance_failed",
            Self::PassFailed(_) => "session_pass_failed",
            Self::SessionCataloged(_) => "session_cataloged",
            Self::TranscriptNotFound { .. } => "transcript_not_found",
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
            | Self::ForgetConfirmRequired(_)
            | Self::TranscriptMissing(_)
            | Self::TranscriptRefused { .. }
            | Self::PassFailed(_)
            | Self::SessionCataloged(_)
            | Self::TranscriptNotFound { .. }
            | Self::Model(_)
            | Self::Output(_) => None,
        }
    }
}

/// `session pass`'s errors, each the variant it was before the flow moved to
/// `scribe_core::cases::pass` (ORB-27707 step 3).
impl From<scribe_core::cases::pass::PassRunError<CliError>> for CliError {
    fn from(err: scribe_core::cases::pass::PassRunError<CliError>) -> Self {
        use scribe_core::cases::pass::PassRunError;
        match err {
            PassRunError::Core(e) => Self::Core(e),
            PassRunError::SessionNotOn(id) => Self::SessionNotOn(id),
            PassRunError::Port(e) => e,
        }
    }
}

/// The hooks' errors (`hook stop`, the marker catalog, group turn capture,
/// the prompt hook and `/scribe group add`), each the variant it was before the flow moved to
/// `scribe_core::cases` (ORB-27707 step 3b).
impl From<scribe_core::cases::hook::HookError<CliError>> for CliError {
    fn from(err: scribe_core::cases::hook::HookError<CliError>) -> Self {
        use scribe_core::cases::hook::HookError;
        match err {
            HookError::Core(e) => Self::Core(e),
            HookError::Port(e) => e,
            HookError::Read { what, source } => Self::Read { what, source },
            HookError::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
            HookError::Usage(message) => Self::Usage(message),
        }
    }
}

/// `scribe group`'s errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::GroupError<CliError>> for CliError {
    fn from(err: scribe_core::cases::GroupError<CliError>) -> Self {
        use scribe_core::cases::GroupError;
        match err {
            GroupError::Core(e) => Self::Core(e),
            GroupError::Port(e) => e,
            GroupError::Read { what, source } => Self::Read { what, source },
            GroupError::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
            GroupError::Usage(message) => Self::Usage(message),
            GroupError::Model(e) => Self::Model(e),
        }
    }
}

/// `scribe memory`'s errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::memory_cmd::MemoryError<CliError>> for CliError {
    fn from(err: scribe_core::cases::memory_cmd::MemoryError<CliError>) -> Self {
        use scribe_core::cases::memory_cmd::MemoryError;
        match err {
            MemoryError::Core(e) => Self::Core(e),
            MemoryError::Port(e) => e,
            MemoryError::Model(e) => Self::Model(e),
            MemoryError::ConfirmRequired(shown) => {
                Self::ForgetConfirmRequired(crate::output::MemoryPayload::from_shown(&shown).id)
            }
        }
    }
}

/// `consistency scan`'s errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::consistency_scan::ScanError<CliError>> for CliError {
    fn from(err: scribe_core::cases::consistency_scan::ScanError<CliError>) -> Self {
        use scribe_core::cases::consistency_scan::ScanError;
        match err {
            ScanError::Core(e) => Self::Core(e),
            ScanError::Port(e) => e,
        }
    }
}

/// `ask`'s errors, each the variant it was before the flow moved.
/// The record writes' errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::write_error::WriteError<CliError>> for CliError {
    fn from(err: scribe_core::cases::write_error::WriteError<CliError>) -> Self {
        use scribe_core::cases::write_error::WriteError;
        match err {
            WriteError::Core(e) => Self::Core(e),
            WriteError::Port(e) => e,
            WriteError::Usage(message) => Self::Usage(message),
            WriteError::Read { what, source } => Self::Read { what, source },
            WriteError::ConfirmRequired(id) => Self::ForgetConfirmRequired(id),
        }
    }
}

/// The reads' errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::read_error::ReadError<CliError>> for CliError {
    fn from(err: scribe_core::cases::read_error::ReadError<CliError>) -> Self {
        use scribe_core::cases::read_error::ReadError;
        match err {
            ReadError::Core(e) => Self::Core(e),
            ReadError::Port(e) => e,
            ReadError::Usage(message) => Self::Usage(message),
            ReadError::Read { what, source } => Self::Read { what, source },
            ReadError::SegmentNotFound { session, n, count } => {
                Self::SegmentNotFound { session, n, count }
            }
            ReadError::TranscriptNotFound { id, dir } => Self::TranscriptNotFound { id, dir },
            ReadError::TranscriptRefused { id, reason } => Self::TranscriptRefused { id, reason },
        }
    }
}

/// `session catalog`'s errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::catalog::CatalogError<CliError>> for CliError {
    fn from(err: scribe_core::cases::catalog::CatalogError<CliError>) -> Self {
        use scribe_core::cases::catalog::CatalogError;
        match err {
            CatalogError::Core(e) => Self::Core(e),
            CatalogError::Port(e) => e,
            CatalogError::TranscriptNotFound { id, dir } => Self::TranscriptNotFound { id, dir },
            CatalogError::TranscriptRefused { id, reason } => {
                Self::TranscriptRefused { id, reason }
            }
            CatalogError::Read { what, source } => Self::Read { what, source },
            CatalogError::SessionCataloged(id) => Self::SessionCataloged(id),
            CatalogError::Usage(message) => Self::Usage(message),
        }
    }
}

/// `session rebuild`'s errors, each the variant it was before the flow moved.
impl From<scribe_core::cases::rebuild::RebuildError<CliError>> for CliError {
    fn from(err: scribe_core::cases::rebuild::RebuildError<CliError>) -> Self {
        use scribe_core::cases::rebuild::RebuildError;
        match err {
            RebuildError::Core(e) => Self::Core(e),
            RebuildError::Port(e) => e,
            RebuildError::Usage(message) => Self::Usage(message),
            RebuildError::ConfirmRequired {
                id,
                segments,
                passes,
            } => Self::ConfirmRequired {
                id,
                segments,
                passes,
            },
            RebuildError::TranscriptMissing(id) => Self::TranscriptMissing(id),
            RebuildError::TranscriptRefused { id, reason } => {
                Self::TranscriptRefused { id, reason }
            }
            RebuildError::Read { what, source } => Self::Read { what, source },
        }
    }
}
