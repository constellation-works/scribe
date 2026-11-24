//! Tag verbs: adding and updating an agent's store tag, and the checks the
//! CLI runs before it builds a redactor (`scribe tag add`, `scribe tag
//! update`).

use crate::checks::tag::check_description;
use crate::checks::{check_agent_author, refusal};
use crate::record::patch;
use crate::repo::Tags;
use crate::{
    Author, AuthorKind, AuthorName, Redactor, Result, Store, StoreTag, TagName, TagPatch, TagTable,
    WriteRefusal,
};
use time::OffsetDateTime;

/// The tag verbs, implemented for [`Store`].
pub trait TagVerbs {
    /// Check a new tag as `add_agent_tag` does, without locking or writing:
    /// the description, a name no configured or stored tag already holds,
    /// and a routing table that stays unambiguous with it.
    ///
    /// # Errors
    ///
    /// `invalid_text` for a bad description; `tag_conflict` saying the tag
    /// already exists (in `config.toml` or the store) or which tag already
    /// holds the name or an alias.
    fn check_new_tag(&self, tag: &StoreTag) -> Result<()>;

    /// Add an agent tag without changing config.toml.
    fn add_agent_tag(&self, tag: StoreTag, redactor: &Redactor) -> Result<StoreTag>;

    /// Check a tag update without locking or writing, so the CLI can report
    /// an invalid target or alias before constructing a redactor. The write
    /// path repeats these checks under the store lock.
    fn check_agent_tag_update(&self, name: &TagName, patching: &TagPatch) -> Result<()>;

    /// Update an agent tag under the lock, preserving its creator.
    fn update_agent_tag(
        &self,
        name: &TagName,
        patching: TagPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(StoreTag, bool)>;
}

impl TagVerbs for Store {
    fn check_new_tag(&self, tag: &StoreTag) -> Result<()> {
        check_new_tag(self, self.read().tags(), tag)
    }

    fn add_agent_tag(&self, mut tag: StoreTag, redactor: &Redactor) -> Result<StoreTag> {
        check_agent_author(&tag.author.name)?;
        self.write(|tx| {
            check_new_tag(self, tx.tags(), &tag)?;
            tag.author.kind = AuthorKind::Agent;
            tag.updated_by = None;
            let mut tags = tx.tags().all()?;
            redactor.tag(&mut tag);
            tags.push(tag.clone());
            tx.tags().put_all(&tags)?;
            Ok(tag)
        })
    }

    fn check_agent_tag_update(&self, name: &TagName, patching: &TagPatch) -> Result<()> {
        let read = self.read();
        let configured = read.tags().configured()?.tags;
        if configured.resolve(name.as_str()).is_some() {
            return Err(refusal(
                WriteRefusal::TagConfigured,
                "edit config.toml to change a configured tag",
            ));
        }
        let tags = read.tags().all()?;
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

    fn update_agent_tag(
        &self,
        name: &TagName,
        patching: TagPatch,
        author: AuthorName,
        now: OffsetDateTime,
        redactor: &Redactor,
    ) -> Result<(StoreTag, bool)> {
        check_agent_author(&author)?;
        self.write(|tx| {
            let configured = tx.tags().configured()?.tags;
            if configured.resolve(name.as_str()).is_some() {
                return Err(refusal(
                    WriteRefusal::TagConfigured,
                    "edit config.toml to change a configured tag",
                ));
            }
            let mut tags = tx.tags().all()?;
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
            let mut defs = configured
                .merge_store(&tx.tags().all()?)
                .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?
                .0
                .tags()
                .iter()
                .filter(|t| t.name != result.name)
                .cloned()
                .collect::<Vec<_>>();
            defs.push(result.definition());
            TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
            if changed {
                tx.tags().put_all(&tags)?;
            }
            Ok((result, changed))
        })
    }
}

/// The checks of a new tag, over the tags `tags` reads (a read, or the
/// write under way).
fn check_new_tag(store: &Store, tags: Tags<'_>, tag: &StoreTag) -> Result<()> {
    check_description(&tag.description)?;
    let configured = tags.configured()?.tags;
    if let Some(held) = configured.resolve(tag.name.as_str()) {
        let message = if held.name == tag.name {
            format!(
                "tag '{}' already exists; it is configured in {}",
                tag.name,
                store.config_path().display()
            )
        } else {
            format!(
                "'{}' is already an alias of the configured tag '{}'",
                tag.name, held.name
            )
        };
        return Err(refusal(WriteRefusal::TagConflict, message));
    }
    if tags.all()?.iter().any(|t| t.name == tag.name) {
        return Err(refusal(
            WriteRefusal::TagConflict,
            format!(
                "tag '{}' already exists in {}",
                tag.name,
                store.root().join("tags.json").display()
            ),
        ));
    }
    let mut defs = tags.config()?.tags.tags().to_vec();
    defs.push(tag.definition());
    TagTable::new(defs).map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
    Ok(())
}
