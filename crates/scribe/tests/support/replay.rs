//! Replay fixtures for the session rebuild: recorded activity answers
//! played back to the `scribe-session-rebuild` job's plugin steps, and the
//! redaction that turns a private recording into one that can be checked in
//! (docs/design/scribe/specs/replay-fixtures.md).
//!
//! A recording is one directory:
//!
//! - `fixture.json`: `session`, `project` (the directory under
//!   `.claude/projects/` the transcript is placed in), `author`, and
//!   `expect` with the rebuild report's `windows`, `segments` and
//!   `warnings`;
//! - `transcript.jsonl`: the Claude Code transcript;
//! - `store/` (optional): files copied to the data root, such as
//!   `config.toml`;
//! - `seed/` (optional): current-format (`format: 31`) Markdown records and
//!   JSON documents, written into the root's `scribe.db` before the rebuild
//!   (no build reads record files). The synthetic recording's were made by
//!   migrating its earlier file store with an earlier build and exporting
//!   it with `scribe export --as markdown`;
//! - `call-NNN.args`, `call-NNN.in`, `call-NNN.out` per activity call, from
//!   `001`: the activity the job runs (`activity:<name>`), the prompt the
//!   activity renders from scribe's request, and the answer recorded as the
//!   `claude` CLI printed it, whose `structured_output` (or `result` text
//!   parsed as JSON) is the activity's result.
//!
//! The replay answers call `n` from `call-n.out` only when the activity and
//! prompt equal the recorded ones byte for byte. A request that differs, or
//! a call past the last recorded one, writes the reason to a failure file
//! and fails the activity; the harness reports it ahead of anything the
//! rebuild says.
use super::{Fixture, copy_private, make_private, text};
use crate::support::command::Exit::Success;
use crate::support::command::Input::Cli;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The checked-in recordings.
pub fn recordings_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay")
}

/// A recording's `fixture.json`.
#[derive(Clone, Debug)]
pub struct Meta {
    pub session: String,
    pub project: String,
    pub author: String,
    pub expect: Value,
}

impl Meta {
    pub fn read(recording: &Path) -> Result<Self, String> {
        let file = recording.join("fixture.json");
        let raw = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let value: Value =
            serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", file.display()))?;
        let field = |name: &str| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{}: `{name}` is not a string", file.display()))
        };
        Ok(Self {
            session: field("session")?,
            project: field("project")?,
            author: field("author")?,
            expect: value["expect"].clone(),
        })
    }
}

/// The recorded calls, `call-001` onwards, stopping at the first gap.
pub fn recorded_calls(recording: &Path) -> usize {
    (1..)
        .take_while(|n| recording.join(format!("{}.out", call_name(*n))).is_file())
        .count()
}

pub fn call_name(n: usize) -> String {
    format!("call-{n:03}")
}

/// How the fake `claude` treats the recorded requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Answer only a request equal to the recorded one.
    Strict,
    /// Answer in order without comparing; the requests are kept for
    /// [`Run::requests`].
    Record,
}

/// One rebuild driven by a recording.
pub struct Run {
    pub fx: Fixture,
    pub output: Output,
    /// Model calls the rebuild made.
    pub calls: usize,
    /// What the fake `claude` refused, if anything.
    pub replay_failure: Option<String>,
    state: PathBuf,
}

impl Run {
    /// The parsed `--json` report.
    pub fn report(&self) -> Value {
        serde_json::from_slice(&self.output.stdout).unwrap_or(Value::Null)
    }

    /// The request (`.args` and `.in` bytes) the rebuild made as call `n`.
    pub fn request(&self, n: usize) -> (Vec<u8>, Vec<u8>) {
        let read = |ext: &str| {
            std::fs::read(self.state.join(format!("{}.{ext}", call_name(n)))).unwrap_or_default()
        };
        (read("args"), read("in"))
    }

    /// Panic unless the replay consumed every recorded call, refused
    /// nothing and the rebuild exited 0.
    pub fn assert_replayed(&self, recorded: usize) {
        if let Some(failure) = &self.replay_failure {
            panic!(
                "replay refused a request: {failure}\nscribe stderr: {}",
                text(&self.output.stderr)
            );
        }
        assert_eq!(
            self.output.status.code(),
            Some(0),
            "rebuild failed: {}{}",
            text(&self.output.stderr),
            text(&self.output.stdout)
        );
        assert_eq!(
            self.calls, recorded,
            "the rebuild made {} model calls; the recording holds {recorded}",
            self.calls
        );
    }

    /// The written record, through `session show --json`.
    pub fn record(&self, session: &str) -> Value {
        let out = self
            .fx
            .check(Cli(&["session", "show", session, "--json"]), Success);
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }
}

/// Run the `scribe-session-rebuild` job for the recording's session and
/// author through the built binary's plugin steps, on a temp root seeded
/// from `recording`, with its transcript under `HOME/.claude/projects/` and
/// each activity answered from the recording ([`replay_activity`]).
pub fn rebuild(recording: &Path, mode: Mode) -> Run {
    let meta = Meta::read(recording).unwrap_or_else(|e| panic!("{e}"));
    let fx = Fixture::new();
    let store = recording.join("store");
    if store.is_dir() {
        copy_private(&store, &fx.root());
    }
    let seed = recording.join("seed");
    if seed.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&seed)
            .expect("read seed dir")
            .map(|item| item.expect("seed entry").path())
            .collect();
        files.sort();
        let texts: Vec<String> = files
            .iter()
            .map(|path| std::fs::read_to_string(path).expect("read seed file"))
            .collect();
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        super::seed_records(&fx.root(), &texts);
    }
    let project = fx.home().join(".claude/projects").join(&meta.project);
    std::fs::create_dir_all(&project).expect("create projects dir");
    let transcript = project.join(format!("{}.jsonl", meta.session));
    std::fs::copy(recording.join("transcript.jsonl"), &transcript).expect("place transcript");
    // `copy` keeps the recording's mode, which follows the checkout's umask.
    make_private(&transcript);
    let state = fx.elsewhere("replay");
    std::fs::create_dir(&state).expect("create replay state");
    let config = super::pass::claude_config(&fx);
    let out = super::pass::rebuild(
        |step, input| {
            super::pass::plugin_tool(&fx, &[], &format!("session-{step}"), input, &config)
        },
        &serde_json::json!({"session": meta.session, "author": meta.author, "confirm": true}),
        |request, header| replay_activity(recording, &state, mode, request, header),
    );
    let calls = (1..)
        .take_while(|n| state.join(format!("{}.in", call_name(*n))).exists())
        .count();
    let replay_failure = std::fs::read_to_string(state.join("failure")).ok();
    Run {
        fx,
        output: out,
        calls,
        replay_failure,
        state,
    }
}

/// The activity a request goes to, as a call's `.args` records it.
fn activity_args(header: bool) -> &'static [u8] {
    if header {
        b"activity:scribe_session_header_compress\n"
    } else {
        b"activity:scribe_session_distill\n"
    }
}

/// Answer activity call `n` from the recording: save its activity
/// (`.args`) and the prompt the activity renders from its request (`.in`),
/// then answer `call-n.out`'s `structured_output` when the request matches
/// (or, in [`Mode::Record`], always). A request that differs, or a call
/// past the last recorded one, writes the reason to the failure file and
/// fails the activity.
fn replay_activity(
    recording: &Path,
    state: &Path,
    mode: Mode,
    request: &Value,
    header: bool,
) -> Result<Value, super::pass::ActivityFailure> {
    let n = 1
        + (1..)
            .take_while(|n| state.join(format!("{}.in", call_name(*n))).exists())
            .count();
    let id = call_name(n);
    let prompt = if header {
        serde_json::from_value::<scribe_core::models::SessionHeaderRequest>(request.clone())
            .expect("a header request")
            .prompt()
    } else {
        serde_json::from_value::<scribe_core::models::SessionWindowInput>(request.clone())
            .expect("a window request")
            .prompt()
    };
    std::fs::write(state.join(format!("{id}.in")), &prompt).expect("save the prompt");
    std::fs::write(state.join(format!("{id}.args")), activity_args(header))
        .expect("save the activity");
    let fail = |reason: String| {
        let reason = format!("{id}: {reason}");
        let mut failure = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.join("failure"))
            .expect("open the failure file");
        std::io::Write::write_all(&mut failure, format!("{reason}\n").as_bytes())
            .expect("write the failure");
        Err(super::pass::ActivityFailure::failed(format!(
            "replay: {reason}"
        )))
    };
    let Ok(out) = std::fs::read(recording.join(format!("{id}.out"))) else {
        return fail(format!("recording exhausted: no {id}.out was recorded"));
    };
    if mode == Mode::Strict {
        let recorded = |ext: &str| std::fs::read(recording.join(format!("{id}.{ext}"))).ok();
        if recorded("args").as_deref() != Some(activity_args(header)) {
            return fail(format!("arguments differ from {id}.args"));
        }
        if recorded("in").as_deref() != Some(prompt.as_bytes()) {
            return fail(format!("prompt differs from {id}.in"));
        }
    }
    let out: Value = serde_json::from_slice(&out).expect("a recorded answer");
    Ok(recorded_answer(&out))
}

/// The answer a recorded `claude --output-format json` result holds: its
/// `structured_output`, or its `result` text, in a JSON fence or bare,
/// parsed as JSON. The synthetic recording keeps one fenced answer, a
/// failure the historical rebuild met.
fn recorded_answer(out: &Value) -> Value {
    if out["structured_output"].is_object() {
        return out["structured_output"].clone();
    }
    let body = out["result"].as_str().expect("a recorded result").trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .and_then(|b| b.strip_suffix("```"))
        .unwrap_or(body);
    serde_json::from_str(body.trim()).expect("a recorded JSON answer")
}

/// Check a strict replay's outcome against `fixture.json`'s `expect`: the
/// report's `windows`, `segments` and `warnings`, and the record's log.
pub fn assert_expected(recording: &Path, run: &Run) {
    let meta = Meta::read(recording).unwrap_or_else(|e| panic!("{e}"));
    run.assert_replayed(recorded_calls(recording));
    let report = run.report();
    let pass = &report["passes"][0];
    assert_eq!(pass["result"], "written", "{report}");
    for field in ["windows", "segments", "warnings"] {
        assert_eq!(
            pass[field],
            meta.expect[field],
            "{}: {field}",
            recording.display()
        );
    }
    let record = run.record(&meta.session);
    assert_eq!(
        record["log"].as_array().map(Vec::len),
        meta.expect["segments"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok()),
        "written log"
    );
}

/// Rewrite the `.args` and `.in` files of `recording` from a
/// [`Mode::Record`] run, after an intended change to the requests.
pub fn bless(recording: &Path) {
    let run = rebuild(recording, Mode::Record);
    run.assert_replayed(recorded_calls(recording));
    for n in 1..=run.calls {
        let (args, input) = run.request(n);
        let name = call_name(n);
        std::fs::write(recording.join(format!("{name}.args")), args).expect("write args");
        std::fs::write(recording.join(format!("{name}.in")), input).expect("write prompt");
    }
}

pub mod redact;
