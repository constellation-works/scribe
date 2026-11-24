//! What every use case's `run(ctx, input)` runs against (ORB-27707 step 3):
//! the store and the ports the CLI fills, so core reads no environment and
//! discovers no transcript itself.

#[cfg(unix)]
use crate::ClaudeCapturePosition;
use crate::{Config, Project, Provider, Redactor, SessionId, Store};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// The store and the ports a use case runs against. The CLI builds one per
/// command; `E` is its port error, handed back unchanged.
#[derive(Clone, Copy)]
pub struct Ctx<'a, E> {
    /// The data root.
    pub store: &'a Store,
    /// The time a run stamps its records with.
    pub clock: &'a dyn Clock,
    /// The provider transcripts on this host.
    pub transcripts: &'a dyn Transcripts<Error = E>,
    /// The redactor for a config, from the environment the CLI runs in.
    pub redaction: &'a dyn Redaction<Error = E>,
}

/// A port's error: a stable code and a message, as the store's errors have.
pub trait PortError: std::fmt::Display {
    /// The stable error code.
    fn code(&self) -> &'static str;
}

/// The clock: the time a run stamps its records with.
pub trait Clock {
    /// Now, at the precision the CLI has always stamped with.
    fn now(&self) -> OffsetDateTime;

    /// Now, at the system clock's full precision. Only the instruction
    /// check's 30-minute window reads it, as it always has; every other site
    /// reads [`Clock::now`].
    fn now_precise(&self) -> OffsetDateTime;
}

/// The redactor for a config ([`Redactor`]), built from the live values of
/// the variables its `[redact] env` names.
pub trait Redaction {
    /// The port's error.
    type Error;

    /// The redactor for `config`.
    ///
    /// # Errors
    ///
    /// When a configured name has no value in the environment.
    fn redactor(&self, config: &Config) -> Result<Redactor, Self::Error>;
}

/// The provider transcripts on this host: where they are and their bytes,
/// read under the provenance checks.
pub trait Transcripts {
    /// The port's error.
    type Error: PortError;

    /// The Claude Code transcripts directory, when there is one.
    fn dir(&self) -> Option<PathBuf>;

    /// Every `provider` transcript on this host, by session id.
    ///
    /// # Errors
    ///
    /// When the transcripts tree cannot be listed.
    fn files(&self, provider: Provider) -> Result<Vec<(SessionId, PathBuf)>, Self::Error>;

    /// The provenance check of `path` as `session`'s `provider` transcript.
    ///
    /// # Errors
    ///
    /// Why the check refused it.
    fn check(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<TranscriptState, String>;

    /// The bytes of a transcript [`Transcripts::check`] passed.
    ///
    /// # Errors
    ///
    /// When it cannot be opened or read.
    fn read(&self, provider: Provider, path: &Path) -> std::io::Result<Vec<u8>>;

    /// Check and read `path` as `session`'s `provider` transcript; `None`
    /// when there is no file yet.
    ///
    /// # Errors
    ///
    /// [`Unreadable`].
    fn read_checked(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable>;

    /// [`Transcripts::read_checked`] without its Codex rollout check: the
    /// file's path, type, owner, mode and size are checked on one open handle
    /// and its bytes returned unparsed. It differs from `read_checked` only
    /// for Codex, whose rollout it does not parse, so a caller that classifies
    /// the session itself (the instruction check) refuses a `codex exec`
    /// rollout as not interactive before it would refuse it as unreadable.
    ///
    /// # Errors
    ///
    /// [`Unreadable`].
    fn read_checked_unparsed(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable>;

    /// When `path` was last modified, when the file system says.
    fn modified(&self, path: &Path) -> Option<std::time::SystemTime>;

    /// The project a working directory belongs to.
    fn project_of(&self, cwd: &Path) -> Project;

    /// Check `path` as `session`'s `provider` transcript and read its
    /// opening header records; `None` when there is no file yet.
    ///
    /// # Errors
    ///
    /// [`Unreadable`].
    fn claude_header(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable>;

    /// Every `provider` transcript on this host by session id, and the
    /// entries named like one whose file type is refused.
    ///
    /// # Errors
    ///
    /// When the transcripts tree cannot be listed.
    fn discovered(&self, provider: Provider) -> Result<Discovered, Self::Error>;

    /// `session`'s own `provider` transcript, when there is one: another
    /// session's duplicate id elsewhere in the tree is not an error here, a
    /// duplicate of `session` itself still is.
    ///
    /// # Errors
    ///
    /// When the transcripts tree cannot be listed, or `session` is there twice.
    fn locate_own(
        &self,
        provider: Provider,
        session: &SessionId,
    ) -> Result<Option<PathBuf>, Self::Error>;

    /// Check `path` as `session`'s `provider` transcript and read its
    /// complete records after `prior`, the capture's saved position (from
    /// the start when it no longer matches); `None` when there is no file
    /// yet.
    ///
    /// # Errors
    ///
    /// [`Unreadable`].
    #[cfg(unix)]
    fn claude_window(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
        prior: Option<&ClaudeCapturePosition>,
    ) -> Result<Option<ClaudeRead>, Unreadable>;
}

/// One read of a Claude transcript's new records
/// ([`Transcripts::claude_window`]).
#[cfg(unix)]
pub enum ClaudeRead {
    /// The file changed during the read (cut short, truncated or rewritten
    /// in place): read it again.
    Retry,
    /// The new complete records, or `None` when there are none.
    Window(Option<ClaudeWindow>),
}

/// A Claude transcript's new complete records and where they end.
#[cfg(unix)]
pub struct ClaudeWindow {
    /// The records' bytes.
    pub bytes: Vec<u8>,
    /// Whether the read resumed from the saved position.
    pub resumed: bool,
    /// Where the records end, for the next read.
    pub position: ClaudeCapturePosition,
}

/// What the CLI's audit middleware records about the instruction a command
/// acts on, noted as the command runs.
pub trait AuditNotes {
    /// The session the instruction was typed in.
    fn session(&self, session: &str);
    /// The message that carried the instruction.
    fn message(&self, message: &str);
    /// The command's audit line was committed with its write.
    fn committed(&self);
}

/// The transcripts found on this host ([`Transcripts::discovered`]).
#[derive(Debug, Default)]
pub struct Discovered {
    /// The regular transcripts, by session id.
    pub files: Vec<(SessionId, PathBuf)>,
    /// The entries named like a transcript whose file type is refused.
    pub refused: Vec<(SessionId, PathBuf)>,
}

/// Whether a transcript that passed its checks is there yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptState {
    /// The transcript exists and passed.
    Present,
    /// The path is right but there is no file yet.
    Missing,
}

/// Why a transcript could not be read.
#[derive(Debug)]
pub enum Unreadable {
    /// The provenance check refused it; the text is why.
    Refused(String),
    /// The file could not be opened or read.
    Io(std::io::Error),
}

/// How much one run of a routine command may do: at most `max` units
/// (sessions passed, batches judged), and none started after `start_by`
/// except the first, so a plugin call that may run a 240-second unit ends
/// inside Orbit's 300-second backend timeout. The deadline is monotonic
/// ([`std::time::Instant`]), not the [`Clock`].
#[derive(Clone, Copy, Debug)]
pub struct RunLimits {
    max: usize,
    start_by: Option<std::time::Instant>,
}

impl RunLimits {
    /// At most `max` units, with no time limit (the CLI).
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            max,
            start_by: None,
        }
    }

    /// No unit but the first starts after `start_by`.
    #[must_use]
    pub fn start_by(self, start_by: std::time::Instant) -> Self {
        Self {
            start_by: Some(start_by),
            ..self
        }
    }

    /// Whether unit `n` (from 0) may start now.
    #[must_use]
    pub fn may_start(&self, n: usize) -> bool {
        n < self.max
            && (n == 0
                || self
                    .start_by
                    .is_none_or(|by| std::time::Instant::now() < by))
    }

    /// A started session may process another window only before the plugin
    /// deadline. The CLI has no deadline and finishes every window.
    #[must_use]
    pub fn may_continue(&self) -> bool {
        self.start_by
            .is_none_or(|by| std::time::Instant::now() < by)
    }
}
