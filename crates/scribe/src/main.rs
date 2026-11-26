//! `scribe`: the user's recorded preferences, and the records of
//! the sessions they cataloged.
//!
//! `main` is wiring only: capture the environment once (composition), parse,
//! resolve the terminal decision once, resolve the context, run the command
//! (audited in `commands::run`), render. See ARCHITECTURE.md for the layers and
//! docs/design/scribe/ for the design.

// Nothing in this crate uses `print!`: the output module writes to locked
// handles so a closed pipe is an error value, not a panic (STD-01 R13).
#![deny(clippy::print_stderr, clippy::print_stdout)]
// Unit tests use unwrap/expect for fixture setup; production call sites remain linted.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

mod cli;
mod commands;
mod error;
mod help;
mod output;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    // The one read of the process environment, in composition (STD-02 R3, R7).
    let env = scribe_core::composition::Env::from_process();
    let cli = match cli::Cli::parse_args(&args, output::help_width(&env.terminal)) {
        Ok(cli) => cli,
        Err(err) => return output::finish_parse_error(&err, &args, &env.terminal),
    };
    let console = output::Console::detect(&env.terminal, cli.global.requested_format());
    output::init_logging(env.log.as_deref());
    let outcome = scribe_core::composition::Context::resolve(cli.global.root.as_deref(), &env)
        .map_err(error::CliError::from)
        .and_then(|ctx| {
            let ctx = ctx.with_provider(cli.global.provider.unwrap_or_default());
            let output = commands::run(cli.command, &ctx)?;
            let shorts = if console.shortens_ids() {
                commands::short_ids(&output, &ctx)
            } else {
                scribe_core::ShortIds::default()
            };
            Ok((output, shorts))
        });
    output::finish(&console, outcome)
}

#[cfg(test)]
mod tests;
