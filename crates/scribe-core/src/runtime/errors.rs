//! The errors the use cases share (ORB-27921): [`ReadError`], of the read
//! use cases (`entry`, `proposal`, `session` and `mark` lists and shows,
//! `search` and `transcript`), [`WriteError`], of the record writes
//! (`entry`, `proposal`, `tag` and `session activate|deactivate`), and
//! [`HostError`], of composition and the host's ports.

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

/// What composition and the host's ports fail with: no data directory, an
/// unreadable store or config, a transcript tree that cannot be listed, a
/// missing redaction value, or arguments that contradict the host. Each has
/// the message and code the CLI has always reported for it.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The store refused or failed.
    #[error(transparent)]
    Core(#[from] crate::Error),
    /// Arguments or a host state that together make no sense; exits 2.
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
    /// A file or directory the host was pointed at could not be read.
    #[error("cannot read {what}")]
    Read {
        /// What was being read.
        what: String,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },
}

impl HostError {
    /// The stable `snake_case` code (STD-01 R19).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Core(e) => e.code(),
            Self::Usage(_) => "usage_error",
            Self::NoDataDir => "no_data_dir",
            Self::RedactEnvUnset(_) => "redact_env_unset",
            Self::Read { .. } => "io_error",
        }
    }
}

impl super::PortError for HostError {
    fn code(&self) -> &'static str {
        HostError::code(self)
    }
}
