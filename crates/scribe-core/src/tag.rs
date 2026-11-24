//! The tag table: which tags exist, their aliases, and how long an entry in
//! each stays fresh.
//!
//! Tags come from `config.toml` (`[tags.<name>]`), merged over the shipped
//! defaults and agent-created `tags.json`. The shipped set includes
//! `decision`, `handoff`, `open-item`, `preferences` and the action tag
//! `standard`. Declaring `[tags.<name>]`
//! adds a new tag and overrides only the fields set on a shipped tag. A
//! config word that collides with a shipped name or alias wins: that shipped
//! tag loses the word, and `tag list` warns ("Config tags merge with the
//! shipped set").
//! An action tag's entries are work items a consumer outside scribe picks
//! up; `ask` leaves them out unless asked. A memory tag configured with
//! `require_evidence = true` (such as `finding` when configured) records an
//! agent's measured or verified result with an evidence pointer: no consumer,
//! no transcript window, found again only through `ask` or search.
//! A tag's capture rules make it a catcher (2_design.md §15.2: `from`,
//! `route`, `triggers`, `criteria`, `not`, `max_open`): they decide which
//! turns, the user's, the assistant's or both, the session pass may claim
//! under it, and what a claim becomes. `preferences` and `standard` ship
//! rules, the shipped values below, and config overrides them field by
//! field; any other catcher is config alone (specs/catchers.md). Pure values
//! and no I/O (STD-02 R5).

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
    /// An agent entry must quote the user's words verbatim from one of their
    /// typed turns in the session it names (`standard`, `handoff`, or config
    /// `require_quote = true`).
    pub require_quote: bool,
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

/// Whose turns a tag's capture rules read (`from`, 2_design.md §15.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureFrom {
    /// The user's typed turns, with the pass's exclusions.
    User,
    /// The assistant's turns. A claim from one records the speaker, and is
    /// never presented as the user's words.
    Assistant,
    /// Both.
    Both,
}

impl CaptureFrom {
    /// The config token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Both => "both",
        }
    }

    /// Parse the config token; `daniel`, the pre-format-23 spelling, reads
    /// as `user`.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "user" | "daniel" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Whether a turn by `speaker` is read.
    #[must_use]
    pub fn reads(self, speaker: crate::entry::Speaker) -> bool {
        use crate::entry::Speaker;
        match (self, speaker) {
            (Self::User | Self::Both, Speaker::User)
            | (Self::Assistant | Self::Both, Speaker::Assistant) => true,
            (Self::User, Speaker::Assistant) | (Self::Assistant, Speaker::User) => false,
        }
    }
}

/// What a claim under the tag becomes (`route`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureRoute {
    /// A claim the user settles (§5.3).
    Propose,
    /// The pass writes the entry itself, with no claim: a collector
    /// (§11.6, §15.2).
    Write,
}

impl CaptureRoute {
    /// The config token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Propose => "propose",
            Self::Write => "write",
        }
    }

    /// Parse the config token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "propose" => Some(Self::Propose),
            "write" => Some(Self::Write),
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
    /// The most open claims the tag may hold ([`DEFAULT_MAX_OPEN`] when
    /// unset), under the store-wide cap.
    pub max_open: Option<Rule<NonZeroU32>>,
}

/// The longest `criteria` or `not` text, in characters.
pub const CAPTURE_TEXT_CHARS: usize = 300;

/// A catcher's open-claim cap when `max_open` is unset (2_design.md §15.2).
pub const DEFAULT_MAX_OPEN: u32 = 10;

/// The shipped capture rules: `(tag, route, triggers, criteria, not)`
/// (2_design.md §15.2), every one reading the user's turns. Any other
/// catcher, such as the `idea` and `issue` examples of specs/catchers.md,
/// is config alone.
const SHIPPED_CAPTURE: [(&str, CaptureRoute, &[&str], &str, &str); 2] = [
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
        "A standing rule from {user} that applies beyond the work at hand.",
        "A choice about one page or task; an acknowledgement; a one-off command.",
    ),
    (
        "standard",
        CaptureRoute::Propose,
        &[
            "as a standard",
            "our standard",
            "every repo",
            "all repos",
            "across repos",
            "house style",
            "convention",
        ],
        "A rule for how code or docs are written across {user}'s projects.",
        "A rule for one repository or one change; a preference about how agents talk or work.",
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
                from: Some(Rule::shipped(CaptureFrom::User)),
                route: Some(Rule::shipped(*route)),
                triggers: Some(Rule::shipped(
                    triggers.iter().map(|t| (*t).to_owned()).collect(),
                )),
                criteria: Some(Rule::shipped((*criteria).to_owned())),
                not: Some(Rule::shipped((*not).to_owned())),
                max_open: None,
            })
            .unwrap_or_default()
    }

    /// The triggers the session pass matches for the tag; `None` for a tag
    /// that is no catcher (no `from`, `route` or `triggers`).
    #[must_use]
    pub fn pass_triggers(&self) -> Option<&[String]> {
        self.from.as_ref()?;
        self.route.as_ref()?;
        self.triggers.as_ref().map(|t| t.value.as_slice())
    }

    /// Whose turns the catcher reads, if it is one.
    #[must_use]
    pub fn from(&self) -> Option<CaptureFrom> {
        self.from.as_ref().map(|r| r.value)
    }

    /// The most open claims the tag may hold.
    #[must_use]
    pub fn max_open(&self) -> u32 {
        self.max_open
            .as_ref()
            .map_or(DEFAULT_MAX_OPEN, |r| r.value.get())
    }

    /// Whether the pass writes a checked claim under the tag as an entry
    /// itself (`route = "write"`, 2_design.md §11.6) rather than opening a
    /// candidate proposal.
    #[must_use]
    pub fn writes(&self) -> bool {
        self.pass_triggers().is_some() && self.route() == Some(CaptureRoute::Write)
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
pub const SHIPPED_ACTION_TAGS: [(&str, &str, bool); 1] = [(
    "standard",
    "A rule for how code or docs are written across {user}'s projects, in {user}'s words.",
    true,
)];

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

    /// Whether an agent entry under this tag must carry `--quote` verified
    /// verbatim against one of the user's typed turns in the session it names.
    #[must_use]
    pub fn requires_quote(&self) -> bool {
        self.require_quote || self.action.is_some_and(|a| a.require_quote)
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

    /// The tags scribe ships, sorted by name.
    #[must_use]
    pub fn defaults() -> Self {
        let mut tags: Vec<TagDef> = Self::shipped_preferences()
            .into_iter()
            .chain(Self::shipped_handoff())
            .chain(Self::shipped(crate::entry::DECISION_TAG))
            .chain(Self::shipped(crate::entry::OPEN_ITEM_TAG))
            .collect();
        for (name, _, _) in SHIPPED_ACTION_TAGS {
            if let Some(def) = Self::shipped_action(name) {
                tags.push(def);
            }
        }
        tags.sort_by(|left, right| left.name.cmp(&right.name));
        Self { tags }
    }

    /// The shipped tag named `name`.
    /// A partial `[tags.<name>]` table inherits the fields it omits from this.
    #[must_use]
    pub fn shipped(name: &str) -> Option<TagDef> {
        match name {
            "preferences" => Self::shipped_preferences(),
            crate::handoff::HANDOFF_TAG => Self::shipped_handoff(),
            crate::entry::DECISION_TAG => Self::shipped_agent_record(
                name,
                ["decisions", "dec"],
                "A decision that outlives its task.",
            ),
            crate::entry::OPEN_ITEM_TAG => Self::shipped_agent_record(
                name,
                ["open-items", "item"],
                "A question or action that waits for {user}.",
            ),
            _ => Self::shipped_action(name),
        }
    }

    fn shipped_agent_record(name: &str, aliases: [&str; 2], description: &str) -> Option<TagDef> {
        Some(TagDef {
            name: name.parse().ok()?,
            aliases: aliases
                .into_iter()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?,
            description: Some(description.to_owned()),
            stale_after_days: None,
            action: None,
            require_quote: false,
            require_evidence: false,
            capture: CaptureRules::default(),
        })
    }

    /// The shipped `handoff` tag: a memory tag with `require_quote = true`
    /// and no capture rules (2_design.md §16).
    #[must_use]
    pub fn shipped_handoff() -> Option<TagDef> {
        Some(TagDef {
            name: TagName::try_from(crate::handoff::HANDOFF_TAG.to_owned()).ok()?,
            aliases: Vec::new(),
            description: Some(String::from(
                "An agent-written session handoff, recorded on {user}'s request.",
            )),
            stale_after_days: None,
            action: None,
            require_quote: true,
            require_evidence: false,
            capture: CaptureRules::default(),
        })
    }

    /// The shipped `preferences` tag on its own: aliases `preference` and
    /// `pref`, the shipped description, stale after 180 days.
    #[must_use]
    pub fn shipped_preferences() -> Option<TagDef> {
        let name = |s: &str| TagName::try_from(s.to_owned());
        (|| {
            Ok::<_, crate::entry::EntryError>(TagDef {
                name: name("preferences")?,
                aliases: vec![name("preference")?, name("pref")?],
                description: Some(String::from(
                    "A standing rule {user} wants agents to follow.",
                )),
                stale_after_days: NonZeroU32::new(180),
                action: None,
                require_quote: false,
                require_evidence: false,
                capture: CaptureRules::shipped("preferences"),
            })
        })()
        .ok()
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
                    require_quote: *require_quote,
                    require_evidence: false,
                    capture: CaptureRules::shipped(n),
                })
            })
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
            require_quote: false,
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
