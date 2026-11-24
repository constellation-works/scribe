//! The CLI's side of the use cases' ports ([`scribe_core::cases::Ctx`]):
//! the clock, the provider transcripts on this host and the redactor, all
//! from this process's environment.

use crate::app::Context;
use crate::commands::hook::{payload, project};
use crate::error::CliError;
use scribe_core::cases::{
    Clock, Ctx, PortError, Redaction, TranscriptState, Transcripts, Unreadable,
};
use scribe_core::{Config, Project, Provider, Redactor, SessionId};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

impl Context {
    /// The store and this context's ports, for a use case's `run`.
    pub(crate) fn core(&self) -> Ctx<'_, CliError> {
        Ctx {
            store: self.store(),
            clock: self,
            transcripts: self,
            redaction: self,
        }
    }
}

impl PortError for CliError {
    fn code(&self) -> &'static str {
        CliError::code(self)
    }
}

impl Clock for Context {
    fn now(&self) -> OffsetDateTime {
        Context::now(self)
    }
}

impl Redaction for Context {
    type Error = CliError;

    fn redactor(&self, config: &Config) -> Result<Redactor, CliError> {
        Context::redactor(self, config)
    }
}

impl Transcripts for Context {
    type Error = CliError;

    fn dir(&self) -> Option<PathBuf> {
        self.transcripts().map(PathBuf::from)
    }

    fn files(&self, provider: Provider) -> Result<Vec<(SessionId, PathBuf)>, CliError> {
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

    fn project_of(&self, cwd: &Path) -> Project {
        project::of(cwd)
    }
}
