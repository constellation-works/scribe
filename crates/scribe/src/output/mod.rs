//! The output layer: the only code that writes to stdout or stderr.
//!
//! stdout carries the payload and nothing else; notices, errors and logs go
//! to stderr in every mode (STD-01 R12). Writes go through `writeln!` on a
//! locked handle rather than `println!`, so a closed pipe comes back as an
//! `io::Error` that [`finish`] turns into a silent exit 0 (STD-01 R13)
//! instead of a panic.

mod color;
mod payload;
mod popup;
mod printable;
mod render;
mod sink;
mod table;

pub(crate) use payload::{
    AskPayload, BatchFailure, BriefPayload, CoveredPayload, DeactivatedPayload,
    DeactivationFailure, EntryPayload, ExportPayload, GroupBackfill, GroupBackfillMember,
    HitPayload, HookOutcome, HookReply, Kin, MemoryBlockPayload, MemoryPayload, MemoryRulesPayload,
    MemoryScanPayload, MigratePayload, Output, PassPayload, PassReport, PluginError, PluginReply,
    PopupRows, ProposalPayload, ReappliedPayload, Record, Records, RefreshPayload, RevisionPayload,
    ScanReport, SessionHandoffsPayload, SessionPayload, SessionsContext, TagPayload,
    TagShowPayload, TranscriptPayload, TreeRow, WarningPayload,
};
pub(crate) use sink::{FORMAT_ENV, FormatArg, Sink, help_width};

use crate::error::CliError;
use scribe_core::ShortIds;
use serde::Serialize;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;

/// Environment variable holding the log filter (`warn` when unset).
const LOG_ENV: &str = "SCRIBE_LOG";

/// Send `tracing` output to stderr as plain text, filtered by `SCRIBE_LOG`.
///
/// Fail open (STD-02 R31): logging is a side channel, so a filter that does
/// not parse never fails the command. It is reported once and replaced by
/// the default, not silently ignored.
pub(crate) fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let default = || EnvFilter::new("warn");
    let filter = match std::env::var(LOG_ENV) {
        Ok(spec) => EnvFilter::try_new(&spec).unwrap_or_else(|e| {
            let _ = writeln!(
                io::stderr().lock(),
                "warning: ignoring {LOG_ENV}={spec:?} ({e}); logging at warn"
            );
            default()
        }),
        Err(std::env::VarError::NotPresent) => default(),
        Err(std::env::VarError::NotUnicode(_)) => {
            let _ = writeln!(
                io::stderr().lock(),
                "warning: ignoring {LOG_ENV}: not valid UTF-8; logging at warn"
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

/// Render a command's outcome and return the process exit code. A session
/// pass report with a failed pass is printed, then reported as an error
/// naming each failed session, its code and its redacted reason (exit 1,
/// STD-01 R20).
pub(crate) fn finish(sink: &Sink, outcome: Result<(Output, ShortIds), CliError>) -> ExitCode {
    let result = outcome.and_then(|(output, shorts)| {
        emit(sink, &shorts, &output).map_err(CliError::Output)?;
        match &output {
            Output::Pass(report) => report.failure().map_or(Ok(()), Err),
            Output::Init(_)
            | Output::List { .. }
            | Output::Show { .. }
            | Output::Written { .. }
            | Output::GroupBackfill(_)
            | Output::Hook(_)
            | Output::Scan(_)
            | Output::Ask(_)
            | Output::Plugin(_)
            | Output::Popup
            | Output::PopupRows(_)
            | Output::PopupPreview(_) => Ok(()),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Output(e)) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(err) => {
            report_error(sink.mode == sink::OutputMode::Json, &err);
            ExitCode::from(err.exit_code())
        }
    }
}

/// Handle an argument-parsing outcome that is not a parsed command line:
/// `--help` and `--version` (stdout, exit 0) or a usage error (stderr, exit 2).
pub(crate) fn finish_parse_error(err: &clap::Error, args: &[OsString]) -> ExitCode {
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
    let env_format = std::env::var(FORMAT_ENV).ok();
    let rendered = err.render().to_string();
    if sink::raw_args_request_json(args, env_format.as_deref()) {
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

/// Stderr for notices and warnings, which can quote stored text: control
/// characters print as U+FFFD (STD-01 R9).
fn stderr() -> printable::Printable<io::StderrLock<'static>> {
    printable::Printable::new(io::stderr().lock(), false)
}

fn emit(sink: &Sink, shorts: &ShortIds, output: &Output) -> io::Result<()> {
    if let Output::List {
        warnings, notices, ..
    } = output
    {
        // Fail open (STD-02 R31): a notice that cannot reach stderr never
        // fails a command whose payload is fine.
        let mut err = stderr();
        for warning in warnings {
            let _ = writeln!(err, "warning: skipped {}", warning.error);
        }
        for notice in notices {
            let _ = writeln!(err, "{notice}");
        }
    }
    if let Output::Ask(payload) = output {
        let mut err = stderr();
        for warning in &payload.warnings {
            let _ = writeln!(err, "warning: skipped {}", warning.error);
        }
    }
    if let Output::Pass(report) = output {
        let mut err = stderr();
        for pass in &report.passes {
            for warning in &pass.warnings {
                let _ = writeln!(err, "warning: session {}: {warning}", pass.session);
            }
        }
        for notice in &report.notices {
            let _ = writeln!(err, "{notice}");
        }
    }
    if let Output::Scan(report) = output {
        let mut err = stderr();
        for notice in &report.notices {
            let _ = writeln!(err, "{notice}");
        }
    }
    if let Output::Show {
        record: Record::MemoryBlock(block),
    } = output
    {
        let mut err = stderr();
        for warning in &block.warnings {
            let _ = writeln!(err, "{warning}");
        }
    }
    if let Output::Show {
        record: Record::Brief(brief),
    } = output
    {
        let mut err = stderr();
        for warning in &brief.warnings {
            let _ = writeln!(err, "{warning}");
        }
    }
    if let Output::Written { notices, .. } = output {
        let mut err = stderr();
        for notice in notices {
            let _ = writeln!(err, "{notice}");
        }
    }
    let stdout = io::stdout().lock();
    let color = match output {
        // Machine output: the encoder escapes control characters itself,
        // and quotes stay verbatim.
        Output::Hook(_) | Output::Plugin(_) => None,
        Output::PopupRows(rows) => Some(rows.color),
        Output::Init(_)
        | Output::List { .. }
        | Output::Show { .. }
        | Output::Written { .. }
        | Output::Pass(_)
        | Output::GroupBackfill(_)
        | Output::Scan(_)
        | Output::Ask(_)
        | Output::Popup
        | Output::PopupPreview(_) => Some(sink.color),
    };
    let mut out: Box<dyn Write> = match color {
        Some(color) if sink.mode != sink::OutputMode::Json => {
            Box::new(printable::Printable::new(stdout, color))
        }
        Some(_) | None => Box::new(stdout),
    };
    match output {
        Output::Init(report) => render::init(&mut out, sink, report)?,
        Output::List {
            records,
            total,
            warnings,
            message_matches,
            ..
        } => render::list(
            &mut out,
            sink,
            shorts,
            records,
            *total,
            warnings,
            *message_matches,
        )?,
        Output::Show { record } | Output::Written { record, .. } => {
            render::show(&mut out, sink, shorts, record)?;
        }
        Output::GroupBackfill(report) => render::group_backfill(&mut out, sink, report)?,
        Output::Hook(reply) => render::hook(&mut out, reply)?,
        Output::Pass(report) => render::pass(&mut out, sink, shorts, report)?,
        Output::Scan(report) => render::scan(&mut out, sink, report)?,
        Output::Ask(payload) => render::ask(&mut out, sink, payload)?,
        Output::Plugin(reply) => render::plugin(&mut out, reply)?,
        Output::Popup => {}
        Output::PopupRows(rows) => {
            let color = rows.color && sink::resolve_color(true, &sink::SinkEnv::from_process());
            popup::rows(&mut out, rows, color)?;
        }
        Output::PopupPreview(record) => popup::preview(&mut out, record)?,
    }
    out.flush()
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

#[derive(Serialize)]
struct ErrorPayload<'a> {
    error: &'a str,
    code: &'a str,
}

fn write_json_error(message: &str, code: &str) {
    let payload = ErrorPayload {
        error: message,
        code,
    };
    let mut err = stderr();
    if serde_json::to_writer(&mut err, &payload).is_ok() {
        let _ = writeln!(err);
    }
}

#[cfg(test)]
mod tests;
