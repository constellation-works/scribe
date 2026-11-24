//! Composition (STD-02 R2): the one place that resolves anything about the
//! host. It captures the environment once ([`Env::from_process`]), picks the
//! data directory, constructs the store, and hands the apps resolved values
//! (STD-02 R3); it composes the model adapter
//! ([`ClaudeCliModels`](crate::providers::ClaudeCliModels)) and knows nothing
//! about argument parsing or rendering. Outside this module and
//! `providers/`, nothing in the core reads an environment variable or
//! touches a file (`scripts/check-dependency-direction.sh`).
//!
//! [`Context`] is generic over its ports' error `E`, which the binary sets to
//! its own error until the core's error mapping lands (ORB-27921 step 3,
//! PR 4); the context's own failures are [`HostError`]s, which `E` is built
//! from.

use crate::providers::{ClaudeCliModels, allowed_env};
pub use crate::runtime::errors::HostError;
use crate::{Config, Label, Provider, Redactor, Store, unresolved_redact_env};
use std::ffi::OsString;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;

/// Environment variable naming the data directory.
pub const ROOT_ENV: &str = "SCRIBE_ROOT";

/// Environment variable Claude Code sets on its hooks: `cli` when a person
/// types into the session.
const ENTRYPOINT_ENV: &str = "CLAUDE_CODE_ENTRYPOINT";

/// Environment variable that moves Claude Code's state directory.
const CLAUDE_CONFIG_ENV: &str = "CLAUDE_CONFIG_DIR";

/// The host label used when the host name cannot be read or is not a label.
const UNKNOWN_HOST: &str = "unknown-host";

/// Directory under `$HOME` used when nothing else names one.
const DEFAULT_DIR: &str = ".scribe";

/// The configuration-relevant environment, captured once.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// `SCRIBE_ROOT`.
    pub root: Option<OsString>,
    /// `HOME`.
    pub home: Option<OsString>,
    /// `CLAUDE_CONFIG_DIR`: where Claude Code keeps its state instead of
    /// `~/.claude`, and so its transcripts.
    pub claude_config_dir: Option<OsString>,
    /// Optional Codex state directory.
    pub codex_home: Option<OsString>,
    /// `CLAUDE_CODE_ENTRYPOINT`: `cli` in an interactive Claude Code
    /// session, `sdk-cli` for every `claude -p` run (2_design.md §12.5).
    pub claude_entrypoint: Option<OsString>,
}

impl Env {
    /// Read the variables from the real process environment.
    #[must_use]
    pub fn from_process() -> Self {
        Self {
            root: std::env::var_os(ROOT_ENV),
            home: std::env::var_os("HOME"),
            claude_config_dir: std::env::var_os(CLAUDE_CONFIG_ENV),
            codex_home: std::env::var_os("CODEX_HOME"),
            claude_entrypoint: std::env::var_os(ENTRYPOINT_ENV),
        }
    }
}

/// Everything a command needs, resolved. `E` is the error its ports report
/// ([`crate::runtime::Transcripts`], [`crate::runtime::Redaction`]).
pub struct Context<E = HostError> {
    store: Store,
    transcripts: Option<PathBuf>,
    codex_transcripts: Option<PathBuf>,
    codex_archived: Option<PathBuf>,
    provider: Provider,
    /// Whether this process runs under an interactive Claude Code session
    /// (`cli` or `claude-desktop`): only then does `SessionStart` inject
    /// the memory page (2_design.md §13.6).
    interactive: bool,
    /// The instruction the running command acts on, for its audit line;
    /// shared by the provider-scoped views of one context.
    instruction: Arc<Mutex<InstructionNote>>,
    /// Whether a stepwise command already committed its success audit inside
    /// its final store transaction, so the enclosing guard must not emit a
    /// duplicate or unwind line.
    audit_committed: Arc<AtomicBool>,
    error: PhantomData<fn() -> E>,
}

impl<E> Clone for Context<E> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            transcripts: self.transcripts.clone(),
            codex_transcripts: self.codex_transcripts.clone(),
            codex_archived: self.codex_archived.clone(),
            provider: self.provider,
            interactive: self.interactive,
            instruction: Arc::clone(&self.instruction),
            audit_committed: Arc::clone(&self.audit_committed),
            error: PhantomData,
        }
    }
}

impl<E> std::fmt::Debug for Context<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("store", &self.store)
            .field("transcripts", &self.transcripts)
            .field("codex_transcripts", &self.codex_transcripts)
            .field("codex_archived", &self.codex_archived)
            .field("provider", &self.provider)
            .field("interactive", &self.interactive)
            .field("instruction", &self.instruction)
            .field("audit_committed", &self.audit_committed)
            .finish()
    }
}

/// The ids of the user's instruction a write ran on (2_design.md §12.4): the
/// session from `--session`, and the turn's message once the check found it.
/// Ids only; the instruction's words never reach the audit log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstructionNote {
    /// The `--session` the instruction was looked for in.
    pub session: Option<String>,
    /// The uuid of the turn that backs it, once the check passed.
    pub message: Option<String>,
}

impl<E> Context<E> {
    /// Resolve the context. Data directory precedence: `--root` >
    /// `SCRIBE_ROOT` > `$HOME/.scribe`.
    ///
    /// # Errors
    ///
    /// [`HostError::NoDataDir`] when nothing names a data directory.
    pub fn resolve(root_flag: Option<&Path>, env: &Env) -> Result<Self, HostError> {
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
            interactive: env
                .claude_entrypoint
                .as_deref()
                .and_then(|e| e.to_str())
                .is_some_and(|e| crate::instruction::INTERACTIVE_ENTRYPOINTS.contains(&e)),
            instruction: Arc::default(),
            audit_committed: Arc::default(),
            error: PhantomData,
        })
    }

    /// A context for the store at `root` and the transcripts under
    /// `transcripts`, as the plugin's settings name them (`plugin call`
    /// never reads `SCRIBE_ROOT` or `HOME`).
    #[must_use]
    pub fn at(root: PathBuf, transcripts: Option<PathBuf>) -> Self {
        Self {
            store: Store::open(root),
            transcripts,
            codex_transcripts: None,
            codex_archived: None,
            provider: Provider::ClaudeCode,
            interactive: false,
            instruction: Arc::default(),
            audit_committed: Arc::default(),
            error: PhantomData,
        }
    }

    /// A provider-scoped view sharing the same store.
    #[must_use]
    pub fn with_provider(&self, provider: Provider) -> Self {
        Self {
            provider,
            ..self.clone()
        }
    }
    /// Explicit Codex state root for sandboxed plugin callers; no env fallback.
    #[must_use]
    pub fn with_codex_home(mut self, root: Option<PathBuf>) -> Self {
        self.codex_transcripts = root.as_ref().map(|r| r.join("sessions"));
        self.codex_archived = root.map(|r| r.join("archived_sessions"));
        self
    }
    /// All permitted transcript roots for this provider, including archives.
    pub fn transcript_roots(&self) -> Vec<&Path> {
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
    pub fn note_instruction_session(&self, session: &str) {
        if let Ok(mut note) = self.instruction.lock() {
            note.session = Some(session.to_owned());
        }
    }

    /// Note the turn a passing instruction check found.
    pub fn note_instruction_message(&self, message: &str) {
        if let Ok(mut note) = self.instruction.lock() {
            note.message = Some(message.to_owned());
        }
    }

    /// What the running command noted of its instruction.
    pub fn instruction_note(&self) -> InstructionNote {
        self.instruction
            .lock()
            .map(|note| note.clone())
            .unwrap_or_default()
    }

    /// Note that a stepwise command already committed its success audit inside
    /// its final store write transaction.
    pub fn note_audit_committed(&self) {
        self.audit_committed.store(true, Ordering::Release);
    }

    /// Shared flag telling the enclosing `AuditGuard` that the audit row was
    /// already committed inside a store write transaction.
    pub fn audit_committed_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.audit_committed)
    }

    /// The active transcript provider.
    /// Whether this runs under an interactive Claude Code session.
    #[must_use]
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// The active transcript provider.
    #[must_use]
    pub fn provider(&self) -> Provider {
        self.provider
    }
    /// Config with provider-specific model defaults resolved.
    ///
    /// # Errors
    ///
    /// When the store or `config.toml` cannot be read.
    pub fn config(&self) -> Result<Config, HostError> {
        Ok(self
            .store
            .read()
            .tags()
            .config()?
            .for_provider(self.provider))
    }

    /// Refuse scaffolding when the selected runtime is not executable on PATH.
    ///
    /// # Errors
    ///
    /// [`HostError::Usage`] when it is not.
    pub fn require_runtime(&self) -> Result<(), HostError> {
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
            return Err(HostError::Usage(format!(
                "{program} is not executable on PATH; install the selected runtime and rerun scribe init"
            )));
        }
        Ok(())
    }

    /// The scribe store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Where Claude Code keeps session transcripts:
    /// `$CLAUDE_CONFIG_DIR/projects`, else `$HOME/.claude/projects`. `None`
    /// when neither is set, which makes every transcript fail the
    /// provenance check.
    pub fn transcripts(&self) -> Option<&Path> {
        match self.provider {
            Provider::ClaudeCode => self.transcripts.as_deref(),
            Provider::Codex => self.codex_transcripts.as_deref(),
        }
    }

    /// This host's name as a record label, for the records it stamps.
    pub fn host(&self) -> Label {
        Label::sanitized(host_name().as_deref().unwrap_or_default(), UNKNOWN_HOST)
    }

    /// The redactor for `config`: the live values of the variables it names,
    /// read from this process's environment. Fails closed when a
    /// `[redact] env` name has no value here (a plugin routine whose
    /// `env_pass` does not carry it, for instance): persisting text then
    /// would store the very value the operator configured scribe to mask,
    /// unmasked and without warning (STD-05 R13). The model credentials
    /// are not checked: they are alternatives, so one may be unset.
    ///
    /// # Errors
    ///
    /// [`HostError::RedactEnvUnset`] when a name has no value.
    pub fn redactor(&self, config: &Config) -> Result<Redactor, HostError> {
        let missing = unresolved_redact_env(config, |name| std::env::var(name).ok());
        if !missing.is_empty() {
            tracing::warn!(
                names = %missing.join(","),
                "refusing to write: a configured [redact] env name has no value in this environment"
            );
            return Err(HostError::RedactEnvUnset(missing));
        }
        Ok(Redactor::from_config(config, |name| {
            std::env::var(name).ok()
        }))
    }

    /// The model adapter for `config`: `claude` run with `PATH`, `HOME`,
    /// `LANG` and the configured credential variables from this process's
    /// environment, and nothing else (STD-05 R10). Its failure reports mask
    /// every configured secret, as persisted text does (STD-05 R14).
    pub fn models(&self, config: &Config) -> ClaudeCliModels {
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
        let named = if config.models.provider == Provider::Codex {
            Vec::new()
        } else {
            config.models.credentials.clone()
        };
        ClaudeCliModels::new(env)
            .with_provider(config.models.provider)
            .with_credentials(&named)
            .with_redactor(Redactor::from_config(config, |name| {
                std::env::var(name).ok()
            }))
    }

    /// The current time, whole seconds, for records created by this invocation.
    pub fn now(&self) -> OffsetDateTime {
        let now = OffsetDateTime::now_utc();
        now.replace_nanosecond(0).unwrap_or(now)
    }
}

/// The transcripts directory precedence, separated so tests can check it.
#[must_use]
pub fn transcripts_dir(env: &Env) -> Option<PathBuf> {
    let non_empty = |v: &Option<OsString>| v.clone().filter(|s| !s.is_empty());
    non_empty(&env.claude_config_dir)
        .map(PathBuf::from)
        .or_else(|| non_empty(&env.home).map(|home| PathBuf::from(home).join(".claude")))
        .map(|dir| dir.join("projects"))
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    scribe_tools::host::name()
}

#[cfg(not(unix))]
fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty())
}

/// The data-directory precedence, separated so tests can check it directly.
///
/// # Errors
///
/// [`HostError::NoDataDir`] when nothing names one.
pub fn resolve_root(root_flag: Option<&Path>, env: &Env) -> Result<PathBuf, HostError> {
    let non_empty = |v: &Option<OsString>| v.clone().filter(|s| !s.is_empty());
    if let Some(flag) = root_flag {
        return Ok(flag.to_path_buf());
    }
    if let Some(root) = non_empty(&env.root) {
        return Ok(PathBuf::from(root));
    }
    non_empty(&env.home)
        .map(|home| PathBuf::from(home).join(DEFAULT_DIR))
        .ok_or(HostError::NoDataDir)
}
