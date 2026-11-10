//! `config.toml`: tags, model ids, cadence, the session age limit and the
//! credential names whose values the redactor masks.
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
//! window_messages = 40
//! window_chars = 15000
//! segment_windows = 3
//!
//! [sessions]
//! inactive_after_days = 120
//!
//! [redact]
//! env = ["GITHUB_TOKEN"]
//!
//! [projects.scribe]
//! paths = ["codebases/scribe"]
//! ```

use crate::entry::TagName;
use crate::record::{Label, Project, is_kebab};
use crate::session::Cadence;
use crate::tag::{TagDef, TagTable};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};

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
pub struct ModelIds {
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

impl Default for ModelIds {
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

/// `[sessions]`: when the consistency routine deactivates a session by
/// age (docs/design/scribe/2_design.md §5.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionsConfig {
    /// Days since a session's last message after which the routine
    /// deactivates it (route `age`), unless it is pinned.
    pub inactive_after_days: NonZeroU32,
}

impl SessionsConfig {
    /// The default age limit, in days.
    pub const DEFAULT_INACTIVE_AFTER_DAYS: NonZeroU32 = match NonZeroU32::new(120) {
        Some(n) => n,
        None => NonZeroU32::MIN,
    };
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            inactive_after_days: Self::DEFAULT_INACTIVE_AFTER_DAYS,
        }
    }
}

/// The parsed configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The tags.
    pub tags: TagTable,
    /// The model ids and credential names.
    pub models: ModelIds,
    /// How often an opted-in session gets a pass.
    pub catalog: Cadence,
    /// Per-call message bounds, also configured under `[catalog]`.
    pub catalog_window: crate::pass::WindowLimits,
    /// Windows before scribe seals an open segment (default 3).
    pub segment_windows: NonZeroU32,
    /// When sessions are deactivated by age.
    pub sessions: SessionsConfig,
    /// Further environment variables whose values are masked.
    pub redact_env: Vec<String>,
    /// Path fragments per project slug, for transcript attribution. None
    /// by default (docs/design/scribe/2_design.md §5.8).
    pub projects: BTreeMap<Project, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tags: TagTable::defaults(),
            models: ModelIds::default(),
            catalog: Cadence::default(),
            catalog_window: crate::pass::WindowLimits::default(),
            segment_windows: crate::pass::SEGMENT_WINDOWS,
            sessions: SessionsConfig::default(),
            redact_env: Vec::new(),
            projects: BTreeMap::new(),
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
        let defaults = ModelIds::default();
        let models = ModelIds {
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
        let window = crate::pass::WindowLimits::default();
        Ok(Self {
            tags,
            models,
            catalog: Cadence {
                every_minutes: raw.catalog.every_minutes.unwrap_or(cadence.every_minutes),
                every_turns: raw.catalog.every_turns.unwrap_or(cadence.every_turns),
            },
            catalog_window: crate::pass::WindowLimits {
                messages: raw.catalog.window_messages.unwrap_or(window.messages),
                chars: raw.catalog.window_chars.unwrap_or(window.chars),
            },
            segment_windows: raw
                .catalog
                .segment_windows
                .unwrap_or(crate::pass::SEGMENT_WINDOWS),
            sessions: SessionsConfig {
                inactive_after_days: raw
                    .sessions
                    .inactive_after_days
                    .unwrap_or(SessionsConfig::DEFAULT_INACTIVE_AFTER_DAYS),
            },
            redact_env: raw.redact.env,
            projects: projects_from(text, raw.projects)?,
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
    sessions: RawSessions,
    #[serde(default)]
    redact: RawRedact,
    #[serde(default)]
    projects: BTreeMap<String, RawProject>,
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
    window_messages: Option<NonZeroUsize>,
    window_chars: Option<NonZeroUsize>,
    segment_windows: Option<NonZeroU32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSessions {
    inactive_after_days: Option<NonZeroU32>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRedact {
    #[serde(default)]
    env: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProject {
    #[serde(default)]
    paths: Vec<String>,
}

/// `[projects.<slug>]` with a kebab-case slug and at least one path.
fn projects_from(
    text: &str,
    raw: BTreeMap<String, RawProject>,
) -> Result<BTreeMap<Project, Vec<String>>, ConfigError> {
    let mut projects = BTreeMap::new();
    for (slug, project) in raw {
        let line = project_line(text, &slug);
        let err = |message: String| ConfigError { line, message };
        if !is_kebab(&slug, Label::MAX_CHARS) {
            return Err(err(format!(
                "projects.{slug}: slug must be lowercase kebab-case, at most {} characters",
                Label::MAX_CHARS
            )));
        }
        let project_id =
            Project::try_from(slug.clone()).map_err(|e| err(format!("projects.{slug}: {e}")))?;
        let mut paths = Vec::new();
        for path in project.paths {
            let path = path.trim().to_owned();
            if path.is_empty() {
                return Err(err(format!(
                    "projects.{slug}.paths: a path must not be empty"
                )));
            }
            if !paths.iter().any(|seen: &String| seen == &path) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Err(err(format!(
                "projects.{slug}.paths: give at least one path fragment"
            )));
        }
        projects.insert(project_id, paths);
    }
    Ok(projects)
}

/// The line of `[projects.<slug>]`, in either bare or quoted form.
fn project_line(text: &str, slug: &str) -> Option<usize> {
    let needles = [
        format!("[projects.{slug}]"),
        format!("[projects.\"{slug}\"]"),
        format!("[projects.'{slug}']"),
    ];
    needles
        .iter()
        .find_map(|needle| text.find(needle).map(|at| line_of(text, at)))
}
