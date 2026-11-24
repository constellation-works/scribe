//! The group shapes the store keeps and hands its callers: a group with its
//! named seats, a new group's draft, the seat a session holds, and what a
//! member's Claude capture has seen of a message id.

use crate::{Project, Related, SessionId};
use serde::Serialize;
use std::collections::BTreeSet;

/// The group metadata and summary carried by a project brief.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LastGroup {
    /// Stable group id.
    pub id: String,
    /// Group title.
    pub title: String,
    /// Closing time, if closed.
    pub closed: Option<String>,
    /// Summary produced at close, if still retained.
    pub summary: Option<String>,
}

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

/// The first origin claim and whether a parseable message has already been
/// seen for an id. These are separate because a metadata line can supply
/// claims even when it does not produce a message.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaudeSeen {
    /// The first origin claim for the id: whether it permits the turn.
    pub permitted: Option<bool>,
    /// Whether a parseable message has been seen for the id.
    pub message: bool,
}
