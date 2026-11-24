//! `scribe brief` and the session-start brief (ORB-27707 step 3d): the
//! memory block, then the last sessions, this project's first, then every
//! open claim, grouped by tag with the id that settles it
//! (`crate::brief`).
//!
//! Model-free and read-only: it reads the memory, the session records and
//! the proposals, takes no lock and writes nothing.

use crate::apps::memory as memory_app;
use crate::brief::{self, Brief, DEFAULT_SESSIONS};
use crate::runtime::{Ctx, PortError};
use crate::{
    EntryFilter, EntryStatus, MemoryView, Project, Provider, SessionFilter, SessionId, ShortIds,
};
use std::path::Path;

/// What `brief` runs.
pub struct BriefInput<'i> {
    /// List this project's sessions first.
    pub project: Option<&'i Project>,
    /// How many of the last sessions to show.
    pub sessions: usize,
    /// The provider whose transcripts name sessions for short ids.
    pub provider: Provider,
}

/// What `brief` read: the brief, the memory view its block came from, and
/// the short ids to print it with.
#[derive(Debug)]
pub struct BriefPage {
    /// The brief, its memory set to the block's text.
    pub brief: Brief,
    /// The memory as read.
    pub view: MemoryView,
    /// Display names for the brief's ids.
    pub ids: ShortIds,
}

/// Read the brief.
///
/// # Errors
///
/// A store read error.
pub fn run<E: PortError>(ctx: &Ctx<'_, E>, input: &BriefInput<'_>) -> crate::Result<BriefPage> {
    let view = ctx.store.read().memory().view()?;
    let brief = build(
        ctx,
        Some(memory_app::text(&view)),
        input.project,
        None,
        input.sessions,
    )?;
    let ids = short_ids(ctx, input.provider, &brief);
    Ok(BriefPage { brief, view, ids })
}

/// The brief for `project` under the `memory` block, leaving out `exclude`
/// (the session starting). Read-only: no lock, no write.
fn build<E>(
    ctx: &Ctx<'_, E>,
    memory: Option<String>,
    project: Option<&Project>,
    exclude: Option<&SessionId>,
    sessions: usize,
) -> crate::Result<Brief> {
    let listed = ctx.store.read().sessions().list(&SessionFilter {
        include_inactive: true,
        ..SessionFilter::default()
    })?;
    let handoffs = ctx.store.read().entries().list(&EntryFilter {
        tag: crate::HANDOFF_TAG.parse().ok(),
        status: Some(EntryStatus::Active),
        ..EntryFilter::default()
    })?;
    let proposals = ctx.store.read().proposals().all()?;
    let mut brief = brief::build(
        &listed.listing.items,
        &handoffs.listing.items,
        &proposals,
        project,
        exclude,
        sessions,
    );
    brief.memory = memory;
    if let Some(project) = project {
        brief.last_group = ctx.store.read().groups().last(project)?;
    }
    Ok(brief)
}

/// Resolve display names against all known sessions, including the
/// `provider` transcripts on disk.
fn short_ids<E: PortError>(ctx: &Ctx<'_, E>, provider: Provider, brief: &Brief) -> ShortIds {
    let mut ids = ctx.store.read().sessions().ids().unwrap_or_default();
    ids.extend(
        ctx.transcripts
            .files(provider)
            .unwrap_or_default()
            .into_iter()
            .map(|(id, _)| id.to_string()),
    );
    ids.extend(brief.sessions.iter().map(|s| s.id.to_string()));
    let mut shorts = ShortIds::new(ids);
    if let Ok(numbers) = ctx.store.read().sessions().numbers() {
        shorts.set_aliases(
            numbers
                .into_iter()
                .map(|(id, n)| (id.to_string(), format!("s-{n}"))),
        );
    }
    shorts
}

/// The memory block's text, or `None` when memory holds nothing or the
/// store cannot be read (logged, so the session still starts).
fn memory_text<E>(ctx: &Ctx<'_, E>) -> Option<String> {
    match ctx.store.read().memory().view() {
        Ok(view) => {
            for warning in memory_app::warnings(&view, ctx.store.root()) {
                tracing::warn!("{warning}");
            }
            (!memory_app::is_empty(&view)).then(|| memory_app::text(&view))
        }
        Err(err) => {
            tracing::warn!(%err, "cannot read the memory; not injecting the memory block");
            None
        }
    }
}

/// The session-start brief for `session`'s project, as text, or `None`
/// when it has no memory, last group, session or claim, or the store cannot
/// be read (logged, so the session still starts). Memory that cannot be
/// read is left out (logged); the rest is still given. With no `cwd`, every
/// project's sessions by date alone.
pub(crate) fn at_start<E: PortError>(
    ctx: &Ctx<'_, E>,
    provider: Provider,
    session: &SessionId,
    cwd: Option<&str>,
) -> Option<String> {
    let project = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| ctx.transcripts.project_of(Path::new(cwd)));
    let memory = memory_text(ctx);
    match build(
        ctx,
        memory,
        project.as_ref(),
        Some(session),
        DEFAULT_SESSIONS,
    ) {
        Ok(brief)
            if brief.memory.is_none()
                && brief.last_group.is_none()
                && brief.sessions.is_empty()
                && brief.claims.is_empty() =>
        {
            None
        }
        Ok(brief) => Some(brief.text_with_ids(&short_ids(ctx, provider, &brief))),
        Err(err) => {
            tracing::warn!(%err, "cannot read the sessions or proposals; not injecting the brief");
            None
        }
    }
}
