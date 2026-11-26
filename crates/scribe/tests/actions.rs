//! End-to-end tests of action tags (docs/design/scribe/2_design.md §4.6):
//! `#scribe <tag>:` markers handed to the agent, a stored transcript window, a
//! config-marked action tag, agent entries with their cancel window and
//! quote check, `#scribe retire` on an agent's entry, pickups, the
//! intake's read surface (`entry list --tag --after --window`), `tag list`'s
//! action columns and the session-start stuck line.
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/actions.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use crate::support::command::Text;
use serde_json::{Value, json};
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, make_private};

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

    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "task",
            "--author",
            "opus",
            "--related",
            &format!("session:{SESSION}"),
            "add a --dry-run flag to the sync",
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "task-2");
    assert_eq!(added["author"], json!({"kind": "agent", "name": "opus"}));
    assert_eq!(
        added["window"],
        Value::Null,
        "an agent entry keeps no window"
    );

    let listed = json_of(&fx.check(
        &[
            "list",
            "--tag",
            "task",
            "--after",
            "2026-01-01T00:00:00Z",
            "--window",
            "--json",
        ],
        Success,
    ));
    assert_eq!(listed["total"], 2);
    let first = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "task-1")
        .unwrap();
    assert_eq!(first["window"]["turns"][0]["text"], "the sync is slow");

    backdate(&fx, "task", "task-1");
    fx.check(
        &[
            "pickup", "task-1", "--author", "intake", "--result", "task:T-9",
        ],
        Success,
    );
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
    let tags = json_of(&fx.check(Cli(&["tag", "list", "--json"]), Success));
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
    fx.check(
        &[
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
        ],
        Success,
    );
    // The config that declared `task` goes away; the entry stays.
    std::fs::remove_file(&config).unwrap();

    let list = json_of(&fx.check(&["list", "--tag", "task", "--json"], Success));
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
    fx.check(
        &[
            "add",
            "--tag",
            "idea",
            "--author",
            "opus",
            "profile the sync",
        ],
        Success,
    );
    let fresh = entry(&fx, "idea-1", &[]);
    assert_eq!(fresh["action"]["past_cancel_window"], false);
    assert!(fresh["action"]["cancel_window_ends"].is_string());
    assert_eq!(
        refused(&fx, &["entry", "pickup", "idea-1", "--author", "intake"]),
        "cancel_window"
    );

    let echo = claude.prompt(&fx, "#scribe retire idea-1");
    assert!(echo.contains("retired idea-1"), "{echo}");
    let retired = json_of(&fx.check(&["list", "--tag", "idea", "--json"], Success));
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
    fx.check(
        &[
            "add",
            "--tag",
            "idea",
            "--author",
            "opus",
            "profile the sync",
        ],
        Success,
    );
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
    fx.check(Cli(&args), Success);
    let before = support::exported(&fx.root(), "pickups.json").unwrap();
    fx.check(Cli(&args), Success);
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

    fx.check(
        &["pickup", "idea-1", "--author", "intake", "--rejected"],
        Success,
    );
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

    let list = json_of(&fx.check(&["list", "--tag", "standard", "--json"], Success));
    assert_eq!(list["total"], 1);
    let first = &list["entries"][0];
    assert_eq!(first["window"], Value::Null);
    let created = first["created"].as_str().unwrap().to_owned();

    let with = json_of(&fx.check(
        &[
            "list",
            "--tag",
            "standard",
            "--after",
            "2026-01-01T00:00:00Z",
            "--window",
            "--json",
        ],
        Success,
    ));
    assert_eq!(
        with["entries"][0]["window"]["turns"][0]["text"],
        "every size needs its unit"
    );
    let after = json_of(&fx.check(
        &["list", "--tag", "standard", "--after", &created, "--json"],
        Success,
    ));
    assert_eq!(after["total"], 0, "--after is strictly after the cursor");
    let bad = fx.check(&["list", "--after", "yesterday"], Capture);
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
    let added = json_of(&fx.check(Cli(&args), Success));
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
        fx.check(Cli(&args), Success);
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
    fx.check(&["pickup", "idea-1", "--author", "intake"], Success);
    fx.check(&["pickup", "standard-1", "--author", "intake"], Success);
    assert_eq!(claude.session_start(&fx), "");
}

include!("actions/spoken_instructions.rs");
