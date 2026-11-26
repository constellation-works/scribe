//! Store tags, and the text of `config.toml`.
//!
//! The config is parsed and its tags merged with these in scribe-core
//! (`config`, `apps/tag`).

use super::Tags;
use super::{Access, ReadWrite};
use crate::db::files;
use crate::db::{self, docs};
use crate::error::{Error, Result};
use crate::{CONFIG_FILE, Store, StoreTag};

impl<A: Access> Tags<'_, A> {
    /// The text of `config.toml` and its path, or `None` when there is
    /// none. Core parses it (`scribe_core::config`).
    ///
    /// # Errors
    ///
    /// The file's read errors; `store_corrupt` when it is not UTF-8.
    pub fn config_text(&self) -> Result<Option<(std::path::PathBuf, String)>> {
        self.store.config_text()
    }

    /// The store tags (`scribe tag add`).
    ///
    /// # Errors
    ///
    /// The store's read errors.
    pub fn all(&self) -> Result<Vec<StoreTag>> {
        self.store.store_tags()
    }

    /// The path of `config.toml`, whether or not it exists.
    #[must_use]
    pub fn config_path(&self) -> std::path::PathBuf {
        self.store.config_path()
    }
}

/// The writes, on the repository a [`Tx`](super::Tx) gives: a
/// [`Snapshot`](super::Snapshot)'s cannot write.
impl Tags<'_, ReadWrite> {
    /// Replace the store tags.
    ///
    /// # Errors
    ///
    /// The store's write errors.
    pub fn put_all(&self, tags: &[StoreTag]) -> Result<()> {
        self.store.write_tags(tags)
    }
}

/// The store tags' stamp file.
pub(crate) const TAGS_FILE: &str = "tags.json";

impl Store {
    pub(crate) fn config_text(&self) -> Result<Option<(std::path::PathBuf, String)>> {
        let path = self.root.join(CONFIG_FILE);
        if !self.root_exists()? || !cw_fs::inspect_file(&path)? {
            return Ok(None);
        }
        let bytes = files::read(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|e| Error::corrupt(&path, e))?;
        Ok(Some((path, text.to_owned())))
    }

    /// The store tags (`scribe tag add`), without writing.
    pub(crate) fn store_tags(&self) -> Result<Vec<StoreTag>> {
        let mut tags = self.read_or_empty(docs::tags)?;
        tags.retain(|tag| !tag.name.is_decision_or_item());
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
    /// The path of `config.toml`, whether or not it exists.
    #[must_use]
    pub(crate) fn config_path(&self) -> std::path::PathBuf {
        self.root.join(crate::CONFIG_FILE)
    }
}
