//! Pickup verbs: a consumer's note on an action entry (2_design.md §4.6).

use crate::action::{self, Pickup};

use super::written::{EntryWritten, written};
use crate::apps::tag::check_agent_author;
use crate::runtime::errors::WriteError;
use crate::runtime::{Ctx, PortError, resolve};
use crate::store::refusal;
use crate::{Author, EntryStatus, Result, Store};
use crate::{AuthorName, Provider, Related, WriteRefusal};
use time::OffsetDateTime;

/// The pickup verbs, implemented for [`Store`].
pub trait PickupVerbs {
    /// Record that the consumer `author` picked up the action entry `id`
    /// (an id or a unique prefix), with the record it made, or with
    /// `rejected`, that Daniel rejected that result. The same note again
    /// changes nothing. Refused for an entry under a tag that is not an
    /// action tag; for an agent-written entry still inside its cancel
    /// window; and, unless `rejected`, for an entry that is not active (a
    /// retired entry is cancelled, so nothing should be made of it).
    ///
    /// # Errors
    ///
    /// `reserved_author`, `entry_not_found`, `not_action_tag`,
    /// `cancel_window` or `entry_not_active`, and store failures.
    fn record_pickup(
        &self,
        id: &str,
        author: AuthorName,
        result: Option<Related>,
        rejected: bool,
        now: OffsetDateTime,
    ) -> Result<(Pickup, bool)>;
}

impl PickupVerbs for Store {
    fn record_pickup(
        &self,
        id: &str,
        author: AuthorName,
        result: Option<Related>,
        rejected: bool,
        now: OffsetDateTime,
    ) -> Result<(Pickup, bool)> {
        check_agent_author(&author)?;
        self.write(|tx| {
            let entry = tx.entries().resolve(id)?;
            let config = tx.tags().config()?;
            if config.tags.action(&entry.tag).is_none() {
                return Err(refusal(
                    WriteRefusal::NotActionTag,
                    format!(
                        "{} is not an action tag; only action entries are picked up (run 'scribe tag list')",
                        entry.tag
                    ),
                ));
            }
            let mut pickups = tx.pickups().all()?;
            let state = action::state(&entry, &pickups, now);
            if !state.past_cancel_window {
                return Err(refusal(
                    WriteRefusal::CancelWindow,
                    format!(
                        "{} is inside its cancel window; pick it up after {}",
                        entry.id,
                        state
                            .cancel_window_ends
                            .and_then(|t| t
                                .format(&time::format_description::well_known::Rfc3339)
                                .ok())
                            .unwrap_or_default()
                    ),
                ));
            }
            if !rejected && entry.status != EntryStatus::Active {
                return Err(refusal(
                    WriteRefusal::EntryNotActive,
                    format!(
                        "{} is {}; a retired or replaced entry is not picked up",
                        entry.id, entry.status
                    ),
                ));
            }
            let author = Author::agent(author);
            if let Some(found) = pickups.iter().find(|p| {
                p.entry == entry.id
                    && p.author == author
                    && p.rejected == rejected
                    && p.result == result
            }) {
                return Ok((found.clone(), false));
            }
            let pickup = Pickup {
                entry: entry.id,
                author,
                at: now,
                result,
                rejected,
            };
            pickups.push(pickup.clone());
            tx.pickups().put_all(&pickups)?;
            Ok((pickup, true))
        })
    }
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
