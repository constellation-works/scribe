//! `scribe tag list`: the config and store tags. Read-only: no lock, no
//! audit, no write.

use super::ConfigReads;
use super::rules::tags::MergeStore;
use crate::runtime::Ctx;
use crate::store::refusal;
use crate::tag::{self, TagSummary};
use crate::{Result, Skipped, Snapshot, WriteRefusal};

/// What [`list`] found.
#[derive(Debug)]
pub struct TagList {
    /// Every configured tag and every tag with entries, by name.
    pub tags: Vec<TagSummary>,
    /// Config overrides reported to the caller.
    pub warnings: Vec<String>,
    /// Entry files left out of the counts because they did not parse.
    pub skipped: Vec<Skipped>,
}

/// Every tag, with what was skipped and the config's warnings.
///
/// # Errors
///
/// A store error.
pub fn list<E>(ctx: &Ctx<'_, E>) -> Result<TagList> {
    list_tags(&ctx.store.read())
}

/// Every configured tag and every tag that has entries, with counts.
///
/// # Errors
///
/// A store error, or `tag_conflict` when a store tag claims a configured
/// name or alias.
pub fn list_tags(read: &Snapshot<'_>) -> Result<TagList> {
    let config = read.tags().configured()?;
    let stored = read.tags().all()?;
    let (table, mut warnings) = config
        .tags
        .merge_store(&stored)
        .map_err(|e| refusal(WriteRefusal::TagConflict, e.to_string()))?;
    warnings.extend(config.tag_warnings);
    let configured = &config.tags;
    let (entries, skipped) = read.entries().all()?;
    let pickups = read.pickups().all()?;
    let counts = crate::action::tag_counts(&entries, &table, &pickups);
    let mut tags = tag::summarize(&table, &entries);
    for tag in &mut tags {
        if let Some((retired, rejected)) = counts.get(&tag.name) {
            tag.retired = *retired;
            tag.rejected = *rejected;
        }
        tag.source = if configured.get(&tag.name).is_some() {
            Some("config")
        } else if table.get(&tag.name).is_some() {
            Some("store")
        } else {
            None
        };
        if tag.source == Some("store") {
            tag.author = stored
                .iter()
                .find(|t| t.name == tag.name)
                .map(|t| t.author.clone());
        }
    }
    Ok(TagList {
        tags,
        skipped,
        warnings,
    })
}
