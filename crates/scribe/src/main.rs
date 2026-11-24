//! `scribe`: the user's recorded preferences, and the records of
//! the sessions they cataloged.
//!
//! `main` is wiring only: parse, resolve the output sink once, resolve the
//! context, run the command (audited in `commands::run`), render. See
//! ARCHITECTURE.md for the layers and docs/design/scribe/ for the design.

// Nothing in this crate uses `print!`: the output module writes to locked
// handles so a closed pipe is an error value, not a panic (STD-01 R13).
#![deny(clippy::print_stderr, clippy::print_stdout)]
// Unit tests use unwrap/expect for fixture setup; production call sites remain linted.
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

mod audit_middleware;
mod cli;
mod commands;
mod error;
mod help;
mod output;

use std::process::ExitCode;

/// The resolved context every command runs against: composition's, with
/// this binary's error as its ports' error until the core maps errors
/// itself (ORB-27921 step 3, PR 4).

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let cli = match cli::Cli::parse_args(&args) {
        Ok(cli) => cli,
        Err(err) => return output::finish_parse_error(&err, &args),
    };
    let sink = output::Sink::from_process(cli.global.requested_format());
    output::init_logging();
    let outcome = scribe_core::composition::Context::resolve(
        cli.global.root.as_deref(),
        &scribe_core::composition::Env::from_process(),
    )
    .map_err(error::CliError::from)
    .and_then(|ctx| {
        let ctx = ctx.with_provider(cli.global.provider.unwrap_or_default());
        let output = commands::run(cli.command, &ctx)?;
        let shorts = if sink.shortens_ids() {
            commands::short_ids(&output, &ctx)
        } else {
            scribe_core::ShortIds::default()
        };
        Ok((output, shorts))
    });
    output::finish(&sink, outcome)
}

#[cfg(test)]
mod tests;
