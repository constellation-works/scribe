//! The session records' rules (ruling (f)): whether a link may be added
//! (`check_link`), the children and tree derived from the records' links,
//! and the due rule a switched-on session's cadence applies
//! (`Cadence::pass_due`). Pure values and no I/O (STD-02 R5).

use crate::session::{
    Cadence, LinkError, LinkType, SessionControl, SessionId, SessionLink, SessionRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use time::{Duration, OffsetDateTime};

/// Whether `link` may be added to `child`'s links. `known` holds every
/// session scribe knows (a record or a switch); `graph` every session's
/// links, recorded or pending. `Ok(false)` means the link is already there
/// and nothing changes; `Ok(true)` that it is new and valid.
///
/// # Errors
///
/// The target is `child` itself, unknown, would close a cycle (following
/// links of either type from the target reaches `child`), or `child`
/// already has another `spawned_from` parent.
pub fn check_link(
    child: &SessionId,
    link: &SessionLink,
    known: &BTreeSet<SessionId>,
    graph: &BTreeMap<SessionId, Vec<SessionLink>>,
) -> Result<bool, LinkError> {
    if &link.session == child {
        return Err(LinkError::SelfLink);
    }
    if !known.contains(&link.session) {
        return Err(LinkError::UnknownSession(link.session.clone()));
    }
    let own = graph.get(child).map_or(&[][..], Vec::as_slice);
    if own.contains(link) {
        return Ok(false);
    }
    if link.kind == LinkType::SpawnedFrom
        && let Some(parent) = own.iter().find(|l| l.kind == LinkType::SpawnedFrom)
    {
        return Err(LinkError::Conflict(parent.session.clone()));
    }
    // Walk every link out of the target; reaching the child is a loop.
    let mut seen: BTreeSet<&SessionId> = BTreeSet::new();
    let mut stack = vec![&link.session];
    while let Some(at) = stack.pop() {
        if at == child {
            return Err(LinkError::Cycle(link.session.clone()));
        }
        if seen.insert(at) {
            stack.extend(graph.get(at).into_iter().flatten().map(|l| &l.session));
        }
    }
    Ok(true)
}

/// A session that links to another one, seen from the one it links to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    /// How the child relates to this session.
    pub kind: LinkType,
    /// The child session.
    pub session: SessionId,
    /// The child's record title.
    pub title: String,
}

/// Every session's children, derived from the records' links: for each
/// linked-to id, the records that link to it, oldest first (by `started`,
/// then id). A session with no children has no key.
#[must_use]
pub fn children(records: &[SessionRecord]) -> BTreeMap<SessionId, Vec<Child>> {
    let mut sorted: Vec<&SessionRecord> = records.iter().collect();
    sorted.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
    let mut out: BTreeMap<SessionId, Vec<Child>> = BTreeMap::new();
    for record in sorted {
        for link in &record.links {
            out.entry(link.session.clone()).or_default().push(Child {
                kind: link.kind,
                session: record.id.clone(),
                title: record.title.clone(),
            });
        }
    }
    out
}

/// The session a record nests under in a tree: its `spawned_from` parent,
/// else the first session it `continues`.
#[must_use]
pub fn tree_parent(record: &SessionRecord) -> Option<&SessionId> {
    record
        .links
        .iter()
        .find(|l| l.kind == LinkType::SpawnedFrom)
        .or_else(|| record.links.first())
        .map(|l| &l.session)
}

/// `records` in tree order with each one's depth: every root (a record
/// whose tree parent is not among `records`) in the given order, each
/// followed depth-first by the records nested under it, in the given order.
/// Every record appears exactly once; links never form a cycle, and a
/// record reached twice is not repeated.
#[must_use]
pub fn tree_order(records: &[SessionRecord]) -> Vec<(usize, usize)> {
    let index: BTreeMap<&SessionId, usize> = records
        .iter()
        .enumerate()
        .map(|(i, r)| (&r.id, i))
        .collect();
    let mut nested: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut roots = Vec::new();
    for (i, record) in records.iter().enumerate() {
        match tree_parent(record).and_then(|p| index.get(p)) {
            Some(&parent) if parent != i => nested.entry(parent).or_default().push(i),
            Some(_) | None => roots.push(i),
        }
    }
    let mut out = Vec::with_capacity(records.len());
    let mut placed = vec![false; records.len()];
    let mut stack: Vec<(usize, usize)> = roots.into_iter().rev().map(|i| (0, i)).collect();
    loop {
        while let Some((depth, i)) = stack.pop() {
            if std::mem::replace(&mut placed[i], true) {
                continue;
            }
            out.push((depth, i));
            if let Some(kids) = nested.get(&i) {
                stack.extend(kids.iter().rev().map(|&k| (depth + 1, k)));
            }
        }
        // A loop with no root (only hand-edited files can make one) still
        // lists each of its records once, as roots.
        match placed.iter().position(|p| !p) {
            Some(i) => stack.push((0, i)),
            None => break,
        }
    }
    out
}

impl Cadence {
    /// The due rule the `Stop` hook applies after each reply: a session that
    /// is on needs a pass once `every_minutes` have passed since
    /// `last_pass`, or `every_turns` user turns have arrived since it. A
    /// session that is off is never due; one already due stays due.
    #[must_use]
    pub fn pass_due(
        &self,
        control: &SessionControl,
        last_pass: OffsetDateTime,
        now: OffsetDateTime,
        turns_since: u32,
    ) -> bool {
        if !control.on {
            return false;
        }
        let waited = now - last_pass >= Duration::minutes(i64::from(self.every_minutes.get()));
        control.due || waited || turns_since >= self.every_turns.get()
    }
}
