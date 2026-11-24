//! The configuration as the commands read it: `config.toml` parsed
//! (`crate::config`), with the store tags merged in, config claiming names
//! and aliases first.

use super::rules::tags::MergeStore;
use crate::config::Config;
use crate::repository::Tags;
use crate::store::refusal;
use crate::{Result, TagDef, TagTable, WriteRefusal};

/// The config reads on the store's tags handle.
pub trait ConfigReads {
    /// The config, with the store tags merged in.
    ///
    /// # Errors
    ///
    /// The config's and the store's read errors.
    fn config(&self) -> Result<Config>;

    /// The config as `config.toml` (or the shipped defaults) holds it,
    /// without the store tags.
    ///
    /// # Errors
    ///
    /// The config's read errors.
    fn configured(&self) -> Result<Config>;

    /// The tag a word names (its name or an alias, as `#scribe` resolves
    /// it) with its effective definition, capture rules included, and where
    /// it is defined: `config` (config.toml or the shipped defaults) or
    /// `store` (`tags.json`). Read-only: no lock, no write.
    ///
    /// # Errors
    ///
    /// `tag_not_found` when no tag has that name or alias; a bad config or
    /// store file as when loading either.
    fn definition(&self, word: &str) -> Result<(TagDef, &'static str)>;
}

impl ConfigReads for Tags<'_> {
    fn config(&self) -> Result<Config> {
        let mut config = self.configured()?;
        config.tags = merge_tags(self, &config.tags)?.0;
        Ok(config)
    }

    fn configured(&self) -> Result<Config> {
        crate::config::load(self.config_text()?)
    }

    fn definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
        let configured = self.configured()?.tags;
        if let Some(def) = configured.resolve(word) {
            return Ok((def.clone(), "config"));
        }
        let (table, _) = merge_tags(self, &configured)?;
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
}

/// Merge routing definitions, letting config claim names and aliases first.
fn merge_tags(tags: &Tags<'_>, configured: &TagTable) -> Result<(TagTable, Vec<String>)> {
    configured
        .merge_store(&tags.all()?)
        .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))
}

impl crate::composition::Context {
    /// Config with provider-specific model defaults resolved.
    ///
    /// # Errors
    ///
    /// When the store or `config.toml` cannot be read.
    #[cfg(any(test, feature = "test-support"))]
    pub fn config(&self) -> Result<Config, crate::CommandError> {
        Ok(self
            .store()
            .read()
            .tags()
            .config()?
            .for_provider(self.provider()))
    }
}
