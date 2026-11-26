//! `scribe`: the user's recorded preferences, and the records of
//! the sessions they cataloged.
//!
//! `main` is wiring only: composition reads selected host, configuration and
//! log values individually and captures the full process environment once;
//! parsing probes help width from that snapshot; after parsing, the console
//! probe, requested format and same snapshot determine the output mode. It resolves the context, runs
//! the command (audited in `commands::run`) and renders. See ARCHITECTURE.md
//! for the layers and docs/design/scribe/ for the design.

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
    // Selected values are read individually; composition also captures one full
    // environment snapshot for help width and the post-parse console decision.
    let env = scribe_core::composition::Env::from_process();
    // Clap needs the captured terminal width before it can parse --help.
    let cli = match cli::Cli::parse_args(&args, output::help_width(&env.terminal)) {
        Ok(cli) => cli,
        Err(err) => return output::finish_parse_error(&err, &args, &env.terminal),
    };
    // The normal console decision combines the console probe, parsed request
    // and the same snapshot.
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
