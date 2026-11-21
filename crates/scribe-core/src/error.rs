//! The crate's one error type.
//!
//! Callers branch on the variant (or on [`Error::code`]), never on the message
//! (STD-02 R10). `Display` carries only this layer's sentence; the underlying
//! cause stays on [`std::error::Error::source`], so a surface can print the
//! whole chain once without repeating it.

use crate::config::ConfigError;
use crate::entry::EntryId;
use crate::session::{LinkError, SessionId};
use std::io;
use std::path::PathBuf;

/// Everything a [`crate::Store`] operation can fail with.
///
/// `#[non_exhaustive]`: this is the library's public boundary (STD-02 R11), so
/// adding a variant is not a breaking change. Surfaces map it through
/// [`Error::code`] instead of matching variants.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Project scaffold would overwrite an unowned or modified artifact.
    #[error("cannot initialize {}: {reason}", path.display())]
    InitConflict {
        /// Conflicting target.
        path: PathBuf,
        /// Actionable refusal.
        reason: String,
    },
    /// A direct write violated a domain rule.
    #[error("{message}")]
    AgentWrite {
        /// Stable refusal code.
        kind: WriteRefusal,
        /// Actionable explanation.
        message: String,
    },

    /// No entry has this id. The message names the file looked for, so a
    /// mis-resolved data directory is visible (STD-02 R26).
    #[error("no entry {id}: {} does not exist", path.display())]
    EntryNotFound {
        /// The id that was looked up.
        id: EntryId,
        /// The file it would be in.
        path: PathBuf,
    },
    /// No session record has this id.
    #[error("no session record {id} under {}", path.display())]
    SessionNotFound {
        /// The id that was looked up.
        id: SessionId,
        /// The directory searched.
        path: PathBuf,
    },
    /// An ordinary pass attempted to rewrite an already sealed segment.
    #[error("session {id} would change sealed segment {n}; keep sealed segments unchanged")]
    SealedSegmentChanged {
        /// Session being written.
        id: SessionId,
        /// Sealed segment number.
        n: u32,
    },
    /// The record on disk changed since the pass loaded it.
    #[error("session {id} record changed since it was loaded; retry from the current record")]
    RecordChanged {
        /// Session whose record changed.
        id: SessionId,
    },
    /// A due live session must finish its pass before rebuilding.
    #[error(
        "session {0} is on and due; run 'scribe session pass --session <id>' before rebuilding"
    )]
    SessionBusy(SessionId),
    /// A session link was refused: unknown target, self-link, cycle,
    /// a second parent, or a session not cataloged.
    #[error("cannot link this session: {0}")]
    Link(#[from] LinkError),
    /// A filesystem operation failed.
    #[error("cannot {action} {}", path.display())]
    Io {
        /// What was being attempted, as a verb phrase ("read", "create directory").
        action: &'static str,
        /// The path it was attempted on.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: io::Error,
    },
    /// A store file exists but is not one this build can parse: bad JSON or
    /// YAML, a missing or unknown key, a bad value, or a record whose id does
    /// not match its file name. The message names the file.
    #[error("{} is not a valid store file", path.display())]
    Corrupt {
        /// The store file.
        path: PathBuf,
        /// What the parser rejected, naming the key where it can.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// `config.toml` has a bad key or value; it is refused, never defaulted.
    #[error("{} is not a valid scribe config", path.display())]
    Config {
        /// The config file.
        path: PathBuf,
        /// What was wrong, naming the key.
        #[source]
        source: ConfigError,
    },
    /// The store was written by a newer build (STD-03 R10: refuse, never
    /// rewrite or reinterpret newer state).
    #[error(
        "{} uses store format {found}, but this build reads format {supported} or older; upgrade this tool",
        path.display()
    )]
    NewerFormat {
        /// The store file.
        path: PathBuf,
        /// The format the file declares.
        found: u32,
        /// The newest format this build understands.
        supported: u32,
    },
    /// Another writer held the store lock past the deadline (STD-03 R7).
    #[error(
        "{} is locked by {holder} (waited {waited_ms} ms); retry when it finishes",
        path.display()
    )]
    Locked {
        /// The lock file.
        path: PathBuf,
        /// Who holds it, as the holder recorded it (pid, label, time).
        holder: String,
        /// How long this call waited before giving up.
        waited_ms: u128,
    },
    /// A store file is a symbolic link (STD-05 R7, R9).
    #[error(
        "refusing {}: it is a symbolic link, and the store uses only regular files; replace the link with the file it points to",
        path.display()
    )]
    Symlink {
        /// The link.
        path: PathBuf,
    },
    /// Store state belongs to another user (STD-05 R9).
    #[error(
        "refusing {}: it is owned by uid {owner}, but this process runs as uid {uid}",
        path.display()
    )]
    ForeignOwner {
        /// The directory or file.
        path: PathBuf,
        /// Its owner.
        owner: u32,
        /// This process's effective uid.
        uid: u32,
    },
    /// Store state is open to other users: a group or other mode bit is set
    /// (STD-05 R8, R9).
    #[error(
        "refusing {}: mode {mode:o} is open to other users; fix with: chmod {fix} {}",
        path.display(),
        path.display()
    )]
    Permissions {
        /// The directory or file.
        path: PathBuf,
        /// Its permission bits.
        mode: u32,
        /// The owner-only mode to set: `700` for a directory, `600` for a file.
        fix: &'static str,
    },
    /// Proposals were already persisted, then a later step failed.
    /// [`AfterPersistError::closed`] names those proposals so the command can
    /// still audit them. The message, cause chain and [`Error::code`] are the
    /// later failure's (STD-02 R31).
    #[error(transparent)]
    AfterPersist(Box<AfterPersistError>),
}

/// A store failure that occurred after proposals were already persisted.
///
/// Transparent to [`std::fmt::Display`] and [`std::error::Error::source`]:
/// both delegate directly to `source`, so error formatters walking the cause
/// chain never repeat the inner error's message.
#[derive(Debug)]
pub struct AfterPersistError {
    /// Ids already written: proposals closed and entries the session pass
    /// wrote itself.
    pub closed: Vec<String>,
    /// The step that failed afterwards.
    pub source: Error,
}

impl std::fmt::Display for AfterPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)
    }
}

impl std::error::Error for AfterPersistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.source)
    }
}

impl Error {
    /// A stable `snake_case` code for this failure, for machine output
    /// (STD-01 R19). Adding a code is fine; changing one is a breaking change.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InitConflict { .. } => "init_conflict",
            Self::AgentWrite { kind, .. } => kind.code(),
            Self::EntryNotFound { .. } => "entry_not_found",
            Self::SessionNotFound { .. } => "session_not_found",
            Self::SealedSegmentChanged { .. } => "sealed_segment_changed",
            Self::RecordChanged { .. } => "record_changed",
            Self::SessionBusy(_) => "session_busy",
            Self::Link(err) => err.code(),
            Self::Io { .. } => "io_error",
            Self::Corrupt { .. } => "store_corrupt",
            Self::Config { .. } => "config_invalid",
            Self::NewerFormat { .. } => "store_too_new",
            Self::Locked { .. } => "store_locked",
            Self::Symlink { .. } => "store_symlink",
            Self::ForeignOwner { .. } => "store_foreign_owner",
            Self::Permissions { .. } => "store_permissions",
            Self::AfterPersist(after) => after.source.code(),
        }
    }

    pub(crate) fn corrupt(
        path: impl Into<PathBuf>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Corrupt {
            path: path.into(),
            source: source.into(),
        }
    }

    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

/// `Result` with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Domain classification of a refused direct write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteRefusal {
    /// `daniel_only`.
    DanielOnly,
    /// `reserved_author`.
    ReservedAuthor,
    /// `invalid_text`.
    InvalidText,
    /// `invalid_topics`.
    InvalidTopics,
    /// `tag_not_found`.
    TagNotFound,
    /// `tag_conflict`.
    TagConflict,
    /// `tag_configured`.
    TagConfigured,
    /// `proposal_invalid`.
    ProposalInvalid,
    /// `proposal_settled`.
    ProposalSettled,
    /// `proposal_not_agent`.
    ProposalNotAgent,
    /// `entry_not_active`.
    EntryNotActive,
    /// `entry_not_found`.
    EntryNotFound,
    /// `proposal_not_found`.
    ProposalNotFound,
    /// `id_ambiguous`.
    IdAmbiguous,
    /// Invalid id syntax, retaining the CLI usage-error contract.
    InvalidId,
    /// `quote_required`: an agent entry under a tag that requires Daniel's
    /// quote came without one, or without the session to check it in.
    QuoteRequired,
    /// `quote_not_found`: the quote is not verbatim in Daniel's typed turns
    /// of the session named, or that transcript cannot be read here.
    QuoteNotFound,
    /// `not_action_tag`: a quote or a pickup on a tag that is not an action
    /// tag.
    NotActionTag,
    /// `cancel_window`: a pickup of an agent entry still inside its cancel
    /// window.
    CancelWindow,
    /// `evidence_required`: an entry under a `require_evidence` tag came
    /// without an evidence pointer, or an evidence pointer was given for a
    /// tag that does not take one.
    EvidenceRequired,
    /// `finding_cap`: the writing session already holds the documented
    /// maximum of findings.
    FindingCap,
    /// `instruction_not_found`: the session's transcript is not on this
    /// host or not readable, or `--instruction` is in no turn Daniel
    /// plainly typed in it (docs/design/scribe/2_design.md §12.2).
    InstructionNotFound,
    /// `instruction_stale`: the instruction turn is not one of his last
    /// three typed turns, or is more than 30 minutes old.
    InstructionStale,
    /// `instruction_mismatch`: the instruction does not name the target,
    /// or does not name the act, or its turn or the reply it answers holds
    /// a negation or exception marker ("don't", "not", "never", "but",
    /// "only", ...).
    InstructionMismatch,
    /// `not_interactive`: the instruction's session is not one Daniel
    /// types into (§12.5).
    NotInteractive,
    /// `already_covered`: an active entry of the same tag and project
    /// already says this statement, so nothing was written.
    AlreadyCovered,
    /// `reserved_tag`: an entry write named the `issue` tag, which belongs
    /// to issue records (docs/design/scribe/2_design.md §11.4).
    ReservedTag,
    /// `issue_not_found`: no issue has the id.
    IssueNotFound,
    /// `occurrence_not_found`: the issue has no occurrence with the number.
    OccurrenceNotFound,
    /// `issue_forgotten`: the issue was forgotten, so nothing is written to
    /// it.
    IssueForgotten,
}
impl WriteRefusal {
    /// Stable machine-readable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::DanielOnly => "daniel_only",
            Self::ReservedAuthor => "reserved_author",
            Self::InvalidText => "invalid_text",
            Self::InvalidTopics => "invalid_topics",
            Self::TagNotFound => "tag_not_found",
            Self::TagConflict => "tag_conflict",
            Self::TagConfigured => "tag_configured",
            Self::ProposalInvalid => "proposal_invalid",
            Self::ProposalSettled => "proposal_settled",
            Self::ProposalNotAgent => "proposal_not_agent",
            Self::EntryNotActive => "entry_not_active",
            Self::EntryNotFound => "entry_not_found",
            Self::ProposalNotFound => "proposal_not_found",
            Self::IdAmbiguous => "id_ambiguous",
            Self::InvalidId => "usage_error",
            Self::QuoteRequired => "quote_required",
            Self::QuoteNotFound => "quote_not_found",
            Self::NotActionTag => "not_action_tag",
            Self::CancelWindow => "cancel_window",
            Self::EvidenceRequired => "evidence_required",
            Self::FindingCap => "finding_cap",
            Self::InstructionNotFound => "instruction_not_found",
            Self::InstructionStale => "instruction_stale",
            Self::InstructionMismatch => "instruction_mismatch",
            Self::NotInteractive => "not_interactive",
            Self::AlreadyCovered => "already_covered",
            Self::ReservedTag => "reserved_tag",
            Self::IssueNotFound => "issue_not_found",
            Self::OccurrenceNotFound => "occurrence_not_found",
            Self::IssueForgotten => "issue_forgotten",
        }
    }
}
