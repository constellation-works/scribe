//! The host's side of the apps' ports ([`Ctx`]): the clock, the provider
//! transcripts on this host and the redactor, implemented on composition's
//! [`Context`]; every port reports a [`CommandError`].

#[cfg(test)]
mod tests;
pub(crate) mod transcripts;

#[cfg(unix)]
use super::ClaudeRead;
use super::{
    AuditNotes, Clock, Ctx, Discovered, Redaction, TranscriptState, Transcripts, Unreadable,
};
#[cfg(unix)]
use crate::ClaudeCapturePosition;
use crate::composition::Context;
use crate::runtime::errors::CommandError;
use crate::{Config, Project, Provider, Redactor, SessionId};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use transcripts as payload;

impl Context {
    /// The store and this context's ports, for a use case's `run`.
    pub(crate) fn core(&self) -> Ctx<'_, CommandError> {
        Ctx {
            store: self.store(),
            clock: self,
            transcripts: self,
            redaction: self,
        }
    }

    /// Display names for session ids, unique across the stored sessions,
    /// this provider's transcripts on disk and `extra` (the group members
    /// and openers the output names) ([`resolve::short_ids`](crate::runtime::resolve::short_ids)).
    #[must_use]
    pub fn short_ids(&self, extra: impl IntoIterator<Item = String>) -> crate::ShortIds {
        crate::runtime::resolve::short_ids(&self.core(), self.provider(), extra)
    }
}

impl AuditNotes for Context {
    fn session(&self, session: &str) {
        self.note_instruction_session(session);
    }

    fn message(&self, message: &str) {
        self.note_instruction_message(message);
    }

    fn committed(&self) {
        self.note_audit_committed();
    }
}

impl Clock for Context {
    fn now(&self) -> OffsetDateTime {
        Context::now(self)
    }

    fn now_precise(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

impl Redaction for Context {
    type Error = CommandError;

    fn redactor(&self, config: &Config) -> Result<Redactor, CommandError> {
        Context::redactor(self, config)
    }
}

impl Transcripts for Context {
    type Error = CommandError;

    fn dir(&self, provider: Provider) -> Option<PathBuf> {
        self.with_provider(provider)
            .transcripts()
            .map(PathBuf::from)
    }

    fn files(&self, provider: Provider) -> Result<Vec<(SessionId, PathBuf)>, CommandError> {
        payload::files_for(&self.with_provider(provider))
    }

    fn check(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<TranscriptState, String> {
        payload::check(&self.with_provider(provider), path, session)
    }

    fn read(&self, provider: Provider, path: &Path) -> std::io::Result<Vec<u8>> {
        payload::read_for(&self.with_provider(provider), path)
    }

    fn read_checked(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable> {
        payload::read_checked(&self.with_provider(provider), path, session)
    }

    fn read_checked_unparsed(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable> {
        payload::read_checked_bytes(&self.with_provider(provider), path, session)
    }

    fn modified(&self, path: &Path) -> Option<std::time::SystemTime> {
        scribe_tools::transcript::modified(path)
    }

    fn project_of(&self, cwd: &Path) -> Project {
        scribe_tools::project::of(cwd)
    }

    fn claude_header(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
    ) -> Result<Option<Vec<u8>>, Unreadable> {
        payload::claude_header(&self.with_provider(provider), path, session)
    }

    fn locate_own(
        &self,
        provider: Provider,
        session: &SessionId,
    ) -> Result<Option<PathBuf>, CommandError> {
        payload::locate_own(&self.with_provider(provider), session)
    }

    fn discovered(&self, provider: Provider) -> Result<Discovered, CommandError> {
        payload::discovered_for(&self.with_provider(provider))
    }

    #[cfg(unix)]
    fn claude_window(
        &self,
        provider: Provider,
        path: &Path,
        session: &SessionId,
        prior: Option<&ClaudeCapturePosition>,
    ) -> Result<Option<ClaudeRead>, Unreadable> {
        payload::claude_window(&self.with_provider(provider), path, session, prior)
    }
}
