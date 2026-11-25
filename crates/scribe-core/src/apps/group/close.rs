//! `scribe group close` (ORB-27707 step 3c): summarize and close a group from
//! its opener, a seat holder or a typed quote.

use super::verbs::CloseGroupOutcome;
use super::verbs::close as flow;
use super::{Error as GroupError, checked_transcript};
use crate::apps::tag::ConfigReads;
use crate::group::{GroupCloser, GroupConsent};
use crate::runtime::{AuditNotes, Ctx, PortError};
use crate::{Provider, SessionId, WriteRefusal};

/// The audited command name.
pub const CLOSE_COMMAND: &str = "group close";

/// What `group close` runs.
pub struct GroupCloseInput<'i> {
    /// The calling session, from the provider's session variable
    /// ([`Context::caller`](crate::composition::Context::caller)); `None`
    /// refuses the command as a usage error.
    pub session: Option<SessionId>,
    /// Group id, unique prefix, or project group slug.
    pub id: String,
    /// Words the user typed in this session naming the group.
    pub quote: Option<String>,
    /// The provider whose session calls.
    pub provider: Provider,
    /// The supplied summary, or none for an explicit no-summary close.
    pub summary: Option<String>,
    /// Optional optimistic evidence digest.
    pub if_evidence: Option<String>,
    /// Where the instruction and the committed audit are noted.
    pub notes: &'i dyn AuditNotes,
}

/// What `group close` did.
#[derive(Debug)]
pub struct GroupClosed {
    /// The group's canonical id.
    pub id: String,
    /// Whether this close changed the store.
    pub changed: bool,
    /// Notices for the user.
    pub notices: Vec<String>,
}

/// Close the group.
///
/// # Errors
///
/// [`GroupError`]: a refused quote or membership, a model failure, a store
/// error, or a port.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: GroupCloseInput<'_>,
) -> Result<GroupClosed, GroupError<E>> {
    let GroupCloseInput {
        session,
        id,
        quote,
        provider,
        summary,
        if_evidence,
        notes,
    } = input;
    let session = super::calling(session, provider)?;
    let store = ctx.store;
    let mut notices = Vec::new();
    notes.session(session.as_str());
    let now = ctx.clock.now();
    let started = std::time::Instant::now();
    let config = store.read().tags().config()?.for_provider(provider);
    // Resolve the alias once: consent, close and audit all bind to this
    // canonical id, so a slug reused after a close cannot redirect them.
    let group = store.read().groups().get(&id)?;
    let consent = consent(ctx, provider, &session, quote, &group.id)?;
    if let Some(consent) = &consent {
        notes.message(consent.message());
    }
    let closer = consent
        .as_ref()
        .map_or(GroupCloser::Session(&session), GroupCloser::Quoted);
    let redactor = ctx.redaction.redactor(&config).map_err(GroupError::Port)?;
    let outcome = flow::apply(
        store,
        &group.id,
        closer,
        now,
        &redactor,
        summary.as_deref(),
        (if_evidence.as_deref(), |group_id| crate::AuditEvent {
            at: now,
            command: CLOSE_COMMAND.to_owned(),
            status: crate::AuditStatus::Success,
            target: Some(group_id.to_owned()),
            error_code: None,
            author: None,
            instruction_session: Some(session.as_str().to_owned()),
            instruction_message: consent.as_ref().map(|c| c.message().to_owned()),
            issue: None,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }),
    )?;
    if matches!(&outcome, CloseGroupOutcome::Closed { .. }) {
        notes.committed();
    }
    let (id, changed) = outcome.into_result()?;
    if !changed {
        notices.push("group is already closed".into());
    }
    Ok(GroupClosed {
        id,
        changed,
        notices,
    })
}

fn consent<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    quote: Option<String>,
    id: &str,
) -> Result<Option<GroupConsent>, GroupError<E>> {
    match quote {
        None => Ok(None),
        Some(quote) if quote.trim().is_empty() => Err(crate::Error::AgentWrite {
            kind: WriteRefusal::QuoteRequired,
            message: "pass --quote with the user's own words from this session".into(),
        }
        .into()),
        Some(quote) => {
            let bytes = checked_transcript(ctx, provider, session)?.ok_or_else(|| {
                crate::Error::AgentWrite {
                    kind: WriteRefusal::QuoteNotFound,
                    message: format!(
                        "cannot verify a typed quote in session {session} on this host"
                    ),
                }
            })?;
            Ok(Some(GroupConsent::verify_close(
                session.clone(),
                provider,
                &bytes,
                &quote,
                id,
            )?))
        }
    }
}

/// Read the authorized, bounded close evidence without writing an audit row.
pub(super) fn evidence<E: PortError>(
    ctx: &Ctx<'_, E>,
    session: Option<SessionId>,
    provider: Provider,
    id: String,
    quote: Option<String>,
) -> Result<serde_json::Value, GroupError<E>> {
    let session = super::calling(session, provider)?;
    let group = ctx.store.read().groups().get(&id)?;
    let consent = consent(ctx, provider, &session, quote, &group.id)?;
    let closer = consent
        .as_ref()
        .map_or(GroupCloser::Session(&session), GroupCloser::Quoted);
    let snapshot =
        flow::evidence(ctx.store, &group.id, closer)?.ok_or_else(|| crate::Error::AgentWrite {
            kind: WriteRefusal::GroupMembership,
            message: "group is already closed".into(),
        })?;
    let config = ctx.store.read().tags().config()?;
    let instructions = config.instructions.group_summary();
    Ok(
        serde_json::json!({"id": group.id, "evidence": snapshot.evidence, "related": snapshot.related, "evidence_sha256": snapshot.digest(), "instructions": instructions}),
    )
}
