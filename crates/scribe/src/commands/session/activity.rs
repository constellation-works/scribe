//! `session deactivate` and `session activate`: the `manual` route of a
//! session's `active` flag (docs/design/scribe/2_design.md §5.7).
//!
//! An agent or Daniel hides a session whose feature no longer exists, with
//! a reason (and a cite of what says so), or shows it again, optionally
//! pinning it (`keep_active`) so the consistency routine never deactivates
//! it. scribe never reads a repository to decide whether a feature exists:
//! that judgement is the caller's, and the reason records it.
//!
//! Each verb makes one locked store call ([`scribe_core::Store::set_active`])
//! and prints the record in the `show` shape. A change is one audit line
//! (`target` `session:<id>`); a repeat that changes nothing prints the
//! record, says `nothing changed` on stderr and writes no audit line.

use super::{SessionRef, parse_session_ref};
use crate::app::Context;
use crate::error::CliError;
use crate::output::{Kin, Output, Record, SessionPayload};
use clap::Args;
use scribe_core::activity::{self, ActiveOutcome, ActiveRoute, SetActive};
use scribe_core::{Author, AuthorName, SessionId};

/// The command paths in the audit log.
pub(in crate::commands) const DEACTIVATE_COMMAND: &str = "session deactivate";
pub(in crate::commands) const ACTIVATE_COMMAND: &str = "session activate";

/// The longest `--cite`, in characters.
const CITE_CHARS: usize = 200;

pub(super) const DEACTIVATE_EXAMPLES: &str = "\
Hides the session from session list and ask, as a superseded entry is
hidden; session show still shows it, with the reason.
Deactivating a pinned session also unpins it. Check that the feature is
gone before you deactivate: scribe takes your reason as given.

Examples:
  scribe session deactivate <id> --author opus --reason 'the feature was removed'
  scribe session deactivate <id> --author opus --reason 'replaced by the new pass' --cite 'session:<id>#2'
  scribe session deactivate <id> --author opus --reason '...' --json | jq '.active_change'";

pub(super) const ACTIVATE_EXAMPLES: &str = "\
Shows an inactive session again. --pin exempts it from the consistency
routine's deactivation by age and by removed feature; --unpin lifts that.

Examples:
  scribe session activate <id> --author opus
  scribe session activate <id> --author opus --pin --reason 'still the reference for the store'
  scribe session activate <id> --author opus --unpin";

/// Arguments to `session deactivate`.
#[derive(Debug, Args)]
pub(crate) struct DeactivateArgs {
    /// The session's id, as printed by `session list`, a unique prefix of it (at least four characters), or its cite session:<id>
    #[arg(value_name = "ID", value_parser = parse_session_ref)]
    pub(crate) id: SessionRef,

    /// Who is asking: lowercase letters, digits, '.', '_' or '-', 1-40 characters, such as your crew or model
    #[arg(long, value_name = "NAME")]
    pub(crate) author: AuthorName,

    /// Why the session no longer describes something current, at most 300 characters
    #[arg(long, value_name = "TEXT")]
    pub(crate) reason: String,

    /// The record that says so: session:<id>, session:<id>#<n> or an entry id [default: none]
    #[arg(long, value_name = "REF")]
    pub(crate) cite: Option<String>,
}

/// Arguments to `session activate`.
#[derive(Debug, Args)]
pub(crate) struct ActivateArgs {
    /// The session's id, as printed by `session list --include-inactive`, a unique prefix of it (at least four characters), or its cite session:<id>
    #[arg(value_name = "ID", value_parser = parse_session_ref)]
    pub(crate) id: SessionRef,

    /// Who is asking: lowercase letters, digits, '.', '_' or '-', 1-40 characters, such as your crew or model
    #[arg(long, value_name = "NAME")]
    pub(crate) author: AuthorName,

    /// Why, at most 300 characters [default: scribe says what changed]
    #[arg(long, value_name = "TEXT")]
    pub(crate) reason: Option<String>,

    /// Exempt the session from automatic deactivation (keep_active)
    #[arg(long, conflicts_with = "unpin")]
    pub(crate) pin: bool,

    /// Lift the exemption, so the consistency routine may deactivate it again
    #[arg(long)]
    pub(crate) unpin: bool,
}

/// `session deactivate`.
pub(super) fn deactivate(args: DeactivateArgs, ctx: &Context) -> Result<Output, CliError> {
    let id = ctx.store().resolve_session(&whole_session(&args.id)?)?;
    let reason = reason("--reason", &args.reason)?;
    let cite = args
        .cite
        .as_deref()
        .map(|text| validated_cite(ctx.store(), text))
        .transpose()?;
    let request = SetActive::Deactivate {
        by: Author::agent(args.author),
        route: ActiveRoute::Manual,
        reason,
        cite,
    };
    write(ctx, &id, &request)
}

/// `session activate`.
pub(super) fn activate(args: ActivateArgs, ctx: &Context) -> Result<Output, CliError> {
    let id = ctx.store().resolve_session(&whole_session(&args.id)?)?;
    let reason = args
        .reason
        .as_deref()
        .map(|r| reason("--reason", r))
        .transpose()?;
    let pin = match (args.pin, args.unpin) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    };
    let request = SetActive::Activate {
        by: Author::agent(args.author),
        reason,
        pin,
    };
    write(ctx, &id, &request)
}

/// Apply `request` in one locked store call and print the record.
fn write(ctx: &Context, id: &SessionId, request: &SetActive) -> Result<Output, CliError> {
    let store = ctx.store();
    let redactor = ctx.redactor(&store.config()?)?;
    let (record, outcome) = store.set_active(id, request, ctx.now(), &redactor)?;
    let mut notices = Vec::new();
    match outcome {
        ActiveOutcome::Changed { unpinned: true } => notices.push(format!(
            "{id} was pinned (keep_active); deactivating it unpinned it"
        )),
        ActiveOutcome::Changed { unpinned: false } => {}
        ActiveOutcome::Unchanged | ActiveOutcome::Pinned => {
            let state = match (record.active, record.keep_active) {
                (true, true) => "active and pinned",
                (true, false) => "active",
                (false, _) => "inactive",
            };
            notices.push(format!("nothing changed: {id} is already {state}"));
        }
    }
    // The links, children and switch come from a read after the write.
    let view = store.session(id)?;
    let payload = SessionPayload::new(
        &view.record,
        view.control.as_ref(),
        Kin {
            children: &view.children,
            titles: &view.linked_titles,
        },
    )
    .with_log(&view.record.log);
    Ok(Output::Written {
        record: Record::Session(Box::new(payload)),
        changed: outcome.changed(),
        notices,
    })
}

/// The session a write verb names: an id or `session:<id>`, never one
/// segment, since activity belongs to the whole session.
fn whole_session(id: &SessionRef) -> Result<SessionId, CliError> {
    match id.segment {
        Some(n) => Err(CliError::Usage(format!(
            "'session:{}#{n}' names one segment; a session is activated or deactivated whole, so give session:{}",
            id.id, id.id
        ))),
        None => Ok(id.id.clone()),
    }
}

/// A reason: not blank, at most [`activity::REASON_CHARS`] characters.
fn reason(flag: &str, text: &str) -> Result<String, CliError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(CliError::Usage(format!("{flag} must not be empty")));
    }
    let chars = text.chars().count();
    if chars > activity::REASON_CHARS {
        return Err(CliError::Usage(format!(
            "{flag} is {chars} characters; at most {} are kept",
            activity::REASON_CHARS
        )));
    }
    Ok(text.to_owned())
}

/// A `--cite` that names a real record: `session:<id>`, `session:<id>#<n>`
/// (that segment must exist) or an entry id. Returns the canonical cite.
fn validated_cite(store: &scribe_core::Store, text: &str) -> Result<String, CliError> {
    let text = cite(text)?;
    let invalid = |why: String| {
        CliError::Usage(format!(
            "--cite '{text}' is not a known record: {why}; use session:<id>, session:<id>#<n> or an entry id"
        ))
    };
    if let Some(rest) = text.strip_prefix("session:") {
        let (id, segment) = match rest.split_once('#') {
            Some((id, n)) => {
                let n = n.parse::<std::num::NonZeroU32>().map_err(|_| {
                    invalid(format!("invalid segment '{n}'; expected session:<id>#<n>"))
                })?;
                (id, Some(n))
            }
            None => (rest, None),
        };
        let id = id
            .parse::<SessionId>()
            .map_err(|e| invalid(e.to_string()))?;
        let resolved = store
            .resolve_session(&id)
            .map_err(|e| invalid(e.to_string()))?;
        if let Some(n) = segment {
            let view = store
                .session(&resolved)
                .map_err(|e| invalid(e.to_string()))?;
            if view.record.segment(n.get()).is_none() {
                return Err(invalid(format!("session {resolved} has no segment {n}")));
            }
            return Ok(format!("session:{resolved}#{n}"));
        }
        return Ok(format!("session:{resolved}"));
    }
    if text.contains(':') || text.contains('#') {
        return Err(invalid(String::from(
            "expected session:<id>, session:<id>#<n> or an entry id",
        )));
    }
    let entry = store
        .resolve_entry(&text)
        .map_err(|e| invalid(e.to_string()))?;
    Ok(entry.id.to_string())
}

/// A cite: one token of at most [`CITE_CHARS`] characters.
fn cite(text: &str) -> Result<String, CliError> {
    let text = text.trim();
    let one_token = !text.is_empty() && !text.chars().any(char::is_whitespace);
    if !one_token || text.chars().count() > CITE_CHARS {
        return Err(CliError::Usage(format!(
            "--cite must be one reference of at most {CITE_CHARS} characters, such as session:<id>#<n> or an entry id"
        )));
    }
    Ok(text.to_owned())
}
