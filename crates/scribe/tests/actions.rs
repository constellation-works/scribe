//! End-to-end tests of action tags (docs/design/scribe/2_design.md §4.6):
//! `#scribe <tag>:` markers handed to the agent, a stored transcript window, a
//! config-marked action tag, agent entries with their cancel window and
//! quote check, `#scribe retire` on an agent's entry, pickups, the
//! intake's read surface (`entry list --tag --after --window`), `tag list`'s
//! action columns and the session-start stuck line.
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use support::{Fixture, make_private, text};

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";

/// A Claude Code layout under the fixture's home.
struct Claude {
    transcript: PathBuf,
    cwd: PathBuf,
}

impl Claude {
    fn new(fx: &Fixture) -> Self {
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        Self {
            transcript: project.join(format!("{SESSION}.jsonl")),
            cwd,
        }
    }

    /// Write the transcript: `(uuid, role, text)` turns, a minute apart,
    /// the last a minute ago.
    fn write(&self, turns: &[(&str, &str, &str)]) {
        let mut out = format!(
            "{}\n",
            json!({"type": "session-owner", "sessionId": SESSION})
        );
        let now = time::OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap();
        for (n, (uuid, role, body)) in turns.iter().enumerate() {
            let content = if *role == "user" {
                json!(body)
            } else {
                json!([{"type": "text", "text": body}])
            };
            let ago = i64::try_from(turns.len() - n).unwrap();
            let at = (now - time::Duration::minutes(ago))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            let line = json!({"type": role, "uuid": uuid, "sessionId": SESSION,
                "timestamp": at,
                "message": {"role": role, "content": content}});
            out.push_str(&format!("{line}\n"));
        }
        std::fs::write(&self.transcript, out).unwrap();
        make_private(&self.transcript);
    }

    /// Run the `UserPromptSubmit` hook for `prompt`; the reply's
    /// `systemMessage`.
    fn prompt(&self, fx: &Fixture, prompt: &str) -> String {
        let payload = json!({"session_id": SESSION,
            "transcript_path": self.transcript, "cwd": self.cwd,
            "permission_mode": "default", "hook_event_name": "UserPromptSubmit",
            "prompt": prompt});
        let out = stdin_run(fx, &["hook", "user-prompt-submit"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        reply["systemMessage"].as_str().unwrap().to_owned()
    }

    /// Run the `Stop` hook; the reply's `systemMessage` and
    /// `additionalContext`, empty when it printed nothing.
    fn stop(&self, fx: &Fixture) -> (String, String) {
        let payload = json!({"session_id": SESSION,
            "transcript_path": self.transcript, "cwd": self.cwd,
            "permission_mode": "default", "hook_event_name": "Stop",
            "stop_hook_active": false});
        let out = stdin_run(fx, &["hook", "stop"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        if out.stdout.is_empty() {
            return (String::new(), String::new());
        }
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        let field = |v: &Value| v.as_str().unwrap_or_default().to_owned();
        (
            field(&reply["systemMessage"]),
            field(&reply["hookSpecificOutput"]["additionalContext"]),
        )
    }

    /// The session-start hook's stdout.
    fn session_start(&self, fx: &Fixture) -> String {
        let payload = json!({"session_id": SESSION, "transcript_path": self.transcript,
            "cwd": self.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let out = stdin_run(fx, &["hook", "session-start"], &payload);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        text(&out.stdout)
    }
}

fn stdin_run(fx: &Fixture, args: &[&str], payload: &Value) -> Output {
    piped(fx.cmd(args), payload)
}

fn piped(mut cmd: Command, payload: &Value) -> Output {
    let child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = support::ChildGuard::new(child);
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    guard.finish()
}

fn json_of(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Run `args` with `--json`, require failure, and return the error code.
fn refused(fx: &Fixture, args: &[&str]) -> String {
    let mut args = args.to_vec();
    args.push("--json");
    let out = fx.run(&args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{args:?}: {}",
        text(&out.stdout)
    );
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    err["code"].as_str().unwrap().to_owned()
}

fn entry(fx: &Fixture, id: &str, extra: &[&str]) -> Value {
    let mut args = vec!["entry", "show", id, "--json"];
    args.extend_from_slice(extra);
    json_of(&fx.ok(&args))
}

/// Move entry `id`'s creation back to a fixed past instant, as if it had
/// been written long ago.
fn backdate(fx: &Fixture, _tag: &str, id: &str) {
    support::sql(
        &fx.root(),
        &format!("UPDATE entries SET created = '2026-01-01T00:00:00Z' WHERE id = '{id}'"),
    );
}

/// Seed entry `id` under `tag` as the user's, saying `content`, with
/// `fields` (frontmatter key, YAML value) replacing their `null`s.
fn seed_entry(fx: &Fixture, id: &str, tag: &str, content: &str, fields: &[(&str, &str)]) {
    let mut record = support::entry_text(1, content)
        .replace("id: preferences-1\n", &format!("id: {id}\n"))
        .replace("tag: preferences\n", &format!("tag: {tag}\n"))
        .replace("#scribe preferences:", &format!("#scribe {tag}:"));
    for (key, value) in fields {
        record = record.replace(&format!("\n{key}: null\n"), &format!("\n{key}: {value}\n"));
    }
    support::seed_records(&fx.root(), &[&record]);
}

/// Whether the store holds any entry of `tag`, in any status.
fn entries_of_tag(fx: &Fixture, tag: &str) -> bool {
    let conn = rusqlite::Connection::open_with_flags(
        fx.root().join("scribe.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    );
    let Ok(conn) = conn else { return false };
    conn.query_row("SELECT count(*) FROM entries WHERE tag = ?1", [tag], |r| {
        r.get::<_, i64>(0)
    })
    .unwrap_or(0)
        > 0
}

/// `marks.json`'s records.
fn marks_json(fx: &Fixture) -> Vec<Value> {
    let text = support::exported(&fx.root(), "marks.json").expect("exported marks");
    serde_json::from_str::<Value>(&text).unwrap()["marks"]
        .as_array()
        .unwrap()
        .clone()
}

fn tag_row(fx: &Fixture, name: &str) -> Value {
    let tags = json_of(&fx.ok(&["tag", "list", "--json"]));
    tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
        .unwrap()
}

#[test]
fn a_config_marked_action_tag_is_handed_over_read_and_picked_up() {
    let fx = Fixture::new();
    std::fs::create_dir(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(
        &config,
        "[tags.preferences]\n[tags.task]\naction = true\n[tags.idea]\ndescription = \"ideas to try\"\naction = true\nask_first = true\n",
    )
    .unwrap();
    make_private(&config);
    let claude = Claude::new(&fx);
    let prompt = "#scribe task: profile the sync\n#scribe friction: the sync has no progress bar\n#scribe idea: cache the manifest";
    assert_eq!(
        claude.prompt(&fx, prompt),
        "scribe: marked 3 times — cataloged after the reply",
        "an unknown tag word is no longer refused at submit"
    );
    claude.write(&[
        ("u-1", "user", "the sync is slow"),
        ("u-2", "user", prompt),
        ("a-2", "assistant", "On it."),
    ]);
    let (echo, context) = claude.stop(&fx);
    let lines: Vec<&str> = echo.lines().collect();
    assert_eq!(lines.len(), 3, "{echo}");
    assert!(
        lines
            .iter()
            .all(|l| l.starts_with("scribe: #scribe in your message is the agent's to record — ")),
        "{echo}"
    );
    let context: Vec<&str> = context.lines().collect();
    assert_eq!(context.len(), 3, "{context:?}");
    assert!(
        context[0].contains("scribe entry add --tag task --project scribe"),
        "{context:?}"
    );
    assert!(
        context[1].contains("scribe entry add --tag <tag> --project scribe"),
        "friction is not declared, so it is no hint: {context:?}"
    );
    assert!(
        context[2].contains("scribe entry add --tag idea --project scribe"),
        "{context:?}"
    );
    assert!(!entries_of_tag(&fx, "friction"));
    seed_entry(
        &fx,
        "task-1",
        "task",
        "profile the sync",
        &[(
            "window",
            "{turns: [{role: user, message: u-1, at: 2026-01-02T03:04:05Z, text: \"the sync is slow\", cut: false}], truncated: false, omitted: 0}",
        )],
    );
    let e = entry(&fx, "task-1", &[]);
    assert_eq!(e["window_turns"], 1);
    assert!(e["action"].is_object());

    let task = tag_row(&fx, "task");
    assert_eq!(
        task["action"],
        json!({"mode": "free", "require_quote": false, "retired": 0, "rejected": 0})
    );
    let idea = tag_row(&fx, "idea");
    assert_eq!(
        idea["action"],
        json!({"mode": "ask-first", "require_quote": false, "retired": 0, "rejected": 0})
    );
    assert_eq!(tag_row(&fx, "standard")["action"]["require_quote"], true);
    assert_eq!(tag_row(&fx, "preferences")["action"], Value::Null);

    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "task",
        "--author",
        "opus",
        "--related",
        &format!("session:{SESSION}"),
        "add a --dry-run flag to the sync",
        "--json",
    ]));
    assert_eq!(added["id"], "task-2");
    assert_eq!(added["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        added["window"],
        Value::Null,
        "an agent entry keeps no window"
    );

    let listed = json_of(&fx.ok(&[
        "entry",
        "list",
        "--tag",
        "task",
        "--after",
        "2026-01-01T00:00:00Z",
        "--window",
        "--json",
    ]));
    assert_eq!(listed["total"], 2);
    let first = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "task-1")
        .unwrap();
    assert_eq!(first["window"]["turns"][0]["text"], "the sync is slow");

    backdate(&fx, "task", "task-1");
    fx.ok(&[
        "entry", "pickup", "task-1", "--author", "intake", "--result", "task:T-9",
    ]);
    assert_eq!(entry(&fx, "task-1", &[])["action"]["picked_up"], true);
}

#[test]
fn task_friction_remind_and_finding_are_unknown_tags_without_config_and_not_in_the_tag_list() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let tags = ["task", "friction", "remind", "finding"];
    let prompts: Vec<(String, String)> = tags
        .iter()
        .map(|tag| {
            (
                format!("u-{tag}"),
                format!("#scribe {tag}: profile the sync"),
            )
        })
        .collect();
    let replies: Vec<String> = tags.iter().map(|tag| format!("a-{tag}")).collect();
    let mut turns = vec![("u-1", "user", "the sync is slow")];
    for (n, tag) in tags.iter().enumerate() {
        let (uuid, prompt) = (&prompts[n].0, &prompts[n].1);
        assert_eq!(
            claude.prompt(&fx, prompt),
            "scribe: marked — cataloged after the reply",
            "no refusal at submit"
        );
        turns.extend([
            (uuid.as_str(), "user", prompt.as_str()),
            (replies[n].as_str(), "assistant", "Noted."),
        ]);
        claude.write(&turns);
        let (echo, context) = claude.stop(&fx);
        assert_eq!(
            echo,
            format!("scribe: #scribe in your message is the agent's to record — \"{prompt}\""),
            "{tag}"
        );
        assert!(
            context.contains("scribe entry add --tag <tag> "),
            "{tag} is no hint: {context}"
        );
        let mark = marks_json(&fx)
            .into_iter()
            .find(|m| m["message"] == uuid.as_str())
            .unwrap();
        assert_eq!(mark["outcome"], "refused", "{tag}");
        assert_eq!(mark["code"], "agent_handoff", "{tag}");
        assert!(!entries_of_tag(&fx, tag));
    }
    let tags = json_of(&fx.ok(&["tag", "list", "--json"]));
    let action: Vec<&str> = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["action"].is_object())
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(action, ["standard"]);
}

#[test]
fn entries_under_a_tag_config_no_longer_defines_are_still_listed_shown_and_retired() {
    let fx = Fixture::new();
    std::fs::create_dir(fx.root()).unwrap();
    make_private(&fx.root());
    let config = fx.root().join("config.toml");
    std::fs::write(&config, "[tags.task]\naction = true\n").unwrap();
    make_private(&config);
    let claude = Claude::new(&fx);
    claude.write(&[
        ("u-1", "user", "the sync is slow"),
        ("u-2", "user", "#scribe task: profile the sync"),
        ("a-2", "assistant", "On it."),
    ]);
    let statement = "Profile the sync to find where it is slow.";
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "task",
        "--author",
        "opus",
        "--related",
        &format!("session:{SESSION}"),
        "--quote",
        "profile the sync",
        statement,
    ]);
    // The config that declared `task` goes away; the entry stays.
    std::fs::remove_file(&config).unwrap();

    let list = json_of(&fx.ok(&["entry", "list", "--tag", "task", "--json"]));
    assert_eq!(list["total"], 1, "{list}");
    assert_eq!(list["entries"][0]["id"], "task-1");
    let shown = entry(&fx, "task-1", &[]);
    assert_eq!(shown["content"], statement);
    assert_eq!(shown["quote"], "profile the sync");
    assert_eq!(shown["status"], "active");

    let echo = claude.prompt(&fx, "#scribe retire task-1");
    assert!(echo.contains("retired task-1"), "{echo}");
    assert_eq!(entry(&fx, "task-1", &[])["status"], "retired");
}

#[test]
fn an_agent_entry_waits_out_its_cancel_window_and_retiring_it_cancels_it() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    claude.write(&[]);
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "idea",
        "--author",
        "opus",
        "profile the sync",
    ]);
    let fresh = entry(&fx, "idea-1", &[]);
    assert_eq!(fresh["action"]["past_cancel_window"], false);
    assert!(fresh["action"]["cancel_window_ends"].is_string());
    assert_eq!(
        refused(&fx, &["entry", "pickup", "idea-1", "--author", "intake"]),
        "cancel_window"
    );

    let echo = claude.prompt(&fx, "#scribe retire idea-1");
    assert!(echo.contains("retired idea-1"), "{echo}");
    let retired = json_of(&fx.ok(&["entry", "list", "--tag", "idea", "--json"]));
    assert_eq!(retired["entries"][0]["status"], "retired");
    assert_eq!(tag_row(&fx, "idea")["action"]["retired"], 1);

    backdate(&fx, "idea", "idea-1");
    assert_eq!(
        entry(&fx, "idea-1", &[])["action"]["past_cancel_window"],
        true
    );
    assert_eq!(
        refused(&fx, &["entry", "pickup", "idea-1", "--author", "intake"]),
        "entry_not_active",
        "a cancelled entry is not picked up"
    );
}

#[test]
fn pickups_are_recorded_once_refused_off_action_tags_and_count_rejections() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    claude.write(&[
        ("u-1", "user", "#scribe preferences: ship on fridays"),
        ("a-1", "assistant", "Noted."),
    ]);
    seed_entry(&fx, "preferences-1", "preferences", "ship on fridays", &[]);
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "idea",
        "--author",
        "opus",
        "profile the sync",
    ]);
    backdate(&fx, "idea", "idea-1");
    assert_eq!(
        refused(
            &fx,
            &["entry", "pickup", "preferences-1", "--author", "intake"]
        ),
        "not_action_tag"
    );

    let args = [
        "entry",
        "pickup",
        "idea-1",
        "--author",
        "intake",
        "--result",
        "task:<id>",
    ];
    let args: Vec<String> = args.iter().map(|a| a.replace("<id>", "T-9")).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    fx.ok(&args);
    let before = support::exported(&fx.root(), "pickups.json").unwrap();
    fx.ok(&args);
    assert_eq!(
        support::exported(&fx.root(), "pickups.json").unwrap(),
        before,
        "the same pickup again changes nothing"
    );
    let pickups: Vec<Value> = support::audit_lines(&fx.root())
        .into_iter()
        .filter(|l| l["command"] == "entry pickup")
        .map(|l| json!([l["status"], l["target"], l["error_code"], l["author"]]))
        .collect();
    assert_eq!(
        pickups,
        [
            json!(["failure", "preferences-1", "not_action_tag", "intake"]),
            json!(["success", "idea-1", null, "intake"]),
        ],
        "the refusal and the write are audited; the no-op writes no line"
    );
    let state = &entry(&fx, "idea-1", &[])["action"];
    assert_eq!(state["picked_up"], true);
    assert_eq!(state["pickups"][0]["author"]["name"], "intake");
    assert_eq!(state["pickups"][0]["result"], "task:T-9");

    fx.ok(&[
        "entry",
        "pickup",
        "idea-1",
        "--author",
        "intake",
        "--rejected",
    ]);
    assert_eq!(entry(&fx, "idea-1", &[])["action"]["rejected"], true);
    assert_eq!(tag_row(&fx, "idea")["action"]["rejected"], 1);
}

#[test]
fn the_intake_reads_by_tag_and_cursor_with_the_window_only_on_request() {
    let fx = Fixture::new();
    seed_entry(
        &fx,
        "standard-1",
        "standard",
        "Every size names its unit.",
        &[(
            "window",
            "{turns: [{role: user, message: u-1, at: 2026-01-02T03:04:05Z, text: \"every size needs its unit\", cut: false}], truncated: false, omitted: 0}",
        )],
    );
    seed_entry(&fx, "preferences-1", "preferences", "bytes for sizes", &[]);
    assert!(entry(&fx, "preferences-1", &[])["action"].is_null());

    let list = json_of(&fx.ok(&["entry", "list", "--tag", "standard", "--json"]));
    assert_eq!(list["total"], 1);
    let first = &list["entries"][0];
    assert_eq!(first["window"], Value::Null);
    let created = first["created"].as_str().unwrap().to_owned();

    let with = json_of(&fx.ok(&[
        "entry",
        "list",
        "--tag",
        "standard",
        "--after",
        "2026-01-01T00:00:00Z",
        "--window",
        "--json",
    ]));
    assert_eq!(
        with["entries"][0]["window"]["turns"][0]["text"],
        "every size needs its unit"
    );
    let after = json_of(&fx.ok(&[
        "entry", "list", "--tag", "standard", "--after", &created, "--json",
    ]));
    assert_eq!(after["total"], 0, "--after is strictly after the cursor");
    let bad = fx.run(&["entry", "list", "--after", "yesterday"]);
    assert_eq!(bad.status.code(), Some(2), "not an RFC 3339 instant: usage");
}

#[test]
fn a_quote_is_required_and_checked_against_daniels_typed_turns() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    claude.write(&[
        ("u-1", "user", "No: every size needs its unit, always."),
        ("a-1", "assistant", "Sizes will carry units."),
    ]);
    let session = format!("session:{SESSION}");
    let base = [
        "entry",
        "add",
        "--tag",
        "standard",
        "--author",
        "opus",
        "name the unit in every size",
    ];
    let with = |extra: &[&str]| -> Vec<String> {
        base.iter().chain(extra).map(|s| (*s).to_owned()).collect()
    };
    let run = |extra: &[&str]| {
        let args = with(extra);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        refused(&fx, &args)
    };
    assert_eq!(run(&["--related", &session]), "quote_required");
    assert_eq!(
        run(&["--quote", "every size needs its unit"]),
        "quote_required",
        "no session to check it in"
    );
    assert_eq!(
        run(&["--related", &session, "--quote", "Sizes will carry units"]),
        "quote_not_found",
        "the assistant's words are not Daniel's"
    );
    assert_eq!(
        run(&[
            "--related",
            "session:00000000-0000-4000-8000-000000000000",
            "--quote",
            "every size"
        ]),
        "quote_not_found"
    );
    assert!(!entries_of_tag(&fx, "standard"), "nothing written");

    let args = with(&[
        "--related",
        &session,
        "--quote",
        "every size needs its unit,  always",
        "--json",
    ]);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let added = json_of(&fx.ok(&args));
    assert_eq!(added["quote"], "every size needs its unit,  always");
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--quote",
                "x",
                "y"
            ]
        ),
        "not_action_tag"
    );
}

#[test]
fn session_start_counts_action_entries_waiting_with_no_pickup() {
    let fx = Fixture::new();
    fx.config(support::IDEA_CATCHER);
    let claude = Claude::new(&fx);
    claude.write(&[]);
    assert_eq!(
        claude.session_start(&fx),
        "",
        "an empty store prints nothing"
    );
    let prompt = "#scribe idea: profile the sync\n#scribe standard: name the units\n#scribe preferences: ship on fridays";
    claude.write(&[("u-1", "user", prompt), ("a-1", "assistant", "Noted.")]);
    let session = format!("session:{SESSION}");
    for (tag, content, extra) in [
        ("idea", "Profile the sync.", &[][..]),
        (
            "standard",
            "Sizes name their units.",
            &["--related", &session, "--quote", "name the units"][..],
        ),
        (
            "preferences",
            "Daniel ships on Fridays.",
            &[
                "--related",
                &session,
                "--quote",
                "ship on fridays",
                "--spoken",
            ][..],
        ),
    ] {
        let args = [
            &["entry", "add", "--tag", tag, "--author", "opus"][..],
            extra,
            &[content],
        ]
        .concat();
        fx.ok(&args);
    }
    assert_eq!(claude.session_start(&fx), "", "nothing older than an hour");
    for (tag, id) in [
        ("idea", "idea-1"),
        ("standard", "standard-1"),
        ("preferences", "preferences-1"),
    ] {
        backdate(&fx, tag, id);
    }
    let reply: Value = serde_json::from_str(&claude.session_start(&fx)).unwrap();
    assert_eq!(
        reply["systemMessage"],
        "scribe: 2 action entries not picked up after 1 hour (idea 1, standard 1) — scribe entry list --tag <tag> --status active"
    );
    fx.ok(&["entry", "pickup", "idea-1", "--author", "intake"]);
    fx.ok(&["entry", "pickup", "standard-1", "--author", "intake"]);
    assert_eq!(claude.session_start(&fx), "");
}

#[test]
fn spoken_instruction_entry_requires_verbatim_quote_in_named_session_and_eligible_memory_tag() {
    let fx = Fixture::new();
    fx.config("[tags.rituals]\ndescription = \"Daily rituals\"\n[tags.finding]\nrequire_evidence = true\n[tags.idea]\naction = true\n");
    let claude = Claude::new(&fx);
    let spoken_words = "add that to my preferences: keep progress updates short during long runs";
    claude.write(&[
        ("u-1", "user", "Let us review the workflow."),
        (
            "a-1",
            "assistant",
            "I can record that in your preferences if you ask.",
        ),
        (
            "u-2",
            "user",
            &format!("<pasted_content>never paste rules as typed</pasted_content>\n{spoken_words}"),
        ),
    ]);
    let session = format!("session:{SESSION}");

    // Refused: missing --quote or --related session:<id>.
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_required"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--quote",
                spoken_words,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_required"
    );

    // Refused: non-verbatim quote, assistant words, pasted block, or wrong session.
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "keep updates brief on long runs",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "I can record that in your preferences if you ask.",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                "never paste rules as typed",
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                "session:00000000-0000-4000-8000-000000000000",
                "--quote",
                spoken_words,
                "--spoken",
                "The user prefers short progress updates.",
            ]
        ),
        "quote_not_found"
    );

    // Refused: ineligible tags (action tags, handoff, require_evidence).
    for bad_tag in ["standard", "idea", "handoff", "finding"] {
        assert_eq!(
            refused(
                &fx,
                &[
                    "entry",
                    "add",
                    "--tag",
                    bad_tag,
                    "--author",
                    "opus",
                    "--related",
                    &session,
                    "--quote",
                    spoken_words,
                    "--spoken",
                    "The user prefers short progress updates.",
                ]
            ),
            "not_memory_tag",
            "{bad_tag}"
        );
    }

    // Refused: quote containing a secret that redaction would alter.
    let secret_quote = "keep ghp_abcdefghij123456 out of logs";
    claude.write(&[("u-1", "user", secret_quote)]);
    assert_eq!(
        refused(
            &fx,
            &[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "--related",
                &session,
                "--quote",
                secret_quote,
                "--spoken",
                "The user wants tokens kept out of logs.",
            ]
        ),
        "needs_redaction"
    );
    assert!(
        !entries_of_tag(&fx, "preferences"),
        "nothing written on refusals"
    );

    // Accepted on preferences (and configured memory tag) in an off/unrecorded session.
    claude.write(&[("u-1", "user", spoken_words)]);
    let statement = "The user prefers short progress updates during long runs.";
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &session,
        "--quote",
        "keep progress updates short during long runs",
        "--spoken",
        statement,
        "--json",
    ]));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(added["stated_by"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        added["quote"],
        "keep progress updates short during long runs"
    );
    assert_eq!(added["content"], statement);
    assert_eq!(added["content_from"]["kind"], "agent");
    assert_eq!(added["cite"], Value::Null);
    assert_eq!(added["said"], "");

    // Does not create a session record or switch the off session on.
    let sessions = json_of(&fx.ok(&["session", "list", "--include-inactive", "--json"]));
    assert_eq!(sessions["total"], 0);

    // Works on a user-configured non-action memory tag too.
    let ritual = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "rituals",
        "--author",
        "opus",
        "--related",
        &session,
        "--quote",
        "add that to my preferences",
        "--spoken",
        "The user starts reviews with a clean status check.",
        "--json",
    ]));
    assert_eq!(ritual["id"], "rituals-1");
    assert_eq!(
        ritual["stated_by"],
        json!({"kind": "agent", "name": "opus"})
    );
}

#[test]
fn spoken_instruction_entry_labels_statement_as_agents_and_ask_cites_like_marked_entries() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quote = "always keep progress updates short during long runs";
    claude.write(&[(
        "u-1",
        "user",
        "Add that to my preferences: always keep progress updates short during long runs.",
    )]);
    let statement = "The user prefers concise progress updates during long runs.";
    fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &format!("session:{SESSION}"),
        "--quote",
        quote,
        "--spoken",
        statement,
    ]);

    // `entry show` shows the user's words as `quote` and labels the statement as the agent's.
    let table = text(
        &fx.ok(&["entry", "show", "preferences-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains(
            "stated_by   agent:opus (content is agent (opus)'s statement of the user's turn)"
        ),
        "{table}"
    );
    assert!(table.contains(&format!("quote       {quote}")), "{table}");
    assert!(
        table.contains(&format!("content     {statement}")),
        "{table}"
    );

    let plain = text(&fx.ok(&["entry", "show", "preferences-1"]).stdout);
    assert!(
        plain.contains(&format!("stated_by\tagent:opus\t{quote}")),
        "{plain}"
    );

    // `scribe search` renders the agent's statement label and the user's words.
    let found = json_of(&fx.ok(&["search", "progress updates concise", "--json"]));
    assert_eq!(found["total"], 1, "{found}");
    let snippet = found["hits"][0]["snippet"].as_str().unwrap();
    assert!(
        snippet.contains("agent (opus)'s statement of the user's turn:"),
        "{snippet}"
    );
    assert!(
        snippet.contains(&format!("The user's words: \"{quote}\"")),
        "{snippet}"
    );
}

#[test]
fn spoken_instruction_accepts_claude_desktop_human_turn() {
    let fx = Fixture::new();
    let project = fx.home().join(".claude/projects/-work-scribe");
    std::fs::create_dir_all(&project).unwrap();
    let transcript = project.join(format!("{SESSION}.jsonl"));
    let quote = "always keep progress updates short during long runs";
    let line = json!({
        "type": "user",
        "uuid": "u-desktop",
        "sessionId": SESSION,
        "timestamp": "2026-11-22T21:52:43Z",
        "cwd": "/work/scribe",
        "entrypoint": "claude-desktop",
        "origin": {"kind": "human"},
        "promptSource": "sdk",
        "turnOrigin": "human",
        "message": {"role": "user", "content": quote}
    });
    std::fs::write(&transcript, format!("{line}\n")).unwrap();
    support::make_private(&transcript);

    let statement = "The user prefers short progress updates during long runs.";
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        "--related",
        &format!("session:{SESSION}"),
        "--quote",
        quote,
        "--spoken",
        statement,
        "--json",
    ]));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["quote"], quote);
    assert_eq!(added["content"], statement);
}

#[test]
fn spoken_instruction_entry_remains_editable_for_content_and_metadata() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quote = "always keep progress updates short during long runs";
    claude.write(&[(
        "u-1",
        "user",
        "Add that to my preferences: always keep progress updates short during long runs.",
    )]);
    let added = json_of(&fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        "--project",
        "scribe",
        "--related",
        &format!("session:{SESSION}"),
        "--quote",
        quote,
        "--spoken",
        "The user prefers short progress updates.",
        "--json",
    ]));
    assert_eq!(added["id"], "preferences-1");
    assert_eq!(added["stated_by"], json!({"kind": "agent", "name": "opus"}));

    // Another agent can rewrite the statement; `stated_by` and `updated_by`
    // move to the editing agent while `author` and `quote` stay.
    let edited = json_of(&fx.ok(&[
        "entry",
        "update",
        "preferences-1",
        "--author",
        "sonnet",
        "--content",
        "The user prefers concise progress updates during long runs.",
        "--json",
    ]));
    assert_eq!(edited["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        edited["stated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(
        edited["updated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(edited["quote"], quote);

    // A later metadata-only update by a third agent keeps the record readable
    // and preserves `stated_by`.
    let tagged = json_of(&fx.ok(&[
        "entry",
        "update",
        "preferences-1",
        "--author",
        "haiku",
        "--add-topic",
        "communication",
        "--json",
    ]));
    assert_eq!(
        tagged["stated_by"],
        json!({"kind": "agent", "name": "sonnet"})
    );
    assert_eq!(
        tagged["updated_by"],
        json!({"kind": "agent", "name": "haiku"})
    );
    assert_eq!(tagged["topics"], json!(["communication"]));
    assert_eq!(entry(&fx, "preferences-1", &[]), tagged);
}

#[test]
fn spoken_statement_author_survives_other_agents_and_settling_commands() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let quotes = [
        "always keep progress updates short during long runs",
        "summarize test failures before proposing fixes",
    ];
    let instruction = "please forget preferences-1 completely";
    let now = time::OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .unwrap();
    let mut transcript = String::new();
    for (index, text) in [quotes[0], quotes[1], instruction].iter().enumerate() {
        let at = (now - time::Duration::minutes(i64::try_from(3 - index).unwrap()))
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        transcript.push_str(&format!(
            "{}\n",
            json!({
                "type": "user", "uuid": format!("u-{index}"), "sessionId": SESSION,
                "timestamp": at, "cwd": claude.cwd, "entrypoint": "cli",
                "message": {"role": "user", "content": text}
            })
        ));
    }
    std::fs::write(&claude.transcript, transcript).unwrap();
    make_private(&claude.transcript);
    let session = format!("session:{SESSION}");
    for (index, quote) in quotes.iter().enumerate() {
        fx.ok(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &session,
            "--quote",
            quote,
            "--spoken",
            if index == 0 {
                "Keep updates short."
            } else {
                "Summarize failures first."
            },
        ]);
        let id = format!("preferences-{}", index + 1);
        fx.ok(&[
            "entry",
            "update",
            &id,
            "--author",
            "sonnet",
            "--content",
            if index == 0 {
                "Keep progress updates concise."
            } else {
                "Summarize failures before fixes."
            },
        ]);
    }
    let check = |id: &str, status: &str| {
        let shown = entry(&fx, id, &[]);
        assert_eq!(shown["status"], status);
        assert_eq!(shown["stated_by"], json!({"kind":"agent", "name":"sonnet"}));
        let listed = json_of(&fx.ok(&[
            "entry",
            "list",
            "--tag",
            "preferences",
            "--status",
            status,
            "--json",
        ]));
        assert_eq!(listed["warnings"], json!([]));
        assert!(
            listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == id && e["stated_by"] == shown["stated_by"])
        );
    };
    for (agent, topic) in [("haiku", "communication"), ("opus", "progress")] {
        fx.ok(&[
            "entry",
            "update",
            "preferences-1",
            "--author",
            agent,
            "--add-topic",
            topic,
        ]);
        check("preferences-1", "active");
    }
    fx.ok(&[
        "entry",
        "retire",
        "preferences-1",
        "--author",
        "delta",
        "--reason",
        "changed",
    ]);
    check("preferences-1", "retired");
    fx.ok(&[
        "entry",
        "forget",
        "preferences-1",
        "--author",
        "echo",
        "--session",
        SESSION,
        "--instruction",
        instruction,
        "--confirm",
    ]);
    check("preferences-1", "forgotten");
    assert_eq!(entry(&fx, "preferences-1", &[])["quote"], Value::Null);
    assert!(
        claude
            .prompt(&fx, "#scribe retire preferences-2")
            .contains("retired preferences-2")
    );
    check("preferences-2", "retired");
    assert!(
        claude
            .prompt(&fx, "#scribe forget preferences-2")
            .contains("forgot preferences-2")
    );
    check("preferences-2", "forgotten");
}
