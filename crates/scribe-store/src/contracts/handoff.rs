//! The handoff record's shape (docs/design/scribe/2_design.md §16): the
//! shipped tag, the six headings and the size limit, and the `session:`
//! references an entry carries. The checks a handoff write must pass are the
//! entry app's (`scribe-core`'s `apps/entry/rules/handoff.rs`).

use crate::entry::Entry;
use crate::record::RelatedKind;
use crate::session::SessionId;

/// The shipped tag name for agent-written session handoffs.
pub const HANDOFF_TAG: &str = "handoff";

pub use crate::entry::HandoffMode as Mode;

/// The six required level-2 Markdown headings, in exact order.
pub const HEADINGS: [&str; 6] = [
    "State",
    "In flight",
    "Next",
    "Waiting on you",
    "Do not touch",
    "Pointers",
];

/// The longest handoff body accepted, in characters, so a handoff fits one
/// screen (~40 lines at 75 columns).
pub const MAX_CHARS: usize = 3000;

impl Entry {
    /// Every `session:<id>` reference an entry carries that parses as a
    /// [`SessionId`].
    pub fn session_ids(&self) -> Vec<SessionId> {
        let mut ids = Vec::new();
        for rel in &self.related {
            if rel.kind() == RelatedKind::Session
                && let Ok(id) = rel.target().parse::<SessionId>()
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids
    }
}
