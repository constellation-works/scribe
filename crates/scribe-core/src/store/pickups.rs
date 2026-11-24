//! `pickups.json`: consumers' notes on action entries, and the reads an
//! intake and the session-start hook make over them
//! (docs/design/scribe/2_design.md §4.6).
use super::Store;
use crate::action::{self, Pickup};
use crate::checks::{check_agent_author, refusal};
use crate::{Author, AuthorName, EntryStatus, Related, Result, TagName, WriteRefusal};
use std::collections::BTreeMap;
use time::OffsetDateTime;

impl Store {
    /// Active action entries older than [`action::STUCK_AFTER`] at `now`
    /// with no pickup, counted per tag. Reads only.
    pub fn stuck_actions(&self, now: OffsetDateTime) -> Result<BTreeMap<TagName, usize>> {
        if !self.root_exists()? {
            return Ok(BTreeMap::new());
        }
        let config = self.config()?;
        let (entries, _) = self.load_entries()?;
        Ok(action::stuck(&entries, &config.tags, &self.pickups()?, now))
    }

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
    pub fn record_pickup(
        &self,
        id: &str,
        author: AuthorName,
        result: Option<Related>,
        rejected: bool,
        now: OffsetDateTime,
    ) -> Result<(Pickup, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        self.write_step(|| {
        let entry = self.find_entry(id)?;
        let config = self.config()?;
        if config.tags.action(&entry.tag).is_none() {
            return Err(refusal(
                WriteRefusal::NotActionTag,
                format!(
                    "{} is not an action tag; only action entries are picked up (run 'scribe tag list')",
                    entry.tag
                ),
            ));
        }
        let mut pickups = self.pickups()?;
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
        self.write_pickups(&pickups)?;
        Ok((pickup, true))
            })
    }
}
