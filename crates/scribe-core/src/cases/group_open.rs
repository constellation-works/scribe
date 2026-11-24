//! `scribe group open` (ORB-27707 step 3c): open a group on a verbatim quote
//! the user typed in the calling session.

use crate::cases::group::GroupVerbs;
use crate::cases::group_cmd::{GroupError, locate, redactor, resolve_relateds, typed_turns};
use crate::group::{GroupConsent, NewGroup};
use crate::runtime::{AuditNotes, Ctx, PortError};
use crate::{Project, Provider, Related, SessionId, WriteRefusal};

/// What `group open` runs.
pub struct GroupOpenInput<'i> {
    /// The calling session.
    pub session: SessionId,
    /// Title of the work.
    pub title: String,
    /// Free label for the kind of work.
    pub kind: String,
    /// Project slug.
    pub project: Project,
    /// Task, PR or repository references.
    pub related: Vec<Related>,
    /// Words the user typed in this session.
    pub quote: Option<String>,
    /// The provider whose session calls.
    pub provider: Provider,
    /// Where the instruction's session and message are noted for the audit.
    pub notes: &'i dyn AuditNotes,
}

/// What `group open` did: the group, and a warning per group still open in
/// the project.
#[derive(Debug)]
pub struct GroupOpened {
    /// The new group's id.
    pub id: String,
    /// One notice per group still open in this project.
    pub notices: Vec<String>,
}

/// Open the group.
///
/// # Errors
///
/// [`GroupError`]: a refused quote or consent, a store error, or a port.
pub fn run<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: GroupOpenInput<'_>,
) -> Result<GroupOpened, GroupError<E>> {
    let GroupOpenInput {
        session,
        title,
        kind,
        project,
        related,
        quote,
        provider,
        notes,
    } = input;
    let store = ctx.store;
    let mut notices = Vec::new();
    notes.session(session.as_str());
    let quote = quote
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| crate::Error::AgentWrite {
            kind: WriteRefusal::QuoteRequired,
            message: "pass --quote with the user's own words from this session".into(),
        })?;
    let path = locate(ctx, provider, &session)?;
    let bytes = path
        .and_then(|p| {
            ctx.transcripts
                .read_checked(provider, &p, &session)
                .ok()
                .flatten()
        })
        .ok_or_else(|| crate::Error::AgentWrite {
            kind: WriteRefusal::QuoteNotFound,
            message: format!("cannot verify a typed quote in session {session} on this host"),
        })?;
    let consent = GroupConsent::verify(session, provider, &bytes, &quote)?;
    notes.message(consent.message());
    let draft = NewGroup {
        title,
        kind,
        project,
        related: resolve_relateds(ctx, provider, &related)?,
    };
    // The opener's earlier typed turns are group record from its first
    // line. A transcript that cannot be read as turns stores none.
    let earlier = typed_turns(provider, consent.session(), &bytes)?;
    let opened = store.open_group(
        draft,
        &consent,
        &earlier,
        ctx.clock.now(),
        &redactor(ctx, provider)?,
    )?;
    for id in opened.still_open {
        notices.push(format!("warning: group {id} is still open in this project"));
    }
    Ok(GroupOpened {
        id: opened.id,
        notices,
    })
}
