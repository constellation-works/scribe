//! `scribe group close` (ORB-27707 step 3c): summarize and close a group from
//! its opener, a seat holder or a typed quote.

use crate::cases::group::GroupVerbs;
use crate::cases::group_cmd::{GroupError, checked_transcript};
use crate::cases::{AuditNotes, Ctx, PortError};
use crate::group::{GroupCloser, GroupConsent};
use crate::{CloseGroupOutcome, Config, Models, Provider, SessionId, WriteRefusal};

/// The audited command name.
pub const CLOSE_COMMAND: &str = "group close";

/// What `group close` runs.
pub struct GroupCloseInput<'i, F> {
    /// The calling session.
    pub session: SessionId,
    /// Group id, unique prefix, or project group slug.
    pub id: String,
    /// Words the user typed in this session naming the group.
    pub quote: Option<String>,
    /// The provider whose session calls.
    pub provider: Provider,
    /// The model adapter for the config, built once before the group is read.
    pub models: F,
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
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: GroupCloseInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<GroupClosed, GroupError<E>> {
    let GroupCloseInput {
        session,
        id,
        quote,
        provider,
        models,
        notes,
    } = input;
    let store = ctx.store;
    let mut notices = Vec::new();
    notes.session(session.as_str());
    let now = ctx.clock.now();
    let started = std::time::Instant::now();
    let config = store.read().tags().config()?.for_provider(provider);
    let models = models(&config);
    // Resolve the alias once: consent, close and audit all bind to this
    // canonical id, so a slug reused after a close cannot redirect them.
    let group = store.read().groups().get(&id)?;
    let consent = match quote {
        None => None,
        Some(quote) if quote.trim().is_empty() => {
            return Err(crate::Error::AgentWrite {
                kind: WriteRefusal::QuoteRequired,
                message: "pass --quote with the user's own words from this session".into(),
            }
            .into());
        }
        Some(quote) => {
            let bytes = checked_transcript(ctx, provider, &session)?.ok_or_else(|| {
                crate::Error::AgentWrite {
                    kind: WriteRefusal::QuoteNotFound,
                    message: format!(
                        "cannot verify a typed quote in session {session} on this host"
                    ),
                }
            })?;
            let consent =
                GroupConsent::verify_close(session.clone(), provider, &bytes, &quote, &group.id)?;
            notes.message(consent.message());
            Some(consent)
        }
    };
    let closer = consent
        .as_ref()
        .map_or(GroupCloser::Session(&session), GroupCloser::Quoted);
    let redactor = ctx.redaction.redactor(&config).map_err(GroupError::Port)?;
    let outcome = store.close_group_with_outcome(
        &group.id,
        closer,
        now,
        &redactor,
        |evidence| -> Result<_, GroupError<E>> {
            if checked_transcript(ctx, provider, &session)?.is_none() {
                return Err(crate::Error::AgentWrite {
                    kind: WriteRefusal::GroupMembership,
                    message: format!(
                        "cannot verify the transcript of session {session} on this host"
                    ),
                }
                .into());
            }
            Ok(models.group_summary(&config.models.ask, evidence)?)
        },
        |group_id| crate::AuditEvent {
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
        },
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
