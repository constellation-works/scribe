//! The `admin` app: the store's and a project's upkeep — `scribe init`,
//! `scribe store migrate` and `scribe export` (2_design.md §15).
//!
//! Each command is one call to the store or the project installer. Until
//! migration step 3 the plugin's routine dispatch stays in the binary,
//! which composes a second store from the envelope's root; its tools reach
//! the session, consistency and memory apps through their handlers.

mod init;
mod migrate;
pub mod rules;

pub use init::{
    InitFile, InitReport, init_project, init_project_with_skill_files, integration_hooks,
};
#[cfg(any(test, feature = "test-support"))]
pub use migrate::MigrateTestWrites;

use crate::composition::Context;
use crate::runtime::audit::{Audit, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use crate::{ExportCounts, ExportFormat, Imported, Provider};
use std::path::{Path, PathBuf};

/// An `admin` command.
pub enum Command<'i> {
    /// `init`: install the skill, its files and the provider's hooks in
    /// `target`, pointing the hooks at the running executable.
    Init {
        /// The project directory.
        target: &'i Path,
        /// The provider whose hooks are installed.
        provider: Provider,
        /// The skill's `SKILL.md`.
        skill: &'i str,
        /// The skill's other files: a skill-relative path and its text.
        skill_files: &'i [(&'i str, &'i str)],
    },
    /// `store migrate`: import a file store's archives, dated when it runs.
    Migrate,
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
    Migrated(Box<Imported>),
    /// What `export` wrote.
    Exported(Box<ExportWritten>),
}

/// What `export` wrote, and where and how.
pub struct ExportWritten {
    /// The counts and files written.
    pub exported: ExportCounts,
    /// The directory written into.
    pub dir: PathBuf,
    /// The format written.
    pub format: ExportFormat,
}

/// Why an `admin` command did not run.
pub type Error = crate::Error;

/// Run `command` inside the audit guard `audit` names. `init` first checks
/// the selected runtime is on `PATH` and resolves the running executable,
/// which its hooks point at. Admin is the top of the app order, so no app
/// calls it and it has no `run` of its own.
///
/// # Errors
///
/// A store error, a refused or failed install, a runtime missing from
/// `PATH`, or an executable path that cannot be read or is not UTF-8.
pub fn exec(
    ctx: &Context,
    audit: Option<Audit>,
    command: Command<'_>,
) -> Result<Outcome, CommandError> {
    guarded(ctx, audit, || {
        Ok(match command {
            Command::Init {
                target,
                provider,
                skill,
                skill_files,
            } => {
                ctx.require_runtime()?;
                let binary = crate::composition::current_exe()?;
                Outcome::Initialized(Box::new(init::init_project_with_skill_files(
                    target,
                    provider,
                    &binary,
                    skill,
                    skill_files,
                )?))
            }
            Command::Migrate => {
                Outcome::Migrated(Box::new(migrate::migrate(ctx.store(), ctx.now())?))
            }
            Command::Export { dir, format } => Outcome::Exported(Box::new(ExportWritten {
                exported: ctx.store().export(dir, format)?,
                dir: dir.to_path_buf(),
                format,
            })),
        })
    })
}

/// `init` writes its line only when it changed a file, naming the provider;
/// `store migrate` one, naming the records.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Initialized(report) => {
                Line::written(Some(report.provider.to_string()), report.changed)
            }
            Self::Migrated(_) => vec![Line::success(Some(String::from("store:records")))],
            // `export` writes nothing to the store: never audited.
            Self::Exported(_) => vec![Line::success(None)],
        }
    }
}
