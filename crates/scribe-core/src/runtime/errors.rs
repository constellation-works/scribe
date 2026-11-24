//! The errors the use cases share (ORB-27921): [`ReadError`], of the read
//! use cases (`entry`, `proposal`, `session` and `mark` lists and shows,
//! `search` and `transcript`), and [`WriteError`], of the record writes
//! (`entry`, `proposal`, `tag` and `session activate|deactivate`).

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
