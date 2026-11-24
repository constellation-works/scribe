//! `scribe entry` writes (ORB-27707 step 3d): `add`, `pickup`, `update`,
//! `retire`, `forget`, `answer` and `close`, each checked before the store
//! lock where it can be, and again by the store under it.

use crate::action::{self, ActionState, Pickup};
use crate::cases::entry::EntryVerbs;
use crate::cases::pickup::PickupVerbs;
use crate::cases::write_error::{WriteError, redactor};
use crate::cases::{Ctx, PortError, resolve};
use crate::{AuthorName, Entry, EntryId, EntryPatch, Provider, Related, TagTable};
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

fn written<E>(
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

/// What `entry pickup` runs.
pub struct PickupInput<'i> {
    /// The action entry's id or a unique prefix.
    pub id: &'i str,
    /// The consumer.
    pub author: AuthorName,
    /// What the consumer made of it.
    pub result: Option<&'i Related>,
    /// The user rejected the result.
    pub rejected: bool,
    /// The provider whose transcripts resolve a `session:` result.
    pub provider: Provider,
}

/// `entry pickup`.
///
/// # Errors
///
/// [`WriteError`]: a refusal or a store error.
pub fn pickup<E: PortError>(
    ctx: &Ctx<'_, E>,
    input: PickupInput<'_>,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&input.author)?;
    let result = input
        .result
        .map(|r| resolve::related(ctx, input.provider, r))
        .transpose()?;
    let (pickup, changed) = ctx.store.record_pickup(
        input.id,
        input.author,
        result,
        input.rejected,
        ctx.clock.now(),
    )?;
    let entry = ctx.store.read().entries().get(&pickup.entry)?;
    written(ctx, entry, changed)
}

/// `entry update`: `patch`'s references are resolved here, the ones to
/// remove kept as given too.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn update<E: PortError>(
    ctx: &Ctx<'_, E>,
    id: &str,
    mut patch: EntryPatch,
    author: AuthorName,
    provider: Provider,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    patch.add_related = resolve::relateds(ctx, provider, &patch.add_related)?;
    patch.remove_related =
        resolve::relateds_preserving_input(ctx, provider, &patch.remove_related)?;
    ctx.store.check_agent_entry_update(id, &patch)?;
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .update_agent_entry(id, patch, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}

/// `entry close`: settle an open item with a decision or a note.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn close<E>(
    ctx: &Ctx<'_, E>,
    id: &str,
    decision: Option<&EntryId>,
    note: Option<&str>,
    author: AuthorName,
) -> Result<EntryWritten, WriteError<E>> {
    crate::check_agent_author(&author)?;
    if let Some(note) = note {
        crate::check_reason(note)?;
    }
    let redactor = redactor(ctx)?;
    let (entry, changed) =
        ctx.store
            .close_open_item(id, decision, note, author, ctx.clock.now(), &redactor)?;
    written(ctx, entry, changed)
}
