//! The host's side of the apps' ports ([`Ctx`]): the clock, the provider
//! transcripts on this host and the redactor, implemented on composition's
//! [`Context`]. `E`, the ports' error, is built from the context's own
//! [`HostError`]s.

#[cfg(test)]
mod tests;
pub(crate) mod transcripts;

#[cfg(unix)]
use super::ClaudeRead;
use super::{
    AuditNotes, Clock, Ctx, Discovered, PortError, Redaction, Runtime, TranscriptState,
    Transcripts, Unreadable,
};
#[cfg(unix)]
use crate::ClaudeCapturePosition;
use crate::composition::{Context, HostError};
use crate::{Config, Project, Provider, Redactor, SessionId};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use transcripts as payload;

impl<E: PortError + From<HostError>> Context<E> {
    /// The store and this context's ports, for a use case's `run`.
    pub fn core(&self) -> Ctx<'_, E> {
        Ctx {
            store: self.store(),
            clock: self,
            transcripts: self,
            redaction: self,
        }
    }

    /// The store and this context's ports, for an app's `exec`.
    pub fn runtime(&self) -> Runtime<'_, E> {
        Runtime::from_parts(self.store(), self, self, self)
    }
}

impl<E> AuditNotes for Context<E> {
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

impl<E> Clock for Context<E> {
    fn now(&self) -> OffsetDateTime {
        Context::<E>::now(self)
    }

    fn now_precise(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

impl<E: From<HostError>> Redaction for Context<E> {
    type Error = E;

    fn redactor(&self, config: &Config) -> Result<Redactor, E> {
        Context::<E>::redactor(self, config).map_err(E::from)
    }
}

impl<E: PortError + From<HostError>> Transcripts for Context<E> {
    type Error = E;

    fn dir(&self) -> Option<PathBuf> {
        self.transcripts().map(PathBuf::from)
    }

    fn files(&self, provider: Provider) -> Result<Vec<(SessionId, PathBuf)>, E> {
        payload::files_for(&self.with_provider(provider)).map_err(E::from)
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

    fn locate_own(&self, provider: Provider, session: &SessionId) -> Result<Option<PathBuf>, E> {
        payload::locate_own(&self.with_provider(provider), session).map_err(E::from)
    }

    fn discovered(&self, provider: Provider) -> Result<Discovered, E> {
        payload::discovered_for(&self.with_provider(provider)).map_err(E::from)
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
