//! `pickups.json`: consumers' notes on action entries, and the reads an
//! intake and the session-start hook make over them
//! (docs/design/scribe/2_design.md §4.6).
use super::Store;
use crate::action;
use crate::{Result, TagName};
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
}
