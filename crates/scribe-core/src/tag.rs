//! The tag table: which tags exist, their aliases, and how long an entry in
//! each stays fresh.
//!
//! Tags come from `config.toml` (`[tags.<name>]`) and agent-created
//! `tags.json`; config claims take precedence. Without a config the defaults below apply: `decisions` and
//! `preferences` (docs/design/scribe/4_decisions.md, "Only decisions and
//! preferences ship by default"). Pure values and no I/O (STD-02 R5).

use crate::entry::{Entry, TagName};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use time::{Date, Duration};

/// Why a tag table was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    /// Two tags claim the same name or alias, so a `#scribe` naming it would
    /// be ambiguous.
    #[error(
        "tag word '{word}' is claimed by both {first} and {second}; give each tag its own names"
    )]
    Ambiguous {
        /// The name or alias claimed twice.
        word: String,
        /// The first tag claiming it.
        first: String,
        /// The second tag claiming it.
        second: String,
    },
}

/// One effective routing definition, from config or the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagDef {
    /// The tag's name, which is also its directory under `entries/`.
    pub name: TagName,
    /// Other words that name it in a `#scribe` line (`decision`, `pref`).
    pub aliases: Vec<TagName>,
    /// What belongs in it, for the refusal list and `tag list`.
    pub description: Option<String>,
    /// Days after which an entry is due for review; `None` is never.
    pub stale_after_days: Option<NonZeroU32>,
}

impl TagDef {
    /// The `review_after` date for an entry created on `created`.
    #[must_use]
    pub fn review_after(&self, created: Date) -> Option<Date> {
        self.stale_after_days
            .and_then(|days| created.checked_add(Duration::days(i64::from(days.get()))))
    }
}

/// Effective tags, sorted by name, with no name or alias claimed twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagTable {
    tags: Vec<TagDef>,
}

impl TagTable {
    /// A table of `tags`, refused when a name or alias is claimed twice.
    pub fn new(mut tags: Vec<TagDef>) -> Result<Self, TagError> {
        tags.sort_by(|a, b| a.name.cmp(&b.name));
        let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
        for tag in &tags {
            for word in std::iter::once(&tag.name).chain(&tag.aliases) {
                if let Some(first) = claimed.insert(word.as_str(), tag.name.as_str()) {
                    return Err(TagError::Ambiguous {
                        word: word.to_string(),
                        first: first.to_owned(),
                        second: tag.name.to_string(),
                    });
                }
            }
        }
        Ok(Self { tags })
    }

    /// Merge store definitions in memory; config claims win with explicit warnings.
    pub(crate) fn merge_store(&self, stored: &[StoreTag]) -> Result<(Self, Vec<String>), TagError> {
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

    /// The tags scribe ships with: `decisions` (aliases `decision`, `dec`;
    /// never stale) and `preferences` (aliases `preference`, `pref`; stale
    /// after 180 days).
    #[must_use]
    pub fn defaults() -> Self {
        let name = |s: &str| TagName::try_from(s.to_owned());
        let tags = (|| {
            Ok::<_, crate::entry::EntryError>(vec![
                TagDef {
                    name: name("decisions")?,
                    aliases: vec![name("decision")?, name("dec")?],
                    description: Some(String::from("choices Daniel has made")),
                    stale_after_days: None,
                },
                TagDef {
                    name: name("preferences")?,
                    aliases: vec![name("preference")?, name("pref")?],
                    description: Some(String::from("how Daniel likes things done")),
                    stale_after_days: NonZeroU32::new(180),
                },
            ])
        })()
        .unwrap_or_default();
        Self { tags }
    }

    /// The tags, sorted by name.
    #[must_use]
    pub fn tags(&self) -> &[TagDef] {
        &self.tags
    }

    /// The tag with this exact name.
    #[must_use]
    pub fn get(&self, name: &TagName) -> Option<&TagDef> {
        self.tags.iter().find(|t| &t.name == name)
    }

    /// The tag a word in a `#scribe` line names: its name or an alias,
    /// compared case-insensitively.
    #[must_use]
    pub fn resolve(&self, word: &str) -> Option<&TagDef> {
        let word = word.trim().to_lowercase();
        self.tags
            .iter()
            .find(|t| t.name.as_str() == word || t.aliases.iter().any(|a| a.as_str() == word))
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
        .map(|(name, entries)| TagSummary {
            author: None,
            source: table.get(&name).map(|_| "config"),
            def: table.get(&name).cloned(),
            name,
            entries,
        })
        .collect()
}

/// A tag created through a CLI write, persisted in tags.json.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreTag {
    /// Canonical name.
    pub name: TagName,
    /// Alternate names.
    pub aliases: Vec<TagName>,
    /// Redacted description.
    pub description: String,
    /// Optional review interval.
    #[serde(deserialize_with = "crate::record::nullable")]
    pub stale_after_days: Option<NonZeroU32>,
    /// Creator, always an agent.
    pub author: crate::Author,
    /// Last updater, or null.
    #[serde(deserialize_with = "crate::record::nullable")]
    pub updated_by: Option<crate::Author>,
    /// Creation time.
    #[serde(with = "time::serde::rfc3339")]
    pub created: time::OffsetDateTime,
    /// Last change.
    #[serde(with = "time::serde::rfc3339")]
    pub updated: time::OffsetDateTime,
}

impl StoreTag {
    /// Its routing definition.
    #[must_use]
    pub fn definition(&self) -> TagDef {
        TagDef {
            name: self.name.clone(),
            aliases: self.aliases.clone(),
            description: Some(self.description.clone()),
            stale_after_days: self.stale_after_days,
        }
    }
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
