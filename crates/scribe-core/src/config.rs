//! `config.toml`: tags, model ids, cadence and the credential names whose
//! values the redactor masks.
//!
//! Daniel edits the file and scribe only reads it; a missing file means the
//! defaults. A bad key or value is refused at load, naming it, never
//! defaulted (STD-02 R28; docs/design/scribe/4_decisions.md, "Environment
//! settings fall back rather than refuse"). This module parses text; the
//! store reads the file (STD-02 R5).
//!
//! ```toml
//! [tags.decisions]
//! aliases = ["decision", "dec"]
//! description = "choices Daniel has made"
//!
//! [tags.preferences]
//! aliases = ["preference", "pref"]
//! stale_after_days = 180
//!
//! [models]
//! ask = "claude-sonnet-5-5"
//! credentials = ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]
//!
//! [catalog]
//! every_minutes = 20
//! every_turns = 10
//!
//! [redact]
//! env = ["GITHUB_TOKEN"]
//! ```

use crate::entry::TagName;
use crate::session::Cadence;
use crate::tag::{TagDef, TagTable};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::num::NonZeroU32;

/// Why `config.toml` was refused: the parser's message and, when known, the
/// line it points at.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}{message}", line.map(|n| format!("line {n}: ")).unwrap_or_default())]
pub struct ConfigError {
    /// The 1-based line of the offending key or value.
    pub line: Option<usize>,
    /// What was wrong, naming the key.
    pub message: String,
}

/// The model each use runs, by id (docs/design/scribe/2_design.md §8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Models {
    /// Answers `ask`.
    pub ask: String,
    /// Writes session records.
    pub session: String,
    /// Judges candidate pairs.
    pub consistency: String,
    /// Resolves "this" and "it".
    pub referent: String,
    /// The environment variables that carry the model credential; passed to
    /// the model process and masked in everything persisted.
    pub credentials: Vec<String>,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            ask: String::from("claude-sonnet-5-5"),
            session: String::from("claude-haiku-5-5"),
            consistency: String::from("claude-haiku-5-5"),
            referent: String::from("claude-haiku-5-5"),
            credentials: vec![
                String::from("CLAUDE_CODE_OAUTH_TOKEN"),
                String::from("ANTHROPIC_API_KEY"),
            ],
        }
    }
}

/// The parsed configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The tags.
    pub tags: TagTable,
    /// The model ids and credential names.
    pub models: Models,
    /// How often an opted-in session gets a pass.
    pub catalog: Cadence,
    /// Further environment variables whose values are masked.
    pub redact_env: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tags: TagTable::defaults(),
            models: Models::default(),
            catalog: Cadence::default(),
            redact_env: Vec::new(),
        }
    }
}

impl Config {
    /// Parse `config.toml`'s text. A missing table or key takes its default;
    /// an unknown key, a bad value or an ambiguous tag word is refused.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError {
            line: e.span().map(|span| line_of(text, span.start)),
            message: e.message().trim().to_owned(),
        })?;
        let unlocated = |message: String| ConfigError {
            line: None,
            message,
        };
        let tags = match raw.tags {
            None => TagTable::defaults(),
            Some(tags) => {
                let defs = tags
                    .into_iter()
                    .map(|(name, tag)| tag.into_def(name))
                    .collect::<Result<Vec<_>, _>>()?;
                TagTable::new(defs).map_err(|e| unlocated(e.to_string()))?
            }
        };
        let defaults = Models::default();
        let models = Models {
            ask: raw.models.ask.unwrap_or(defaults.ask),
            session: raw.models.session.unwrap_or(defaults.session),
            consistency: raw.models.consistency.unwrap_or(defaults.consistency),
            referent: raw.models.referent.unwrap_or(defaults.referent),
            credentials: raw.models.credentials.unwrap_or(defaults.credentials),
        };
        for name in models.credentials.iter().chain(&raw.redact.env) {
            if !is_env_name(name) {
                return Err(unlocated(format!(
                    "'{name}' is not an environment variable name (letters, digits and '_', not starting with a digit)"
                )));
            }
        }
        let cadence = Cadence::default();
        Ok(Self {
            tags,
            models,
            catalog: Cadence {
                every_minutes: raw.catalog.every_minutes.unwrap_or(cadence.every_minutes),
                every_turns: raw.catalog.every_turns.unwrap_or(cadence.every_turns),
            },
            redact_env: raw.redact.env,
        })
    }

    /// Every environment variable whose value the redactor masks: the model
    /// credentials, then `[redact] env`, each once.
    #[must_use]
    pub fn secret_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for name in self.models.credentials.iter().chain(&self.redact_env) {
            if !names.contains(&name.as_str()) {
                names.push(name);
            }
        }
        names
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .map_or(0, |before| before.matches('\n').count())
        + 1
}

fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    tags: Option<BTreeMap<String, RawTag>>,
    #[serde(default)]
    models: RawModels,
    #[serde(default)]
    catalog: RawCatalog,
    #[serde(default)]
    redact: RawRedact,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTag {
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    stale_after_days: Option<NonZeroU32>,
}

impl RawTag {
    fn into_def(self, name: String) -> Result<TagDef, ConfigError> {
        // An alias is a word Daniel types, so its case folds; a name is also
        // the entries directory, so it must already be lowercase.
        let word = |key: String, value: String| {
            TagName::try_from(value).map_err(|e| ConfigError {
                line: None,
                message: format!("{key}: {e}"),
            })
        };
        let key = format!("tags.{name}");
        Ok(TagDef {
            aliases: self
                .aliases
                .into_iter()
                .map(|alias| word(format!("{key}.aliases"), alias.to_lowercase()))
                .collect::<Result<_, _>>()?,
            name: word(key, name)?,
            description: self.description,
            stale_after_days: self.stale_after_days,
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModels {
    ask: Option<String>,
    session: Option<String>,
    consistency: Option<String>,
    referent: Option<String>,
    credentials: Option<Vec<String>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    every_minutes: Option<NonZeroU32>,
    every_turns: Option<NonZeroU32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRedact {
    #[serde(default)]
    env: Vec<String>,
}
