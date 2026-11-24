//! `session deactivate` and `session activate`: the `manual` route of a
//! session's `active` flag (docs/design/scribe/2_design.md §5.7).
//!
//! An agent or the user hides a session whose feature no longer exists, with
//! a reason (and a cite of what says so), or shows it again, optionally
//! pinning it (`keep_active`) so the consistency routine never deactivates
//! it. scribe never reads a repository to decide whether a feature exists:
//! that judgement is the caller's, and the reason records it.
//!
//! Each verb makes one locked store call ([`scribe_core::SessionVerbs::set_active`])
//! and prints the record in the `show` shape. A change is one audit line
//! (`target` `session:<id>`); a repeat that changes nothing prints the
//! record, says `nothing changed` on stderr and writes no audit line.

use super::{SessionRef, View, parse_session_ref};
use crate::error::CliError;
use crate::output::{Kin, Output, Record, SessionPayload};
use clap::Args;
use scribe_core::AuthorName;
use scribe_core::apps::session::{
    self as app, ActivateInput, ActivityWritten, Command, DeactivateInput, exec,
};
use scribe_core::composition::Context;
use scribe_core::{Audit, AuditSubject};

/// The command paths in the audit log.
pub(in crate::commands) const DEACTIVATE_COMMAND: &str = "session deactivate";
pub(in crate::commands) const ACTIVATE_COMMAND: &str = "session activate";

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

/// What the audit log names for `deactivate` and `activate`: the session the
/// id resolves to, and the agent it acts as.
pub(super) fn subject(ctx: &Context, id: &SessionRef, author: &AuthorName) -> AuditSubject {
    let target = id
        .segment
        .is_none()
        .then(|| id.id.clone())
        .and_then(|id| app::resolved_id(ctx, &id))
        .map(|id| format!("session:{id}"));
    AuditSubject {
        target,
        author: Some(author.as_str().to_owned()),
    }
}

/// `session deactivate`.
pub(super) fn deactivate(
    args: DeactivateArgs,
    audit: Option<Audit>,
    ctx: &Context,
) -> Result<Output, CliError> {
    let input = DeactivateInput {
        target: args.id,
        author: args.author,
        reason: &args.reason,
        cite: args.cite.as_deref(),
    };
    let outcome = exec(ctx, audit, Command::Deactivate(input))?;
    Ok(super::output(outcome, View::default(), ctx))
}

/// `session activate`.
pub(super) fn activate(
    args: ActivateArgs,
    audit: Option<Audit>,
    ctx: &Context,
) -> Result<Output, CliError> {
    let pin = match (args.pin, args.unpin) {
        (true, _) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    };
    let input = ActivateInput {
        target: args.id,
        author: args.author,
        reason: args.reason.as_deref(),
        pin,
    };
    let outcome = exec(ctx, audit, Command::Activate(input))?;
    Ok(super::output(outcome, View::default(), ctx))
}

/// The record as read after the write.
pub(super) fn output(written: ActivityWritten) -> Output {
    let view = &written.view;
    let payload = SessionPayload::new(
        &view.record,
        view.control.as_ref(),
        Kin {
            children: &view.children,
            titles: &view.linked_titles,
        },
    )
    .with_number(view.number)
    .with_log(&view.record.log);
    Output::Written {
        record: Record::Session(Box::new(payload)),
        changed: written.changed,
        notices: written.notices,
    }
}
