//! The tag table: which tags exist, their aliases, and how long an entry in
//! each stays fresh.
//!
//! Tags come from `config.toml` (`[tags.<name>]`) and agent-created
//! `tags.json`; config claims take precedence. Without a config the defaults below apply: `preferences`
//! (docs/design/scribe/4_decisions.md, "Decisions are recorded on their Orbit
//! task, not in a shipped tag"), plus the shipped action tags `standard`
//! and `idea`, which a config with its own tags keeps unless it claims their
//! names ("Action tags are marked in config").
//! An action tag's entries are work items a consumer outside scribe picks
//! up; `ask` leaves them out unless asked. A memory tag configured with
//! `require_evidence = true` (such as `finding` when configured) records an
//! agent's measured or verified result with an evidence pointer: no consumer,
//! no transcript window, found again only through `ask` or search.
//! A tag's capture rules (2_design.md §11.2: `from`, `route`, `triggers`,
//! `criteria`, `not`) decide which of Daniel's turns the session pass may
//! claim under it; `preferences` and `idea` ship rules, the shipped values
//! below, and config overrides them field by field. Pure values and no I/O
//! (STD-02 R5).

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
    /// Other words that name it in a `#scribe` line (`preference`, `pref`).
    pub aliases: Vec<TagName>,
    /// What belongs in it, for the refusal list and `tag list`.
    pub description: Option<String>,
    /// Days after which an entry is due for review; `None` is never.
    pub stale_after_days: Option<NonZeroU32>,
    /// `Some` for an action tag (config `action = true`): its entries are
    /// work items for a consumer, captured with a transcript window.
    pub action: Option<ActionTag>,
    /// An agent entry must carry an evidence pointer and an as-of date
    /// (config `require_evidence = true`).
    /// Memory tags only; action tags refuse the flag in config.
    pub require_evidence: bool,
    /// Which turns the session pass may turn into a claim under it
    /// (docs/design/scribe/2_design.md §11.2): the shipped defaults for its
    /// name, field by field overridden by `[tags.<name>.capture]`. Empty
    /// for a tag with neither, which the pass never captures.
    pub capture: CaptureRules,
}

/// Whose turns a tag's capture rules read (`from`). Only Daniel's typed
/// turns are accepted for now; `assistant` is refused in config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureFrom {
    /// Daniel's typed turns, with the pass's exclusions.
    Daniel,
}

impl CaptureFrom {
    /// The config token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daniel => "daniel",
        }
    }
}

/// What a claim under the tag becomes (`route`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureRoute {
    /// A candidate proposal Daniel settles (§5.3).
    Propose,
    /// The pass writes the entry itself (not built in this step: the pass
    /// captures nothing under a `write` tag yet).
    Write,
    /// The prompt hook tells the agent; the pass captures nothing.
    Agent,
}

impl CaptureRoute {
    /// The config token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Propose => "propose",
            Self::Write => "write",
            Self::Agent => "agent",
        }
    }

    /// Parse the config token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "propose" => Some(Self::Propose),
            "write" => Some(Self::Write),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// Where a capture-rule field's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleOrigin {
    /// scribe's shipped default for the tag.
    Shipped,
    /// `config.toml`, at this 1-based line when known.
    Config {
        /// The line of the key.
        line: Option<usize>,
    },
}

impl RuleOrigin {
    /// `shipped` or `config`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shipped => "shipped",
            Self::Config { .. } => "config",
        }
    }
}

/// One capture-rule field's effective value and its origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule<T> {
    /// The value in force.
    pub value: T,
    /// Where it came from.
    pub origin: RuleOrigin,
}

impl<T> Rule<T> {
    fn shipped(value: T) -> Self {
        Self {
            value,
            origin: RuleOrigin::Shipped,
        }
    }
}

/// A tag's effective capture rules: each field `None` when neither the
/// shipped defaults nor config give it (origin `none`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureRules {
    /// Whose turns are read.
    pub from: Option<Rule<CaptureFrom>>,
    /// What a claim becomes.
    pub route: Option<Rule<CaptureRoute>>,
    /// The trigger phrases ([`crate::trigger`]); a configured list replaces
    /// the shipped one whole, and an empty one captures nothing.
    pub triggers: Option<Rule<Vec<String>>>,
    /// What qualifies, given to the `session` model.
    pub criteria: Option<Rule<String>>,
    /// What does not, given beside `criteria`.
    pub not: Option<Rule<String>>,
}

/// The longest `criteria` or `not` text, in characters.
pub const CAPTURE_TEXT_CHARS: usize = 300;

/// The shipped capture rules: `(tag, route, triggers, criteria, not)`
/// (2_design.md §11.2). `idea` keeps the candidate route until its write
/// route is built; `standard` ships none.
const SHIPPED_CAPTURE: [(&str, CaptureRoute, &[&str], &str, &str); 2] = [
    (
        "idea",
        CaptureRoute::Propose,
        &[
            "have * an idea",
            "had an idea",
            "here's an idea",
            "what do you think about * idea",
            "what if we",
        ],
        "Daniel puts forward something to consider, not an instruction to do it now.",
        "A direction to act now; a preference; a question of fact.",
    ),
    (
        "preferences",
        CaptureRoute::Propose,
        &[
            "always",
            "never",
            "should not",
            "from now on",
            "I prefer",
            "by default",
        ],
        "A standing rule from Daniel that applies beyond the work at hand.",
        "A choice about one page or task (it goes on its Orbit task or the feature's 4_decisions.md); an acknowledgement.",
    ),
];

impl CaptureRules {
    /// The rules scribe ships for the tag named `name`; empty for any other.
    #[must_use]
    pub fn shipped(name: &str) -> Self {
        SHIPPED_CAPTURE
            .iter()
            .find(|(tag, ..)| *tag == name)
            .map(|(_, route, triggers, criteria, not)| Self {
                from: Some(Rule::shipped(CaptureFrom::Daniel)),
                route: Some(Rule::shipped(*route)),
                triggers: Some(Rule::shipped(
                    triggers.iter().map(|t| (*t).to_owned()).collect(),
                )),
                criteria: Some(Rule::shipped((*criteria).to_owned())),
                not: Some(Rule::shipped((*not).to_owned())),
            })
            .unwrap_or_default()
    }

    /// The triggers the session pass matches for the tag: only for Daniel's
    /// turns under the `propose` route; `None` otherwise (no rules, or a
    /// route the pass does not take).
    #[must_use]
    pub fn pass_triggers(&self) -> Option<&[String]> {
        let from = self.from.as_ref()?.value;
        let route = self.route.as_ref()?.value;
        match (from, route) {
            (CaptureFrom::Daniel, CaptureRoute::Propose) => {
                self.triggers.as_ref().map(|t| t.value.as_slice())
            }
            (CaptureFrom::Daniel, CaptureRoute::Write | CaptureRoute::Agent) => None,
        }
    }

    /// The route in force, if any.
    #[must_use]
    pub fn route(&self) -> Option<CaptureRoute> {
        self.route.as_ref().map(|r| r.value)
    }
}

/// How an action tag treats agent-written entries
/// (docs/design/scribe/2_design.md §4.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionTag {
    /// An agent's entry must quote Daniel's correction, found verbatim in
    /// one of Daniel's typed turns of the session it names (`standard`).
    pub require_quote: bool,
    /// Agents ask Daniel before writing an entry here, instead of writing it
    /// unasked.
    pub ask_first: bool,
}

impl ActionTag {
    /// `ask-first` or `free`, as `tag list` shows it.
    #[must_use]
    pub fn mode(self) -> &'static str {
        if self.ask_first { "ask-first" } else { "free" }
    }
}

/// The action tags scribe ships: `(name, description, require_quote)`.
pub const SHIPPED_ACTION_TAGS: [(&str, &str, bool); 2] = [
    ("idea", "a future consideration worth revisiting", false),
    (
        "standard",
        "a rule change for the constellation standards, in Daniel's words",
        true,
    ),
];

/// Historical name of the `finding` tag (no longer shipped by default; kept
/// for approving stored format-10 assistant candidates).
pub const FINDING_TAG: &str = "finding";

/// The most findings one writing session may record (docs/design/scribe/
/// 2_design.md, "Findings"). A write past it is refused and names it.
pub const MAX_FINDINGS_PER_SESSION: usize = 3;

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

    /// The memory tag scribe ships with: `preferences` (aliases `preference`,
    /// `pref`; stale after 180 days).
    #[must_use]
    pub fn defaults() -> Self {
        let name = |s: &str| TagName::try_from(s.to_owned());
        let tags = (|| {
            Ok::<_, crate::entry::EntryError>(vec![TagDef {
                name: name("preferences")?,
                aliases: vec![name("preference")?, name("pref")?],
                description: Some(String::from("how Daniel likes things done")),
                stale_after_days: NonZeroU32::new(180),
                action: None,
                require_evidence: false,
                capture: CaptureRules::shipped("preferences"),
            }])
        })()
        .unwrap_or_default();
        Self { tags }.with_shipped_actions()
    }

    /// The shipped action tag named `name`, as scribe defines it.
    #[must_use]
    pub fn shipped_action(name: &str) -> Option<TagDef> {
        SHIPPED_ACTION_TAGS
            .iter()
            .find(|(n, _, _)| *n == name)
            .and_then(|(n, description, require_quote)| {
                Some(TagDef {
                    name: TagName::try_from((*n).to_owned()).ok()?,
                    aliases: Vec::new(),
                    description: Some((*description).to_owned()),
                    stale_after_days: None,
                    action: Some(ActionTag {
                        require_quote: *require_quote,
                        ask_first: false,
                    }),
                    require_evidence: false,
                    capture: CaptureRules::shipped(n),
                })
            })
    }

    /// This table plus every shipped action tag whose name it does not already
    /// claim (as a name or an alias), so a config that declares its own tags
    /// still has them; a config tag of the same name replaces one.
    #[must_use]
    pub fn with_shipped_actions(mut self) -> Self {
        for (name, _, _) in SHIPPED_ACTION_TAGS {
            if self.resolve(name).is_none()
                && let Some(def) = Self::shipped_action(name)
            {
                self.tags.push(def);
            }
        }
        self.tags.sort_by(|a, b| a.name.cmp(&b.name));
        self
    }

    /// The action settings of the tag named `name`, or `None` for a memory
    /// tag or an unknown one.
    #[must_use]
    pub fn action(&self, name: &TagName) -> Option<ActionTag> {
        self.get(name).and_then(|def| def.action)
    }

    /// Whether the tag named `name` requires an evidence pointer on an
    /// agent entry (a config `require_evidence = true` memory tag).
    #[must_use]
    pub fn require_evidence(&self, name: &TagName) -> bool {
        self.get(name).is_some_and(|def| def.require_evidence)
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
    /// For an action tag: agent-written entries under it that were retired
    /// or forgotten (cancelled).
    pub retired: usize,
    /// For an action tag: agent-written entries under it whose result a
    /// consumer recorded as rejected.
    pub rejected: usize,
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
            retired: 0,
            rejected: 0,
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
            action: None,
            require_evidence: false,
            capture: CaptureRules::default(),
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
