//! What an entry write reports: the entry, whether it changed, its action
//! state and notices.

use crate::action::{self, ActionState, Pickup};
use crate::runtime::Ctx;
use crate::runtime::errors::WriteError;
use crate::{Entry, TagTable};
use time::OffsetDateTime;

/// What an entry command needs to report action state: the tag table, the
/// pickups and the time.
pub struct Actions {
    tags: TagTable,
    pickups: Vec<Pickup>,
    now: OffsetDateTime,
}

impl Actions {
    /// Read them.
    ///
    /// # Errors
    ///
    /// A store read error.
    pub fn load<E>(ctx: &Ctx<'_, E>) -> crate::Result<Self> {
        Ok(Self {
            tags: ctx.store.read().tags().config()?.tags,
            pickups: ctx.store.read().pickups().all()?,
            now: ctx.clock.now(),
        })
    }

    /// `entry`'s action state when its tag is an action tag.
    #[must_use]
    pub fn state(&self, entry: &Entry) -> Option<ActionState> {
        self.tags
            .action(&entry.tag)
            .map(|_| action::state(entry, &self.pickups, self.now))
    }
}

/// What an entry write did: the entry, whether it changed, its action state
/// read after the write, and notices.
#[derive(Debug)]
pub struct EntryWritten {
    /// The entry as written.
    pub entry: Entry,
    /// Whether the write changed it.
    pub changed: bool,
    /// Its action state, when its tag is an action tag.
    pub action: Option<ActionState>,
    /// Notices for the user.
    pub notices: Vec<String>,
}

pub(super) fn written<E>(
    ctx: &Ctx<'_, E>,
    entry: Entry,
    changed: bool,
) -> Result<EntryWritten, WriteError<E>> {
    let action = Actions::load(ctx)?.state(&entry);
    Ok(EntryWritten {
        entry,
        changed,
        action,
        notices: Vec::new(),
    })
}
