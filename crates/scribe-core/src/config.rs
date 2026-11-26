//! `config.toml`: tags, model ids, cadence, the session age limit and the
//! credential names whose values the redactor masks.
//!
//! The user edits the file and scribe only reads it; a missing file means the
//! defaults. A bad key or value is refused at load, naming it, never
//! defaulted (STD-02 R28; docs/design/scribe/4_decisions.md, "Environment
//! settings fall back rather than refuse"). This module parses text; the
//! store reads the file (STD-02 R5) and [`load`] turns its text into a
//! [`Config`]. The tag app merges the store tags in (`apps/tag`).
//!
//! ```toml
//! [tags.preferences]
//! aliases = ["preference", "pref"]
//! stale_after_days = 180
//!
//! [tags.rituals]
//! description = "how the user starts and ends a day"
//!
//! [tags.direction]
//! action = true
//! require_quote = true
//!
//! [tags.idea]
//! ask_first = true
//!
//! [tags.preferences.capture]
//! triggers = ["always", "never", "from now on", "I prefer"]
//! not = "a choice about one task; an acknowledgement"
//!
//! `[tags]` merges with the shipped tags (`preferences`, `idea`, `standard`):
//! declaring another tag adds it, and a partial table for a shipped tag —
//! including one that sets only `[tags.preferences.capture]` — overrides only
//! the fields it sets and inherits the rest (`aliases`, `description`,
//! `stale_after_days`, `action`, `require_quote`, `ask_first`,
//! `require_evidence`, and capture fields). `aliases = []` clears shipped
//! aliases. Omitting `stale_after_days` on a tag with no shipped interval
//! means entries never go stale. A word the config sets that a shipped tag
//! also uses is the user's: the shipped tag loses that alias, or is left out
//! when the word is its name, and `tag list` warns. Two config entries that
//! claim one word are refused. Nothing in config deletes a shipped tag
//! except claiming its name.
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
//! [user]
//! name = "Sam"
//!
//! [instructions]
//! # group_summary = "Emphasize outcomes" # optional Orbit close guidance
//! session = "You write the record of a coding session {user} ran."
//!
//! [projects.scribe]
//! paths = ["codebases/scribe"]
//! ```

/// Default windows before sealing an open segment.
pub const SEGMENT_WINDOWS: std::num::NonZeroU32 = match std::num::NonZeroU32::new(3) {
    Some(n) => n,
    None => std::num::NonZeroU32::MIN,
};

/// Maximum open candidate proposals across the whole store.
pub const MAX_OPEN_CANDIDATES: usize = 20;

mod tags;
pub mod voice;

use crate::record::{Label, Project, is_kebab};
use crate::tag::TagTable;
use tags::RawTag;

pub use scribe_tools::window::{WINDOW_CHARS, WINDOW_MESSAGES, WindowLimits};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::num::{NonZeroU32, NonZeroUsize};

/// How often an opted-in session gets a pass: `[catalog]` in `config.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Minutes since the last pass.
    pub every_minutes: NonZeroU32,
    /// User turns since the last pass.
    pub every_turns: NonZeroU32,
}

impl Default for Cadence {
    fn default() -> Self {
        Self {
            every_minutes: NonZeroU32::new(20).unwrap_or(NonZeroU32::MIN),
            every_turns: NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN),
        }
    }
}

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
    /// Executable transport; independent of the transcript provider.
    pub provider: crate::Provider,
    /// Summarizes a group on close; named for the removed `ask`, whose
    /// model it was (ORB-27911 step 2 decides its future).
    pub ask: String,
    /// Writes session records.
    pub session: String,
    /// Rebuilds the profile in `memory refresh` (2_design.md §15.6).
    pub profile: String,
    /// The environment variables that carry the model credential; passed to
    /// the model process and masked in everything persisted.
    pub credentials: Vec<String>,
}

impl ModelIds {
    /// Approved Codex default: Luna for every operation, with no fallback.
    pub fn codex() -> Self {
        Self {
            provider: crate::Provider::Codex,
            ask: String::from("gpt-6-luna"),
            session: String::from("gpt-6-luna"),
            profile: String::from("gpt-6-luna"),
            credentials: vec![String::from("OPENAI_API_KEY")],
        }
    }
}

impl Default for ModelIds {
    fn default() -> Self {
        Self {
            provider: crate::Provider::ClaudeCode,
            ask: String::from("claude-sonnet-5-5"),
            session: String::from("claude-haiku-5-5"),
            profile: String::from("claude-sonnet-5-5"),
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
    /// Shipped names and aliases this file claimed. `tag list` prints them;
    /// they are not errors, and an empty list means the merge changed nothing
    /// a shipped tag already held.
    pub tag_warnings: Vec<String>,
    /// Keys this file sets that this build no longer reads; they load,
    /// for the compatibility window, and the CLI warns about each.
    pub warnings: Vec<String>,
    /// The model ids and credential names.
    pub models: ModelIds,
    /// Models for Codex-origin work, defaulting to Luna.
    pub codex_models: ModelIds,
    /// How often an opted-in session gets a pass.
    pub catalog: Cadence,
    /// Per-call message bounds, also configured under `[catalog]`.
    pub catalog_window: WindowLimits,
    /// Windows before scribe seals an open segment (default 3).
    pub segment_windows: NonZeroU32,
    /// When sessions are deactivated by age.
    pub sessions: SessionsConfig,
    /// Further environment variables whose values are masked.
    pub redact_env: Vec<String>,
    /// Path fragments per project slug, for transcript attribution. None
    /// by default (docs/design/scribe/2_design.md §5.8).
    pub projects: BTreeMap<Project, Vec<String>>,
    /// `[user] name`: how prompts name the user; "the user" when unset.
    pub user: crate::voice::UserName,
    /// `[instructions]`: per-use instruction texts replacing the shipped ones.
    pub instructions: crate::voice::Instructions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tags: TagTable::defaults(),
            tag_warnings: Vec::new(),
            warnings: Vec::new(),
            models: ModelIds::default(),
            codex_models: ModelIds::codex(),
            catalog: Cadence::default(),
            catalog_window: WindowLimits::default(),
            segment_windows: SEGMENT_WINDOWS,
            sessions: SessionsConfig::default(),
            redact_env: Vec::new(),
            projects: BTreeMap::new(),
            user: crate::voice::UserName::default(),
            instructions: crate::voice::Instructions::default(),
        }
    }
}

impl Config {
    /// Resolve model routing for the originating transcript. Global jobs use
    /// Claude defaults unless their caller explicitly selects Codex.
    pub fn for_provider(&self, provider: crate::Provider) -> Self {
        let mut config = self.clone();
        if provider == crate::Provider::Codex {
            config.models = self.codex_models.clone();
        }
        config
    }

    /// What a request for `model_use` is voiced with: `[user] name` and
    /// that use's `[instructions]` text, when the file sets them.
    #[must_use]
    pub fn voice(&self, model_use: crate::voice::ModelUse) -> crate::voice::Voice {
        crate::voice::Voice {
            user: self.user.clone(),
            instructions: self.instructions.get(model_use).map(str::to_owned),
        }
    }

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
        let (tags, tag_warnings) = tags::resolve(raw.tags, text)?;
        // `models.referent` named the form-3 referent call, removed with
        // §14 step 2 (2_design.md §14.6). A config that sets it still loads
        // for the 30-day compatibility window.
        let mut warnings: Vec<String> = [
            (raw.models.referent.is_some(), "models.referent"),
            (raw.codex.models.referent.is_some(), "codex.models.referent"),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, key)| {
            format!(
                "config key {key} is unused: #scribe markers are handed to the agent, and models.annotation is unused too; remove it"
            )
        })
        .collect();
        // Memory has one fixed shape with no switches (2_design.md §15.5):
        // every `[memory]` key loads with a warning for the 30-day
        // compatibility window, and is then refused.
        for key in raw.memory.keys() {
            warnings.push(format!(
                "config key memory.{key} is deprecated (config_deprecated): memory has one fixed shape with no switches; remove it"
            ));
        }
        if raw.instructions.memory_pairs.is_some() {
            warnings.push(String::from(
                "config key instructions.memory_pairs is deprecated (config_deprecated): memory consolidation is removed; remove it",
            ));
        }
        // `[models] consistency` and `annotation` (and the codex twins) name
        // no model since ORB-27911 step 2a: accepted, checked and ignored.
        let ignored: Vec<String> = [
            &raw.models.consistency,
            &raw.models.annotation,
            &raw.codex.models.consistency,
            &raw.codex.models.annotation,
        ]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
        let models = raw.models.resolve(ModelIds::default());
        let codex_models = raw.codex.models.resolve(ModelIds::codex());
        for name in models
            .credentials
            .iter()
            .chain(&codex_models.credentials)
            .chain(&raw.redact.env)
        {
            if !is_env_name(name) {
                return Err(unlocated(format!(
                    "'{name}' is not an environment variable name (letters, digits and '_', not starting with a digit)"
                )));
            }
        }
        for model in [&models, &codex_models]
            .into_iter()
            .flat_map(|ids| [&ids.ask, &ids.session, &ids.profile])
            .chain(&ignored)
        {
            if model.trim().is_empty() || model.chars().any(char::is_control) {
                return Err(unlocated(String::from(
                    "model ids must be nonempty and contain no control characters",
                )));
            }
        }
        let cadence = Cadence::default();
        let window = WindowLimits::default();
        Ok(Self {
            tags,
            tag_warnings,
            warnings,
            models,
            codex_models,
            catalog: Cadence {
                every_minutes: raw.catalog.every_minutes.unwrap_or(cadence.every_minutes),
                every_turns: raw.catalog.every_turns.unwrap_or(cadence.every_turns),
            },
            catalog_window: WindowLimits {
                messages: raw.catalog.window_messages.unwrap_or(window.messages),
                chars: raw.catalog.window_chars.unwrap_or(window.chars),
            },
            segment_windows: raw.catalog.segment_windows.unwrap_or(SEGMENT_WINDOWS),
            sessions: SessionsConfig {
                inactive_after_days: raw
                    .sessions
                    .inactive_after_days
                    .unwrap_or(SessionsConfig::DEFAULT_INACTIVE_AFTER_DAYS),
            },
            redact_env: raw.redact.env,
            projects: projects_from(text, raw.projects)?,
            user: match raw.user.name {
                None => crate::voice::UserName::default(),
                Some(name) => crate::voice::UserName::named(&name).map_err(unlocated)?,
            },
            instructions: raw.instructions.resolve().map_err(unlocated)?,
        })
    }

    /// Every environment variable whose value the redactor masks: the model
    /// credentials, then `[redact] env`, each once.
    #[must_use]
    pub fn secret_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for name in self
            .models
            .credentials
            .iter()
            .chain(&self.codex_models.credentials)
            .chain(&self.redact_env)
        {
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
    codex: RawCodex,
    #[serde(default)]
    catalog: RawCatalog,
    #[serde(default)]
    sessions: RawSessions,
    #[serde(default)]
    redact: RawRedact,
    #[serde(default)]
    projects: BTreeMap<String, RawProject>,
    /// Deprecated: every key is read only to warn that it is unused.
    #[serde(default)]
    memory: toml::Table,
    #[serde(default)]
    user: RawUser,
    #[serde(default)]
    instructions: RawInstructions,
}

/// `[user]`: how prompts name the user.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawUser {
    name: Option<String>,
}

/// `[instructions]`: one optional text per model use
/// ([`crate::voice::ModelUse::key`]).
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInstructions {
    /// Accepted, checked and ignored: `#scribe` markers go to the agent
    /// (ORB-27911 step 2a).
    annotation: Option<String>,
    session: Option<String>,
    session_header: Option<String>,
    /// Accepted, checked and ignored: `ask` is removed (ORB-27911); the key goes with the model config in step 2.
    ask: Option<String>,
    /// Accepted, checked and ignored: the consistency scan calls no model
    /// (ORB-27911 step 2a).
    consistency: Option<String>,
    /// Deprecated: read only to warn that it is unused.
    memory_pairs: Option<String>,
    profile: Option<String>,
    /// Guidance for Orbit group close summaries.
    group_summary: Option<String>,
    /// Accepted, checked and ignored, as `consistency`.
    removal: Option<String>,
}

impl RawInstructions {
    fn resolve(self) -> Result<crate::voice::Instructions, String> {
        use crate::voice::ModelUse;
        let mut out = crate::voice::Instructions::default();
        let mut set = |model_use: ModelUse, text: Option<String>| match text {
            Some(text) => out.set(model_use, &text),
            None => Ok(()),
        };
        let ignored = |key: &str, text: &Option<String>| match text {
            Some(text) => crate::voice::check_instruction(key, text).map(|_| ()),
            None => Ok(()),
        };
        ignored("annotation", &self.annotation)?;
        set(ModelUse::Session, self.session)?;
        set(ModelUse::SessionHeader, self.session_header)?;
        ignored("ask", &self.ask)?;
        ignored("consistency", &self.consistency)?;
        set(ModelUse::Profile, self.profile)?;
        if let Some(text) = self.group_summary {
            out.set_group_summary(&text)?;
        }
        ignored("removal", &self.removal)?;
        Ok(out)
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModels {
    provider: Option<crate::Provider>,
    /// Picks the group-close summary model (the removed `ask`'s key).
    ask: Option<String>,
    session: Option<String>,
    /// Accepted, checked and ignored: the consistency scan calls no model
    /// (ORB-27911 step 2a).
    consistency: Option<String>,
    /// Deprecated: read only to warn that it is unused.
    referent: Option<String>,
    /// Accepted, checked and ignored: `#scribe` markers go to the agent
    /// (ORB-27911 step 2a).
    annotation: Option<String>,
    profile: Option<String>,
    credentials: Option<Vec<String>>,
}

impl RawModels {
    fn resolve(self, default: ModelIds) -> ModelIds {
        let provider = self.provider.unwrap_or(default.provider);
        let defaults = if provider != default.provider {
            match provider {
                crate::Provider::Codex => ModelIds::codex(),
                crate::Provider::ClaudeCode => ModelIds::default(),
            }
        } else {
            default
        };
        ModelIds {
            provider,
            ask: self.ask.unwrap_or(defaults.ask),
            session: self.session.unwrap_or(defaults.session),
            profile: self.profile.unwrap_or(defaults.profile),
            credentials: self.credentials.unwrap_or(defaults.credentials),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCodex {
    #[serde(default)]
    models: RawModels,
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

/// Claude Code `entrypoint` values of a session Daniel types into: the
/// terminal (`cli`) and the desktop app (`claude-desktop`). `sdk-cli` is
/// `claude -p` and the Agent SDK, which every Orbit run, routine and drain
/// uses; it and every other value are refused (§12.5).
pub const INTERACTIVE_ENTRYPOINTS: &[&str] = &["cli", "claude-desktop"];

/// The configuration `config.toml`'s text (`Tags::config_text`) holds, or the
/// defaults when there is none. A bad key or value is refused, naming it.
pub(crate) fn load(text: Option<(std::path::PathBuf, String)>) -> crate::Result<Config> {
    let Some((path, text)) = text else {
        return Ok(Config::default());
    };
    let config = Config::parse(&text).map_err(|source| crate::Error::Config {
        path,
        source: Box::new(source),
    })?;
    // Keys this build no longer reads: logged once per process, however
    // often a command loads the config.
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !config.warnings.is_empty() && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        for warning in &config.warnings {
            tracing::warn!("{warning}");
        }
    }
    Ok(config)
}

/// The store's [`Redactor`](crate::Redactor) built from a [`Config`], at the
/// call it had in the store (`Redactor::from_config`).
pub trait RedactorFromConfig {
    /// A redactor for the variables `config` names, their values looked up
    /// with `lookup` (the process environment, in the CLI).
    fn from_config(config: &Config, lookup: impl Fn(&str) -> Option<String>) -> Self;
}

impl RedactorFromConfig for crate::Redactor {
    fn from_config(config: &Config, lookup: impl Fn(&str) -> Option<String>) -> Self {
        Self::new(
            config
                .secret_names()
                .into_iter()
                .filter_map(|name| lookup(name).map(|value| (name.to_owned(), value))),
        )
    }
}

/// The `[redact] env` names in `config` whose value `lookup` cannot supply
/// (absent or empty). The model credentials are not included: they are
/// alternatives, so one may legitimately be unset. A caller that persists
/// free text refuses to write while this is non-empty, rather than store
/// text it was told to mask but cannot (STD-05 R13).
#[must_use]
pub fn unresolved_redact_env(
    config: &Config,
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    config
        .redact_env
        .iter()
        .filter(|name| lookup(name).is_none_or(|value| value.is_empty()))
        .cloned()
        .collect()
}
