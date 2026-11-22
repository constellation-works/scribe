//! Store tags and their config-first routing table.
use super::agent::{check_agent_author, refusal};
use super::{Redactor, Store, format, fsio};
use crate::WriteRefusal;
use crate::record::patch;
use crate::{
    Author, AuthorKind, AuthorName, Result, StoreTag, TagDef, TagName, TagPatch, TagTable,
};
use time::OffsetDateTime;

/// Refuse a tag description that is blank or over 300 characters.
///
/// # Errors
///
/// [`WriteRefusal::InvalidText`] for either.
pub fn check_description(description: &str) -> Result<()> {
    if description.trim().is_empty() || description.chars().count() > 300 {
        return Err(refusal(
            WriteRefusal::InvalidText,
            "description must contain 1-300 characters",
        ));
    }
    Ok(())
}

impl Store {
    /// Load the optional agent tag document without writing or locking.
    pub fn store_tags(&self) -> Result<Vec<StoreTag>> {
        let path = self.root.join("tags.json");
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_tags(&path, &fsio::read(&path)?)
    }

    /// The tag a word names (its name or an alias, as `#scribe` resolves
    /// it) with its effective definition, capture rules included, and where
    /// it is defined: `config` (config.toml or the shipped defaults) or
    /// `store` (`tags.json`). Read-only: no lock, no write.
    ///
    /// # Errors
    ///
    /// `tag_not_found` when no tag has that name or alias; a bad config or
    /// store file as when loading either.
    pub fn tag_definition(&self, word: &str) -> Result<(TagDef, &'static str)> {
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
    pub fn config_path(&self) -> std::path::PathBuf {
        self.root.join(super::CONFIG_FILE)
    }

    /// Merge routing definitions, letting config claim names and aliases first.
    pub(super) fn merge_tags(&self, configured: &TagTable) -> Result<(TagTable, Vec<String>)> {
        configured
            .merge_store(&self.store_tags()?)
            .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))
    }

    /// Check a new tag as `add_agent_tag` does, without locking or writing:
    /// the description, a name no configured or stored tag already holds,
    /// and a routing table that stays unambiguous with it.
    ///
    /// # Errors
    ///
    /// `invalid_text` for a bad description; `tag_conflict` saying the tag
    /// already exists (in `config.toml` or the store) or which tag already
    /// holds the name or an alias.
    pub fn check_new_tag(&self, tag: &StoreTag) -> Result<()> {
        check_description(&tag.description)?;
        let configured = self.config_only()?.tags;
        if let Some(held) = configured.resolve(tag.name.as_str()) {
            let message = if held.name == tag.name {
                format!(
                    "tag '{}' already exists; it is configured in {}",
                    tag.name,
                    self.root.join(super::CONFIG_FILE).display()
                )
            } else {
                format!(
                    "'{}' is already an alias of the configured tag '{}'",
                    tag.name, held.name
                )
            };
            return Err(refusal(WriteRefusal::TagConflict, message));
        }
        if self.store_tags()?.iter().any(|t| t.name == tag.name) {
            return Err(refusal(
                WriteRefusal::TagConflict,
                format!(
                    "tag '{}' already exists in {}",
                    tag.name,
                    self.root.join("tags.json").display()
                ),
            ));
        }
        let mut defs = self.config()?.tags.tags().to_vec();
        defs.push(tag.definition());
        TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
        Ok(())
    }

    /// Add an agent tag without changing config.toml.
    pub fn add_agent_tag(&self, mut tag: StoreTag, redactor: &Redactor) -> Result<StoreTag> {
        check_agent_author(&tag.author.name)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("tag add")?;
        self.check_new_tag(&tag)?;
        tag.author.kind = AuthorKind::Agent;
        tag.updated_by = None;
        let mut tags = self.store_tags()?;
        redactor.tag(&mut tag);
        tags.push(tag.clone());
        self.write_tags(&tags)?;
        Ok(tag)
    }

    /// Check a tag update without locking or writing, so the CLI can report
    /// an invalid target or alias before constructing a redactor. The write
    /// path repeats these checks under the store lock.
    pub fn check_agent_tag_update(&self, name: &TagName, patching: &TagPatch) -> Result<()> {
        let configured = self.config_only()?.tags;
        if configured.resolve(name.as_str()).is_some() {
            return Err(refusal(
                WriteRefusal::TagConfigured,
                "edit config.toml to change a configured tag",
            ));
        }
        let tags = self.store_tags()?;
        let mut tag = tags
            .iter()
            .find(|tag| &tag.name == name)
            .cloned()
            .ok_or_else(|| refusal(WriteRefusal::TagNotFound, "no store tag with that name"))?;
        if let Some(description) = &patching.description {
            check_description(description)?;
            tag.description.clone_from(description);
        }
        tag.aliases
            .retain(|alias| configured.resolve(alias.as_str()).is_none());
        patch(
            &mut tag.aliases,
            &patching.add_aliases,
            &patching.remove_aliases,
        );
        if let Some(days) = patching.stale_after_days {
            tag.stale_after_days = Some(days);
        }
        let (merged, _) = configured
            .merge_store(&tags)
            .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
        let mut defs = merged
            .tags()
            .iter()
            .filter(|existing| existing.name != tag.name)
            .cloned()
            .collect::<Vec<_>>();
        defs.push(tag.definition());
        TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
        Ok(())
    }

    /// Update an agent tag under the lock, preserving its creator.
    pub fn update_agent_tag(
        &self,
        name: &TagName,
        patching: TagPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(StoreTag, bool)> {
        check_agent_author(&author)?;
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("tag update")?;
        let configured = self.config_only()?.tags;
        if configured.resolve(name.as_str()).is_some() {
            return Err(refusal(
                WriteRefusal::TagConfigured,
                "edit config.toml to change a configured tag",
            ));
        }
        let mut tags = self.store_tags()?;
        let i = tags
            .iter()
            .position(|t| &t.name == name)
            .ok_or_else(|| refusal(WriteRefusal::TagNotFound, "no store tag with that name"))?;
        let before = tags[i].clone();
        let tag = &mut tags[i];
        tag.aliases
            .retain(|a| configured.resolve(a.as_str()).is_none());
        if let Some(description) = patching.description {
            check_description(&description)?;
            tag.description = redactor.mask(&description);
        }
        patch(
            &mut tag.aliases,
            &patching.add_aliases,
            &patching.remove_aliases,
        );
        if let Some(days) = patching.stale_after_days {
            tag.stale_after_days = Some(days);
        }
        let changed = *tag != before;
        if changed {
            tag.updated = now;
            tag.updated_by = Some(Author::agent(author));
        }
        let result = tag.clone();
        let mut defs = self
            .merge_tags(&configured)?
            .0
            .tags()
            .iter()
            .filter(|t| t.name != result.name)
            .cloned()
            .collect::<Vec<_>>();
        defs.push(result.definition());
        TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
        if changed {
            self.write_tags(&tags)?;
        }
        Ok((result, changed))
    }

    fn write_tags(&self, tags: &[StoreTag]) -> Result<()> {
        let path = self.root.join("tags.json");
        fsio::write_atomic(&path, &format::encode_tags(&path, tags)?)
    }
}
