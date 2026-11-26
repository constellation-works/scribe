//! The output layer: the only code that writes to stdout or stderr.
//!
//! The sink is the kit's (`cw-cli`): stdout carries the payload and nothing
//! else, in a table, tab-separated lines or JSON; notices, warnings and errors
//! go to stderr in every mode (STD-01 R12). A closed stdout latches the sink
//! shut and the command runs to its end. After EPIPE, [`finish`] exits
//! silently with 0, including when a session pass report failed (STD-01 R13).
//! The renderers here only build what the sink writes: a table, a detail view,
//! or the payload's JSON.
//!
//! The process environment is captured by composition and passed in; this
//! module reads none of it (STD-02 R3, R7).

mod color;
mod payload;
mod printable;
mod render;
mod usage;

pub(crate) use cw_cli::Sink;
#[cfg(test)]
pub(crate) use payload::PassPayload;
pub(crate) use payload::{
    BriefPayload, CoveredPayload, DeactivatedPayload, DeactivationFailure, EntryPayload,
    ExportPayload, GroupBackfill, GroupBackfillMember, HitPayload, Kin, MemoryBlockPayload,
    MemoryPayload, MemoryRulesPayload, MemoryScanPayload, MigratePayload, Output, PassFailure,
    PluginError, PluginReply, ProposalPayload, ReappliedPayload, Record, Records, RefreshPayload,
    RefreshRetryPayload, RevisionPayload, ScanReport, SessionHandoffsPayload, SessionPayload,
    TagPayload, TagShowPayload, TranscriptPayload, TreeRow, WarningPayload,
};

use crate::error::CliError;
use cw_app::Env;
use cw_cli::{Format, Mode, Probe, Terminal};
use scribe_core::ShortIds;
use scribe_core::composition::{APP, LOG};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::ExitCode;

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

    /// The sink `output` is written through. A document the user reads in
    /// JSON is indented, as it always was; a hook reply, a plugin reply and
    /// a group backfill stay compact, and the machine replies are JSON in
    /// every mode.
    fn sink(&self, output: &Output) -> Sink {
        match output {
            Output::Hook(_) | Output::Plugin(_) => Sink::stdio(self.json),
            Output::GroupEvidence(_) | Output::PassStep(_) => Sink::stdio(self.json).pretty_json(),
            Output::GroupBackfill(_) => Sink::stdio(self.terminal),
            Output::Init(_)
            | Output::List { .. }
            | Output::Show { .. }
            | Output::Written { .. }
            | Output::Pass(_)
            | Output::Scan(_) => Sink::stdio(self.terminal).pretty_json(),
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

/// A failure of the sink as an `io::Error`, the form the binary's error wraps.
fn io_error(error: cw_cli::Error) -> io::Error {
    match error {
        cw_cli::Error::Output(error) => error,
        cw_cli::Error::Encode(error) => io::Error::from(error),
        error @ cw_cli::Error::Summaries { .. } => io::Error::other(error),
    }
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
                io::stderr().lock(),
                "warning: ignoring {log_name}={spec:?} ({e}); logging at warn"
            );
            default()
        }),
        None => default(),
        Some(None) => {
            let _ = writeln!(
                io::stderr().lock(),
                "warning: ignoring {log_name}: not valid UTF-8; logging at warn"
            );
            default()
        }
    };
    // Ignore the error: it only means a subscriber is already installed.
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
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
pub(crate) fn finish(console: &Console, outcome: Result<(Output, ShortIds), CliError>) -> ExitCode {
    let result = outcome.and_then(|(output, shorts)| {
        let mut sink = console.sink(&output);
        emit(&mut sink, &shorts, &output).map_err(|e| CliError::Output(io_error(e)))?;
        // A closed stdout is success; any other failed write is reported now
        // that the command is done. EPIPE also suppresses a later pass error.
        sink.finish().map_err(|e| CliError::Output(io_error(e)))?;
        if sink.is_closed() {
            return Ok(());
        }
        match &output {
            Output::Pass(report) => report.failure().map_or(Ok(()), Err),
            Output::Init(_)
            | Output::List { .. }
            | Output::Show { .. }
            | Output::Written { .. }
            | Output::PassStep(_)
            | Output::GroupEvidence(_)
            | Output::GroupBackfill(_)
            | Output::Hook(_)
            | Output::Scan(_)
            | Output::Plugin(_) => Ok(()),
        }
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
        let written = io::stdout()
            .lock()
            .write_all(err.render().to_string().as_bytes());
        return match written {
            Err(e) if e.kind() != io::ErrorKind::BrokenPipe => ExitCode::from(1),
            _ => ExitCode::from(code),
        };
    }
    let env_format = terminal.app(APP).format().and_then(|value| value.to_str());
    let rendered = err.render().to_string();
    if usage::raw_args_request_json(args, env_format) {
        write_json_error(&usage_message(&rendered), "usage_error");
    } else {
        let _ = io::stderr().lock().write_all(rendered.as_bytes());
    }
    ExitCode::from(code)
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

/// Several lines of stored text on stderr, one escaped line per line break.
fn notice<O: Write, E: Write>(sink: &mut Sink<O, E>, text: &str) {
    sink.notice_lines(text.split('\n'));
}

fn emit<O: Write, E: Write>(
    sink: &mut Sink<O, E>,
    shorts: &ShortIds,
    output: &Output,
) -> cw_cli::Result<()> {
    // Notices and warnings go first, and to stderr in every mode. The sink
    // never fails on them (STD-02 R31): one that cannot reach stderr never
    // fails a command whose payload is fine.
    match output {
        Output::List {
            warnings, notices, ..
        } => {
            for warning in warnings {
                sink.warn(&format!("skipped {}", warning.error));
            }
            for text in notices {
                notice(sink, text);
            }
        }
        Output::Pass(report) => {
            for pass in &report.passes {
                for warning in &pass.warnings {
                    sink.warn(&format!("session {}: {warning}", pass.session));
                }
            }
            for text in &report.notices {
                notice(sink, text);
            }
        }
        Output::Scan(report) => {
            for text in &report.notices {
                notice(sink, text);
            }
        }
        Output::Show {
            record: Record::MemoryBlock(block),
        } => {
            for warning in &block.warnings {
                notice(sink, warning);
            }
        }
        Output::Show {
            record: Record::Brief(brief),
        } => {
            for warning in &brief.warnings {
                notice(sink, warning);
            }
        }
        Output::Written { notices, .. } => {
            for text in notices {
                notice(sink, text);
            }
        }
        Output::Init(_)
        | Output::Show { .. }
        | Output::GroupEvidence(_)
        | Output::GroupBackfill(_)
        | Output::PassStep(_)
        | Output::Hook(_)
        | Output::Plugin(_) => {}
    }
    match output {
        Output::Init(report) => render::init(sink, report),
        Output::List {
            records,
            total,
            warnings,
            message_matches,
            ..
        } => render::list(sink, shorts, records, *total, warnings, *message_matches),
        Output::Show { record } | Output::Written { record, .. } => {
            render::show(sink, shorts, record)
        }
        // Machine output: the document is the payload, whatever the mode.
        Output::GroupEvidence(value) | Output::PassStep(value) => {
            sink.emit_text(value, |_| String::new())
        }
        Output::GroupBackfill(report) => render::group_backfill(sink, shorts, report),
        Output::Hook(reply) => render::hook(sink, reply),
        Output::Pass(report) => render::pass(sink, shorts, report),
        Output::Scan(report) => render::scan(sink, shorts, report),
        Output::Plugin(reply) => render::plugin(sink, reply),
    }
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
        let _ = writeln!(io::stderr().lock(), "error: {message}");
    }
}

fn write_json_error(message: &str, code: &str) {
    let payload = ErrorPayload {
        error: message,
        code,
    };
    let mut err = printable::Printable::new(io::stderr().lock());
    if serde_json::to_writer(&mut err, &payload).is_ok() {
        let _ = writeln!(err);
    }
}

#[cfg(test)]
mod tests;
