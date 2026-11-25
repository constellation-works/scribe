//! The store's errors: [`Error`], [`Result`] and [`WriteRefusal`] (moved
//! from the domain in ORB-27921 PR 5-10), and the two refusals the store
//! builds itself: an agent-write refusal, and the refusal of a write to a
//! forgotten session, the deliberate exception ARCHITECTURE.md names.
//!
//! Callers branch on the variant (or on [`Error::code`]), never on the message
//! (STD-02 R10). `Display` carries only this layer's sentence; the underlying
//! cause stays on [`std::error::Error::source`], so a surface can print the
//! whole chain once without repeating it.

use crate::entry::EntryId;
use crate::session::{LinkError, SessionId};
use std::io;
use std::path::PathBuf;

/// Everything a `Store` operation (scribe-core) can fail with.
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
        /// What was wrong, naming the key (`ConfigError`, in scribe-core).
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// `scribe.db` refused an operation that is not a lock timeout or a
    /// damaged file (2_design.md §15.7).
    #[error("cannot {action} {}", path.display())]
    Database {
        /// What was being attempted, as a verb phrase.
        action: &'static str,
        /// The database file.
        path: PathBuf,
        /// SQLite's error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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
    /// A write or migration could not acquire its lock before the deadline (STD-03 R7).
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
            Self::Database { .. } => "store_database",
            Self::NewerFormat { .. } => "store_too_new",
            Self::Locked { .. } => "store_locked",
            Self::Symlink { .. } => "store_symlink",
            Self::ForeignOwner { .. } => "store_foreign_owner",
            Self::Permissions { .. } => "store_permissions",
        }
    }

    /// A file at `path` that does not parse.
    pub fn corrupt(
        path: impl Into<PathBuf>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Corrupt {
            path: path.into(),
            source: source.into(),
        }
    }

    /// An I/O failure `action` on `path`.
    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
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
    /// No group matched the requested id.
    GroupNotFound,
    /// The caller already holds a different seat or group.
    GroupMembership,
    /// The group evidence changed after it was read.
    GroupEvidenceChanged,
    /// Another live transcript holds the seat.
    SeatOccupied,
    /// `user_only`.
    UserOnly,
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
    /// `memory_not_found`: no memory item has this id, or no occurrence
    /// this number.
    MemoryNotFound,
    /// `memory_not_now`: `memory ack` of an item outside `now`.
    MemoryNotNow,
    /// `memory_changed`: the profile, its cursor or the memory files changed
    /// while `memory refresh` asked the model; nothing was written.
    MemoryChanged,
    /// `revision_not_found`: no profile revision has this number.
    RevisionNotFound,
    /// `profile_shape`: the rebuilt profile is not the seven headings in
    /// order.
    ProfileShape,
    /// `profile_pinned`: the rebuilt profile changed Pinned.
    ProfilePinned,
    /// `profile_too_long`: the rebuilt profile is over 6,000 characters.
    ProfileTooLong,
    /// `profile_unknown_link`: the rebuilt profile links an entry id that
    /// does not exist.
    ProfileUnknownLink,
    /// `profile_sensitive`: the rebuilt profile or a promoted milestone holds
    /// a credential, a finance word or a sensitive event.
    ProfileSensitive,
    /// `profile_milestone`: a promoted milestone names no passed `now` item,
    /// or has no real date, title or text within bounds.
    ProfileMilestone,
    /// `migrate_blocked`: `store migrate` was refused: a session is switched
    /// on, a rebuild checkpoint waits, or no archive is left to import.
    MigrateBlocked,
    /// `export_target_exists`: `export` was given a directory that is not
    /// empty. The store refuses it itself, as the precondition of its own
    /// export write.
    ExportTargetExists,
    /// `session_forgotten`: a write to a session `#scribe forget` erased
    /// (its record, or switching it on); a forgotten session takes none.
    SessionForgotten,
    /// `handoff_shape`: a handoff body lacks one of the six fixed headings,
    /// has them out of order, repeats or invents a heading, or has text
    /// before `## State`.
    HandoffShape,
    /// `handoff_too_long`: a handoff body exceeds [`crate::handoff::MAX_CHARS`].
    HandoffTooLong,
    /// `handoff_session`: a handoff names more than one distinct session or an
    /// invalid session reference, or `entry update` attempted to add, remove,
    /// or replace a handoff's `session:` reference.
    HandoffSession,
    /// `not_memory_tag`: `--spoken` was used on a tag that is not an
    /// eligible non-action memory tag (such as an action tag, `handoff`, or
    /// a `require_evidence` tag).
    NotMemoryTag,
    /// `needs_redaction`: a spoken-instruction quote contains text redaction
    /// would alter, so storing it verbatim would leak or change the quote.
    NeedsRedaction,
    /// `not_agent_note`: `entry update --content` targeted an entry whose
    /// content did not come from a direct agent write (such as an approved
    /// assistant claim).
    NotAgentNote,
    /// `decided_by_required`: a decision lacks its decider.
    DecidedByRequired,
    /// `asked_by_required`: an open item lacks its asker.
    AskedByRequired,
    /// `record_fields`: record flags were used under a different tag.
    RecordFields,
    /// `invalid_options`: the option count or text is invalid.
    InvalidOptions,
    /// `invalid_recommendation`: the option index is missing or invalid.
    InvalidRecommendation,
    /// `recommendation_why`: the reason for recommending an option is missing or misplaced.
    RecommendationWhy,
    /// `record_required`: a decision lacks its task, PR or repository reference.
    RecordRequired,
    /// `decision_too_long`: a decision exceeds 3,000 characters.
    DecisionTooLong,
    /// `decision_session`: a patch would change a user decision’s source session.
    DecisionSession,
    /// `decision_quote_not_allowed`: an agent decision includes a user quote.
    DecisionQuoteNotAllowed,
    /// `rationale_not_allowed`: a user decision includes an agent rationale.
    RationaleNotAllowed,
    /// `rationale_required`: an agent decision lacks its rationale.
    RationaleRequired,
    /// `invalid_settles`: the link does not name an open item.
    InvalidSettles,
    /// `invalid_supersedes`: the link does not name an active decision.
    InvalidSupersedes,
    /// `supersede_user_decision`: an agent decision would replace a user or untyped trial decision.
    SupersedeUserDecision,
    /// `not_open_item`: `entry answer` or `entry close` named an entry not under `open-item`.
    NotOpenItem,
    /// `item_settled`: the open item is already closed or withdrawn.
    ItemSettled,
    /// `not_answered`: a choice item closes only once the user's answer is recorded.
    NotAnswered,
    /// `invalid_decision`: `entry close --decision` names no active decision that settles the item.
    InvalidDecision,
}
impl WriteRefusal {
    /// Stable machine-readable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::GroupNotFound => "group_not_found",
            Self::GroupMembership => "group_membership",
            Self::GroupEvidenceChanged => "group_evidence_changed",
            Self::SeatOccupied => "seat_occupied",
            Self::UserOnly => "user_only",
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
            Self::MemoryNotFound => "memory_not_found",
            Self::MemoryNotNow => "memory_not_now",
            Self::MemoryChanged => "memory_changed",
            Self::RevisionNotFound => "revision_not_found",
            Self::ProfileShape => "profile_shape",
            Self::ProfilePinned => "profile_pinned",
            Self::ProfileTooLong => "profile_too_long",
            Self::ProfileUnknownLink => "profile_unknown_link",
            Self::ProfileSensitive => "profile_sensitive",
            Self::ProfileMilestone => "profile_milestone",
            Self::MigrateBlocked => "migrate_blocked",
            Self::ExportTargetExists => "export_target_exists",
            Self::SessionForgotten => "session_forgotten",
            Self::HandoffShape => "handoff_shape",
            Self::HandoffTooLong => "handoff_too_long",
            Self::HandoffSession => "handoff_session",
            Self::NotMemoryTag => "not_memory_tag",
            Self::NeedsRedaction => "needs_redaction",
            Self::NotAgentNote => "not_agent_note",
            Self::DecidedByRequired => "decided_by_required",
            Self::AskedByRequired => "asked_by_required",
            Self::RecordFields => "record_fields",
            Self::InvalidOptions => "invalid_options",
            Self::InvalidRecommendation => "invalid_recommendation",
            Self::RecommendationWhy => "recommendation_why",
            Self::RecordRequired => "record_required",
            Self::DecisionTooLong => "decision_too_long",
            Self::DecisionSession => "decision_session",
            Self::DecisionQuoteNotAllowed => "decision_quote_not_allowed",
            Self::RationaleNotAllowed => "rationale_not_allowed",
            Self::RationaleRequired => "rationale_required",
            Self::InvalidSettles => "invalid_settles",
            Self::InvalidSupersedes => "invalid_supersedes",
            Self::SupersedeUserDecision => "supersede_user_decision",
            Self::NotOpenItem => "not_open_item",
            Self::ItemSettled => "item_settled",
            Self::NotAnswered => "not_answered",
            Self::InvalidDecision => "invalid_decision",
        }
    }
}

/// An agent-write refusal of `kind`, saying `message`.
pub fn refusal(kind: WriteRefusal, message: impl Into<String>) -> Error {
    Error::AgentWrite {
        kind,
        message: message.into(),
    }
}

/// The refusal of a write to or on behalf of a forgotten session
/// (`session_forgotten`); `what` says what is refused.
pub fn forgotten_refusal(id: &crate::SessionId, what: &str) -> Error {
    refusal(
        WriteRefusal::SessionForgotten,
        format!("session {id} was forgotten (#scribe forget); {what}"),
    )
}
