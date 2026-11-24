//! `scribe mark`: the `#scribe` markers scribe has not cataloged yet
//! (docs/design/scribe/2_design.md §14.5). `list` shows each one that is
//! pending a retry or that every model call failed for (given up,
//! `model_failed`); `retry` catalogs one session's again from its
//! transcript, as its `Stop` would, after opening its given-up markers.
//!
//! A hook runs with the environment of the app that started it, which may
//! lack the model credential a login shell exports, so a marker can fail
//! in a hook and still be cataloged by `retry` from a shell.

use super::{ListNotice, list_output};
use crate::app::Context;
use crate::audit_middleware::{AuditMeta, AuditSubject};
use crate::error::CliError;
use crate::output::{Output, Records};
use clap::{Args, Subcommand};
use scribe_core::Config;
use scribe_core::SessionId;
use scribe_core::cases::mark_retry::{self, MarkRetried, MarkRetryInput};

pub(super) const EXAMPLES: &str = "\
Examples:
  scribe mark list
  scribe mark list --session <id> --json
  scribe mark retry <id>";

const LIST_EXAMPLES: &str = "\
A marker is listed while it is pending (the session's next reply retries it)
or given up (outcome refused, code model_failed: every call failed). It holds
ids only; the line it marks is in the session's transcript.

Examples:
  scribe mark list
  scribe mark list --session <id> --json";

const RETRY_EXAMPLES: &str = "\
Run it from a shell whose environment holds the model credential
([models] credentials in config.toml). Each given-up marker of the session
gets its attempts back, then every marker of the session's transcript that
nothing settled yet is sent to the model, as the Stop hook would, with no
per-run bound. Exits 1 with model_failed when a call fails again; the marker
stays pending.

Examples:
  scribe mark retry <id>
  scribe --provider codex mark retry <id>";

/// Verbs on the `mark` noun.
#[derive(Debug, Subcommand)]
pub(crate) enum MarkCommand {
    /// List #scribe markers not cataloged yet: pending a retry, or given up
    #[command(after_help = LIST_EXAMPLES)]
    List(ListArgs),
    /// Catalog a session's given-up and pending #scribe markers now
    ///
    /// Opens the session's given-up markers again and catalogs every unsettled marker of its transcript now, as the Stop hook would.
    #[command(after_help = RETRY_EXAMPLES)]
    Retry(RetryArgs),
}

/// Arguments to `mark list`.
#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Only this session's markers: a provider session id or a unique prefix [default: every session]
    #[arg(long, value_name = "SESSION")]
    session: Option<SessionId>,
}

/// Arguments to `mark retry`.
#[derive(Debug, Args)]
pub(crate) struct RetryArgs {
    /// The session whose markers to catalog: a provider session id or a unique prefix
    #[arg(value_name = "SESSION")]
    session: SessionId,
}

impl MarkCommand {
    /// `list` reads only; `retry` writes and may call a model, so it
    /// commits its own steps.
    pub(super) fn audit(&self) -> Option<AuditMeta> {
        match self {
            Self::List(_) => None,
            Self::Retry(_) => Some(AuditMeta {
                command: "mark retry",
                stepwise: true,
            }),
        }
    }

    /// `retry` names its session, resolved when it resolves.
    pub(super) fn subject(&self, ctx: &Context) -> AuditSubject {
        match self {
            Self::List(_) => AuditSubject::default(),
            Self::Retry(args) => AuditSubject {
                target: Some(format!(
                    "session:{}",
                    super::resolve_session_or_typed(ctx, &args.session)
                        .unwrap_or_else(|_| args.session.clone())
                )),
                author: None,
            },
        }
    }
}

pub(super) fn run(command: MarkCommand, ctx: &Context) -> Result<Output, CliError> {
    match command {
        MarkCommand::List(args) => list(&args, ctx),
        MarkCommand::Retry(args) => retry(&args, ctx),
    }
}

/// Read-only: no lock, no audit, no write.
fn list(args: &ListArgs, ctx: &Context) -> Result<Output, CliError> {
    let session = args
        .session
        .as_ref()
        .map(|id| super::resolve_session_or_typed(ctx, id))
        .transpose()?;
    let marks = ctx.store().read().marks().uncataloged(session.as_ref())?;
    let notice = ListNotice {
        noun: "uncataloged #scribe markers",
        criteria: session.iter().map(|id| format!("--session {id}")).collect(),
        empty: "no uncataloged #scribe markers: every marker has an entry or an answer",
    };
    let total = marks.len();
    Ok(list_output(Records::Marks(marks), total, &[], &notice))
}

/// Open the session's given-up markers, then catalog its transcript's
/// unsettled markers as `Stop` does. The markers this run opened or
/// recorded (written at or after its start) are listed; the echoes go to
/// stderr.
fn retry(args: &RetryArgs, ctx: &Context) -> Result<Output, CliError> {
    let session = super::resolve_session_or_typed(ctx, &args.session)?;
    let MarkRetried { touched, echoes } = mark_retry::run(
        &ctx.core(),
        MarkRetryInput {
            session: &session,
            provider: ctx.provider(),
            host: ctx.host(),
            models: |config: &Config| ctx.models(config),
        },
    )?;
    let total = touched.len();
    let mut output = list_output(
        Records::Marks(touched),
        total,
        &[],
        &ListNotice {
            noun: "#scribe markers",
            criteria: Vec::new(),
            empty: "nothing to retry: the session has no uncataloged #scribe marker",
        },
    );
    if let Output::List { notices, .. } = &mut output {
        notices.extend(echoes);
    }
    Ok(output)
}
