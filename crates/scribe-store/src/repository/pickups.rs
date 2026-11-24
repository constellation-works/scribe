//! `pickups.json`: consumers' notes on action entries
//! (docs/design/scribe/2_design.md §4.6).

use super::Pickups;
use crate::Store;
use crate::action::Pickup;
use crate::db::{self, docs};
use crate::error::Result;

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
