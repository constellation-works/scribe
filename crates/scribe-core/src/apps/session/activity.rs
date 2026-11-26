//! `scribe session activate|deactivate` (ORB-27707 step 3d): an agent's
//! manual activity change, in one locked store call (§7, S10).

use super::verbs::SessionVerbs;
use crate::SessionView;
use crate::activity::{self, ActiveOutcome, ActiveRoute, SetActive};
use crate::apps::entry;
use crate::apps::tag::redactor;
use crate::runtime::Ctx;
use crate::runtime::errors::WriteError;
use crate::{Author, AuthorName, SessionId};
use std::num::NonZeroU32;

/// The longest `--cite`.
const CITE_CHARS: usize = 200;

/// A session named by id (or a prefix of one) or by its cite, optionally
/// with a segment. The id is resolved against the store by the handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRef {
    /// The id or a unique prefix.
    pub id: SessionId,
    /// `#<n>`, when given.
    pub segment: Option<NonZeroU32>,
}

/// `<id>`, `session:<id>` or `session:<id>#<n>`.
///
/// # Errors
///
/// A segment that is not a number from 1, or an invalid id.
pub fn parse_session_ref(raw: &str) -> Result<SessionRef, String> {
    let rest = raw.strip_prefix("session:").unwrap_or(raw);
    let (id, segment) = match rest.split_once('#') {
        Some((id, n)) => {
            let n = n.parse::<NonZeroU32>().map_err(|_| {
                format!("invalid segment '{n}' in '{raw}': expected session:<id>#<n> with n from 1")
            })?;
            (id, Some(n))
        }
        None => (rest, None),
    };
    let id = id.parse::<SessionId>().map_err(|e| e.to_string())?;
    Ok(SessionRef { id, segment })
}

/// What `session deactivate` runs.
pub struct DeactivateInput<'i> {
    /// The session.
    pub target: SessionRef,
    /// The agent.
    pub author: AuthorName,
    /// Why.
    pub reason: &'i str,
    /// A record that shows why.
    pub cite: Option<&'i str>,
}

/// What `session activate` runs.
pub struct ActivateInput<'i> {
    /// The session.
    pub target: SessionRef,
    /// The agent.
    pub author: AuthorName,
    /// Why, if given.
    pub reason: Option<&'i str>,
    /// Pin (`Some(true)`), unpin (`Some(false)`) or leave the pin.
    pub pin: Option<bool>,
}

/// What a write did: the record after it, read again, whether it changed
/// and the notices.
#[derive(Debug)]
pub struct ActivityWritten {
    /// The session as read after the write.
    pub view: SessionView,
    /// Whether the write changed it.
    pub changed: bool,
    /// Notices for the user.
    pub notices: Vec<String>,
}

/// `session deactivate`.
///
/// # Errors
///
/// [`WriteError`]: a usage error, a refusal, the redactor, or a store error.
pub fn deactivate<E>(
    ctx: &Ctx<'_, E>,
    input: DeactivateInput<'_>,
) -> Result<ActivityWritten, WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let id = ctx
        .store
        .read()
        .sessions()
        .resolve(&whole_session(&input.target)?)?;
    let reason = reason("--reason", input.reason)?;
    let cite = input
        .cite
        .map(|text| validated_cite(ctx, text))
        .transpose()?;
    let request = SetActive::Deactivate {
        by: Author::agent(input.author),
        route: ActiveRoute::Manual,
        reason,
        cite,
    };
    write(ctx, &id, &request)
}

/// `session activate`.
///
/// # Errors
///
/// [`WriteError`]: a usage error, a refusal, the redactor, or a store error.
pub fn activate<E>(
    ctx: &Ctx<'_, E>,
    input: ActivateInput<'_>,
) -> Result<ActivityWritten, WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let id = ctx
        .store
        .read()
        .sessions()
        .resolve(&whole_session(&input.target)?)?;
    let reason = input.reason.map(|r| reason("--reason", r)).transpose()?;
    let request = SetActive::Activate {
        by: Author::agent(input.author),
        reason,
        pin: input.pin,
    };
    write(ctx, &id, &request)
}

/// Apply `request` in one locked store call and read the record again.
fn write<E>(
    ctx: &Ctx<'_, E>,
    id: &SessionId,
    request: &SetActive,
) -> Result<ActivityWritten, WriteError<E>> {
    let store = ctx.store;
    let redactor = redactor(ctx)?;
    let (record, outcome) = store.set_active(id, request, ctx.clock.now(), &redactor)?;
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
    let view = super::session_view(&store.read(), id)?;
    Ok(ActivityWritten {
        view,
        changed: outcome.changed(),
        notices,
    })
}

/// The session a write verb names: an id or `session:<id>`, never one
/// segment, since activity belongs to the whole session.
fn whole_session<E>(target: &SessionRef) -> Result<SessionId, WriteError<E>> {
    match target.segment {
        Some(n) => Err(WriteError::Usage(format!(
            "'session:{}#{n}' names one segment; a session is activated or deactivated whole, so give session:{}",
            target.id, target.id
        ))),
        None => Ok(target.id.clone()),
    }
}

/// A reason: not blank, at most [`activity::REASON_CHARS`] characters.
fn reason<E>(flag: &str, text: &str) -> Result<String, WriteError<E>> {
    let text = text.trim();
    if text.is_empty() {
        return Err(WriteError::Usage(format!("{flag} must not be empty")));
    }
    let chars = text.chars().count();
    if chars > activity::REASON_CHARS {
        return Err(WriteError::Usage(format!(
            "{flag} is {chars} characters; at most {} are kept",
            activity::REASON_CHARS
        )));
    }
    Ok(text.to_owned())
}

/// A cite: one token of at most [`CITE_CHARS`] characters.
fn cite_token<E>(text: &str) -> Result<&str, WriteError<E>> {
    let text = text.trim();
    let one_token = !text.is_empty() && !text.chars().any(char::is_whitespace);
    if !one_token || text.chars().count() > CITE_CHARS {
        return Err(WriteError::Usage(format!(
            "--cite must be one reference of at most {CITE_CHARS} characters, such as session:<id>#<n> or an entry id"
        )));
    }
    Ok(text)
}

/// A `--cite` that names a record in the store: `session:<id>`,
/// `session:<id>#<n>` (the segment must exist) or an entry id. A unique
/// prefix is accepted and stored as the full reference. A reference that is
/// not of one of those shapes, or names nothing, is a usage error carrying the
/// lookup's own message (which names the id and where it looked).
fn validated_cite<E>(ctx: &Ctx<'_, E>, text: &str) -> Result<String, WriteError<E>> {
    let text = cite_token(text)?;
    let store = ctx.store;
    let not_a_record = |error: crate::Error| {
        let names_no_record = matches!(
            error,
            crate::Error::EntryNotFound { .. }
                | crate::Error::SessionNotFound { .. }
                | crate::Error::AgentWrite {
                    kind: crate::WriteRefusal::EntryNotFound | crate::WriteRefusal::InvalidId,
                    ..
                }
        );
        if names_no_record {
            WriteError::Usage(format!("--cite '{text}' is not a known record: {error}"))
        } else {
            WriteError::Core(error)
        }
    };
    if text.starts_with("session:") {
        let reference = parse_session_ref(text).map_err(|e| {
            WriteError::Usage(format!("--cite '{text}' is not a session reference: {e}"))
        })?;
        let id = store
            .read()
            .sessions()
            .resolve(&reference.id)
            .map_err(not_a_record)?;
        return match reference.segment {
            None => Ok(format!("session:{id}")),
            Some(n) => {
                let view = store.read().sessions().get(&id)?;
                if view.record.log.iter().any(|segment| segment.n == n) {
                    Ok(format!("session:{id}#{n}"))
                } else {
                    Err(WriteError::Usage(format!(
                        "--cite '{text}' names segment {n}, but session {id} has {} segment(s)",
                        view.record.log.len()
                    )))
                }
            }
        };
    }
    let entry = entry::resolve_entry(store, text).map_err(not_a_record)?;
    Ok(entry.id.to_string())
}
