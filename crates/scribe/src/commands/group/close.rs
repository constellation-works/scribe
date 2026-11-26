use super::outcome::output;
use crate::{error::CliError, output::Output};
use clap::Args;
use scribe_core::apps::group::{Command, GroupCloseInput, exec};
use scribe_core::composition::Context;

pub(super) const CLOSE_EXAMPLES: &str = "Draft with `scribe group evidence <id> --json`, then use --summary and --if-evidence, or run `orbit job run scribe-group-close`.\nExample: scribe group close <id> --summary 'Review complete'";

/// Arguments to `group close`.
#[derive(Debug, Args)]
pub(crate) struct CloseArgs {
    /// Group id, unique prefix, or project group slug
    id: String,
    /// Words the user typed in this session that contain the whole group id (g-1
    /// does not match g-12); closes the group for a session that holds no seat in it
    #[arg(long)]
    quote: Option<String>,
    /// Summary text, or - to read stdin
    #[arg(long, conflicts_with = "no_summary")]
    summary: Option<String>,
    /// Close without a summary
    #[arg(long)]
    no_summary: bool,
    /// Refuse if the evidence has changed
    #[arg(long)]
    if_evidence: Option<String>,
}

impl CloseArgs {
    /// Whether neither `--summary` nor `--no-summary` was given.
    pub(super) fn lacks_summary(&self) -> bool {
        self.summary.is_none() && !self.no_summary
    }
}

pub(super) fn run(args: CloseArgs, ctx: &Context) -> Result<Output, CliError> {
    let command = Command::Close(GroupCloseInput {
        session: ctx.caller(),
        id: args.id,
        quote: args.quote,
        provider: ctx.provider(),
        summary: match args.summary {
            Some(s) if s == "-" => {
                use std::io::Read;
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|source| CliError::Read {
                        what: "stdin".into(),
                        source,
                    })?;
                Some(text)
            }
            other => other,
        },
        if_evidence: args.if_evidence,
        notes: ctx,
    });
    Ok(output(exec(ctx, command)?))
}
