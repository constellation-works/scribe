//! Install the binary-embedded generic skill and selected provider hooks.

use super::admin;
use crate::{Context, error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::admin::{Command, exec};
use std::path::PathBuf;

const SKILL: &str = include_str!("../../../../skills/scribe/SKILL.md");

macro_rules! skill_files {
    ($($path:literal),+ $(,)?) => {
        &[$(($path, include_str!(concat!("../../../../skills/scribe/", $path)))),+]
    };
}

const SKILL_FILES: &[(&str, &str)] = skill_files![
    "evals/cases.md",
    "references/action-tags.md",
    "references/capture-controls.md",
    "references/catchers.md",
    "references/handoffs.md",
    "references/lookups.md",
    "references/memory.md",
    "references/project-init.md",
    "references/session-groups.md",
    "references/session-refresh.md",
    "references/settlement.md",
    "references/store-maintenance.md",
];

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Existing project directory to initialize; unrelated configuration is preserved
    #[arg(long, value_name = "DIR")]
    target: PathBuf,
}

pub(super) fn run(args: InitArgs, ctx: &Context) -> Result<Output, CliError> {
    ctx.require_runtime()?;
    let binary = std::env::current_exe().map_err(|source| CliError::Read {
        what: String::from("installed scribe executable"),
        source,
    })?;
    if binary.to_str().is_none() {
        return Err(CliError::Usage(String::from(
            "scribe executable path is not UTF-8; install it at a UTF-8 path before initializing hooks",
        )));
    }
    let command = Command::Init {
        target: &args.target,
        provider: ctx.provider(),
        binary: &binary,
        skill: SKILL,
        skill_files: SKILL_FILES,
    };
    Ok(admin::output(exec(&ctx.runtime(), command)?))
}
