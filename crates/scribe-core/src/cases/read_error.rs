//! The error of the read use cases (ORB-27707 step 3d): `entry`,
//! `proposal`, `session` and `mark` lists and shows, `search` and
//! `transcript`.

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
