//! The errors the use cases share (ORB-27921): [`ReadError`], of the read
//! use cases (`entry`, `proposal`, `session` and `mark` lists and shows,
//! `search` and `transcript`), [`WriteError`], of the record writes
//! (`entry`, `proposal`, `tag` and `session activate|deactivate`), and
//! [`CommandError`], every failure a command can end with: the error the
//! ports and composition report, and every app error maps into.

use crate::SessionId;
use std::path::PathBuf;

/// Why a read did not run.
#[derive(Debug)]
pub enum ReadError<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// A usage error (exit 2).
    Usage(String),
    /// A transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// No such segment in the session.
    SegmentNotFound {
        /// The session.
        session: SessionId,
        /// The segment asked for.
        n: u32,
        /// How many it has.
        count: usize,
    },
    /// No transcript of the session on this host.
    TranscriptNotFound {
        /// The id looked up.
        id: SessionId,
        /// The transcripts directory searched.
        dir: PathBuf,
    },
    /// The transcript failed its provenance check.
    TranscriptRefused {
        /// The session.
        id: SessionId,
        /// Why.
        reason: String,
    },
}

impl<E> From<crate::Error> for ReadError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// Why a record write did not run.
#[derive(Debug)]
pub enum WriteError<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// A usage error (exit 2).
    Usage(String),
    /// A transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// A permanent write without `--confirm`: the record it would change.
    ConfirmRequired(String),
}

impl<E> From<crate::Error> for WriteError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// The redactor for the stored config, as the record writes build it.
pub(crate) fn redactor<E>(
    ctx: &crate::runtime::Ctx<'_, E>,
) -> Result<crate::Redactor, WriteError<E>> {
    let config = ctx.store.read().tags().config()?;
    ctx.redaction.redactor(&config).map_err(WriteError::Port)
}

/// Every failure an app's command can end with, as the CLI has always
/// reported it: the same message (its `Display`) and the same stable code
/// ([`CommandError::code`]). The ports report it too, so a host failure and
/// an app's refusal reach the caller, and the audit line, alike.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// The store refused or failed.
    #[error(transparent)]
    Core(#[from] crate::Error),
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
        source: std::io::Error,
    },
    /// `session pass --session` named a session that is not switched on.
    #[error(
        "session {0} is not switched on, so it is never passed; /scribe on in that session switches it on"
    )]
    SessionNotOn(crate::SessionId),
    /// `session show --segment <n>` (or `session:<id>#<n>`) named a segment
    /// the record's log does not have.
    #[error("session {session} has no segment {n}: its log has {count} segment(s)")]
    SegmentNotFound {
        /// The session.
        session: crate::SessionId,
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
        /// The session.
        id: crate::SessionId,
        /// Its segments.
        segments: usize,
        /// Its passes.
        passes: u32,
    },
    /// `entry forget` cannot be undone and requires explicit confirmation.
    #[error(
        "forgetting {0} blanks its text for good and cannot be undone; pass --confirm to forget it"
    )]
    ForgetConfirmRequired(String),
    /// No transcript is available for rebuilding this record.
    #[error("no transcript for session {0}; its record is unchanged")]
    TranscriptMissing(crate::SessionId),
    /// A rebuild's transcript failed the same provenance check as a pass.
    #[error("transcript for session {id} refused: {reason}")]
    TranscriptRefused {
        /// The session.
        id: crate::SessionId,
        /// Why the check refused it.
        reason: String,
    },
    /// `session catalog` named a session that already has a record or a
    /// switch it did not write.
    #[error(
        "session {0} already has a record or was switched on; use 'scribe session pass --session {0}' or 'scribe session rebuild {0} --author <name> --confirm'"
    )]
    SessionCataloged(crate::SessionId),
    /// No Claude Code transcript has this id.
    #[error("no transcript {id} under {}", dir.display())]
    TranscriptNotFound {
        /// The id that was looked up.
        id: crate::SessionId,
        /// The transcripts directory that was searched.
        dir: std::path::PathBuf,
    },
    /// A model call failed.
    #[error(transparent)]
    Model(#[from] crate::models::ModelError),
}

impl CommandError {
    /// The stable `snake_case` code (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
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
            Self::SessionCataloged(_) => "session_cataloged",
            Self::TranscriptNotFound { .. } => "transcript_not_found",
            Self::Model(e) => e.code(),
        }
    }
}

impl super::PortError for CommandError {
    fn code(&self) -> &'static str {
        CommandError::code(self)
    }
}

/// The record writes' errors, each the variant it was before the flow moved.
impl From<WriteError<CommandError>> for CommandError {
    fn from(err: WriteError<CommandError>) -> Self {
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
impl From<ReadError<CommandError>> for CommandError {
    fn from(err: ReadError<CommandError>) -> Self {
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
