//! Composition (STD-02 R2): the one place that resolves configuration, picks
//! the data directory and constructs the store. It hands the domain resolved
//! values (STD-02 R3), composes the model adapter (`ClaudeCliModels`) and
//! the Orbit task adapter (`OrbitTasks`), and
//! knows nothing about argument parsing or rendering;
//! `scripts/check-dependency-direction.sh` rejects `crate::cli`,
//! `crate::command` and `crate::output` imports here.

use crate::error::CliError;
use crate::models::{ClaudeCliModels, allowed_env};
use crate::tasks::{self, OrbitTasks};
use scribe_core::{Config, Label, Redactor, Store};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Environment variable naming the data directory.
pub(crate) const ROOT_ENV: &str = "SCRIBE_ROOT";

/// Environment variable that moves Claude Code's state directory.
const CLAUDE_CONFIG_ENV: &str = "CLAUDE_CONFIG_DIR";

/// The host label used when the host name cannot be read or is not a label.
const UNKNOWN_HOST: &str = "unknown-host";

/// Directory under `$HOME` used when nothing else names one.
const DEFAULT_DIR: &str = ".scribe";

/// The configuration-relevant environment, captured once.
#[derive(Clone, Debug, Default)]
pub(crate) struct AppEnv {
    /// `SCRIBE_ROOT`.
    pub(crate) root: Option<OsString>,
    /// `HOME`.
    pub(crate) home: Option<OsString>,
    /// `CLAUDE_CONFIG_DIR`: where Claude Code keeps its state instead of
    /// `~/.claude`, and so its transcripts.
    pub(crate) claude_config_dir: Option<OsString>,
}

impl AppEnv {
    /// Read the variables from the real process environment.
    pub(crate) fn from_process() -> Self {
        Self {
            root: std::env::var_os(ROOT_ENV),
            home: std::env::var_os("HOME"),
            claude_config_dir: std::env::var_os(CLAUDE_CONFIG_ENV),
        }
    }
}

/// Everything a command needs, resolved.
#[derive(Debug)]
pub(crate) struct Context {
    store: Store,
    transcripts: Option<PathBuf>,
}

impl Context {
    /// Resolve the context. Data directory precedence: `--root` >
    /// `SCRIBE_ROOT` > `$HOME/.scribe`.
    pub(crate) fn resolve(root_flag: Option<&Path>, env: &AppEnv) -> Result<Self, CliError> {
        let root = resolve_root(root_flag, env)?;
        tracing::debug!(root = %root.display(), "resolved data directory");
        Ok(Self {
            store: Store::open(root),
            transcripts: transcripts_dir(env),
        })
    }

    /// A context for the store at `root` and the transcripts under
    /// `transcripts`, as the plugin's settings name them (`plugin call`
    /// never reads `SCRIBE_ROOT` or `HOME`).
    pub(crate) fn at(root: PathBuf, transcripts: Option<PathBuf>) -> Self {
        Self {
            store: Store::open(root),
            transcripts,
        }
    }

    /// The scribe store.
    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    /// Where Claude Code keeps session transcripts:
    /// `$CLAUDE_CONFIG_DIR/projects`, else `$HOME/.claude/projects`. `None`
    /// when neither is set, which makes every transcript fail the
    /// provenance check.
    pub(crate) fn transcripts(&self) -> Option<&Path> {
        self.transcripts.as_deref()
    }

    /// This host's name as a record label, for the records it stamps.
    pub(crate) fn host(&self) -> Label {
        Label::sanitized(host_name().as_deref().unwrap_or_default(), UNKNOWN_HOST)
    }

    /// The redactor for `config`: the live values of the variables it names,
    /// read from this process's environment.
    pub(crate) fn redactor(&self, config: &Config) -> Redactor {
        Redactor::from_config(config, |name| std::env::var(name).ok())
    }

    /// The model adapter for `config`: `claude` run with `PATH`, `HOME`,
    /// `LANG` and the configured credential variables from this process's
    /// environment, and nothing else (STD-05 R10).
    pub(crate) fn models(&self, config: &Config) -> ClaudeCliModels {
        ClaudeCliModels::new(allowed_env(&config.models.credentials, |name| {
            std::env::var_os(name)
        }))
    }

    /// The Orbit task adapter: `$ORBIT_BIN` (else `orbit`) run with `PATH`,
    /// `HOME`, `LANG` and the `ORBIT_*` variables from this process's
    /// environment, and nothing else (STD-05 R10).
    pub(crate) fn tasks(&self) -> OrbitTasks {
        OrbitTasks::new(
            tasks::program(std::env::var_os(tasks::PROGRAM_ENV)),
            tasks::allowed_env(std::env::vars_os()),
        )
    }

    /// The current time, whole seconds, for records created by this invocation.
    pub(crate) fn now(&self) -> OffsetDateTime {
        let now = OffsetDateTime::now_utc();
        now.replace_nanosecond(0).unwrap_or(now)
    }
}

/// The transcripts directory precedence, separated so tests can check it.
pub(crate) fn transcripts_dir(env: &AppEnv) -> Option<PathBuf> {
    let non_empty = |v: &Option<OsString>| v.clone().filter(|s| !s.is_empty());
    non_empty(&env.claude_config_dir)
        .map(PathBuf::from)
        .or_else(|| non_empty(&env.home).map(|home| PathBuf::from(home).join(".claude")))
        .map(|dir| dir.join("projects"))
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let uname = rustix::system::uname();
    uname
        .nodename()
        .to_str()
        .ok()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

#[cfg(not(unix))]
fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty())
}

/// The data-directory precedence, separated so tests can check it directly.
pub(crate) fn resolve_root(root_flag: Option<&Path>, env: &AppEnv) -> Result<PathBuf, CliError> {
    let non_empty = |v: &Option<OsString>| v.clone().filter(|s| !s.is_empty());
    if let Some(flag) = root_flag {
        return Ok(flag.to_path_buf());
    }
    if let Some(root) = non_empty(&env.root) {
        return Ok(PathBuf::from(root));
    }
    non_empty(&env.home)
        .map(|home| PathBuf::from(home).join(DEFAULT_DIR))
        .ok_or(CliError::NoDataDir)
}
