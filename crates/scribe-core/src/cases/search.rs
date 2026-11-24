//! `scribe search` (ORB-27707 step 3d): one query over the entries, the
//! sessions and, for `--kind message` or a default search that finds
//! nothing else, the stored messages (2_design.md §15.2).

use crate::ask::{Corpus, RecordKind};
use crate::cases::Ctx;
use crate::cases::read_error::ReadError;
use crate::search::{self, Found, SearchError, SearchQuery};
use crate::{EntryFilter, SessionFilter, Skipped};

/// What `search` found.
pub struct Searched {
    /// The hits.
    pub found: Found,
    /// The group it was scoped to.
    pub group: Option<crate::group::Group>,
    /// The entry and session files that could not be read.
    pub skipped: Vec<Skipped>,
    /// Whether the messages were read from a read-only copy of the store.
    pub read_only_copy: bool,
    /// For a default search that found no entry or session: how many stored
    /// messages match.
    pub message_matches: Option<usize>,
}

/// A refused query as a usage error (exit 2).
fn usage<E>(e: SearchError) -> ReadError<E> {
    match e {
        SearchError::NoTerms(_) => ReadError::Usage(e.to_string()),
    }
}

/// Run `query`, scoped to the sessions of `group` when given. The group,
/// the tag config, the entries and the sessions are read first; messages
/// are ranked by the store, not by `search`.
///
/// # Errors
///
/// [`ReadError::Usage`] for a query with no terms, an unknown group, or a
/// store error.
pub fn run<E>(
    ctx: &Ctx<'_, E>,
    query: &SearchQuery,
    group: Option<&str>,
) -> Result<Searched, ReadError<E>> {
    let store = ctx.store;
    let group = group.map(|id| store.read().groups().get(id)).transpose()?;
    let members = group.as_ref().map(crate::group::Group::members);
    let config = store.read().tags().config()?;
    let entries = store
        .read()
        .entries()
        .list_scoped(&EntryFilter::default(), members.as_ref())?;
    let sessions = store.read().sessions().list_scoped(
        &SessionFilter {
            include_inactive: true,
            ..SessionFilter::default()
        },
        members.as_ref(),
    )?;
    // The stored messages `query` keeps, and whether the store was opened as
    // a read-only copy.
    let stored_messages = |query: &SearchQuery| -> Result<(Found, bool), ReadError<E>> {
        let tokens = crate::ask::tokens(&query.terms);
        let stored = match &group {
            Some(group) => store.read().groups().search_messages(&group.id, &tokens)?,
            None => store.read().messages().search(&tokens)?,
        };
        let found = search::messages(query, &stored, &sessions.listing.items).map_err(usage)?;
        Ok((found, stored.read_only_copy))
    };
    let mut read_only_copy = false;
    let found = if query.kind == Some(RecordKind::Message) {
        let (found, copy) = stored_messages(query)?;
        read_only_copy = copy;
        found
    } else {
        let corpus = Corpus {
            entries: &entries.listing.items,
            sessions: &sessions.listing.items,
            messages: &[],
            tags: &config.tags,
            user: &config.user,
        };
        search::search(query, corpus).map_err(usage)?
    };
    // A default search that finds no entry or session says when stored
    // messages match, since a line the user typed is only ever a message.
    // The count uses the same filters, so the command lists those messages.
    // A tag leaves messages out too: they carry none, so they never stand in.
    let mut message_matches = None;
    if query.kind.is_none() && query.tag.is_none() && found.total == 0 {
        let message_query = SearchQuery {
            kind: Some(RecordKind::Message),
            ..query.clone()
        };
        let (stored, copy) = stored_messages(&message_query)?;
        if stored.total > 0 {
            message_matches = Some(stored.total);
            read_only_copy = copy;
        }
    }
    let mut skipped = entries.skipped;
    skipped.extend(sessions.skipped);
    Ok(Searched {
        found,
        group,
        skipped,
        read_only_copy,
        message_matches,
    })
}
