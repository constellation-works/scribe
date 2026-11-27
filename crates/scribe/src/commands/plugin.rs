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
//! The session-pass, memory-refresh and group-close tools prepare and
//! apply Orbit activities. The consistency scan runs deterministically.
//!
//! The store root comes from `[plugins.scribe] root`, and the transcripts
//! directory from `[plugins.scribe] transcripts`, never from `SCRIBE_ROOT`
//! or `HOME`: the plugin's sandbox grants access to those paths.
//! The routine tools write,
//! and are audited inside the call (each app's exec runs the core's audit
//! guard with the routine's audit declared here), under the same command names as
//! the CLI verbs (`session pass`, `consistency scan`) and in the store they
//! change — `plugin call` itself has no audit entry, because which store a
//! call writes is known only once the envelope is read (4_decisions.md).

use super::{consistency, session};
use crate::error::CliError;
use crate::output::{Output, PluginError, PluginReply};
use clap::{Args, Subcommand};
use scribe_core::composition::Context;
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
pub(super) const PLUGIN_EXAMPLES: &str = "\
The Orbit plugin's backend runs this; it is not meant to be typed.

Examples:
  printf '%s' \"$envelope\" | scribe plugin call";

const CALL_EXAMPLES: &str = "\
Reads one Orbit exec envelope on stdin and prints one reply envelope on
stdout, exiting 0 whenever a reply was printed. The store is the plugin's
[plugins.scribe] root setting, an absolute path.

Examples:
  printf '%s' '{\"schema_version\":1,\"tool\":\"scribe.session-pass-prepare\",\"input\":{},\"context\":{\"config\":{\"root\":\"/home/me/.scribe\"}}}' | scribe plugin call";

/// The namespace the plugin's tools live in.
const NAMESPACE: &str = "scribe";
/// The most envelope bytes read from stdin.
const MAX_ENVELOPE: u64 = 1024 * 1024;
/// The envelope version this backend speaks.
const SCHEMA_VERSION: u64 = 1;

/// The `root` default in `plugin.yaml`, which means unset. Orbit grants
/// writes only to a directory that exists outside the install tree (it
/// resolves an empty or relative path against that tree), so the default
/// is a directory every host has, and no store is ever opened there.
const ROOT_UNSET: &str = "/var/tmp";
/// The `transcripts` default: empty (a read grant may name the install
/// tree).
const TRANSCRIPTS_UNSET: &str = "";

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupCloseRequest {
    group: String,
    session: scribe_core::SessionId,
    provider: scribe_core::Provider,
    #[serde(default)]
    quote: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    evidence_sha256: Option<String>,
}

/// The input of a routine tool: an empty object
/// (`schemas/empty.input.json`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RoutineInput {}

/// Which transcripts a routine's [`Context`] reads.
enum Transcripts {
    /// This directory, or none.
    Fixed(Option<PathBuf>),
    /// The `transcripts` setting, which a `codex_home` setting may leave
    /// empty: the context then reads Codex's transcripts only.
    CodexFallback,
}

/// Run one `plugin` verb.
pub(super) fn run(command: PluginCommand) -> Result<Output, CliError> {
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
            Ok(Output::Plugin(call(&raw)))
        }
    }
}

fn failure(code: &'static str, message: impl Into<String>, retryable: bool) -> PluginReply {
    PluginReply::Error(PluginError {
        code,
        message: message.into(),
        retryable,
    })
}

/// Run the call `raw` holds; every outcome is a reply.
fn call(raw: &[u8]) -> PluginReply {
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
        "group-close-prepare" => group_close(&envelope.input, &envelope.context.config, false),
        "group-close-apply" => group_close(&envelope.input, &envelope.context.config, true),
        "session-pass-prepare"
        | "session-pass-apply"
        | "session-pass-fail"
        | "session-catalog-begin"
        | "session-rebuild-prepare"
        | "session-rebuild-apply"
        | "session-rebuild-finish" => session_pass(verb, envelope.input, &envelope.context.config),
        "consistency-scan" => consistency_scan(&envelope.input, &envelope.context.config),
        "memory-refresh-prepare" => {
            memory_refresh_prepare(&envelope.input, &envelope.context.config)
        }
        "memory-refresh-apply" => memory_refresh_apply(&envelope.input, &envelope.context.config),
        _ => failure(
            "unknown_tool",
            format!("{} is not a scribe tool", envelope.tool),
            false,
        ),
    }
}

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

/// The reply for a routine command's result.
fn routine_reply(command: &'static str, result: Result<Output, CliError>) -> PluginReply {
    match result {
        Ok(Output::Scan(report)) => PluginReply::Scan(report),
        Ok(_) => failure(
            "internal_error",
            format!("{command} returned no report"),
            false,
        ),
        Err(err) => routine_error(&err),
    }
}

/// The routine's [`Context`] over the plugin's `root` and `codex_home`
/// settings, or the reply that refuses them.
fn routine_context(config: &Value, transcripts: Transcripts) -> Result<Context, PluginReply> {
    let root = store_root(config)?;
    let codex_home = codex_home(config)?;
    let (transcripts, codex_only) = match transcripts {
        Transcripts::Fixed(dir) => (dir, false),
        Transcripts::CodexFallback => match transcripts_dir(config) {
            Ok(dir) => (Some(dir), false),
            Err(_)
                if codex_home.is_some()
                    && config
                        .get("transcripts")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty() =>
            {
                (None, true)
            }
            Err(reply) => return Err(reply),
        },
    };
    let routine = Context::at(root, transcripts).with_codex_home(codex_home);
    Ok(if codex_only {
        routine.with_provider(scribe_core::Provider::Codex)
    } else {
        routine
    })
}

/// The failure reply for `err`; `retryable` is the caller's retry rule.
fn error_reply(err: &CliError, retryable: bool) -> PluginReply {
    failure(err.code(), message_of(err), retryable)
}

/// The failure reply for a routine's `err`: retryable only when the store
/// itself was locked.
fn routine_error(err: &CliError) -> PluginReply {
    error_reply(
        err,
        matches!(err, CliError::Core(e) if e.code() == "store_locked"),
    )
}

/// The crew the setting `key` names, `haiku` when it is unset or blank.
fn crew_setting<'a>(config: &'a Value, key: &str) -> &'a str {
    config
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("haiku")
}

fn session_pass(verb: &str, input: Value, config: &Value) -> PluginReply {
    let mut input = if input.is_null() {
        serde_json::json!({})
    } else {
        input
    };
    let abort = if verb == "session-pass-fail" {
        match input
            .as_object_mut()
            .and_then(|input| input.remove("abort"))
        {
            Some(Value::Bool(abort)) => abort,
            None => false,
            Some(_) => {
                return failure(
                    "invalid_input",
                    "session-pass-fail abort must be a boolean",
                    false,
                );
            }
        }
    } else {
        false
    };
    let step = match session::pass::Step::parse(verb, input) {
        Ok(step) => step,
        Err(err) => return failure("invalid_input", message_of(&err), false),
    };
    let routine = match routine_context(config, Transcripts::CodexFallback) {
        Ok(routine) => routine,
        Err(reply) => return reply,
    };
    match step.run(&routine) {
        Ok(value) if abort => {
            let message = value["pass"]["session"].as_str().map_or_else(
                || "the session distillation activity failed".to_owned(),
                |id| format!("the distillation activity failed for session {id}"),
            );
            let code = match value["pass"]["error_code"].as_str() {
                Some("model_timeout") => "model_timeout",
                Some("model_unavailable") => "model_unavailable",
                Some("model_malformed") => "model_malformed",
                _ => "model_failed",
            };
            failure(code, message, false)
        }
        Ok(mut value) => {
            if verb == "session-pass-prepare" || verb == "session-rebuild-prepare" {
                for key in ["session_distill_crew", "session_header_crew"] {
                    value[key] = Value::String(crew_setting(config, key).to_owned());
                }
            }
            PluginReply::Data(value)
        }
        Err(err) => session_pass_error(err),
    }
}

/// The failure reply for a session pass step's `err`: a usage error is
/// `invalid_input`, and any error carrying the `store_locked` code is
/// retryable, whether or not it is a [`CliError::Core`].
fn session_pass_error(err: CliError) -> PluginReply {
    if let CliError::Usage(message) = err {
        return failure("invalid_input", message, false);
    }
    error_reply(&err, err.code() == "store_locked")
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

fn consistency_scan(input: &Value, config: &Value) -> PluginReply {
    if let Err(reply) = routine_input("scribe.consistency-scan", input) {
        return reply;
    }
    let routine = match routine_context(config, Transcripts::Fixed(None)) {
        Ok(routine) => routine,
        Err(reply) => return reply,
    };
    let result = consistency::scan(&routine);
    routine_reply(consistency::COMMAND, result)
}

fn memory_refresh_prepare(input: &Value, config: &Value) -> PluginReply {
    use scribe_core::apps::memory::{self, Command, Outcome};
    if let Err(reply) = routine_input("scribe.memory-refresh-prepare", input) {
        return reply;
    }
    let routine = match routine_context(config, Transcripts::Fixed(None)) {
        Ok(routine) => routine,
        Err(reply) => return reply,
    };
    match memory::exec(&routine, Command::Prepare) {
        Ok(Outcome::Prepared(prepared)) => match serde_json::to_value(prepared) {
            Ok(mut value) => {
                value["crew"] = serde_json::json!(crew_setting(config, "profile_refresh_crew"));
                PluginReply::Data(value)
            }
            Err(err) => failure(
                "internal_error",
                format!("cannot encode prepared refresh: {err}"),
                false,
            ),
        },
        Ok(_) => failure(
            "internal_error",
            "unexpected memory refresh prepare result",
            false,
        ),
        Err(err) => routine_error(&CliError::from(err)),
    }
}

fn memory_refresh_apply(input: &Value, config: &Value) -> PluginReply {
    use scribe_core::apps::memory::{
        self, ApplyRefreshOutcome, ApplyRefreshRequest, Command, Outcome,
    };
    let request: ApplyRefreshRequest = match serde_json::from_value(input.clone()) {
        Ok(value) => value,
        Err(err) => {
            return failure(
                "invalid_input",
                format!("scribe.memory-refresh-apply input: {err}"),
                false,
            );
        }
    };
    let routine = match routine_context(config, Transcripts::Fixed(None)) {
        Ok(routine) => routine,
        Err(reply) => return reply,
    };
    let command = Command::Apply {
        provider: routine.provider(),
        request: Box::new(request),
        routine: true,
    };
    match memory::exec(&routine, command) {
        Ok(Outcome::Applied(outcome)) => match *outcome {
            ApplyRefreshOutcome::Written(w) => PluginReply::Data(serde_json::json!({
                "status": "written",
                "result": "written",
                "revision": w.revision,
                "sessions": w.sessions,
                "backlog": w.backlog,
                "milestones": w.milestones,
                "removed": w.removed,
                "code": null,
                "correction": null,
            })),
            ApplyRefreshOutcome::Retry { code, correction } => {
                PluginReply::Data(serde_json::json!({
                    "status": "retry",
                    "result": null,
                    "revision": null,
                    "sessions": 0,
                    "backlog": 0,
                    "milestones": [],
                    "removed": [],
                    "code": code,
                    "correction": correction,
                }))
            }
        },
        Ok(_) => failure(
            "internal_error",
            "unexpected memory refresh apply result",
            false,
        ),
        Err(err) => routine_error(&CliError::from(err)),
    }
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

fn group_close(input: &Value, config: &Value, apply: bool) -> PluginReply {
    use scribe_core::apps::group::{self, Command, GroupCloseInput, Outcome};
    let input: GroupCloseRequest = match serde_json::from_value(input.clone()) {
        Ok(value) => value,
        Err(err) => return failure("invalid_input", format!("group close input: {err}"), false),
    };
    if apply && (input.summary.is_none() || input.evidence_sha256.is_none()) {
        return failure(
            "invalid_input",
            "summary and evidence_sha256 are required for apply",
            false,
        );
    }
    let routine = match routine_context(config, Transcripts::Fixed(transcripts_dir(config).ok())) {
        Ok(routine) => routine.with_provider(input.provider),
        Err(reply) => return reply,
    };
    let command = if apply {
        Command::Close(GroupCloseInput {
            session: Some(input.session),
            id: input.group,
            quote: input.quote,
            provider: input.provider,
            summary: input.summary,
            if_evidence: input.evidence_sha256,
            notes: &routine,
        })
    } else {
        Command::Evidence {
            session: Some(input.session),
            id: input.group,
            quote: input.quote,
            provider: input.provider,
        }
    };
    match group::exec(&routine, command) {
        Ok(Outcome::Evidence(mut value)) => {
            value["crew"] = serde_json::json!(crew_setting(config, "group_summary_crew"));
            PluginReply::Data(value)
        }
        Ok(Outcome::Written(value)) => {
            PluginReply::Data(serde_json::json!({"group": value.group, "changed": value.changed}))
        }
        Ok(_) => failure("internal_error", "unexpected group close result", false),
        Err(err) => error_reply(&CliError::from(err), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn locked() -> scribe_core::Error {
        scribe_core::Error::Locked {
            path: PathBuf::from("/store/scribe.db"),
            holder: String::from("another scribe"),
            waited_ms: 5,
        }
    }

    fn retryable(reply: PluginReply) -> bool {
        let PluginReply::Error(error) = reply else {
            panic!("expected an error reply");
        };
        error.retryable
    }

    fn step_locked() -> CliError {
        CliError::SessionPassStep {
            code: "store_locked",
            message: String::from("store locked"),
        }
    }

    #[test]
    fn a_routine_error_is_retryable_only_for_a_locked_core_store() {
        assert!(retryable(routine_error(&CliError::Core(locked()))));
        assert!(!retryable(routine_error(&step_locked())));
        assert!(!retryable(routine_error(&CliError::Usage(String::from(
            "x"
        )))));
    }

    #[test]
    fn an_error_reply_carries_the_code_message_and_callers_retry_rule() {
        let err = step_locked();
        let PluginReply::Error(error) = error_reply(&err, true) else {
            panic!("expected an error reply");
        };
        assert_eq!(error.code, "store_locked");
        assert_eq!(error.message, "store locked");
        assert!(error.retryable);
        // group close passes no retry, even for a locked core store.
        assert!(!retryable(error_reply(&CliError::Core(locked()), false)));
    }

    #[test]
    fn a_session_pass_step_error_with_a_store_locked_code_is_retryable() {
        let reply = session_pass_error(step_locked());
        assert!(retryable(reply));
        let reply = session_pass_error(CliError::Core(locked()));
        assert!(retryable(reply));
        let PluginReply::Error(error) = session_pass_error(CliError::Usage(String::from("bad")))
        else {
            panic!("expected an error reply");
        };
        assert_eq!(error.code, "invalid_input");
        assert!(!error.retryable);
    }

    #[test]
    fn a_blank_or_missing_crew_setting_is_haiku() {
        let config = json!({"a": "sonnet", "b": "  ", "c": 7});
        assert_eq!(crew_setting(&config, "a"), "sonnet");
        assert_eq!(crew_setting(&config, "b"), "haiku");
        assert_eq!(crew_setting(&config, "c"), "haiku");
        assert_eq!(crew_setting(&config, "d"), "haiku");
    }

    #[test]
    fn the_routine_context_needs_a_configured_store() {
        let config = json!({"root": "/var/tmp"});
        let Err(PluginReply::Error(error)) = routine_context(&config, Transcripts::Fixed(None))
        else {
            panic!("an unset root is refused with an error reply");
        };
        assert_eq!(error.code, "not_configured");
    }

    #[test]
    fn session_pass_falls_back_to_codex_only_when_transcripts_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        let root = dir.join("root");
        let codex = dir.join("codex");
        let both = json!({"root": root, "codex_home": codex});
        let routine = routine_context(&both, Transcripts::CodexFallback).unwrap();
        assert_eq!(routine.provider(), scribe_core::Provider::Codex);
        let no_codex = json!({"root": root});
        assert!(routine_context(&no_codex, Transcripts::CodexFallback).is_err());
        let relative = json!({"root": root, "codex_home": codex, "transcripts": "rel"});
        assert!(routine_context(&relative, Transcripts::CodexFallback).is_err());
        let set = json!({"root": root, "transcripts": dir});
        let routine = routine_context(&set, Transcripts::CodexFallback).unwrap();
        assert_eq!(routine.provider(), scribe_core::Provider::ClaudeCode);
    }
}
