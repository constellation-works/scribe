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
//! - `scribe.ask` runs [`super::question::answer_with`], the code behind
//!   `question ask`, against the store at the plugin's `root` setting, so
//!   both return the same payload for the same store.
//! - `scribe.session-pass` and `scribe.consistency-scan` are declared for
//!   the routines but not yet wired (the routines ship in a later slice):
//!   they answer `not_implemented`.
//!
//! The store root comes from `[plugins.scribe] root`, never from
//! `SCRIBE_ROOT` or `HOME`: the plugin's sandbox grants read access to that
//! path alone. Every call here reads only, so none is audited.

use super::question::{answer_with, date_parser, question_parser};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{Output, PluginError, PluginReply};
use clap::{Args, Subcommand};
use scribe_core::ask::{Question, RecordKind, Scope};
use scribe_core::models::ModelError;
use scribe_core::{Project, Store, TagName, Topic};
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
  printf '%s' '{\"schema_version\":1,\"tool\":\"scribe.ask\",\"input\":{\"question\":\"how do we merge agent PRs?\"},\"context\":{\"config\":{\"root\":\"/home/me/.scribe\"}}}' | scribe plugin call";

/// The namespace the plugin's tools live in.
const NAMESPACE: &str = "scribe";
/// The most envelope bytes read from stdin.
const MAX_ENVELOPE: u64 = 1024 * 1024;
/// The envelope version this backend speaks.
const SCHEMA_VERSION: u64 = 1;

/// Verbs on the `plugin` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    /// Run one Orbit plugin tool call: an exec envelope on stdin, one reply envelope on stdout
    #[command(after_help = CALL_EXAMPLES)]
    Call(CallArgs),
}

/// Arguments to `plugin call`: none (the envelope is on stdin).
#[derive(Debug, Args)]
pub(crate) struct CallArgs {}

impl PluginCommand {
    /// `None`: every tool wired today reads only. The routine tools are
    /// audited where they write, when they are wired.
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

/// `scribe.ask`'s input (`schemas/ask.input.json`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskInput {
    question: String,
    #[serde(default)]
    scope: ScopeInput,
    #[serde(default)]
    include_hidden: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeInput {
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    since: Option<String>,
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
        "ask" => ask(envelope.input, &envelope.context.config, ctx),
        "session-pass" => failure(
            "not_implemented",
            "scribe.session-pass is not wired to the plugin yet; run 'scribe session pass'",
            false,
        ),
        "consistency-scan" => failure(
            "not_implemented",
            "scribe.consistency-scan is not built yet",
            false,
        ),
        _ => failure(
            "unknown_tool",
            format!("{} is not a scribe tool", envelope.tool),
            false,
        ),
    }
}

/// The store the plugin's `root` setting names: an absolute path.
fn store_root(config: &Value) -> Result<PathBuf, PluginReply> {
    let root = config
        .get("root")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = PathBuf::from(root);
    if root.is_empty() || !path.is_absolute() {
        return Err(failure(
            "not_configured",
            "set [plugins.scribe] root to the scribe data directory, an absolute path such as /home/<user>/.scribe",
            false,
        ));
    }
    Ok(path)
}

fn ask(input: Value, config: &Value, ctx: &Context) -> PluginReply {
    let input: AskInput = match serde_json::from_value(input) {
        Ok(input) => input,
        Err(e) => return failure("invalid_input", format!("scribe.ask input: {e}"), false),
    };
    let question = match parse_question(input) {
        Ok(question) => question,
        Err(message) => return failure("invalid_input", message, false),
    };
    let root = match store_root(config) {
        Ok(root) => root,
        Err(reply) => return reply,
    };
    let store = Store::open(root);
    match answer_with(
        &store,
        question,
        |config| ctx.models(config),
        &ctx.tasks().with_workspaces(task_workspaces(config)),
        ctx.now(),
    ) {
        Ok(payload) => PluginReply::Ask(Box::new(payload)),
        Err(err) => {
            let retryable = matches!(
                err,
                CliError::Model(ModelError::Timeout { .. } | ModelError::Failed(_))
            );
            failure(err.code(), message_of(&err), retryable)
        }
    }
}

/// The workspaces the plugin's `task_workspaces` setting names; none means
/// the workspace the call is bound to.
fn task_workspaces(config: &Value) -> Vec<String> {
    config
        .get("task_workspaces")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
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

/// The input as a [`Question`], checked as `question ask` checks its flags.
fn parse_question(input: AskInput) -> Result<Question, String> {
    let text = question_parser(&input.question).map_err(|e| format!("question: {e}"))?;
    let tags = input
        .scope
        .tags
        .into_iter()
        .map(|t| t.parse::<TagName>().map_err(|e| format!("scope.tags: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let topics = input
        .scope
        .topics
        .into_iter()
        .map(|t| t.parse::<Topic>().map_err(|e| format!("scope.topics: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let project = input
        .scope
        .project
        .map(|p| {
            p.parse::<Project>()
                .map_err(|e| format!("scope.project: {e}"))
        })
        .transpose()?;
    let kind = match input.scope.kind.as_deref() {
        None => None,
        Some("entry") => Some(RecordKind::Entry),
        Some("session") => Some(RecordKind::Session),
        Some(other) => {
            return Err(format!("scope.kind: {other:?} is not entry or session"));
        }
    };
    let since = input
        .scope
        .since
        .map(|d| date_parser(&d).map_err(|e| format!("scope.since: {e}")))
        .transpose()?;
    Ok(Question {
        text,
        scope: Scope {
            tags,
            topics,
            project,
            kind,
            since,
        },
        include_hidden: input.include_hidden,
    })
}
