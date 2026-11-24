//! `ClaudeCliModels`: the `Models` port reached through the `claude` CLI
//! (docs/design/scribe/2_design.md §8; 4_decisions.md, "Models are called
//! through the claude CLI").
//!
//! Each call runs
//! `claude -p --safe-mode --tools "" --model <id> --output-format json --json-schema <schema>`
//! with the prompt on stdin. `--safe-mode` keeps the user's hooks and
//! plugins, `scribe hook` among them, out of the call; `--tools ""` gives the
//! model no tools. The child:
//!
//! - launches from a cleared environment holding only [`BASELINE_ENV`] and
//!   the credential names `config.toml` lists, never a `SCRIBE_*` variable
//!   (STD-05 R10, R11); the credential travels in the environment, never in
//!   argv (R12);
//! - leads its own process group (STD-03 R11), so everything it starts is
//!   signalled with it;
//! - has its stdout and stderr drained while it runs, each up to a byte
//!   limit (R15), and its prompt written from a thread, so no pipe fills;
//! - at its budget gets SIGTERM to the group, a grace period, then SIGKILL
//!   to the group (R12); on every path the group is swept with SIGKILL and
//!   the child reaped before the call returns (R13). The group is signalled
//!   only while its leader is unreaped, so its id cannot have been reused
//!   (R14): the leader's exit is observed with `waitid(WNOWAIT)`, which
//!   leaves it waitable.
//!
//! The adapter spawns processes, so it lives in the CLI crate and the core
//! stays free of `std::process` (ARCHITECTURE.md).

mod codex;

use scribe_core::models::{
    ModelError, PROFILE_SCHEMA, ProfileAnswer, ProfileRequest, SESSION_HEADER_SCHEMA,
    SessionAnswer, SessionHeader, SessionHeaderRequest, SessionRequest,
};
use scribe_core::{Models, Provider, Redactor};
use serde::Deserialize;
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The program run, found on the child's `PATH`.
pub(crate) const PROGRAM: &str = "claude";

/// The variables every model call gets besides the configured credentials:
/// none of them carries a secret.
pub(crate) const BASELINE_ENV: [&str; 3] = ["PATH", "HOME", "LANG"];

/// The prefix of scribe's own variables, which never reach a model child.
const OWN_PREFIX: &str = "SCRIBE_";

/// The most stdout bytes kept; the rest is read and dropped.
const MAX_STDOUT: usize = 1024 * 1024;
/// The most stderr bytes kept.
const MAX_STDERR: usize = 64 * 1024;
/// The most characters of the child's own words an error message carries.
const MAX_DETAIL_CHARS: usize = 200;

/// How long the group has between SIGTERM and SIGKILL.
const GRACE: Duration = Duration::from_secs(1);
/// How often the supervisor looks for the leader's exit.
const POLL: Duration = Duration::from_millis(20);
/// How long the drained pipes may stay open after the group is swept (only a
/// process that left the group can hold them).
const DRAIN_WAIT: Duration = Duration::from_secs(2);

/// The environment a model child gets: [`BASELINE_ENV`] then each name in
/// `credentials`, each once, with the value `lookup` finds; a name with no
/// value is left out, and a `SCRIBE_*` name is never passed even when
/// configured (STD-05 R11).
pub(crate) fn allowed_env(
    credentials: &[String],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    let mut names: Vec<&str> = BASELINE_ENV.to_vec();
    for name in credentials {
        if !name.starts_with(OWN_PREFIX) && !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    names
        .into_iter()
        .filter_map(|name| lookup(name).map(|value| (OsString::from(name), value)))
        .collect()
}

/// The redactor a new adapter starts with: the values of the credential
/// variables its child was given (STD-05 R14). The baseline variables and the
/// Codex home path are not secrets, and stay readable in diagnostics.
fn credential_redactor(env: &[(OsString, OsString)]) -> Redactor {
    Redactor::new(
        env.iter()
            .filter(|(name, _)| {
                !matches!(name.to_str(), Some(n) if BASELINE_ENV.contains(&n) || n == "CODEX_HOME")
            })
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            }),
    )
}

/// The `claude` arguments for one call (the prompt goes on stdin).
pub(crate) fn argv(model: &str, schema: &str) -> Vec<String> {
    [
        "-p",
        "--safe-mode",
        "--tools",
        "",
        "--model",
        model,
        "--output-format",
        "json",
        "--json-schema",
        schema,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// The `Models` port over the `claude` CLI.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeCliModels {
    provider: Provider,
    program: OsString,
    env: Vec<(OsString, OsString)>,
    /// Masks secret values and credential shapes in every failure this
    /// adapter reports (see [`ClaudeCliModels::with_redactor`]).
    redactor: Redactor,
    /// The configured credential names, when none of them is set in this
    /// process's environment; empty otherwise. A failed call then names
    /// them, since `claude` refuses to run without a login
    /// ([`ClaudeCliModels::with_credentials`]).
    unset_credentials: Vec<String>,
}

impl ClaudeCliModels {
    /// Run [`PROGRAM`] with exactly `env` (see [`allowed_env`]). Failures
    /// mask the child's own credential values until
    /// [`ClaudeCliModels::with_redactor`] widens the set.
    pub(crate) fn new(env: Vec<(OsString, OsString)>) -> Self {
        Self {
            provider: Provider::ClaudeCode,
            program: OsString::from(PROGRAM),
            redactor: credential_redactor(&env),
            env,
            unset_credentials: Vec::new(),
        }
    }

    /// Note the configured credential `names` (`[models] credentials`). When
    /// none of them reached the child's environment, a failed call says so:
    /// a hook inherits the environment of the app that started it, and an
    /// app not started from a login shell may lack the variable the login
    /// shell exports, so `claude` exits refusing to run.
    pub(crate) fn with_credentials(mut self, names: &[String]) -> Self {
        let set = |name: &String| {
            self.env
                .iter()
                .any(|(key, _)| key.as_os_str() == name.as_str())
        };
        self.unset_credentials = if names.is_empty() || names.iter().any(set) {
            Vec::new()
        } else {
            names.to_vec()
        };
        self
    }

    /// Select the configured executable transport, with no model-name guessing.
    pub(crate) fn with_provider(mut self, provider: Provider) -> Self {
        self.provider = provider;
        self
    }

    /// Mask failures with `redactor`: the composition root passes every
    /// configured secret name (`Config::secret_names`), as persisted text does.
    pub(crate) fn with_redactor(mut self, redactor: Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// One call: the model's structured answer.
    fn call(
        &self,
        model: &str,
        schema: &str,
        prompt: String,
        budget: Duration,
    ) -> Result<Value, ModelError> {
        if self.provider == Provider::Codex {
            return codex::call(&self.env, &self.redactor, model, schema, prompt, budget);
        }
        let ran = supervise(
            self.command(&argv(model, schema)),
            prompt.into_bytes(),
            budget,
        )?;
        match ran.ended {
            Ended::Exited(status) if status.success() => structured(&ran.stdout, &self.redactor),
            Ended::Exited(status) => Err(ModelError::Failed(format!(
                "{PROGRAM} ended with {status}{}{}",
                self.unset_hint(),
                detail(&ran.stderr, &self.redactor)
                    .or_else(|| detail(&ran.stdout, &self.redactor))
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ))),
            Ended::TimedOut => Err(ModelError::Timeout { budget }),
        }
    }

    /// Why a call may have failed when no credential reached the child.
    fn unset_hint(&self) -> String {
        if self.unset_credentials.is_empty() {
            return String::new();
        }
        format!(
            " (none of {} is set in scribe's environment)",
            self.unset_credentials.join(", ")
        )
    }

    fn command(&self, args: &[String]) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(args)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd
    }
}

impl Models for ClaudeCliModels {
    fn group_summary(
        &self,
        model: &str,
        evidence: &scribe_core::group::CloseEvidence,
    ) -> Result<String, ModelError> {
        const SCHEMA: &str = r#"{"type":"object","properties":{"summary":{"type":"string","minLength":1,"maxLength":4000}},"required":["summary"],"additionalProperties":false}"#;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Summary {
            summary: String,
        }
        let value = self.call(
            model,
            SCHEMA,
            evidence.prompt(),
            scribe_core::models::ASK_BUDGET,
        )?;
        serde_json::from_value::<Summary>(value)
            .map(|answer| answer.summary)
            .map_err(|e| malformed(&self.redactor, "not a group summary", &e))
    }

    fn session(&self, request: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        let value = self.call(
            &request.model,
            &request.schema(),
            request.prompt(),
            request.budget,
        )?;
        serde_json::from_value(value)
            .map_err(|e| malformed(&self.redactor, "not a session record", &e))
    }

    fn session_header(&self, request: &SessionHeaderRequest) -> Result<SessionHeader, ModelError> {
        let value = self.call(
            &request.model,
            SESSION_HEADER_SCHEMA,
            request.prompt(),
            request.budget,
        )?;
        serde_json::from_value(value)
            .map_err(|e| malformed(&self.redactor, "not a session header", &e))
    }

    fn profile(&self, request: &ProfileRequest) -> Result<ProfileAnswer, ModelError> {
        let value = self.call(
            &request.model,
            PROFILE_SCHEMA,
            request.prompt(),
            request.budget,
        )?;
        serde_json::from_value(value)
            .map_err(|e| malformed(&self.redactor, "not a profile answer", &e))
    }
}

/// The structured answer in `claude --output-format json` output: the
/// result object's `structured_output`, or its `result` text parsed as JSON.
/// A result marked `is_error` is a failure, its text masked and bounded.
fn structured(stdout: &Drained, redactor: &Redactor) -> Result<Value, ModelError> {
    if stdout.truncated {
        return Err(ModelError::Malformed(format!(
            "more than {MAX_STDOUT} bytes of output"
        )));
    }
    let value: Value = serde_json::from_slice(&stdout.bytes)
        .map_err(|e| ModelError::Malformed(format!("{PROGRAM} output is not JSON: {e}")))?;
    let text = value.get("result").and_then(Value::as_str);
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(ModelError::Failed(text.map_or_else(
            || String::from("an error result"),
            |text| bounded(&redactor.mask(text)),
        )));
    }
    if let Some(found) = value.get("structured_output").filter(|v| v.is_object()) {
        return Ok(found.clone());
    }
    let text =
        text.ok_or_else(|| ModelError::Malformed(String::from("no result in the output")))?;
    let body = text.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .and_then(|b| b.strip_suffix("```"))
        .unwrap_or(body);
    serde_json::from_str(body.trim())
        .map_err(|e| ModelError::Malformed(format!("the result is not JSON: {e}")))
}

/// The last non-empty line of a child's output, masked and bounded, for an
/// error. The captured stream is masked as a whole before a line is chosen
/// or cut, so the cut never splits a credential the mask would have matched.
/// A stream cut at its limit ends mid-line; that partial line is left out,
/// since it may hold only the start of a credential.
fn detail(out: &Drained, redactor: &Redactor) -> Option<String> {
    let lossy = String::from_utf8_lossy(&out.bytes);
    let text: &str = &lossy;
    let kept = if out.truncated && !text.ends_with('\n') {
        &text[..text.rfind('\n').map_or(0, |at| at + 1)]
    } else {
        text
    };
    let masked = redactor.mask(kept);
    masked
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(bounded)
}

/// A malformed-answer error with its parse error masked. serde names the
/// value of a mistyped field, and that value is the answer's own text.
fn malformed(redactor: &Redactor, what: &str, error: &serde_json::Error) -> ModelError {
    ModelError::Malformed(redactor.mask(&format!("{what}: {error}")))
}

/// `text` cut to [`MAX_DETAIL_CHARS`], the cut marked (STD-03 R15).
fn bounded(text: &str) -> String {
    if text.chars().count() <= MAX_DETAIL_CHARS {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(MAX_DETAIL_CHARS).collect();
    cut.push('…');
    cut
}

/// How a supervised child ended.
#[derive(Debug)]
pub(crate) enum Ended {
    /// It exited (or was killed by a signal it did not get from us).
    Exited(ExitStatus),
    /// It ran past its budget and its group was killed.
    TimedOut,
}

/// Output drained from one pipe, up to its limit.
#[derive(Debug, Default)]
pub(crate) struct Drained {
    pub(crate) bytes: Vec<u8>,
    pub(crate) truncated: bool,
}

/// What a supervised run left.
#[derive(Debug)]
pub(crate) struct Ran {
    pub(crate) ended: Ended,
    pub(crate) stdout: Drained,
    pub(crate) stderr: Drained,
}

/// Run `cmd` (already set to lead its own group, with piped stdio), writing
/// `input` to its stdin, within `budget`. The child is reaped and its group
/// swept before this returns, on every path.
pub(crate) fn supervise(
    mut cmd: Command,
    input: Vec<u8>,
    budget: Duration,
) -> Result<Ran, ModelError> {
    let program = OsStr::new(cmd.get_program()).to_string_lossy().into_owned();
    let child = cmd
        .spawn()
        .map_err(|e| ModelError::Unavailable(format!("{program}: {e}")))?;
    let mut guard = Group::new(child);
    let child = guard.child_mut();
    if let Some(mut stdin) = child.stdin.take() {
        // A child that never reads gets EPIPE once it dies; the thread ends.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let stdout = drain(child.stdout.take(), MAX_STDOUT);
    let stderr = drain(child.stderr.take(), MAX_STDERR);

    let deadline = Instant::now() + budget;
    let timed_out = !guard.wait_until(deadline);
    if timed_out {
        guard.signal(Signal::Term);
        guard.wait_until(Instant::now() + GRACE);
    }
    let status = guard.finish();
    let collect = |rx: mpsc::Receiver<Drained>| rx.recv_timeout(DRAIN_WAIT).unwrap_or_default();
    let (stdout, stderr) = (collect(stdout), collect(stderr));
    let ended = match (timed_out, status) {
        (true, _) => Ended::TimedOut,
        (false, Some(status)) => Ended::Exited(status),
        (false, None) => {
            return Err(ModelError::Failed(format!(
                "{program} could not be waited for"
            )));
        }
    };
    Ok(Ran {
        ended,
        stdout,
        stderr,
    })
}

/// Read `pipe` to its end on a thread, keeping at most `limit` bytes.
fn drain<R: Read + Send + 'static>(pipe: Option<R>, limit: usize) -> mpsc::Receiver<Drained> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Drained::default();
        if let Some(mut pipe) = pipe {
            let mut buf = [0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = limit.saturating_sub(out.bytes.len());
                        out.bytes.extend_from_slice(&buf[..n.min(room)]);
                        out.truncated |= n > room;
                    }
                }
            }
        }
        let _ = tx.send(out);
    });
    rx
}

/// Which signal to send a group.
#[derive(Clone, Copy, Debug)]
enum Signal {
    Term,
    Kill,
}

/// A spawned child leading its own process group, swept and reaped on drop
/// if [`Group::finish`] was not reached (a panic included).
struct Group {
    child: Option<Child>,
}

impl Group {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    #[allow(clippy::expect_used)] // set in `new`, taken only by `finish`/drop
    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("the child is held until finish")
    }

    /// Wait until the leader exits (true) or `deadline` passes (false),
    /// leaving it unreaped so its group id stays ours.
    fn wait_until(&mut self, deadline: Instant) -> bool {
        loop {
            if self.leader_exited() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::sleep(POLL.min(deadline - now));
        }
    }

    #[cfg(unix)]
    fn leader_exited(&mut self) -> bool {
        use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
        let Some(child) = &self.child else {
            return true;
        };
        let pid = Pid::from_child(child);
        matches!(
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ),
            Ok(Some(_))
        )
    }

    #[cfg(not(unix))]
    fn leader_exited(&mut self) -> bool {
        // Without waitid(WNOWAIT) the leader is polled by reaping it; there
        // is no process group whose id could be reused.
        self.child
            .as_mut()
            .is_none_or(|c| matches!(c.try_wait(), Ok(Some(_))))
    }

    /// Signal the whole group. Only called while the leader is unreaped.
    #[cfg(unix)]
    fn signal(&mut self, signal: Signal) {
        use rustix::process::{Pid, kill_process_group};
        let Some(child) = &self.child else {
            return;
        };
        let sig = match signal {
            Signal::Term => rustix::process::Signal::TERM,
            Signal::Kill => rustix::process::Signal::KILL,
        };
        // ESRCH cannot happen while the leader is unreaped; any other
        // failure leaves the reap below to end the leader.
        let _ = kill_process_group(Pid::from_child(child), sig);
    }

    #[cfg(not(unix))]
    fn signal(&mut self, _signal: Signal) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
        }
    }

    /// Sweep the group with SIGKILL and reap the leader: its exit status,
    /// or `None` when it could not be waited for.
    fn finish(&mut self) -> Option<ExitStatus> {
        self.signal(Signal::Kill);
        let mut child = self.child.take()?;
        child.wait().ok()
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.finish();
        }
    }
}
