//! The `plugin` noun: `call`, the Orbit exec backend
//! (`plugin/.orbit-plugin/bin/scribe-plugin` execs it; 2_design.md §1).
//!
//! It reads Orbit's envelope on stdin —
//! `{"schema_version": 1, "tool": "scribe.<verb>", "input": {...},
//! "context": {"config": {"root": ...}, ...}}` — runs the tool, and prints
//! exactly one envelope line, `{"ok": true, "output": ...}` or
//! `{"ok": false, "error": {"code", "message", "retryable"}}`, exiting 0
//! either way (a non-zero exit is an execution error to the host).
//!
//! - `scribe.session-pass` runs `session pass` (the `scribe-session-pass`
//!   routine's job) and `scribe.consistency-scan` runs `consistency scan`
//!   (the `scribe-consistency` routine's), and `scribe.memory-refresh` runs
//!   `memory refresh` (the `scribe-memory-refresh` routine's), on the same code as the CLI
//!   verbs, with the same reports. Both are `agent_callable: false` and
//!   `mcp_scope: none` in `plugin.yaml`: only the routines reach them. So
//!   that a call ends inside Orbit's 300 s backend timeout, no session
//!   after the first starts once [`PASS_START_BY`] has passed; what is
//!   left is reported as remaining for the next run. The consistency scan
//!   calls no model and needs no such bound.
//!
//! The store root comes from `[plugins.scribe] root`, and the transcripts
//! directory from `[plugins.scribe] transcripts`, never from `SCRIBE_ROOT`
//! or `HOME`: the plugin's sandbox grants access to those paths alone. A
//! session-pass run with any failed pass is an error envelope,
//! `session_pass_failed`.
//! The routine tools write,
//! and are audited here, inside the call, under the same command names as
//! the CLI verbs (`session pass`, `consistency scan`) and in the store they
//! change — `plugin call` itself has no audit entry, because which store a
//! call writes is known only once the envelope is read (4_decisions.md).

use super::{RunLimits, consistency, session};
use crate::Context;
use crate::audit_middleware::{AuditGuard, AuditMeta};
use crate::error::CliError;
use crate::output::{Output, PassFailure, PassReport, PluginError, PluginReply, Record};
use clap::{Args, Subcommand};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long after a call starts `scribe.session-pass` may start another
/// session: a session may take two 121-second asks, and the call must end
/// inside Orbit's 300-second backend timeout.
const PASS_START_BY: Duration = Duration::from_secs(50);

pub(super) const PLUGIN_EXAMPLES: &str = "\
The Orbit plugin's backend runs this; it is not meant to be typed.

Examples:
  printf '%s' \"$envelope\" | scribe plugin call";

const CALL_EXAMPLES: &str = "\
Reads one Orbit exec envelope on stdin and prints one reply envelope on
stdout, exiting 0 whenever a reply was printed. The store is the plugin's
[plugins.scribe] root setting, an absolute path.

Examples:
  printf '%s' '{\"schema_version\":1,\"tool\":\"scribe.session-pass\",\"input\":{},\"context\":{\"config\":{\"root\":\"/home/me/.scribe\"}}}' | scribe plugin call";

/// The namespace the plugin's tools live in.
const NAMESPACE: &str = "scribe";
/// The most envelope bytes read from stdin.
const MAX_ENVELOPE: u64 = 1024 * 1024;
/// The envelope version this backend speaks.
const SCHEMA_VERSION: u64 = 1;

/// Verbs on the `plugin` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    /// Run one Orbit plugin tool call from stdin
    ///
    /// Reads an exec envelope on stdin and writes one reply envelope on stdout.
    #[command(after_help = CALL_EXAMPLES)]
    Call(CallArgs),
}

/// Arguments to `plugin call`: none (the envelope is on stdin).
#[derive(Debug, Args)]
pub(crate) struct CallArgs {}

impl PluginCommand {
    /// `None`: the routine tools audit
    /// themselves in the store their call names (see the module docs).
    pub(super) fn audit(&self) -> Option<crate::audit_middleware::AuditMeta> {
        match self {
            Self::Call(_) => None,
        }
    }
}

/// Run one `plugin` verb.
pub(super) fn run(command: PluginCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        PluginCommand::Call(CallArgs {}) => {
            let mut raw = Vec::new();
            std::io::stdin()
                .take(MAX_ENVELOPE)
                .read_to_end(&mut raw)
                .map_err(|source| CliError::Read {
                    what: String::from("stdin"),
                    source,
                })?;
            Ok(Output::Plugin(call(&raw, ctx)))
        }
    }
}

/// The envelope Orbit writes on stdin.
#[derive(Deserialize)]
struct Envelope {
    schema_version: u64,
    tool: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    context: EnvelopeContext,
}

#[derive(Default, Deserialize)]
struct EnvelopeContext {
    #[serde(default)]
    config: Value,
}

fn failure(code: &'static str, message: impl Into<String>, retryable: bool) -> PluginReply {
    PluginReply::Error(PluginError {
        code,
        message: message.into(),
        retryable,
    })
}

/// Run the call `raw` holds; every outcome is a reply.
fn call(raw: &[u8], ctx: &Context) -> PluginReply {
    let envelope: Envelope = match serde_json::from_slice(raw) {
        Ok(envelope) => envelope,
        Err(e) => {
            return failure(
                "invalid_envelope",
                format!("stdin is not an Orbit exec envelope: {e}"),
                false,
            );
        }
    };
    if envelope.schema_version != SCHEMA_VERSION {
        return failure(
            "invalid_envelope",
            format!(
                "envelope schema_version {} is not {SCHEMA_VERSION}",
                envelope.schema_version
            ),
            false,
        );
    }
    let verb = envelope
        .tool
        .strip_prefix(NAMESPACE)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or_default();
    match verb {
        "session-pass" => session_pass(&envelope.input, &envelope.context.config, ctx),
        "consistency-scan" => consistency_scan(&envelope.input, &envelope.context.config, ctx),
        "memory-refresh" => memory_refresh(&envelope.input, &envelope.context.config, ctx),
        _ => failure(
            "unknown_tool",
            format!("{} is not a scribe tool", envelope.tool),
            false,
        ),
    }
}

/// The `root` default in `plugin.yaml`, which means unset. Orbit grants
/// writes only to a directory that exists outside the install tree (it
/// resolves an empty or relative path against that tree), so the default
/// is a directory every host has, and no store is ever opened there.
const ROOT_UNSET: &str = "/var/tmp";
/// The `transcripts` default: empty (a read grant may name the install
/// tree).
const TRANSCRIPTS_UNSET: &str = "";

/// The absolute path the setting `key` names, unless it is empty, relative
/// or its `unset` default.
fn setting_path(
    config: &Value,
    key: &str,
    unset: &str,
    help: &str,
) -> Result<PathBuf, PluginReply> {
    let value = config.get(key).and_then(Value::as_str).unwrap_or_default();
    let path = PathBuf::from(value);
    if value.is_empty() || value == unset || !path.is_absolute() {
        return Err(failure(
            "not_configured",
            format!("set [plugins.scribe] {key} to {help}"),
            false,
        ));
    }
    Ok(path)
}

/// The store the plugin's `root` setting names.
fn store_root(config: &Value) -> Result<PathBuf, PluginReply> {
    setting_path(
        config,
        "root",
        ROOT_UNSET,
        "the scribe data directory, an absolute path such as /home/<user>/.scribe",
    )
}

/// The input of a routine tool: an empty object
/// (`schemas/empty.input.json`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutineInput {}

/// Check a routine tool's `input`: `{}` (or absent).
fn routine_input(tool: &str, input: &Value) -> Result<(), PluginReply> {
    if input.is_null() {
        return Ok(());
    }
    serde_json::from_value::<RoutineInput>(input.clone())
        .map(|RoutineInput {}| ())
        .map_err(|e| failure("invalid_input", format!("{tool} input: {e}"), false))
}

/// The transcripts directory the plugin's `transcripts` setting names.
fn transcripts_dir(config: &Value) -> Result<PathBuf, PluginReply> {
    setting_path(
        config,
        "transcripts",
        TRANSCRIPTS_UNSET,
        "Claude Code's projects directory, an absolute path such as /home/<user>/.claude/projects",
    )
}

/// Run a routine command against `routine_ctx`, audited there as `command`.
fn audited(
    routine_ctx: &Context,
    command: &'static str,
    run: impl FnOnce(&Context) -> Result<Output, CliError>,
) -> PluginReply {
    let mut audit = AuditGuard::start(
        routine_ctx.store(),
        AuditMeta {
            command,
            stepwise: true,
        },
        routine_ctx.now(),
    );
    let result = run(routine_ctx);
    audit.mark_result(&result);
    drop(audit);
    match result {
        Ok(Output::Pass(report)) => PluginReply::Pass(report),
        Ok(Output::Scan(report)) => PluginReply::Scan(report),
        Ok(Output::Written {
            record: Record::MemoryRefresh(report),
            ..
        }) => PluginReply::Refresh(report),
        Ok(_) => failure(
            "internal_error",
            format!("{command} returned no report"),
            false,
        ),
        Err(err) => {
            let retryable = matches!(err, CliError::Core(ref e) if e.code() == "store_locked");
            failure(err.code(), message_of(&err), retryable)
        }
    }
}

fn session_pass(input: &Value, config: &Value, _ctx: &Context) -> PluginReply {
    if let Err(reply) = routine_input("scribe.session-pass", input) {
        return reply;
    }
    let root = match store_root(config) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    let codex_home = match codex_home(config) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    let transcripts = match transcripts_dir(config) {
        Ok(path) => Some(path),
        Err(_)
            if codex_home.is_some()
                && config
                    .get("transcripts")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .is_empty() =>
        {
            None
        }
        Err(reply) => return reply,
    };
    let routine = Context::at(root, transcripts).with_codex_home(codex_home);
    let limits =
        RunLimits::new(scribe_core::pass::MAX_PER_RUN).start_by(Instant::now() + PASS_START_BY);
    let reply = audited(&routine, session::pass::COMMAND, |routine| {
        session::pass::run_with(routine, |config| routine.models(config), limits, &[])
    });
    pass_reply(reply)
}

/// The routine's reply for a session pass report: an error envelope when any
/// pass in it failed, as the CLI exits 1, so the tool result and the Orbit
/// run are not a success. The message names each failed session with its
/// code and redacted reason; the audit lines carry the codes only.
fn pass_reply(reply: PluginReply) -> PluginReply {
    let failed = match &reply {
        PluginReply::Pass(report) => failed_passes(report),
        PluginReply::Scan(_) | PluginReply::Refresh(_) | PluginReply::Error(_) => None,
    };
    failed.unwrap_or(reply)
}

/// The error for a pass report with a failed pass, `None` when none failed:
/// the CLI's message with each session's code and redacted reason. Not
/// retryable: each failed session stays due, and the next run retries it.
fn failed_passes(report: &PassReport) -> Option<PluginReply> {
    let err = report.failure()?;
    Some(failure(err.code(), err.to_string(), false))
}

/// Optional explicit provider state root shared by every plugin model path.
fn codex_home(config: &Value) -> Result<Option<PathBuf>, PluginReply> {
    match config
        .get("codex_home")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(_) => setting_path(
            config,
            "codex_home",
            "",
            "Codex's state directory, an absolute path such as /home/<user>/.codex",
        )
        .map(Some),
        None => Ok(None),
    }
}

fn consistency_scan(input: &Value, config: &Value, _ctx: &Context) -> PluginReply {
    if let Err(reply) = routine_input("scribe.consistency-scan", input) {
        return reply;
    }
    let root = match store_root(config) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    let codex_home = match codex_home(config) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    let routine = Context::at(root, None).with_codex_home(codex_home);
    audited(&routine, consistency::COMMAND, |routine| {
        consistency::scan(routine)
    })
}

fn memory_refresh(input: &Value, config: &Value, _ctx: &Context) -> PluginReply {
    if let Err(reply) = routine_input("scribe.memory-refresh", input) {
        return reply;
    }
    let root = match store_root(config) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    let codex_home = match codex_home(config) {
        Ok(path) => path,
        Err(reply) => return reply,
    };
    let routine = Context::at(root, None).with_codex_home(codex_home);
    audited(&routine, "memory refresh", |routine| {
        let config = routine.config()?;
        super::memory::refresh(routine, &routine.models(&config))
    })
}

/// `err` with its cause chain, as `report_error` folds it.
fn message_of(err: &CliError) -> String {
    let mut message = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}
