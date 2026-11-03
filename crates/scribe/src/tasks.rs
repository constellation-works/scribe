//! `OrbitTasks`: the `Tasks` port reached through `orbit tool run`
//! (docs/design/scribe/2_design.md §6.2, §6.5).
//!
//! A search runs `orbit tool run orbit.search` for task ids, then
//! `orbit tool run orbit.task.show` for each hit's description and execution
//! summary. Inside the Orbit plugin the calls go back through the host's
//! callback channel (`orbit_tools` in `plugin.yaml`): the child inherits the
//! callback descriptor and the `ORBIT_*` variables the host set. From the
//! CLI they run against the workspace the current directory is in. The
//! plugin's `task_workspaces` setting widens the search to the workspaces
//! it names (Orbit refuses that inside a managed run, so it is opt-in).
//!
//! The program is `$ORBIT_BIN` when set, else `orbit` on `PATH`. Each child
//! gets a cleared environment holding only `PATH`, `HOME`, `LANG` and the
//! `ORBIT_*` variables, never a `SCRIBE_*` one (STD-05 R10, R11), and is
//! supervised exactly as a model child is ([`crate::models::supervise`]):
//! its own process group, drained pipes, a budget per call and one for the
//! whole search, the group swept and the child reaped on every path
//! (STD-03 R11–R15).
//!
//! Like `models.rs`, this module spawns processes, so it lives in the CLI
//! crate and the core stays free of `std::process` (ARCHITECTURE.md).

use crate::models::{BASELINE_ENV, Ended, Ran, supervise};
use scribe_core::ask::{Task, TaskError, TaskQuery, Tasks};
use scribe_core::models::ModelError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// The program run when `ORBIT_BIN` is not set.
pub(crate) const PROGRAM: &str = "orbit";
/// The variable naming the Orbit binary; the plugin host sets it.
pub(crate) const PROGRAM_ENV: &str = "ORBIT_BIN";
/// The prefix of the variables passed through: the plugin host's callback
/// channel and workspace binding.
const PASS_PREFIX: &str = "ORBIT_";
/// The prefix of scribe's own variables, which never reach the child.
const OWN_PREFIX: &str = "SCRIBE_";
/// How long one `orbit tool run` may take.
const CALL_BUDGET: Duration = Duration::from_secs(15);
/// How long one search may take, every call included.
const SEARCH_BUDGET: Duration = Duration::from_secs(30);
/// The task fields read for each hit.
const FIELDS: [&str; 7] = [
    "id",
    "title",
    "status",
    "tags",
    "description",
    "execution_summary",
    "updated_at",
];

/// The `Tasks` port over `orbit tool run`.
#[derive(Clone, Debug)]
pub(crate) struct OrbitTasks {
    program: OsString,
    env: Vec<(OsString, OsString)>,
    workspaces: Vec<String>,
    call_budget: Duration,
    search_budget: Duration,
}

impl OrbitTasks {
    /// Run `program` with exactly `env` (see [`allowed_env`]).
    pub(crate) fn new(program: OsString, env: Vec<(OsString, OsString)>) -> Self {
        Self {
            program,
            env,
            workspaces: Vec::new(),
            call_budget: CALL_BUDGET,
            search_budget: SEARCH_BUDGET,
        }
    }

    /// The same, searching these workspaces (`orbit.search`'s `workspaces`)
    /// instead of the one the call is bound to.
    pub(crate) fn with_workspaces(mut self, workspaces: Vec<String>) -> Self {
        self.workspaces = workspaces;
        self
    }

    /// The same, with other budgets (tests).
    #[cfg(test)]
    pub(crate) fn with_budgets(mut self, call: Duration, search: Duration) -> Self {
        self.call_budget = call;
        self.search_budget = search;
        self
    }

    /// One `orbit tool run <tool> --input <input>`: its JSON output.
    fn run(&self, tool: &str, input: &Value, deadline: Instant) -> Result<Value, TaskError> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(TaskError(format!(
                "the search ran past {} s",
                self.search_budget.as_secs()
            )));
        }
        let budget = left.min(self.call_budget);
        let mut cmd = Command::new(&self.program);
        cmd.args(["tool", "run", tool, "--input"])
            .arg(input.to_string())
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
        let Ran { ended, stdout, .. } =
            supervise(cmd, Vec::new(), budget).map_err(|e| match e {
                ModelError::Unavailable(detail) => TaskError(format!("cannot start {detail}")),
                other @ (ModelError::Timeout { .. }
                | ModelError::Failed(_)
                | ModelError::Malformed(_)) => TaskError(other.to_string()),
            })?;
        match ended {
            Ended::Exited(status) if status.success() => {}
            Ended::Exited(status) => {
                return Err(TaskError(format!("{tool} ended with {status}")));
            }
            Ended::TimedOut => {
                return Err(TaskError(format!(
                    "{tool} did not answer within {} s",
                    budget.as_secs()
                )));
            }
        }
        if stdout.truncated {
            return Err(TaskError(format!("{tool} printed too much")));
        }
        serde_json::from_slice(&stdout.bytes)
            .map_err(|e| TaskError(format!("{tool} output is not JSON: {e}")))
    }
}

/// The environment an Orbit child gets: [`BASELINE_ENV`] and every
/// `ORBIT_*` variable in `vars`, never a `SCRIBE_*` one.
pub(crate) fn allowed_env(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            !name.starts_with(OWN_PREFIX)
                && (BASELINE_ENV.contains(&name.as_ref()) || name.starts_with(PASS_PREFIX))
        })
        .collect()
}

/// The program to run: `ORBIT_BIN` when it is set and not empty, else
/// [`PROGRAM`].
pub(crate) fn program(orbit_bin: Option<OsString>) -> OsString {
    orbit_bin
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| OsString::from(PROGRAM))
}

#[derive(Deserialize)]
struct SearchOutput {
    results: Vec<Hit>,
}

#[derive(Deserialize)]
struct Hit {
    kind: String,
    id: String,
}

#[derive(Deserialize)]
struct ShownTask {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    execution_summary: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl Tasks for OrbitTasks {
    fn search(&self, query: &TaskQuery) -> Result<Vec<Task>, TaskError> {
        let deadline = Instant::now() + self.search_budget;
        let mut input = json!({
            "query": query.text,
            "kind": "task",
            "all": true,
            "limit": query.limit,
        });
        if !query.tags.is_empty() {
            input["tag"] = json!(query.tags);
        }
        if !self.workspaces.is_empty() {
            input["workspaces"] = json!(self.workspaces);
        }
        let found: SearchOutput =
            serde_json::from_value(self.run("orbit.search", &input, deadline)?)
                .map_err(|e| TaskError(format!("orbit.search answered in another shape: {e}")))?;
        let mut tasks = Vec::new();
        for hit in found
            .results
            .into_iter()
            .filter(|h| h.kind == "task")
            .take(query.limit)
        {
            let shown = self.run(
                "orbit.task.show",
                &json!({ "id": hit.id, "fields": FIELDS }),
                deadline,
            )?;
            let t: ShownTask = serde_json::from_value(shown).map_err(|e| {
                TaskError(format!("orbit.task.show answered in another shape: {e}"))
            })?;
            tasks.push(Task {
                id: t.id,
                title: t.title,
                status: t.status,
                tags: t.tags,
                updated: t
                    .updated_at
                    .as_deref()
                    .and_then(|at| OffsetDateTime::parse(at, &Rfc3339).ok()),
                description: t.description.unwrap_or_default(),
                execution_summary: t.execution_summary.unwrap_or_default(),
            });
        }
        Ok(tasks)
    }
}
