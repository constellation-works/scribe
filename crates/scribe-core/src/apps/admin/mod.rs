//! The `admin` app: the store's and a project's upkeep — `scribe init`,
//! `scribe store migrate` and `scribe export` (2_design.md §15).
//!
//! Each command is one call to the store or the project installer. Until
//! migration step 3 the plugin's routine dispatch stays in the binary,
//! which composes a second store from the envelope's root; its tools reach
//! the session, consistency and memory apps through their handlers.

use crate::runtime::{Ctx, PortError, Runtime};
use crate::{ExportFormat, Exported, InitReport, Moved, Provider};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// An `admin` command.
pub enum Command<'i> {
    /// `init`: install the skill, its files and the provider's hooks in
    /// `target`, pointing the hooks at `binary`.
    Init {
        /// The project directory.
        target: &'i Path,
        /// The provider whose hooks are installed.
        provider: Provider,
        /// The scribe executable the hooks run.
        binary: &'i Path,
        /// The skill's `SKILL.md`.
        skill: &'i str,
        /// The skill's other files: a skill-relative path and its text.
        skill_files: &'i [(&'i str, &'i str)],
    },
    /// `store migrate`: import a file store's archives, dated `now`.
    Migrate {
        /// When the import runs.
        now: OffsetDateTime,
    },
    /// `export`: write every record kind into `dir` as `format`.
    Export {
        /// The directory to write into.
        dir: &'i Path,
        /// Markdown or JSONL.
        format: ExportFormat,
    },
}

/// What an `admin` command returns.
pub enum Outcome {
    /// What `init` installed.
    Initialized(Box<InitReport>),
    /// What `store migrate` imported.
    Migrated(Box<Moved>),
    /// What `export` wrote.
    Exported(Box<ExportWritten>),
}

/// What `export` wrote, and where and how.
pub struct ExportWritten {
    /// The counts and files written.
    pub exported: Exported,
    /// The directory written into.
    pub dir: PathBuf,
    /// The format written.
    pub format: ExportFormat,
}

/// Why an `admin` command did not run.
pub type Error = crate::Error;

/// Run `command`: the interim entry point until migration step 3 starts the
/// runtime from an `Env` and wraps the audit guard.
///
/// # Errors
///
/// A store error, or a refused or failed install.
pub fn exec<E: PortError>(
    runtime: &Runtime<'_, E>,
    command: Command<'_>,
) -> Result<Outcome, Error> {
    run(&runtime.ctx(), command)
}

/// Run `command` against `ctx`: dispatch only.
pub(crate) fn run<E: PortError>(ctx: &Ctx<'_, E>, command: Command<'_>) -> Result<Outcome, Error> {
    Ok(match command {
        Command::Init {
            target,
            provider,
            binary,
            skill,
            skill_files,
        } => Outcome::Initialized(Box::new(crate::init_project_with_skill_files(
            target,
            provider,
            binary,
            skill,
            skill_files,
        )?)),
        Command::Migrate { now } => Outcome::Migrated(Box::new(ctx.store.migrate_store(now)?)),
        Command::Export { dir, format } => Outcome::Exported(Box::new(ExportWritten {
            exported: ctx.store.export(dir, format)?,
            dir: dir.to_path_buf(),
            format,
        })),
    })
}
