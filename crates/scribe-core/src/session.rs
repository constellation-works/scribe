//! Session records, the on/off control state, and the rule that marks a
//! session pass due.
//!
//! A session record is one file, `sessions/<yyyy>/<mm>/<id>.md`: YAML
//! frontmatter and four Markdown sections, `## Summary`, `## Goals`,
//! `## Outcome` and `## Next` (docs/design/scribe/2_design.md §3). The text
//! is model-written: `ask` labels it a session summary and never quotes it
//! as Daniel's words. Pure values (STD-02 R14) and no I/O (STD-02 R5).
//!
//! As for entries, every frontmatter key is required and an unknown one is
//! refused; never rename or retype a field without a store format bump.
//!
//! A record's `links` say how the session relates to other sessions
//! (`spawned_from`, `continues`), as Daniel asserted them with
//! `/scribe on --from` or `/scribe link`. A link is stored once, on the
//! child; a parent's children are derived at read time ([`children`]) and
//! the parent record is never rewritten for them. [`check_link`] refuses an
//! unknown target, a self-link and a cycle.

use crate::record::{Label, Listing, Project, Related, Topic, Topics, nullable};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
use std::str::FromStr;
use time::{Duration, OffsetDateTime};

/// Why a value was rejected as a session field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    /// The id had characters outside letters, digits, `.`, `_` and `-`.
    #[error(
        "invalid session id '{0}': use 1-{max} letters, digits, '.', '_' or '-', not starting with '.'",
        max = SessionId::MAX_CHARS
    )]
    InvalidId(String),
    /// A link type other than `spawned_from` or `continues`.
    #[error("invalid link type '{0}': use spawned_from or continues")]
    InvalidLinkType(String),
}

/// A Claude Code session id (a uuid in practice). Also the record's file
/// name, so it is restricted to characters that are safe in one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    /// The longest id accepted, in characters.
    pub const MAX_CHARS: usize = 128;

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SessionId {
    type Error = SessionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let valid = !value.is_empty()
            && value.len() <= Self::MAX_CHARS
            && !value.starts_with('.')
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if valid {
            Ok(Self(value))
        } else {
            Err(SessionError::InvalidId(value))
        }
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

impl FromStr for SessionId {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_owned())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether more passes will update a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    /// The session may still be updated.
    Open,
    /// The session-end pass ran; the record is complete.
    Final,
}

impl SessionStatus {
    /// The canonical lowercase token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Final => "final",
        }
    }
}

/// The four body sections, in file order, with their character budgets.
/// A pass that writes more than a budget is refused (S4); a stored record is
/// read whatever its length.
pub const SECTIONS: [(&str, usize); 4] = [
    ("Summary", 1500),
    ("Goals", 600),
    ("Outcome", 2500),
    ("Next", 1000),
];

/// A phrase Daniel coined or defined in a session, kept in his exact words
/// so `ask` can find where he said it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Term {
    /// The phrase, as Daniel writes it (`graduate to level 4`).
    pub term: String,
    /// A verbatim span of Daniel's own turn that defines or first uses it.
    pub said: String,
    /// The uuid of that turn's message.
    pub message: String,
}

/// How a session relates to the session a link points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    /// A branched, forked or side session points at the session that
    /// started it: its parent.
    SpawnedFrom,
    /// A later session picks up an earlier one's work. Compaction keeps the
    /// session id, so it needs no link.
    Continues,
}

impl LinkType {
    /// The canonical token, as stored and printed.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpawnedFrom => "spawned_from",
            Self::Continues => "continues",
        }
    }
}

impl FromStr for LinkType {
    type Err = SessionError;

    /// `spawned_from` or `continues`; `spawned-from` is accepted too.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "spawned_from" | "spawned-from" => Ok(Self::SpawnedFrom),
            "continues" => Ok(Self::Continues),
            _ => Err(SessionError::InvalidLinkType(s.to_owned())),
        }
    }
}

impl fmt::Display for LinkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A typed link from one session (the child, which stores it) to another.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionLink {
    /// How the child relates to `session`.
    #[serde(rename = "type")]
    pub kind: LinkType,
    /// The session pointed at.
    pub session: SessionId,
}

/// Why a link was refused. Each has a stable code ([`LinkError::code`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// The target is neither a session record nor a session ever switched on.
    #[error("no session {0} is known to scribe: it has no record and was never switched on")]
    UnknownSession(SessionId),
    /// A session cannot link to itself.
    #[error("a session cannot link to itself")]
    SelfLink,
    /// The link would close a loop: the target already leads back here.
    #[error("linking to {0} would make a cycle: it already links back to this session")]
    Cycle(SessionId),
    /// The session already has a `spawned_from` parent, another one.
    #[error("this session is already spawned_from {0}; a session has one parent")]
    Conflict(SessionId),
    /// The session itself is unknown: neither on nor recorded.
    #[error(
        "this session is not cataloged; use /scribe on --from <id> to switch it on and link it"
    )]
    NotCataloged,
}

impl LinkError {
    /// A stable `snake_case` code for machine output and the audit log.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownSession(_) => "link_unknown_session",
            Self::SelfLink => "link_self",
            Self::Cycle(_) => "link_cycle",
            Self::Conflict(_) => "link_conflict",
            Self::NotCataloged => "link_session_off",
        }
    }
}

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

/// A stored session record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    /// The session id; also the file name.
    pub id: SessionId,
    /// Whether more passes will update it.
    pub status: SessionStatus,
    /// The repository the session ran in.
    pub project: Project,
    /// Filter labels the session model chose, reusing existing ones first.
    pub topics: Topics,
    /// References the session mentions, model-extracted.
    pub related: Vec<Related>,
    /// How this session relates to others, as Daniel asserted it; each
    /// link is stored here, on the child, only.
    pub links: Vec<SessionLink>,
    /// Phrases Daniel coined or defined in the session, in his words.
    pub terms: Vec<Term>,
    /// The tool, such as `claude-code`.
    pub provider: Label,
    /// The host it ran on.
    pub host: Label,
    /// When the record was first written.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// When a pass last rewrote it.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
    /// A one-line title, model-written.
    pub title: String,
    /// The session's working directory.
    pub cwd: String,
    /// When the session started; it also names the `<yyyy>/<mm>` directory.
    #[serde(with = "time::serde::rfc3339")]
    pub started: OffsetDateTime,
    /// When Daniel switched cataloging on.
    #[serde(with = "time::serde::rfc3339")]
    pub on_since: OffsetDateTime,
    /// The uuid of the last transcript message folded in.
    #[serde(deserialize_with = "nullable")]
    pub scribed_through: Option<String>,
    /// Successful model record updates, one per persisted window.
    pub passes: u32,
    /// How many items `next` lists.
    pub next_open: u32,
    /// `## Summary`.
    #[serde(skip)]
    pub summary: String,
    /// `## Goals`.
    #[serde(skip)]
    pub goals: String,
    /// `## Outcome`.
    #[serde(skip)]
    pub outcome: String,
    /// `## Next`.
    #[serde(skip)]
    pub next: String,
}

/// Which session records a listing returns. Every field set must match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Only sessions in this project.
    pub project: Option<Project>,
    /// Only sessions carrying this topic.
    pub topic: Option<Topic>,
    /// At most this many of the matching sessions.
    pub limit: Option<NonZeroUsize>,
}

impl SessionFilter {
    /// Whether `record` passes every criterion (the limit aside).
    #[must_use]
    pub fn matches(&self, record: &SessionRecord) -> bool {
        self.project.as_ref().is_none_or(|p| &record.project == p)
            && self
                .topic
                .as_ref()
                .is_none_or(|t| record.topics.contains(t))
    }

    /// Apply the filter to `records`, oldest first (by `started`, then id),
    /// then the limit.
    #[must_use]
    pub fn select(&self, mut records: Vec<SessionRecord>) -> Listing<SessionRecord> {
        records.sort_by(|a, b| a.started.cmp(&b.started).then_with(|| a.id.cmp(&b.id)));
        Listing::select(records, |r| self.matches(r), self.limit)
    }
}

/// One session's cataloging switch, in `control.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionControl {
    /// Whether cataloging is on.
    pub on: bool,
    /// When it was last switched on.
    #[serde(with = "time::serde::rfc3339::option")]
    pub since: Option<OffsetDateTime>,
    /// Whether a pass is due.
    pub due: bool,
    /// Whether the due pass is the session-end one.
    #[serde(rename = "final")]
    pub final_pass: bool,
    /// Consecutive failed session passes; success resets the counter.
    pub consecutive_failures: u32,
    /// Earliest automatic retry; explicit `--session` bypasses it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub retry_after: Option<OffsetDateTime>,
    /// Links Daniel asserted before the session had a record; the next
    /// pass that writes the record moves them onto it and clears this.
    pub pending_links: Vec<SessionLink>,
}

/// Every session's switch, keyed by session id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlState {
    /// The sessions that were ever switched on.
    pub sessions: BTreeMap<SessionId, SessionControl>,
}

impl ControlState {
    /// The switch for `id`; a session never switched on has none.
    #[must_use]
    pub fn get(&self, id: &SessionId) -> Option<&SessionControl> {
        self.sessions.get(id)
    }
}

/// How often an opted-in session gets a pass: `[catalog]` in `config.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Minutes since the last pass.
    pub every_minutes: NonZeroU32,
    /// User turns since the last pass.
    pub every_turns: NonZeroU32,
}

impl Default for Cadence {
    fn default() -> Self {
        Self {
            every_minutes: NonZeroU32::new(20).unwrap_or(NonZeroU32::MIN),
            every_turns: NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN),
        }
    }
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
