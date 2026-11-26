//! The tag table's rules (ruling (f)): merging store tags under the
//! configured table, the `tag list` summary, and the `tag update` patch.
//! Pure values and no I/O (STD-02 R5).

use crate::entry::{Entry, TagName};
use crate::tag::{StoreTag, TagDef, TagError, TagTable};
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// Store tags merged under a configured table.
pub trait MergeStore: Sized {
    /// Merge store definitions in memory; config claims win with explicit warnings.
    fn merge_store(&self, stored: &[StoreTag]) -> Result<(Self, Vec<String>), TagError>;
}

impl MergeStore for TagTable {
    fn merge_store(&self, stored: &[StoreTag]) -> Result<(Self, Vec<String>), TagError> {
        let mut tags = self.tags().to_vec();
        let mut warnings = Vec::new();
        for tag in stored {
            if let Some(configured) = self.resolve(tag.name.as_str()) {
                warnings.push(format!(
                    "store tag {} is shadowed by config tag {}",
                    tag.name, configured.name
                ));
                continue;
            }
            let mut def = tag.definition();
            def.aliases.retain(|alias| {
                if let Some(configured) = self.resolve(alias.as_str()) {
                    warnings.push(format!(
                        "store tag {} alias {alias} is shadowed by config tag {}",
                        tag.name, configured.name
                    ));
                    false
                } else {
                    true
                }
            });
            tags.push(def);
        }
        Ok((Self::new(tags)?, warnings))
    }
}

/// One row of `tag list`: a tag and how many entries it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagSummary {
    /// Creator for store tags; absent for configured or orphaned tags.
    pub author: Option<crate::Author>,
    /// Routing source, or none for orphaned directories.
    pub source: Option<&'static str>,
    /// The tag's name.
    pub name: TagName,
    /// Its effective definition; `None` for an orphaned entry directory.
    pub def: Option<TagDef>,
    /// How many entries it holds, any status.
    pub entries: usize,
    /// For an action tag: agent-written entries under it that were retired
    /// or forgotten (cancelled).
    pub retired: usize,
    /// For an action tag: agent-written entries under it whose result a
    /// consumer recorded as rejected.
    pub rejected: usize,
}

/// Fields accepted by tag update.
#[derive(Clone, Debug, Default)]
pub struct TagPatch {
    /// Replace the description.
    pub description: Option<String>,
    /// Aliases to add.
    pub add_aliases: Vec<TagName>,
    /// Aliases to remove.
    pub remove_aliases: Vec<TagName>,
    /// Replace the interval when supplied.
    pub stale_after_days: Option<NonZeroU32>,
}

/// Every configured tag, plus any tag that has entries but no
/// configuration, sorted by name, each with its entry count.
#[must_use]
pub fn summarize(table: &TagTable, entries: &[Entry]) -> Vec<TagSummary> {
    let mut counts: BTreeMap<TagName, usize> = BTreeMap::new();
    for tag in table.tags() {
        counts.entry(tag.name.clone()).or_default();
    }
    for entry in entries {
        *counts.entry(entry.tag.clone()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(name, entries)| {
            let source = table.get(&name).map(|_| "config");
            let def = table.get(&name).cloned();
            TagSummary {
                author: None,
                source,
                name,
                def,
                entries,
                retired: 0,
                rejected: 0,
            }
        })
        .collect()
}
