//! Terminal detection, sink selection, logging setup, parse-error handling,
//! stderr notices/errors and command completion.
//!
//! This is the only module that writes directly to stdout or stderr or probes
//! terminal state (STD-01 R12, R13, R17; STD-02 R15).

use super::printable::Printable;
use super::record::WarningPayload;
use super::usage::raw_args_request_json;
use crate::error::CliError;
use cw_app::Env;
use cw_cli::{Format, Mode, Probe, Terminal};
use scribe_core::ShortIds;
use scribe_core::composition::{APP, LOG};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::ExitCode;

pub(crate) use cw_cli::Sink;

/// How a command's output chooses its sink.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SinkStyle {
    /// Always compact JSON on stdout regardless of `--format` (hook, plugin).
    MachineCompactJson,
    /// Always pretty JSON on stdout regardless of `--format` (group evidence, pass step).
    MachinePrettyJson,
    /// User's requested mode; compact JSON in JSON mode (group backfill).
    Compact,
    /// User's requested mode; pretty JSON in JSON mode (standard commands).
    Pretty,
}

/// An output payload that can be rendered through a [`Console`] sink.
pub(crate) trait RenderOutput {
    /// Which sink style this output requires.
    fn sink_style(&self) -> SinkStyle;

    /// Write notices, warnings and the payload to `sink`.
    fn emit<O: Write, E: Write>(
        &self,
        sink: &mut Sink<O, E>,
        shorts: &ShortIds,
    ) -> cw_cli::Result<()>;

    /// Post-emission check when stdout stayed open (for example, a failed pass).
    fn after_emit(&self) -> Result<(), CliError> {
        Ok(())
    }
}

/// The one terminal decision of an invocation, and the sinks built from it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Console {
    /// How the user asked for the payload.
    terminal: Terminal,
    /// The decision for output a program reads, whatever the user asked:
    /// the same streams, always JSON.
    json: Terminal,
}

impl Console {
    /// Decide once, from the captured environment, the streams and the
    /// `--format` the command line asked for.
    pub(crate) fn detect(env: &Env, requested: Option<Format>) -> Self {
        let probe = Probe::detect();
        Self {
            terminal: Terminal::decide(env, APP, probe, requested),
            json: Terminal::decide(env, APP, probe, Some(Format::Json)),
        }
    }

    /// Whether rendering needs session display names; JSON keeps provider ids.
    pub(crate) fn shortens_ids(&self) -> bool {
        self.terminal.mode() != Mode::Json
    }

    /// The sink for `style`. A document the user reads in JSON is indented, as
    /// it always was; a hook reply, a plugin reply and a group backfill stay
    /// compact, and the machine replies are JSON in every mode.
    fn sink(&self, style: SinkStyle) -> Sink {
        match style {
            SinkStyle::MachineCompactJson => Sink::stdio(self.json),
            SinkStyle::MachinePrettyJson => Sink::stdio(self.json).pretty_json(),
            SinkStyle::Compact => Sink::stdio(self.terminal),
            SinkStyle::Pretty => Sink::stdio(self.terminal).pretty_json(),
        }
    }
}

#[derive(Serialize)]
struct ErrorPayload<'a> {
    error: &'a str,
    code: &'a str,
}

/// Columns the root help may wrap command rows into, decided from the
/// captured process environment `terminal`.
///
/// `None` when stdout is not a terminal or no width is known, so each
/// command stays on one line. Piped `--help`, which is how the golden is
/// captured, therefore does not depend on the machine (STD-01 R17, R24).
pub(crate) fn help_width(terminal: &Env) -> Option<usize> {
    Terminal::detect(terminal, APP, None).width()
}

/// Send `tracing` output to stderr as plain text, filtered by `log`: the
/// value of `SCRIBE_LOG` that composition captured (`None` when unset).
///
/// Fail open (STD-02 R31): logging is a side channel, so a filter that does
/// not parse never fails the command. It is reported once and replaced by
/// the default, not silently ignored.
pub(crate) fn init_logging(log: Option<&OsStr>) {
    use tracing_subscriber::EnvFilter;
    let log_name = LOG.name_for(APP);
    let default = || EnvFilter::new("warn");
    let filter = match log.map(OsStr::to_str) {
        Some(Some(spec)) => EnvFilter::try_new(spec).unwrap_or_else(|e| {
            let _ = writeln!(
                stderr_lock(),
                "warning: ignoring {log_name}={spec:?} ({e}); logging at warn"
            );
            default()
        }),
        None => default(),
        Some(None) => {
            let _ = writeln!(
                stderr_lock(),
                "warning: ignoring {log_name}: not valid UTF-8; logging at warn"
            );
            default()
        }
    };
    // Ignore the error: it only means a subscriber is already installed.
    let _ = tracing_subscriber::fmt()
        .with_writer(stderr_handle)
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init();
}

/// Render a command's outcome and return the process exit code. On writable
/// stdout a session pass report with a failed pass is printed, then reported
/// as an error naming each failed session, its code and its redacted reason
/// (exit 1, STD-01 R20); on a closed stdout the report ends silently with
/// exit 0 and no aggregate pass error (STD-01 R13).
pub(crate) fn finish<T: RenderOutput>(
    console: &Console,
    outcome: Result<(T, ShortIds), CliError>,
) -> ExitCode {
    let result = outcome.and_then(|(output, shorts)| {
        let mut sink = console.sink(output.sink_style());
        output
            .emit(&mut sink, &shorts)
            .map_err(|e| CliError::Output(io_error(e)))?;
        // A closed stdout is success; any other failed write is reported now
        // that the command is done. EPIPE also suppresses a later pass error.
        sink.finish().map_err(|e| CliError::Output(io_error(e)))?;
        if sink.is_closed() {
            return Ok(());
        }
        output.after_emit()
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            report_error(console.terminal.mode() == Mode::Json, &err);
            ExitCode::from(err.exit_code())
        }
    }
}

/// Handle an argument-parsing outcome that is not a parsed command line:
/// `--help` and `--version` (stdout, exit 0) or a usage error (stderr, exit 2).
pub(crate) fn finish_parse_error(err: &clap::Error, args: &[OsString], terminal: &Env) -> ExitCode {
    let code = u8::try_from(err.exit_code()).unwrap_or(2);
    if !err.use_stderr() {
        let written = stdout_lock().write_all(err.render().to_string().as_bytes());
        return match written {
            Err(e) if e.kind() != io::ErrorKind::BrokenPipe => ExitCode::from(1),
            _ => ExitCode::from(code),
        };
    }
    let env_format = terminal.app(APP).format().and_then(|value| value.to_str());
    let rendered = err.render().to_string();
    if raw_args_request_json(args, env_format) {
        write_json_error(&usage_message(&rendered), "usage_error");
    } else {
        let _ = stderr_lock().write_all(rendered.as_bytes());
    }
    ExitCode::from(code)
}

/// Several lines of stored text on stderr, one escaped line per line break.
pub(crate) fn notice<O: Write, E: Write>(sink: &mut Sink<O, E>, text: &str) {
    sink.notice_lines(text.split('\n'));
}

/// Emit a list's skipped-file warnings and notices to stderr.
pub(crate) fn emit_list_diagnostics<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    warnings: &[WarningPayload],
    notices: &[String],
) {
    for warning in warnings {
        sink.warn(&format!("skipped {}", warning.error));
    }
    for text in notices {
        notice(sink, text);
    }
}

/// A failure of the sink as an `io::Error`, the form the binary's error wraps.
fn io_error(error: cw_cli::Error) -> io::Error {
    match error {
        cw_cli::Error::Output(error) => error,
        cw_cli::Error::Encode(error) => io::Error::from(error),
        error @ cw_cli::Error::Summaries { .. } => io::Error::other(error),
    }
}

/// The message of a rendered clap error: everything before its `Usage:`
/// block, without the `error: ` prefix, as one line. clap puts the detail
/// (the missing argument, the rejected value) on the lines after the first.
fn usage_message(rendered: &str) -> String {
    let detail: Vec<&str> = rendered
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .map(str::trim)
        .collect();
    let joined = detail.join(" ");
    joined.strip_prefix("error: ").unwrap_or(&joined).to_owned()
}

/// Report an error on stderr: one JSON object in JSON mode, else one
/// `error: <message>` line with the cause chain folded in (STD-01 R19).
fn report_error(json: bool, err: &CliError) {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    if let Some(hint) = err.hint() {
        message.push_str("; ");
        message.push_str(hint);
    }
    if json {
        write_json_error(&message, err.code());
    } else {
        let _ = writeln!(stderr_lock(), "error: {message}");
    }
}

fn write_json_error(message: &str, code: &str) {
    let payload = ErrorPayload {
        error: message,
        code,
    };
    let mut err = Printable::new(stderr_lock());
    if serde_json::to_writer(&mut err, &payload).is_ok() {
        let _ = writeln!(err);
    }
}

/// The locked standard output stream.
#[expect(
    clippy::disallowed_methods,
    reason = "console.rs is the CLI output boundary for stdout (STD-02 R15)"
)]
fn stdout_lock() -> io::StdoutLock<'static> {
    io::stdout().lock()
}

/// The locked standard error stream.
#[expect(
    clippy::disallowed_methods,
    reason = "console.rs is the CLI output boundary for stderr (STD-02 R15)"
)]
fn stderr_lock() -> io::StderrLock<'static> {
    io::stderr().lock()
}

/// The standard error handle the log subscriber writes through.
#[expect(
    clippy::disallowed_methods,
    reason = "console.rs is the CLI log-subscriber boundary for stderr (STD-02 R15)"
)]
fn stderr_handle() -> io::Stderr {
    io::stderr()
}
