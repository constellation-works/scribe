//! Install the binary-embedded generic skill and selected provider hooks.

use crate::{app::Context, error::CliError, output::Output};
use clap::Args;
use std::path::PathBuf;

const SKILL: &str = include_str!("../../../../skills/scribe/SKILL.md");

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
    let report = scribe_core::init_project(&args.target, ctx.provider(), &binary, SKILL)?;
    Ok(Output::Init(report))
}
