use crate::support;
use crate::support::command::Exit::Capture;
use crate::support::command::Input::Cli;
use crate::support::command::Json;
use crate::support::command::with_env;
use crate::support::command::{Expectation, Input};
use crate::support::{
    Fixture, context_of, make_private,
    pass::{self, ActivityFailure},
    text,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Output};

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &["session"], expect)
    }
}

#[path = "session_pass/setup.rs"]
mod setup;
pub(super) use setup::{Responses, Setup, answer, line};

pub(super) const ON: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

pub(super) const NEVER: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e60";

pub(super) const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";

pub(super) const NEVER_SAID: &str = "a session never switched on, which scribe must never read";

/// Marks claim `n` approved by Daniel without writing its entry: the
/// journal an approval leaves durable before the entry is written.
pub(super) fn approve_claim_in_db(root: &std::path::Path, n: i64) {
    support::sql(
        root,
        &format!(
            "UPDATE claims SET state = 'approved', settled_at = '2026-11-15T23:00:00Z',
                 settled_author = '{{\"kind\":\"daniel\",\"name\":\"daniel\"}}',
                 settled_by = c_cite WHERE n = {n};"
        ),
    );
}

/// Stages claims in the shape `proposals.json` held them (`id: "p-N"`,
/// `candidate: {..}`) as rows of `claims`. A store without a `scribe.db`
/// gets an empty one first: no build reads a `proposals.json` any more.
pub(super) fn seed_claims(root: &std::path::Path, proposals: &[Value]) {
    if !root.join("scribe.db").exists() {
        support::seed_records(root, &[]);
    }
    let conn = rusqlite::Connection::open(root.join("scribe.db")).expect("open scribe.db");
    let json_text = |v: &Value| -> Option<String> { (!v.is_null()).then(|| v.to_string()) };
    let plain = |v: &Value| -> Option<String> { v.as_str().map(str::to_owned) };
    for p in proposals {
        let n: i64 = p["id"]
            .as_str()
            .unwrap()
            .trim_start_matches("p-")
            .parse()
            .unwrap();
        let c = &p["candidate"];
        let has = i64::from(c.is_object());
        conn.execute(
            "INSERT INTO claims (n, kind, older, newer, reason, quotes, state, opened_at,
                 settled_at, settled_by, author, updated_by, settled_author, closed_reason,
                 instruction, has_candidate, c_tag, c_role, c_quote, c_context, c_statement,
                 c_subject, c_evidence, c_cite, c_project, c_provider, c_host, c_admitted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            rusqlite::params![
                n,
                plain(&p["kind"]),
                plain(&p["older"]),
                plain(&p["newer"]),
                plain(&p["reason"]),
                p.get("quotes")
                    .map_or_else(|| "[]".to_owned(), Value::to_string),
                plain(&p["state"]),
                plain(&p["opened_at"]),
                plain(&p["settled_at"]),
                json_text(&p["settled_by"]),
                p["author"].to_string(),
                json_text(&p["updated_by"]),
                json_text(&p["settled_author"]),
                plain(&p["closed_reason"]),
                json_text(&p["instruction"]),
                has,
                plain(&c["tag"]),
                plain(&c["role"]),
                plain(&c["quote"]),
                plain(&c["context"]),
                plain(&c["statement"]),
                plain(&c["subject"]),
                plain(&c["evidence"]),
                json_text(&c["cite"]),
                plain(&c["project"]),
                plain(&c["provider"]),
                plain(&c["host"]),
                json_text(&c["admitted"]),
            ],
        )
        .expect("insert claim");
    }
}

pub(super) fn setup_stored_assistant_finding_candidate(quote: &str) -> Setup {
    setup_stored_assistant_finding_candidate_with(quote, "exp-5342a4d6")
}

/// The pass has run; the stored claim is then a row of `claims` (a
/// `proposals.json` seeded now would be ignored, the documents having moved
/// into `scribe.db`).
pub(super) fn setup_stored_assistant_finding_candidate_with(quote: &str, evidence: &str) -> Setup {
    let s = Setup::new();
    s.write(
        ON,
        &[
            line("user", "u-1", "Please measure the reviewer mix effect."),
            line("assistant", "a-find", quote),
        ],
    );
    s.answer(&answer());
    s.scribe("/scribe on");
    s.pass_session(ON);
    let cite = json!({"session": ON, "message": "a-find", "at": "2026-11-03T06:00:00Z",
        "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/nonexistent/work/scribe"});
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    conn.execute(
        "INSERT INTO claims (n, kind, older, newer, reason, quotes, state, opened_at, author,
             has_candidate, c_tag, c_role, c_quote, c_evidence, c_cite, c_project, c_provider, c_host)
         VALUES (1, 'candidate', 'finding-1', NULL, 'Assistant finding statement', '[]', 'open',
             '2026-11-03T06:01:00Z', ?1, 1, 'finding', 'assistant', ?2, ?3, ?4, 'scribe',
             'claude-code', 'host')",
        rusqlite::params![
            json!({"kind": "scribe", "name": "session-pass"}).to_string(),
            quote,
            evidence,
            cite.to_string()
        ],
    )
    .unwrap();
    s
}

/// `command` with `--json` before the `--` that `terms` follows. The root is
/// one quoted word and may hold a literal ` -- ` when the temporary directory
/// does, so the terminator is found from the end, where the terms are, and
/// not as the first occurrence in the line.
#[cfg(unix)]
pub(super) fn json_before_terminator(command: &str, terms: &str) -> String {
    let head = command
        .strip_suffix(terms)
        .and_then(|rest| rest.strip_suffix(" -- "))
        .unwrap_or_else(|| panic!("the hint ends with `-- {terms}`: {command}"));
    format!("{head} --json -- {terms}")
}

/// Replay `hint` (after `prefix`) in `sh` with `--json` before its terminator,
/// and check that it lists the one message `u-1`.
#[cfg(unix)]
pub(super) fn assert_message_hint_replays(fx: &Fixture, hint: &str, prefix: &str, terms: &str) {
    let command = json_before_terminator(
        hint.strip_prefix(prefix)
            .unwrap()
            .strip_suffix(" lists them")
            .unwrap(),
        terms,
    );
    let out = replay_hint(fx, &command, None, false);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");
}

/// A line Claude Code wrote again on resume: the same uuid, timestamp and
/// text as the first copy, from another working directory.
pub(super) fn replayed(kind: &str, uuid: &str, text: &str) -> Value {
    let mut line = line(kind, uuid, text);
    line["cwd"] = json!("/nonexistent/work/elsewhere");
    line
}

/// Use the prepared first message as the fixed answer's segment start.
pub(super) fn answer_at_window_first(setup: &Setup, body: &Value) {
    setup
        .responses
        .replace(Responses::Fixed(body.clone(), true));
}

/// The audit log's lines, oldest first.
pub(super) fn audit_log(setup: &Setup) -> Vec<Value> {
    support::audit_lines(&setup.fx.root())
}

pub(super) fn plugin_pass(setup: &Setup, name: &str, input: Value) -> Value {
    let config = json!({
        "root": setup.fx.root(), "transcripts": setup.fx.home().join(".claude/projects"),
    });
    pass::plugin_step(&setup.fx, &[], name, input, &config).unwrap()
}

/// A pass step's refusal: its error code and message.
pub(super) fn plugin_pass_refusal(setup: &Setup, name: &str, input: Value) -> (String, String) {
    let config = json!({
        "root": setup.fx.root(), "transcripts": setup.fx.home().join(".claude/projects"),
    });
    let reply = pass::plugin_step(&setup.fx, &[], name, input, &config).unwrap_err();
    let reply: Value = serde_json::from_slice(&reply.stdout).unwrap();
    (
        reply["error"]["code"].as_str().unwrap().to_owned(),
        reply["error"]["message"].as_str().unwrap().to_owned(),
    )
}

/// A second session, spawned from `ON`.
pub(super) const CHILD: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";

/// A well-formed id scribe has never seen.
pub(super) const UNKNOWN: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e62";

pub(super) fn rebuild_args(confirm: bool) -> Vec<&'static str> {
    let mut args = vec!["session", "rebuild", ON, "--author", "test-agent", "--json"];
    if confirm {
        args.push("--confirm");
    }
    args
}

pub(super) fn candidate_answer(quote: &str, message: &str, tag: &str) -> Value {
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": message, "tag": tag}]);
    body
}

pub(super) fn proposals(s: &Setup) -> Value {
    s.fx.check(Cli(&["proposal", "list", "--json"]), Json)
}

pub(super) fn type_settlement(s: &Setup, prompt: &str) -> Output {
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!(prompt);
    s.hook("user-prompt-submit", &payload)
}

/// Daniel types `prompt`, holding one `#scribe` marker, as turn `uuid` of
/// the `ON` session; the Stop after the reply hands it to the agent
/// (2_design.md §14), and the agent records it as the command it was
/// handed says: an entry under `tag` stating `said`, with `quote` as his
/// words. Returns the new entry's id.
pub(super) fn mark(
    s: &Setup,
    uuid: &str,
    prompt: &str,
    tag: &str,
    said: &str,
    quote: &str,
) -> String {
    let submitted = type_settlement(s, prompt);
    let echo: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    assert_eq!(
        echo["systemMessage"], "scribe: marked — cataloged after the reply",
        "{echo}"
    );
    s.append(
        ON,
        &[
            line("user", uuid, prompt),
            line("assistant", &format!("a-{uuid}"), "Noted."),
        ],
    );
    let stopped = s.hook("stop", &s.payload("Stop"));
    let reply: Value = serde_json::from_slice(&stopped.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(
        context.contains(&format!("scribe entry add --tag {tag} ")),
        "{reply}"
    );
    let related = format!("session:{ON}");
    let added: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            tag,
            "--author",
            "opus",
            "--related",
            &related,
            "--quote",
            quote,
            "--spoken",
            said,
            "--json",
        ]),
        Json,
    );
    added["id"].as_str().unwrap().to_owned()
}

/// An `idea` claim as the model returns it for a written tag.
pub(super) fn idea_claim(quote: &str, message: &str, statement: &str) -> Value {
    json!({"quote": quote, "message": message, "tag": "idea",
           "statement": statement, "subject": "the routing table", "outcome": "open"})
}

pub(super) fn entries_json(s: &Setup, args: &[&str]) -> Value {
    let mut all = vec!["entry", "list", "--json"];
    all.extend_from_slice(args);
    s.fx.check(Cli(&all), Json)
}

const WEDDING: &str = "My sister's wedding is on 2026-12-12 and I'm still on the seating.";

pub(super) const WEDDING_QUOTE: &str = "My sister's wedding is on 2026-12-12";

const VENUE: &str = "I still have to book the venue for the party sometime.";

const VENUE_QUOTE: &str = "I still have to book the venue";

pub(super) const WEDDING_STEM: &str = "2026-12-12-sister-s-wedding";

pub(super) fn memory_element(kind: &str, title: &str, quote: &str, message: &str) -> Value {
    json!({"kind": kind, "title": title, "text": format!("{title}, as the user said."),
           "quote": quote, "message": message, "date": null, "now_kind": null,
           "update": null, "completes": null})
}

/// An `ON` session with two memory-bearing turns and a model answer naming
/// both: a dated `now` item and an undated candidate.
pub(super) fn memory_setup() -> Setup {
    let s = Setup::new();
    s.append(
        ON,
        &[line("user", "u-w", WEDDING), line("user", "u-v", VENUE)],
    );
    let mut body = answer();
    let mut wedding = memory_element("now", "Sister's wedding", WEDDING_QUOTE, "u-w");
    wedding["date"] = json!("2026-12-12");
    wedding["now_kind"] = json!("event");
    body["memory"] = json!([
        wedding,
        memory_element("now", "Book the party venue", VENUE_QUOTE, "u-v"),
    ]);
    body["signal"] = json!(2);
    s.answer(&body);
    s
}

pub(super) fn memory_json(s: &Setup, args: &[&str]) -> Value {
    let mut all = vec!["memory"];
    all.extend_from_slice(args);
    all.push("--json");
    serde_json::from_slice(&s.run(&all, b"").stdout).unwrap()
}

/// Every file under the store's `memory/`, as text.
pub(super) fn memory_text(s: &Setup) -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = String::new();
    walk(&s.fx.root().join("memory"), &mut out);
    out
}

/// The store as bytes and modes, without SQLite's shared-memory index
/// (its read marks change on any read).
pub(super) fn store_snapshot(s: &Setup) -> Vec<(PathBuf, Vec<u8>, u32)> {
    support::snapshot(&s.fx.root())
        .into_iter()
        .filter(|(path, _, _)| !path.ends_with("scribe.db-shm"))
        .collect()
}

pub(super) fn pass_on(s: &Setup) -> Value {
    serde_json::from_slice(&s.pass_session(ON).stdout).unwrap()
}

/// A profile answer: the template with `projects` under Projects.
pub(super) fn profile_answer(projects: &str) -> Value {
    let profile = format!(
        "## Role and work\n\n## Current focus\n\n## Projects\n- {projects}\n\n## Goals\n\n\
         ## Environment\n\n## How they work\n\n## Pinned\n\n"
    );
    json!({"profile": profile, "milestones": []})
}

/// The ```toml block of specs/catchers.md that declares `[tags.<name>]`.
pub(super) fn catcher_example(name: &str) -> String {
    let spec = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/design/scribe/specs/catchers.md"
    );
    let spec = std::fs::read_to_string(spec).unwrap();
    let header = format!("[tags.{name}]\n");
    spec.split("```toml\n")
        .skip(1)
        .filter_map(|block| block.split_once("```").map(|(toml, _)| toml))
        .find(|toml| toml.contains(&header))
        .unwrap_or_else(|| panic!("no example declares [tags.{name}]"))
        .to_owned()
}

pub(super) fn written_claim(quote: &str, message: &str, tag: &str, statement: &str) -> Value {
    json!({"quote": quote, "message": message, "tag": tag,
           "statement": statement, "subject": tag, "outcome": "open"})
}

pub(super) fn hint_path() -> String {
    let dir = std::path::Path::new(support::BIN).parent().unwrap();
    format!("{}:/usr/bin:/bin", dir.display())
}

pub(super) fn replay_hint(
    fx: &Fixture,
    command: &str,
    cwd: Option<&std::path::Path>,
    use_root: bool,
) -> Output {
    let mut shell = Command::new("sh");
    shell
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("HOME", fx.home())
        .env("PATH", hint_path());
    if use_root {
        shell.env("SCRIBE_ROOT", fx.root());
    }
    if let Some(cwd) = cwd {
        shell.current_dir(cwd);
    }
    fx.check(&mut shell, Capture)
}

pub(super) fn search_relative_root(
    fx: &Fixture,
    parent: &std::path::Path,
    name: &str,
    term: &str,
) -> Output {
    fx.check(
        fx.cmd(&[&format!("--root={name}"), "search", "--", term])
            .current_dir(parent)
            .env("PATH", hint_path()),
        Capture,
    )
}

#[cfg(unix)]
pub(super) fn copy_store_preserving_modes(fx: &Fixture, copy: &std::path::Path) {
    let status = Command::new("cp")
        .arg("-Rp")
        .arg(fx.root())
        .arg(copy)
        .status()
        .unwrap();
    assert!(status.success());
}
