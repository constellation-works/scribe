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
use scribe_core::{Config, Label, Provider, Redactor, Store, unresolved_redact_env};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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
    /// Optional Codex state directory.
    pub(crate) codex_home: Option<OsString>,
}

impl AppEnv {
    /// Read the variables from the real process environment.
    pub(crate) fn from_process() -> Self {
        Self {
            root: std::env::var_os(ROOT_ENV),
            home: std::env::var_os("HOME"),
            claude_config_dir: std::env::var_os(CLAUDE_CONFIG_ENV),
            codex_home: std::env::var_os("CODEX_HOME"),
        }
    }
}

/// Everything a command needs, resolved.
#[derive(Clone, Debug)]
pub(crate) struct Context {
    store: Store,
    transcripts: Option<PathBuf>,
    codex_transcripts: Option<PathBuf>,
    codex_archived: Option<PathBuf>,
    provider: Provider,
    /// The instruction the running command acts on, for its audit line;
    /// shared by the provider-scoped views of one context.
    instruction: Arc<Mutex<InstructionNote>>,
}

/// The ids of Daniel's instruction a write ran on (2_design.md §12.4): the
/// session from `--session`, and the turn's message once the check found it.
/// Ids only; the instruction's words never reach the audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InstructionNote {
    /// The `--session` the instruction was looked for in.
    pub(crate) session: Option<String>,
    /// The uuid of the turn that backs it, once the check passed.
    pub(crate) message: Option<String>,
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
            codex_transcripts: env
                .codex_home
                .clone()
                .map(PathBuf::from)
                .or_else(|| env.home.clone().map(|h| PathBuf::from(h).join(".codex")))
                .map(|d| d.join("sessions")),
            codex_archived: env
                .codex_home
                .clone()
                .map(PathBuf::from)
                .or_else(|| env.home.clone().map(|h| PathBuf::from(h).join(".codex")))
                .map(|d| d.join("archived_sessions")),
            provider: Provider::ClaudeCode,
            instruction: Arc::default(),
        })
    }

    /// A context for the store at `root` and the transcripts under
    /// `transcripts`, as the plugin's settings name them (`plugin call`
    /// never reads `SCRIBE_ROOT` or `HOME`).
    pub(crate) fn at(root: PathBuf, transcripts: Option<PathBuf>) -> Self {
        Self {
            store: Store::open(root),
            transcripts,
            codex_transcripts: None,
            codex_archived: None,
            provider: Provider::ClaudeCode,
            instruction: Arc::default(),
        }
    }

    /// A provider-scoped view sharing the same store.
    pub(crate) fn with_provider(&self, provider: Provider) -> Self {
        Self {
            provider,
            ..self.clone()
        }
    }
    /// Explicit Codex state root for sandboxed plugin callers; no env fallback.
    pub(crate) fn with_codex_home(mut self, root: Option<PathBuf>) -> Self {
        self.codex_transcripts = root.as_ref().map(|r| r.join("sessions"));
        self.codex_archived = root.map(|r| r.join("archived_sessions"));
        self
    }
    /// All permitted transcript roots for this provider, including archives.
    pub(crate) fn transcript_roots(&self) -> Vec<&Path> {
        match self.provider {
            Provider::ClaudeCode => self.transcripts.iter().map(PathBuf::as_path).collect(),
            Provider::Codex => self
                .codex_transcripts
                .iter()
                .chain(&self.codex_archived)
                .map(PathBuf::as_path)
                .collect(),
        }
    }

    /// Note the session a command's instruction is looked for in, before
    /// anything can refuse it, so its audit line names it.
    pub(crate) fn note_instruction_session(&self, session: &str) {
        if let Ok(mut note) = self.instruction.lock() {
            note.session = Some(session.to_owned());
        }
    }

    /// Note the turn a passing instruction check found.
    pub(crate) fn note_instruction_message(&self, message: &str) {
        if let Ok(mut note) = self.instruction.lock() {
            note.message = Some(message.to_owned());
        }
    }

    /// What the running command noted of its instruction.
    pub(crate) fn instruction_note(&self) -> InstructionNote {
        self.instruction
            .lock()
            .map(|note| note.clone())
            .unwrap_or_default()
    }

    /// The active transcript provider.
    pub(crate) fn provider(&self) -> Provider {
        self.provider
    }
    /// Config with provider-specific model defaults resolved.
    pub(crate) fn config(&self) -> Result<Config, CliError> {
        Ok(self.store.config()?.for_provider(self.provider))
    }

    /// Refuse scaffolding when the selected runtime is not executable on PATH.
    pub(crate) fn require_runtime(&self) -> Result<(), CliError> {
        let program = match self.provider {
            Provider::Codex => "codex",
            Provider::ClaudeCode => "claude",
        };
        let found = std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| {
                let path = dir.join(program);
                std::fs::metadata(path).is_ok_and(|m| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        m.is_file() && m.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        m.is_file()
                    }
                })
            })
        });
        if !found {
            return Err(CliError::Usage(format!(
                "{program} is not executable on PATH; install the selected runtime and rerun scribe init"
            )));
        }
        Ok(())
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
        match self.provider {
            Provider::ClaudeCode => self.transcripts.as_deref(),
            Provider::Codex => self.codex_transcripts.as_deref(),
        }
    }

    /// This host's name as a record label, for the records it stamps.
    pub(crate) fn host(&self) -> Label {
        Label::sanitized(host_name().as_deref().unwrap_or_default(), UNKNOWN_HOST)
    }

    /// The redactor for `config`: the live values of the variables it names,
    /// read from this process's environment. Fails closed when a
    /// `[redact] env` name has no value here (a plugin routine whose
    /// `env_pass` does not carry it, for instance): persisting text then
    /// would store the very value the operator configured scribe to mask,
    /// unmasked and without warning (STD-05 R13). The model credentials
    /// are not checked: they are alternatives, so one may be unset.
    pub(crate) fn redactor(&self, config: &Config) -> Result<Redactor, CliError> {
        let missing = unresolved_redact_env(config, |name| std::env::var(name).ok());
        if !missing.is_empty() {
            tracing::warn!(
                names = %missing.join(","),
                "refusing to write: a configured [redact] env name has no value in this environment"
            );
            return Err(CliError::RedactEnvUnset(missing));
        }
        Ok(Redactor::from_config(config, |name| {
            std::env::var(name).ok()
        }))
    }

    /// The model adapter for `config`: `claude` run with `PATH`, `HOME`,
    /// `LANG` and the configured credential variables from this process's
    /// environment, and nothing else (STD-05 R10).
    pub(crate) fn models(&self, config: &Config) -> ClaudeCliModels {
        let mut credentials = config.models.credentials.clone();
        if config.models.provider == Provider::Codex {
            credentials.push(String::from("CODEX_HOME"));
        }
        let mut env = allowed_env(&credentials, |name| std::env::var_os(name));
        if config.models.provider == Provider::Codex
            && let Some(root) = self.codex_transcripts.as_ref().and_then(|p| p.parent())
        {
            env.retain(|(key, _)| key != "CODEX_HOME");
            env.push((OsString::from("CODEX_HOME"), root.as_os_str().to_owned()));
        }
        ClaudeCliModels::new(env).with_provider(config.models.provider)
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
