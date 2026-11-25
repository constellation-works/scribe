//! Arguments and JSON I/O for the session app's deterministic pass commands.

use super::passed;
use crate::error::CliError;
use crate::output::Output;
use clap::Args;
use scribe_core::SessionId;
use scribe_core::apps::session::{
    ApplyInput, Command, FailInput, Outcome, PrepareInput, SessionPassInput, exec,
};
use scribe_core::composition::Context;
use serde_json::Value;
use std::io::Read;

/// Redact, flatten and bound a failure reason, as a pass report has it.
#[cfg(test)]
pub(super) use scribe_core::apps::session::short_reason;

pub(super) const PASS_EXAMPLES: &str =
    "The scribe-session-pass Orbit routine ships disabled. Once enabled on a
configured host it runs every five minutes. Run the job by hand to distill
new windows. It takes up to five due sessions per run and checkpoints each
window before reading the next. Named sessions bypass retry backoff.

Examples:
  scribe session pass
  scribe session pass --session <id> --json
  orbit job run scribe-session-pass
  orbit job run scribe-session-pass --input 'sessions=[\"<id>\"]'";

/// Arguments to session pass.
#[derive(Debug, Args)]
pub(crate) struct PassArgs {
    /// Settle this switched-on session, even during backoff; accepts a unique prefix of at least four characters and can be repeated
    #[arg(long = "session", value_name = "ID")]
    pub(crate) sessions: Vec<SessionId>,
}

pub(super) fn run(args: PassArgs, ctx: &Context) -> Result<Output, CliError> {
    let input = SessionPassInput {
        sessions: args.sessions,
        resolve: true,
        ..Default::default()
    };
    Ok(passed(exec(ctx, Command::Pass(input, ctx.host()))?, ctx))
}

/// Parsed by both the CLI and the plugin backend before accessing a store.
pub(in crate::commands) enum Step {
    Prepare(PrepareInput),
    Apply(Box<ApplyInput>),
    Fail(Box<FailInput>),
}

impl Step {
    pub(in crate::commands) fn parse(name: &str, input: Value) -> Result<Self, CliError> {
        let invalid = |err| CliError::Usage(format!("{name} input: {err}"));
        match name {
            "session-pass-prepare" => Ok(Self::Prepare(
                serde_json::from_value(input).map_err(invalid)?,
            )),
            "session-pass-apply" => Ok(Self::Apply(Box::new(
                serde_json::from_value(input).map_err(invalid)?,
            ))),
            "session-pass-fail" => Ok(Self::Fail(Box::new(
                serde_json::from_value(input).map_err(invalid)?,
            ))),
            _ => Err(CliError::Usage(format!("unknown pass step {name}"))),
        }
    }

    pub(in crate::commands) fn run(self, ctx: &Context) -> Result<Value, CliError> {
        let host = ctx.host();
        let command = match self {
            Self::Prepare(input) => Command::PassPrepare(input, host),
            Self::Apply(input) => Command::PassApply(input, host),
            Self::Fail(input) => Command::PassFail(input, host),
        };
        let value = match exec(ctx, command)? {
            Outcome::PassPrepared(prepared) => serde_json::to_value(prepared),
            Outcome::PassApplied(applied) => serde_json::to_value(applied),
            Outcome::PassFailed(pass) => Ok(serde_json::json!({"pass": pass})),
            Outcome::Listed(_)
            | Outcome::Shown(_)
            | Outcome::Passed(_)
            | Outcome::Activity(_)
            | Outcome::Transcripts(_)
            | Outcome::Transcript(_) => {
                unreachable!("pass step returns its corresponding outcome")
            }
        };
        value.map_err(|err| CliError::Output(err.into()))
    }
}

pub(super) fn step_stdin(name: &str, ctx: &Context) -> Result<Output, CliError> {
    let mut raw = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|source| CliError::Read {
            what: "stdin".into(),
            source,
        })?;
    if raw.len() > 1024 * 1024 {
        return Err(scribe_core::CommandError::Usage("pass input exceeds 1 MiB".into()).into());
    }
    let input = if raw.is_empty() && name == "session-pass-prepare" {
        serde_json::json!({})
    } else {
        serde_json::from_slice(&raw)
            .map_err(|err| scribe_core::CommandError::Usage(format!("{name} input: {err}")))?
    };
    Step::parse(name, input)?.run(ctx).map(Output::PassStep)
}
