//! Session groups and their named seats. Membership belongs to the transcript chain.

use crate::{Error, Project, Provider, Related, SessionId, WriteRefusal, instruction};
use serde::Serialize;

/// A group and its current roster, in seat-name order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Group {
    /// Stable public id, `g-<n>`.
    pub id: String,
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
    /// Provider session in which opening consent was verified.
    pub opener: String,
    /// Task, pull request and repository references in input order.
    pub related: Vec<Related>,
    /// Named seats with at least one member.
    pub seats: Vec<Seat>,
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
}

impl GroupConsent {
    /// Locate the user's own words in the calling session's transcript.
    /// A Claude transcript can contain history copied from another session.
    pub fn verify(
        session: SessionId,
        provider: Provider,
        bytes: &[u8],
        quote: &str,
    ) -> crate::Result<Self> {
        let spoken = match provider {
            Provider::ClaudeCode => {
                let mut local = Vec::new();
                for line in bytes.split(|b| *b == b'\n') {
                    let Ok(row) = serde_json::from_slice::<serde_json::Value>(line) else {
                        continue;
                    };
                    if row
                        .get("sessionId")
                        .is_some_and(|id| id.as_str() != Some(session.as_str()))
                    {
                        continue;
                    }
                    local.extend_from_slice(line);
                    local.push(b'\n');
                }
                instruction::claude_conversation(&local)
            }
            Provider::Codex => instruction::codex_conversation(bytes, &session)?,
        };
        let found = spoken
            .iter()
            .rev()
            .find(|turn| {
                (turn.entrypoint.is_none() || turn.interactive)
                    && turn
                        .typed
                        .as_ref()
                        .is_some_and(|own| crate::verbatim::contains(own, quote))
            })
            .ok_or_else(|| Error::AgentWrite {
                kind: WriteRefusal::QuoteNotFound,
                message:
                    "quote a verbatim span of the user's own typed turn in the calling session"
                        .into(),
            })?;
        Ok(Self {
            session,
            message: found.message.uuid.clone(),
        })
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
}
