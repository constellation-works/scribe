//! The session reads that derive a view from the records: a listing with
//! every session's children and titles, and one session with its children
//! and the titles of the sessions it links to.

use crate::record::Listing;
use crate::session::{self as session_mod, Child};
use crate::store::SessionRow;
use crate::{ControlState, Result, SessionFilter, SessionId, SessionRecord, Skipped, Snapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

/// What [`list_sessions`] found.
#[derive(Debug)]
pub struct SessionList {
    /// The matching session records, oldest first.
    pub listing: Listing<SessionRecord>,
    /// Every session's children, derived from all records' links (not only
    /// the listed ones), by the id they link to.
    pub children: BTreeMap<SessionId, Vec<Child>>,
    /// Every record's title, by id, for naming a linked session.
    pub titles: BTreeMap<SessionId, String>,
    /// Every session's on/off switch, to show beside its record.
    pub control: ControlState,
    /// Inactive sessions the filter left out that match its other
    /// criteria; `0` when it includes inactive ones.
    pub inactive_hidden: usize,
    /// Record files or rows left out because they did not parse.
    pub skipped: Vec<Skipped>,
    /// The session ids of the skipped records, where that much reads.
    pub skipped_sessions: Vec<SessionId>,
    /// Every session's `s-<n>`.
    pub numbers: BTreeMap<SessionId, NonZeroU64>,
    /// When each forgotten session was forgotten.
    pub forgotten: BTreeMap<SessionId, time::OffsetDateTime>,
}

/// Session records matching `filter`, oldest first, with the on/off state
/// beside them; with `members`, the records and their derived lookup data
/// are restricted to the group's member sessions. Reads only.
///
/// # Errors
///
/// The store's read errors.
pub fn list_sessions(
    read: &Snapshot<'_>,
    filter: &SessionFilter,
    members: Option<&BTreeSet<SessionId>>,
) -> Result<SessionList> {
    let mut loaded = read.sessions().loaded()?;
    if let Some(members) = members {
        loaded.records.retain(|r| members.contains(&r.id));
        loaded
            .skipped
            .retain(|(id, _)| id.as_ref().is_some_and(|id| members.contains(id)));
    }
    let control = loaded.control;
    let records = loaded.records;
    let skipped_sessions = loaded
        .skipped
        .iter()
        .filter_map(|(id, _)| id.clone())
        .collect();
    let skipped: Vec<Skipped> = loaded.skipped.into_iter().map(|(_, s)| s).collect();
    let numbers = loaded.numbers;
    let forgotten = loaded.forgotten;
    let inactive_hidden = if filter.include_inactive {
        0
    } else {
        let any = SessionFilter {
            include_inactive: true,
            ..filter.clone()
        };
        records
            .iter()
            .filter(|r| !r.active && any.matches(r))
            .count()
    };
    Ok(SessionList {
        children: session_mod::children(&records),
        titles: records
            .iter()
            .map(|r| (r.id.clone(), r.title.clone()))
            .collect(),
        listing: filter.select(records),
        inactive_hidden,
        control,
        skipped,
        skipped_sessions,
        numbers,
        forgotten,
    })
}

/// One session record with its switch.
#[derive(Debug)]
pub struct SessionView {
    /// The record.
    pub record: SessionRecord,
    /// Its switch in `control.json`, if it has one.
    pub control: Option<crate::session::SessionControl>,
    /// The sessions that link to it, derived from their records.
    pub children: Vec<Child>,
    /// The titles of the sessions its links point at that have a record.
    pub linked_titles: BTreeMap<SessionId, String>,
    /// Its `s-<n>`.
    pub number: Option<NonZeroU64>,
    /// When `#scribe forget` erased it, if it did.
    pub forgotten: Option<time::OffsetDateTime>,
}

/// The session record with this id, its switch, the sessions that link to
/// it and the titles of those it links to. Other records that do not parse
/// are left out of the children. Reads only.
///
/// # Errors
///
/// `session_not_found`, or the store's read errors.
pub fn session_view(read: &Snapshot<'_>, id: &SessionId) -> Result<SessionView> {
    let SessionRow {
        record,
        control,
        number,
        forgotten,
        mut others,
    } = read.sessions().get(id)?;
    let linked_titles = others
        .iter()
        .filter(|r| record.links.iter().any(|l| l.session == r.id))
        .map(|r| (r.id.clone(), r.title.clone()))
        .collect();
    others.push(record.clone());
    let children = session_mod::children(&others)
        .remove(id)
        .unwrap_or_default();
    Ok(SessionView {
        record,
        control,
        children,
        linked_titles,
        number,
        forgotten,
    })
}
