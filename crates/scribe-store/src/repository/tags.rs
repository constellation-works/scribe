//! Store tags and their config-first routing table.
//!
//! The config is read here too: the tags it defines route first.

use super::Tags;
use crate::WriteRefusal;
use crate::db::{self, docs};
use crate::error::refusal;
use crate::error::{Error, Result};
use crate::{CONFIG_FILE, Config, Store, StoreTag, TagDef, fsio};
use scribe_domain::TagTable;

impl Tags<'_> {
    /// The config, with the store tags merged in.
    ///
    /// # Errors
    ///
    /// The config's and the store's read errors.
    pub fn config(&self) -> Result<Config> {
        self.store.config()
    }

    /// The config as `config.toml` (or the shipped defaults) holds it,
    /// without the store tags.
    ///
    /// # Errors
    ///
    /// The config's read errors.
    pub fn configured(&self) -> Result<Config> {
        self.store.config_only()
    }

    /// The store tags (`scribe tag add`).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<StoreTag>> {
        self.store.store_tags()
    }

    /// Replace the store tags.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, tags: &[StoreTag]) -> Result<()> {
        self.store.write_tags(tags)
    }

    /// The definition `word` names, and whether it comes from `config` or
    /// the `store`. Reads only.
    pub fn definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
        self.store.tag_definition(word)
    }

    /// The path of `config.toml`, whether or not it exists.
    #[must_use]
    pub fn config_path(&self) -> std::path::PathBuf {
        self.store.config_path()
    }
}

/// The store tags' stamp file.
pub(crate) const TAGS_FILE: &str = "tags.json";

impl Store {
    /// The configuration: `config.toml` parsed, or the defaults when there
    /// is none. A bad key or value is refused, naming it.
    pub(crate) fn config(&self) -> Result<Config> {
        let mut config = self.config_only()?;
        config.tags = self.merge_tags(&config.tags)?.0;
        Ok(config)
    }

    pub(crate) fn config_only(&self) -> Result<Config> {
        let path = self.root.join(CONFIG_FILE);
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Config::default());
        }
        let bytes = fsio::read(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        let config = Config::parse(text).map_err(|source| Error::Config { path, source })?;
        // Keys this build no longer reads: logged once per process, however
        // often a command loads the config.
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !config.warnings.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            for warning in &config.warnings {
                tracing::warn!("{warning}");
            }
        }
        Ok(config)
    }

    /// The store tags (`scribe tag add`), without writing.
    pub(crate) fn store_tags(&self) -> Result<Vec<StoreTag>> {
        let mut tags = self.read_or_empty(docs::tags)?;
        tags.retain(|tag| !crate::entry::is_decision_or_item(tag.name.as_str()));
        Ok(tags)
    }

    /// Replace the store tags, in the write under way.
    pub(crate) fn write_tags(&self, tags: &[StoreTag]) -> Result<()> {
        self.with_conn(|conn, path| {
            docs::put_tags(conn, path, tags, |e| db::db_error("write tags to", path, e))
        })
    }
}

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
    pub(crate) fn tag_definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
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
    pub(crate) fn config_path(&self) -> std::path::PathBuf {
        self.root.join(crate::CONFIG_FILE)
    }

    /// Merge routing definitions, letting config claim names and aliases first.
    pub(crate) fn merge_tags(&self, configured: &TagTable) -> Result<(TagTable, Vec<String>)> {
        configured
            .merge_store(&self.store_tags()?)
            .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))
    }
}
