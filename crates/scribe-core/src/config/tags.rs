//! Tag config decoding, capture overrides and shipped alias resolution.

use super::{ConfigError, MAX_OPEN_CANDIDATES, line_of};
use crate::entry::TagName;
use crate::tag::{
    ActionTag, CAPTURE_TEXT_CHARS, CaptureFrom, CaptureRoute, CaptureRules, Rule, RuleOrigin,
    TagDef, TagError, TagTable,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawTag {
    /// `None` when the key is omitted, so `preferences` keeps its shipped
    /// aliases. `Some`, including an empty list, is what was configured.
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    stale_after_days: Option<NonZeroU32>,
    #[serde(default)]
    action: Option<bool>,
    #[serde(default)]
    require_quote: Option<bool>,
    #[serde(default)]
    ask_first: Option<bool>,
    #[serde(default)]
    require_evidence: Option<bool>,
    #[serde(default)]
    capture: Option<RawCapture>,
}

/// `[tags.<name>.capture]`: each field overrides the shipped default for
/// that field alone (docs/design/scribe/2_design.md §11.2).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCapture {
    #[serde(default)]
    from: Option<toml::Spanned<String>>,
    #[serde(default)]
    route: Option<toml::Spanned<String>>,
    #[serde(default)]
    triggers: Option<toml::Spanned<Vec<String>>>,
    #[serde(default)]
    criteria: Option<toml::Spanned<String>>,
    #[serde(default)]
    not: Option<toml::Spanned<String>>,
    #[serde(default)]
    max_open: Option<toml::Spanned<u32>>,
}

impl RawCapture {
    /// `shipped` with each configured field in its place, checked.
    fn resolve(
        self,
        key: &str,
        text: &str,
        shipped: CaptureRules,
    ) -> Result<CaptureRules, ConfigError> {
        let at = |span: std::ops::Range<usize>| Some(line_of(text, span.start));
        let origin = |span: std::ops::Range<usize>| RuleOrigin::Config { line: at(span) };
        let refuse = |span: std::ops::Range<usize>, message: String| ConfigError {
            line: at(span),
            message,
        };
        let mut rules = shipped;
        if let Some(from) = self.from {
            let span = from.span();
            let value = CaptureFrom::parse(from.get_ref()).ok_or_else(|| {
                refuse(
                    span.clone(),
                    format!(
                        "{key}.from: '{}' is not one of: user, assistant, both",
                        from.get_ref()
                    ),
                )
            })?;
            rules.from = Some(Rule {
                value,
                origin: origin(span),
            });
        }
        if let Some(route) = self.route {
            let span = route.span();
            let value = CaptureRoute::parse(route.get_ref()).ok_or_else(|| {
                refuse(
                    span.clone(),
                    format!(
                        "{key}.route: '{}' is not one of: propose, write",
                        route.get_ref()
                    ),
                )
            })?;
            rules.route = Some(Rule {
                value,
                origin: origin(span),
            });
        }
        if let Some(triggers) = self.triggers {
            let span = triggers.span();
            let list = triggers.into_inner();
            if list.len() > crate::trigger::MAX_TRIGGERS {
                return Err(refuse(
                    span,
                    format!(
                        "{key}.triggers: at most {} triggers",
                        crate::trigger::MAX_TRIGGERS
                    ),
                ));
            }
            let mut seen: Vec<String> = Vec::new();
            for trigger in list {
                crate::trigger::check(&trigger)
                    .map_err(|e| refuse(span.clone(), format!("{key}.triggers: {e}")))?;
                if !seen.contains(&trigger) {
                    seen.push(trigger);
                }
            }
            rules.triggers = Some(Rule {
                value: seen,
                origin: origin(span),
            });
        }
        for (field, value, slot) in [
            ("criteria", self.criteria, &mut rules.criteria),
            ("not", self.not, &mut rules.not),
        ] {
            if let Some(value) = value {
                let span = value.span();
                let text = value.into_inner().trim().to_owned();
                if text.chars().count() > CAPTURE_TEXT_CHARS || text.chars().any(char::is_control) {
                    return Err(refuse(
                        span,
                        format!(
                            "{key}.{field}: at most {CAPTURE_TEXT_CHARS} characters on one line"
                        ),
                    ));
                }
                *slot = Some(Rule {
                    value: text,
                    origin: origin(span),
                });
            }
        }
        if let Some(max_open) = self.max_open {
            let span = max_open.span();
            let value = NonZeroU32::new(*max_open.get_ref())
                .filter(|n| usize::try_from(n.get()).is_ok_and(|n| n <= MAX_OPEN_CANDIDATES))
                .ok_or_else(|| {
                    refuse(
                        span.clone(),
                        format!(
                            "{key}.max_open: from 1 to {} (the store-wide cap on open claims)",
                            MAX_OPEN_CANDIDATES
                        ),
                    )
                })?;
            rules.max_open = Some(Rule {
                value,
                origin: origin(span),
            });
        }
        // A tag scribe ships no rules for must say whose turns, where they go
        // and what fires, rather than take an invisible default.
        let missing: Vec<&str> = [
            ("from", rules.from.is_none()),
            ("route", rules.route.is_none()),
            ("triggers", rules.triggers.is_none()),
        ]
        .into_iter()
        .filter_map(|(field, absent)| absent.then_some(field))
        .collect();
        if !missing.is_empty() {
            return Err(ConfigError {
                line: None,
                message: format!(
                    "{key} needs {} (scribe ships no capture rules for this tag)",
                    missing.join(", ")
                ),
            });
        }
        Ok(rules)
    }
}

/// One `[tags.<name>]` table. `aliases_set` is true when the file wrote
/// `aliases`, including `aliases = []`. Omitted aliases are inherited and
/// are not claims of this entry.
struct DeclaredTag {
    def: TagDef,
    aliases_set: bool,
}

impl RawTag {
    fn into_def(self, name: String, text: &str) -> Result<DeclaredTag, ConfigError> {
        // An alias is a word Daniel types, so its case folds; a name is also
        // the entries directory, so it must already be lowercase.
        let word = |key: String, value: String| {
            TagName::try_from(value).map_err(|e| ConfigError {
                line: None,
                message: format!("{key}: {e}"),
            })
        };
        let key = format!("tags.{name}");
        // A config tag named like a shipped tag overrides only the fields it
        // sets and inherits the rest. `action = false` makes a shipped action
        // tag a memory tag.
        let shipped = TagTable::shipped(&name);
        let shipped_action = shipped.as_ref().and_then(|def| def.action);
        let require_quote = self
            .require_quote
            .unwrap_or_else(|| shipped.as_ref().is_some_and(TagDef::requires_quote));
        let action = if self.action.unwrap_or(shipped_action.is_some()) {
            Some(ActionTag {
                require_quote,
                ask_first: self
                    .ask_first
                    .unwrap_or_else(|| shipped_action.is_some_and(|a| a.ask_first)),
            })
        } else {
            if self.ask_first.is_some() {
                return Err(ConfigError {
                    line: None,
                    message: format!(
                        "{key}.ask_first applies only to an action tag; add action = true"
                    ),
                });
            }
            None
        };
        // `require_evidence` marks a finding-like memory tag; an action tag
        // refuses it (its evidence is its window).
        let require_evidence = self
            .require_evidence
            .unwrap_or_else(|| shipped.as_ref().is_some_and(|def| def.require_evidence));
        if require_evidence && action.is_some() {
            return Err(ConfigError {
                line: None,
                message: format!(
                    "{key}.require_evidence applies only to a memory tag; remove action = true"
                ),
            });
        }
        let description = self
            .description
            .or_else(|| shipped.as_ref().and_then(|def| def.description.clone()));
        let shipped_capture = shipped
            .as_ref()
            .map_or_else(CaptureRules::default, |def| def.capture.clone());
        let capture = match self.capture {
            None => shipped_capture,
            Some(raw) => raw.resolve(&format!("{key}.capture"), text, shipped_capture)?,
        };
        let aliases_set = self.aliases.is_some();
        let aliases = match self.aliases {
            Some(aliases) => aliases
                .into_iter()
                .map(|alias| word(format!("{key}.aliases"), alias.to_lowercase()))
                .collect::<Result<_, _>>()?,
            None => shipped
                .as_ref()
                .map(|def| def.aliases.clone())
                .unwrap_or_default(),
        };
        Ok(DeclaredTag {
            def: TagDef {
                aliases,
                name: word(key, name)?,
                description,
                stale_after_days: self
                    .stale_after_days
                    .or_else(|| shipped.as_ref().and_then(|def| def.stale_after_days)),
                action,
                require_quote,
                require_evidence,
                capture,
            },
            aliases_set,
        })
    }
}

pub(super) fn resolve(
    tags: Option<BTreeMap<String, RawTag>>,
    text: &str,
) -> Result<(TagTable, Vec<String>), ConfigError> {
    let Some(tags) = tags else {
        return Ok((TagTable::defaults(), Vec::new()));
    };
    let declared = tags
        .into_iter()
        .map(|(name, tag)| tag.into_def(name, text))
        .collect::<Result<Vec<_>, _>>()?;
    merge_with_shipped(declared)
}

/// Words this config entry claims: its name, and its aliases only when the
/// file set `aliases`. Inherited aliases stay shipped until another entry
/// takes them.
fn explicit_words(tag: &DeclaredTag) -> impl Iterator<Item = &str> {
    let set = tag.aliases_set;
    std::iter::once(tag.def.name.as_str()).chain(
        tag.def
            .aliases
            .iter()
            .filter(move |_| set)
            .map(crate::entry::TagName::as_str),
    )
}

fn claimer<'a>(word: &str, declared: &'a [DeclaredTag]) -> Option<&'a str> {
    declared.iter().find_map(|tag| {
        explicit_words(tag)
            .any(|claimed| claimed == word)
            .then_some(tag.def.name.as_str())
    })
}

/// Two of Daniel's own entries claiming one word is `config_invalid`. The
/// message matches [`TagError::Ambiguous`], and the earlier tag is the one
/// earlier by name, as [`TagTable::new`] orders them.
fn refuse_config_collisions(declared: &[DeclaredTag]) -> Result<(), ConfigError> {
    let mut ordered: Vec<&DeclaredTag> = declared.iter().collect();
    ordered.sort_by(|left, right| left.def.name.cmp(&right.def.name));
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
    for tag in ordered {
        for word in explicit_words(tag) {
            if let Some(first) = claimed.insert(word, tag.def.name.as_str()) {
                return Err(ConfigError {
                    line: None,
                    message: TagError::Ambiguous {
                        word: word.to_owned(),
                        first: first.to_owned(),
                        second: tag.def.name.to_string(),
                    }
                    .to_string(),
                });
            }
        }
    }
    Ok(())
}

fn strip_claimed_aliases(
    def: &mut TagDef,
    shipped_name: &str,
    declared: &[DeclaredTag],
    warnings: &mut Vec<String>,
) {
    def.aliases
        .retain(|alias| match claimer(alias.as_str(), declared) {
            Some(who) if who != shipped_name => {
                warnings.push(format!(
                    "shipped tag {shipped_name} alias {alias} is claimed by config tag {who}"
                ));
                false
            }
            _ => true,
        });
}

/// Shipped tags, then Daniel's. A config claim of a shipped name drops that
/// tag; a claim of a shipped alias drops only the alias. Either way the
/// warning names both tags. A shipped tag he declared keeps the fields he
/// did not set, minus aliases another of his entries claimed.
fn merge_with_shipped(declared: Vec<DeclaredTag>) -> Result<(TagTable, Vec<String>), ConfigError> {
    refuse_config_collisions(&declared)?;
    let mut warnings: Vec<String> = Vec::new();
    let mut tags = Vec::new();
    let by_name: BTreeMap<&str, &DeclaredTag> = declared
        .iter()
        .map(|tag| (tag.def.name.as_str(), tag))
        .collect();
    for shipped in TagTable::defaults().tags() {
        let name = shipped.name.as_str();
        match claimer(name, &declared) {
            Some(who) if who != name => {
                warnings.push(format!("shipped tag {name} is claimed by config tag {who}"));
            }
            Some(_) => {
                let Some(declared_tag) = by_name.get(name) else {
                    return Err(ConfigError {
                        line: None,
                        message: format!("config tag {name} was claimed but not parsed"),
                    });
                };
                let mut def = declared_tag.def.clone();
                if !declared_tag.aliases_set {
                    strip_claimed_aliases(&mut def, name, &declared, &mut warnings);
                }
                tags.push(def);
            }
            None => {
                let mut def = shipped.clone();
                strip_claimed_aliases(&mut def, name, &declared, &mut warnings);
                tags.push(def);
            }
        }
    }
    for tag in &declared {
        if TagTable::shipped(tag.def.name.as_str()).is_none() {
            tags.push(tag.def.clone());
        }
    }
    let table = TagTable::new(tags).map_err(|error| ConfigError {
        line: None,
        message: error.to_string(),
    })?;
    Ok((table, warnings))
}
