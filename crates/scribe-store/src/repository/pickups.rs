//! `pickups.json`: consumers' notes on action entries, and the reads an
//! intake and the session-start hook make over them
//! (docs/design/scribe/2_design.md §4.6).

use super::Pickups;
use crate::action::Pickup;
use crate::db::{self, docs};
use crate::error::Result;
use crate::{Store, TagName, action};
use std::collections::BTreeMap;
use time::OffsetDateTime;

impl Pickups<'_> {
    /// Every pickup, oldest first.
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<Pickup>> {
        self.store.pickups()
    }

    /// Replace the pickups.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, pickups: &[Pickup]) -> Result<()> {
        self.store.write_pickups(pickups)
    }

    /// Active action entries past the stuck age at `now` with no pickup,
    /// counted per tag. Reads only.
    pub fn stuck_actions(&self, now: OffsetDateTime) -> Result<BTreeMap<TagName, usize>> {
        self.store.stuck_actions(now)
    }
}

impl Store {
    /// Every pickup, oldest first; none for a missing store. Reads only.
    pub(crate) fn pickups(&self) -> Result<Vec<Pickup>> {
        self.read_or_empty(docs::pickups)
    }

    /// Replace the pickups, in the write under way.
    pub(crate) fn write_pickups(&self, pickups: &[Pickup]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_pickups(conn, path, pickups, |e| {
                db::db_error("write pickups to", path, e)
            })
        })
    }
}

impl Store {
    /// Active action entries older than [`action::STUCK_AFTER`] at `now`
    /// with no pickup, counted per tag. Reads only.
    pub(crate) fn stuck_actions(&self, now: OffsetDateTime) -> Result<BTreeMap<TagName, usize>> {
        if !self.root_exists()? {
            return Ok(BTreeMap::new());
        }
        let config = self.config()?;
        let (entries, _) = self.load_entries()?;
        Ok(action::stuck(&entries, &config.tags, &self.pickups()?, now))
    }
}
