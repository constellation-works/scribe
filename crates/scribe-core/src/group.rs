//! Session groups and their named seats. Membership belongs to the transcript chain.

use crate::{Error, Project, Provider, Related, RelatedKind, SessionId, WriteRefusal, instruction};
use serde::Serialize;
use std::collections::BTreeSet;

/// A group and its current roster, in seat-name order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    /// Stable public id, `g-<n>`.
    pub id: String,
    /// Project group slug, when named by a typed command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// User-facing name of this piece of work.
    pub title: String,
    /// Free kind label.
    pub kind: String,
    /// Project slug.
    pub project: String,
    /// Opening time in RFC 3339.
    pub opened: String,
    /// Closing time when the group has closed.
    pub closed: Option<String>,
    /// The closing summary, unless still open or erased after source removal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Provider session in which opening consent was verified.
    pub opener: String,
    /// Task, pull request and repository references in input order.
    pub related: Vec<Related>,
    /// Named seats with at least one member.
    pub seats: Vec<Seat>,
}

impl Group {
    /// Resolve the group's transcript ids for record filters.
    #[must_use]
    pub fn members(&self) -> BTreeSet<SessionId> {
        self.seats
            .iter()
            .flat_map(|seat| seat.transcripts.iter().cloned())
            .collect()
    }
}

/// The result of opening a group.
pub struct OpenedGroup {
    /// The new group's id.
    pub id: String,
    /// Previously opened groups in the same project that are still open.
    pub still_open: Vec<String>,
}

pub use crate::models::CloseEvidence;

/// Whether an entry belongs to one of these transcripts through its source
/// cite or an explicit session reference (as used by handoffs).
#[must_use]
pub fn entry_belongs(entry: &crate::Entry, members: &BTreeSet<SessionId>) -> bool {
    entry
        .cite
        .as_ref()
        .is_some_and(|cite| members.contains(&cite.session))
        || entry
            .related
            .iter()
            .filter(|related| related.kind() == RelatedKind::Session)
            .filter_map(|related| related.target().parse::<SessionId>().ok())
            .any(|id| members.contains(&id))
}

/// The transcript chain of one named member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Seat {
    /// Name within the group.
    pub name: String,
    /// Free role label.
    pub role: String,
    /// Provider session ids, in join order.
    pub transcripts: Vec<SessionId>,
    /// Newest active handoff entry id across the chain.
    pub newest_handoff: Option<String>,
}

/// The metadata to persist on open.
pub struct NewGroup {
    /// User-facing title.
    pub title: String,
    /// Free kind label.
    pub kind: String,
    /// Project slug.
    pub project: Project,
    /// Task, pull request and repository references.
    pub related: Vec<Related>,
}

/// Evidence from the opener's provider-checked transcript. Contains ids only.
pub struct GroupConsent {
    session: SessionId,
    message: String,
    scope: ConsentScope,
}

/// The one operation a verified quote authorizes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ConsentScope {
    /// Opening a group; names no group.
    Open,
    /// Closing exactly this group, its `g-<n>` id.
    Close(String),
}

impl GroupConsent {
    /// Locate the user's own words in the calling session's transcript.
    /// A Claude transcript can contain history copied from another session.
    /// The consent authorizes opening a group and nothing else.
    pub fn verify(
        session: SessionId,
        provider: Provider,
        bytes: &[u8],
        quote: &str,
    ) -> crate::Result<Self> {
        Self::verify_matching(session, provider, bytes, quote, None)
    }

    fn verify_matching(
        session: SessionId,
        provider: Provider,
        bytes: &[u8],
        quote: &str,
        group: Option<&str>,
    ) -> crate::Result<Self> {
        let spoken = match provider {
            Provider::ClaudeCode => instruction::claude_session_conversation(bytes, &session),
            Provider::Codex => instruction::codex_conversation(bytes, &session)?,
        };
        let mut quote_found = false;
        let found = spoken
            .iter()
            .rev()
            .find(|turn| {
                if !(turn.entrypoint.is_none() || turn.interactive) {
                    return false;
                }
                turn.typed.as_ref().is_some_and(|own| {
                    if !crate::verbatim::contains(own, quote) {
                        return false;
                    }
                    quote_found = true;
                    group.is_none_or(|group| names_group(own, quote, group))
                })
            })
            .ok_or_else(|| {
                if let (true, Some(group)) = (quote_found, group) {
                    Error::AgentWrite {
                        kind: WriteRefusal::GroupMembership,
                        message: format!("the quote must contain {group}, the group it closes"),
                    }
                } else {
                    Error::AgentWrite {
                        kind: WriteRefusal::QuoteNotFound,
                        message: "quote a verbatim span of the user's own typed turn in the calling session".into(),
                    }
                }
            })?;
        Ok(Self {
            session,
            message: found.message.uuid.clone(),
            scope: group.map_or(ConsentScope::Open, |group| {
                ConsentScope::Close(group.to_owned())
            }),
        })
    }

    /// [`GroupConsent::verify`] for closing `group`, its canonical `g-<n>` id.
    /// The quote must also name the group as a whole word, so `g-1` does not
    /// match `g-12`. The consent authorizes closing that group only.
    pub fn verify_close(
        session: SessionId,
        provider: Provider,
        bytes: &[u8],
        quote: &str,
        group: &str,
    ) -> crate::Result<Self> {
        Self::verify_matching(session, provider, bytes, quote, Some(group))
    }

    /// Whether this consent was verified for opening a group.
    #[must_use]
    pub fn authorizes_open(&self) -> bool {
        self.scope == ConsentScope::Open
    }

    /// Whether this consent was verified for closing exactly `group`, its
    /// canonical `g-<n>` id.
    #[must_use]
    pub fn authorizes_close(&self, group: &str) -> bool {
        matches!(&self.scope, ConsentScope::Close(named) if named == group)
    }

    /// Provider session in which the user typed the quote.
    pub fn session(&self) -> &SessionId {
        &self.session
    }

    /// Provider message id for the audit.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Whether a matched quote names the group at boundaries in the typed turn.
fn names_group(source: &str, quote: &str, group: &str) -> bool {
    let source = crate::verbatim::normalize(source);
    let quote = crate::verbatim::normalize(quote);
    let quote = quote.trim_matches(' ');
    source
        .char_indices()
        .filter(|(start, _)| source[*start..].starts_with(quote))
        .any(|(start, _)| {
            quote.match_indices(group).any(|(offset, _)| {
                let at = start + offset;
                let before = source[..at].chars().next_back();
                let after = source[at + group.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
        })
}

/// Who asks to close a group: a session that the store judges against the
/// opener's seat chain, or a session whose verified typed quote names the group.
#[derive(Clone, Copy)]
pub enum GroupCloser<'a> {
    /// The calling session, checked against the opener's seat chain.
    Session(&'a SessionId),
    /// The user's own typed words, verified for closing one group. Needs no
    /// seat, and authorizes no other group or operation.
    Quoted(&'a GroupConsent),
}

/// The open group seat a session holds after start checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatHold {
    /// Group id (`g-<n>`).
    pub group: String,
    /// Seat name within the group.
    pub seat: String,
    /// Seat role label.
    pub role: String,
}

/// Outcome of checking group seat inheritance at session start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeatStart {
    /// No open groups exist in the store (or store/schema predates groups).
    NoOpenGroups,
    /// The starting session already holds a seat.
    AlreadySeated(SeatHold),
    /// The starting session once held a seat that a later session took. Its
    /// row stays as history, so it holds nothing and is never appended again.
    Replaced,
    /// Appended the starting session to the predecessor's open seat.
    Inherited {
        /// Seat the session now holds.
        hold: SeatHold,
        /// Predecessor session it inherited from.
        from: SessionId,
    },
    /// Open groups exist and the session holds no seat.
    Unjoined,
}

/// The automatic seat name for `session` when it joins without naming one: its
/// eight-character prefix, its full id, then the first available numbered
/// candidate. A numbered candidate uses the first `Label::MAX_CHARS - 12`
/// characters of the id followed by `-2`, `-3`, and so on; shorter ids use
/// their whole id as the stem. `taken` must say true for every seat already in
/// the group, so a join never displaces anyone.
///
/// # Errors
///
/// Returns the first error reported by `taken` without probing further
/// candidates.
pub fn automatic_seat<E>(
    session: &SessionId,
    mut taken: impl FnMut(&str) -> std::result::Result<bool, E>,
) -> std::result::Result<crate::Label, E> {
    use crate::Label;
    let id = session.as_str();
    let short: String = id.chars().take(8).collect();
    let stem: String = id.chars().take(Label::MAX_CHARS - 12).collect();
    let mut name = short;
    let mut next = [id.to_owned()]
        .into_iter()
        .chain((2u64..).map(|n| format!("{stem}-{n}")));
    loop {
        let label = Label::sanitized(&name, "member");
        if !taken(label.as_str())? {
            return Ok(label);
        }
        name = next.next().unwrap_or_default();
    }
}

/// One session named by a typed `/scribe group add` line: its id, and its typed
/// turns when its transcript was on disk. `None` adds membership only.
pub struct GroupAddition {
    /// The session to add.
    pub session: SessionId,
    /// Its typed turns, selected as the hook selects them, in transcript order.
    pub turns: Option<Vec<crate::message::NewMessage>>,
}

/// What a typed `/scribe group add` did for one session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Added {
    /// The session joined. `stored` is how many typed turns were stored for it,
    /// or `None` when it had no transcript and only membership was added.
    Joined {
        /// Typed turns stored, when its transcript was read.
        stored: Option<usize>,
    },
    /// The session already belonged to this group: nothing changed for it.
    AlreadyMember,
}

/// What a typed `/scribe group add` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupAdded {
    /// The group's id, `g-<n>`.
    pub id: String,
    /// Whether the line opened the group.
    pub created: bool,
    /// Each named session, in the order named.
    pub sessions: Vec<(SessionId, Added)>,
}

/// One caller's request to hold a seat.
pub struct JoinGroup<'a> {
    /// Group id or prefix.
    pub group: &'a str,
    /// Named seat within the group.
    pub seat: &'a crate::Label,
    /// Role for a new seat, otherwise the existing role.
    pub role: Option<&'a str>,
    /// Provider id of the calling session.
    pub session: &'a SessionId,
    /// Agent joining the seat.
    pub author: &'a crate::AuthorName,
    /// Explicit takeover of a live seat.
    pub replace: bool,
    /// Previous holder whose verified transcript has ended, if any.
    pub ended: Option<&'a SessionId>,
    /// Previous holder verified as the caller's own cleared predecessor, if any.
    pub predecessor: Option<&'a SessionId>,
    /// The calling session's typed turns from the start of its own transcript,
    /// chosen as a member's capture chooses them. Stored with the membership;
    /// the last one becomes the capture cursor. Empty when there is no
    /// readable transcript.
    pub earlier_turns: &'a [crate::message::NewMessage],
}
