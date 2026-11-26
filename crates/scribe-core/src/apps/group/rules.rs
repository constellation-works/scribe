//! The group app's rules (2_design.md §17):
//!
//! - the checks of a group write that run before its transaction: the
//!   consent, the turns a member brings, the names it uses. The checks that
//!   read the group's rows run inside its write;
//! - the group app's requests and outcomes, the consent a quoted turn gives,
//!   and the automatic seat name.

use crate::apps::entry::check_content;
use crate::apps::tag::check_agent_author;
use crate::message::NewMessage;
use crate::store::group_number as number;
use crate::store::refusal;
use crate::store::{NewGroup, SeatHold};
use crate::{Error, Provider, Redactor, RelatedKind, Result, SessionId, WriteRefusal, instruction};

/// A project group slug: lowercase letters, digits and interior hyphens, not a
/// group id, and free of configured sensitive text.
pub fn check_project_slug(slug: &str, redactor: &Redactor) -> Result<()> {
    if slug.is_empty()
        || slug.len() > 64
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        || slug.starts_with('-')
        || slug.ends_with('-')
        || number(slug).is_some()
    {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug must be lowercase letters, digits and interior hyphens, and not a group id",
        ));
    }
    if redactor.mask(slug) != slug {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "group slug contains sensitive text",
        ));
    }
    Ok(())
}

/// A backfill takes only the named session's own user turns.
pub fn check_own_user_turns(session: &SessionId, turns: &[NewMessage]) -> Result<()> {
    if turns
        .iter()
        .any(|m| &m.session != session || m.role != crate::Speaker::User)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group backfill takes only the member's own user turns",
        ));
    }
    Ok(())
}

/// A group opens on consent given for opening one, with the opener's own
/// user turns, a title and kind as content, and no session among its
/// references.
pub fn check_new_group(
    draft: &NewGroup,
    consent: &GroupConsent,
    earlier_turns: &[NewMessage],
) -> Result<()> {
    if !consent.authorizes_open() {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "the quoted consent was given for another operation, not opening a group",
        ));
    }
    check_own_user_turns(consent.session(), earlier_turns)?;
    check_content(&draft.title)?;
    check_content(&draft.kind)?;
    if draft
        .related
        .iter()
        .any(|r| r.kind() == RelatedKind::Session)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group references accept task:, pr: and repo:; use group join for a session",
        ));
    }
    Ok(())
}

/// A project-group backfill takes only each named session's user turns.
pub fn check_additions(additions: &[GroupAddition]) -> Result<()> {
    if additions.iter().any(|addition| {
        addition.turns.as_ref().is_some_and(|turns| {
            turns
                .iter()
                .any(|turn| turn.session != addition.session || turn.role != crate::Speaker::User)
        })
    }) {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "project group backfill takes only the named session's user turns",
        ));
    }
    Ok(())
}

/// A join names an agent author that is not reserved, a role that is
/// content, and the member's own user turns.
pub fn check_join(join: &JoinGroup<'_>) -> Result<()> {
    check_agent_author(join.author)?;
    if let Some(role) = join.role {
        check_content(role)?;
    }
    check_own_user_turns(join.session, join.earlier_turns)
}

/// A capture takes only the member's own verified user turns.
pub fn check_capture_turns(session: &SessionId, messages: &[NewMessage]) -> Result<()> {
    if messages
        .iter()
        .any(|m| &m.session != session || m.role != crate::Speaker::User)
    {
        return Err(refusal(
            WriteRefusal::GroupMembership,
            "group capture takes only the member's verified user turns",
        ));
    }
    Ok(())
}

// --- group requests and outcomes ---

/// The result of opening a group.
pub struct OpenedGroup {
    /// The new group's id.
    pub id: String,
    /// Previously opened groups in the same project that are still open.
    pub still_open: Vec<String>,
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
        let mut clipped = false;
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
                    let Some(group) = group else {
                        return true;
                    };
                    if names_group(own, quote, group) {
                        return true;
                    }
                    // The quote holds the id, but where it matches here the
                    // typed turn extends it into a longer word, such as g-12
                    // for g-1: the extra character may sit inside the quote
                    // or just past its edge. Another span of this turn may
                    // name the id whole; that span was not quoted, so it does
                    // not authorize this.
                    clipped |= crate::verbatim::normalize(quote).contains(group);
                    false
                })
            })
            .ok_or_else(|| {
                if let (true, Some(group)) = (quote_found, group) {
                    let message = if clipped {
                        format!(
                            "the quote must name {group} as a whole id, the group it closes; {group} is not standalone in the matched span, where the typed turn extends it into a longer id, so quote a span where {group} stands alone"
                        )
                    } else {
                        format!("the quote must contain {group}, the group it closes")
                    };
                    Error::AgentWrite {
                        kind: WriteRefusal::GroupMembership,
                        message,
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
    /// When the caller took the snapshot `earlier_turns` came from, before it
    /// read any transcript. It is the membership's join time, so a member that
    /// stored no earlier turn (and so has no cursor) still keeps a typed turn
    /// appended after the snapshot but before the membership committed.
    pub snapshot_at: time::OffsetDateTime,
}
