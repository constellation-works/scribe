//! The `init` app: `scribe init`, which scaffolds the embedded skill and the
//! selected provider's hooks in one existing project directory
//! (2_design.md §15). Project setup is its own subject: it writes the
//! project's files, not the store, and it does not share the `store` app's
//! resource (STD-02@8 §R45; ORB-28770).
//!
//! The command is one call to the project installer (`install.rs`). It calls
//! no other app.

mod install;

pub use install::{
    InitFile, InitReport, init_project, init_project_with_skill_files, integration_hooks,
};

use crate::Provider;
use crate::composition::Context;
use crate::runtime::audit::{Audit, AuditMeta, AuditSubject, Audited, Line, guarded};
use crate::runtime::errors::CommandError;
use std::path::Path;

/// An `init` command.
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
}

impl Command<'_> {
    /// What the audit log records for this command, or `None` when it is
    /// not audited: a read writes nothing, not even an audit line (STD-01
    /// R31). Exhaustive, so a new command cannot compile without deciding
    /// (STD-02 R27).
    #[must_use]
    pub fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::Init { .. } => Some(AuditMeta {
                command: "init",
                stepwise: false,
            }),
        }
    }

    /// The record and agent this command names: none, since `init` names
    /// its own in its audit line.
    #[must_use]
    pub fn subject(&self, _ctx: &Context) -> AuditSubject {
        AuditSubject::default()
    }
}

/// What an `init` command returns.
pub enum Outcome {
    /// What `init` installed.
    Initialized(Box<InitReport>),
}

/// `init` writes its line only when it changed a file, naming the provider.
impl Audited for Outcome {
    fn lines(&self) -> Vec<Line> {
        match self {
            Self::Initialized(report) => {
                Line::written(Some(report.provider.to_string()), report.changed)
            }
        }
    }
}

/// Why an `init` command did not run.
pub type Error = crate::Error;

/// Run `command` inside the audit guard when [`Command::audit`] declares
/// one. `init` first checks the selected runtime is on `PATH` and resolves
/// the running executable, which its hooks point at. Init is the top of the
/// app order, so no app calls it and it has no `run` of its own.
///
/// # Errors
///
/// A refused or failed install, a runtime missing from `PATH`, or an
/// executable path that cannot be read or is not UTF-8.
pub fn exec(ctx: &Context, command: Command<'_>) -> Result<Outcome, CommandError> {
    let audit = Audit::declared(command.audit(), || command.subject(ctx));
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
                Outcome::Initialized(Box::new(install::init_project_with_skill_files(
                    target,
                    provider,
                    &binary,
                    skill,
                    skill_files,
                )?))
            }
        })
    })
}
