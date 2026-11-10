//! Store tags and their config-first routing table.
use super::agent::refusal;
use super::{Redactor, Store, format, fsio};
use crate::WriteRefusal;
use crate::record::patch;
use crate::{Author, AuthorKind, AuthorName, Result, StoreTag, TagName, TagPatch, TagTable};
use time::OffsetDateTime;

impl Store {
    /// Load the optional agent tag document without writing or locking.
    pub fn store_tags(&self) -> Result<Vec<StoreTag>> {
        let path = self.root.join("tags.json");
        if !self.root_exists()? || !fsio::inspect_file(&path)? {
            return Ok(Vec::new());
        }
        format::decode_tags(&path, &fsio::read(&path)?)
    }

    /// Merge routing definitions, letting config claim names and aliases first.
    pub(super) fn merge_tags(&self, configured: &TagTable) -> Result<(TagTable, Vec<String>)> {
        configured
            .merge_store(&self.store_tags()?)
            .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))
    }

    /// Add an agent tag without changing config.toml.
    pub fn add_agent_tag(&self, mut tag: StoreTag, redactor: &Redactor) -> Result<StoreTag> {
        self.prepare_dir(&self.root)?;
        let _lock = self.lock("tag add")?;
        if tag.description.trim().is_empty() || tag.description.chars().count() > 300 {
            return Err(refusal(
                WriteRefusal::InvalidText,
                "description must contain 1-300 characters",
            ));
        }
        tag.author.kind = AuthorKind::Agent;
        tag.updated_by = None;
        let config = self.config()?;
        let mut tags = self.store_tags()?;
        if config.tags.get(&tag.name).is_some() || tags.iter().any(|t| t.name == tag.name) {
            return Err(refusal(WriteRefusal::TagConflict, "tag already exists"));
        }
        let mut defs = config.tags.tags().to_vec();
        defs.push(tag.definition());
        TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
        redactor.tag(&mut tag);
        tags.push(tag.clone());
        self.write_tags(&tags)?;
        Ok(tag)
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
            if description.trim().is_empty() || description.chars().count() > 300 {
                return Err(refusal(
                    WriteRefusal::InvalidText,
                    "description must contain 1-300 characters",
                ));
            }
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
