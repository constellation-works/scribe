//! Store tags and their config-first routing table.
use super::Store;
use crate::WriteRefusal;
use crate::checks::refusal;
use crate::{Result, TagDef, TagTable};

impl Store {
    /// The tag a word names (its name or an alias, as `#scribe` resolves
    /// it) with its effective definition, capture rules included, and where
    /// it is defined: `config` (config.toml or the shipped defaults) or
    /// `store` (`tags.json`). Read-only: no lock, no write.
    ///
    /// # Errors
    ///
    /// `tag_not_found` when no tag has that name or alias; a bad config or
    /// store file as when loading either.
    pub(in crate::store) fn tag_definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
        let configured = self.config_only()?.tags;
        if let Some(def) = configured.resolve(word) {
            return Ok((def.clone(), "config"));
        }
        let (table, _) = self.merge_tags(&configured)?;
        table
            .resolve(word)
            .map(|def| (def.clone(), "store"))
            .ok_or_else(|| {
                refusal(
                    WriteRefusal::TagNotFound,
                    "no tag has that name or alias; see scribe tag list",
                )
            })
    }

    /// The path of `config.toml`, whether or not it exists.
    #[must_use]
    pub(in crate::store) fn config_path(&self) -> std::path::PathBuf {
        self.root.join(super::CONFIG_FILE)
    }

    /// Merge routing definitions, letting config claim names and aliases first.
    pub(super) fn merge_tags(&self, configured: &TagTable) -> Result<(TagTable, Vec<String>)> {
        configured
            .merge_store(&self.store_tags()?)
            .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))
    }
}
