//! End-to-end tests of S6 through the built binary
//! (docs/design/scribe/2_design.md §3, §7, §10 S6): `scribe consistency scan`
//! with a fake `claude` opening one `supersedes` proposal for a pair the
//! model labels and one `stale` proposal per entry due for review, never
//! proposing a settled pair again and re-applying an interrupted approval;
//! `#scribe approve|reject|retire|forget` settling them from a typed
//! prompt; the `SessionStart` hook reporting the open count; and the two
//! routine tools of `scribe plugin call`.
//!
//! Every run is the built binary with a temp `HOME` and data root; nothing
//! touches the real home.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/consistency.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use crate::support::command::Text;
use serde_json::{Value, json};
use std::path::Path;
use subject::*;
use support::{Fixture, make_private, text};

#[test]
fn scan_report_keeps_the_retired_counters_as_neutral_values() {
    let report = Setup::new().scan();
    for key in RETIRED_COUNTERS {
        assert!(report.get(key).is_some(), "{key} missing: {report}");
    }
    for key in RETIRED_COUNTERS
        .into_iter()
        .filter(|k| k.ends_with("batches"))
    {
        assert_eq!(report[key], json!([]), "{key}: {report}");
    }
    for key in RETIRED_COUNTERS
        .into_iter()
        .filter(|k| !k.ends_with("batches"))
    {
        assert_eq!(report[key], 0, "{key}: {report}");
    }
}

#[test]
fn the_routine_schema_names_every_retired_counter_the_report_keeps() {
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../plugin/.orbit-plugin/schemas/routine.output.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let branch = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["title"] == "consistency scan")
        .expect("a consistency scan branch");
    let report = Setup::new().scan();
    for key in RETIRED_COUNTERS {
        assert!(
            branch["required"].as_array().unwrap().contains(&json!(key)),
            "{key} is not required by the routine schema"
        );
        assert!(
            branch["properties"].get(key).is_some(),
            "{key} has no property in the routine schema"
        );
        assert!(report.get(key).is_some(), "{key} missing: {report}");
    }
}

#[test]
fn a_scan_proposes_a_stale_entry_once() {
    let s = Setup::new();
    let report = s.scan();
    assert_eq!(ids(&report["opened"]), ["p-1"], "{report}");
    assert_eq!(report["stale_entries"], 1);
    assert_eq!(s.calls(), 0, "the scan calls no model");

    let p1 = s.proposal("p-1");
    assert_eq!(
        (&p1["kind"], &p1["older"], &p1["newer"]),
        (&json!("stale"), &json!("rituals-1"), &json!(null))
    );
    assert_eq!(p1["state"], "open");

    // The scan changes no entry.
    assert_eq!(s.entry_json("preferences-1")["status"], "active");
    assert_eq!(s.entry_json("rituals-1")["status"], "active");

    // One audit line per proposal opened.
    let audit = s.audit();
    let targets: Vec<&str> = audit
        .iter()
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    assert_eq!(targets, ["p-1"]);
    assert!(
        audit
            .iter()
            .all(|l| l["command"] == "consistency scan" && l["status"] == "success")
    );

    // Again: nothing to offer, no audit line.
    let out = s.run(&["consistency", "scan"], b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("nothing to propose"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(s.calls(), 0);
    assert_eq!(s.audit().len(), 1);
}

#[test]
fn settling_from_a_prompt_approves_rejects_retires_and_forgets() {
    let s = Setup::new();
    // A supersession opened before the scan stopped judging pairs.
    s.write_proposals(&json!([{
        "id": "p-1", "kind": "supersedes", "older": "preferences-1", "newer": "preferences-2",
        "reason": "The newer decision replaces squash merges with rebase merges.",
        "quotes": [{"entry": "preferences-1", "text": "squash-merge agent PRs"},
                   {"entry": "preferences-2", "text": "rebase-merge agent PRs"}],
        "state": "open", "opened_at": "2026-10-12T06:00:00Z",
        "settled_at": null, "settled_by": null
    }]));
    s.scan();
    assert_eq!(
        s.session_start(),
        "{\"systemMessage\":\"scribe: 2 proposals to review — scribe proposal list\"}\n"
    );

    let echo = s.type_prompt("Settling.\n#scribe approve p-1\n#scribe reject p-2");
    assert!(
        echo.contains("approved p-1 — preferences-1 is superseded by preferences-2"),
        "{echo}"
    );
    assert!(echo.contains("rejected p-2"), "{echo}");
    let older = s.entry_json("preferences-1");
    assert_eq!(older["status"], "superseded");
    assert_eq!(older["superseded_by"], "preferences-2");
    let p1 = s.proposal("p-1");
    assert_eq!(p1["state"], "approved");
    assert_eq!(p1["settled_by"]["session"], SESSION);
    assert_eq!(p1["settled_by"]["line"], 2);
    assert_eq!(s.proposal("p-2")["state"], "rejected");
    assert_eq!(
        s.entry_json("rituals-1")["status"],
        "active",
        "reject changes no entry"
    );
    assert_eq!(s.session_start(), "", "nothing open: no message");

    // A settled pair or stale entry is never proposed again.
    let calls = s.calls();
    assert_eq!(s.scan()["opened"], json!([]));
    assert_eq!(s.calls(), calls);

    // Replayed: nothing changes and nothing is audited.
    let lines = s.audit().len();
    let echo = s.type_prompt("#scribe approve p-1");
    assert!(
        echo.contains("already approved — nothing changed"),
        "{echo}"
    );
    assert_eq!(s.audit().len(), lines);

    // Approving a rejected proposal is refused, with its code audited.
    let echo = s.type_prompt("#scribe approve p-2");
    assert!(echo.contains("refused"), "{echo}");
    let last = s.audit().pop().unwrap();
    assert_eq!(last["error_code"], "proposal_settled");

    let echo = s.type_prompt("#scribe retire preferences-2");
    assert!(echo.contains("retired preferences-2"), "{echo}");
    let retired = s.entry_json("preferences-2");
    assert_eq!(retired["status"], "retired");
    assert_eq!(retired["content"], NEWER, "retire keeps the text");

    let echo = s.type_prompt("#scribe forget rituals-1");
    assert!(echo.contains("forgot rituals-1"), "{echo}");
    let forgotten = s.entry_json("rituals-1");
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(
        (&forgotten["said"], &forgotten["content"]),
        (&json!(""), &json!(""))
    );
    let p2 = s.proposal("p-2");
    assert_eq!(p2["reason"], "");
    assert_eq!(p2["quotes"][0]["text"], "");
    let store = support::exported(&s.fx.root(), "proposals.json").unwrap()
        + &support::exported(&s.fx.root(), "entries/rituals/rituals-1.md").unwrap();
    assert!(
        !store.contains(RITUAL),
        "the forgotten words left the store"
    );

    let echo = s.type_prompt("#scribe retire rituals-1");
    assert!(echo.contains("refused"), "{echo}");
    assert_eq!(s.audit().pop().unwrap()["error_code"], "entry_not_active");
    let echo = s.type_prompt("#scribe approve preferences-1");
    assert!(echo.contains("takes a proposal id"), "{echo}");
}

#[test]
fn a_scan_reapplies_an_approval_whose_entry_is_still_active() {
    let s = Setup::new();
    s.write_proposals(&json!([{
        "id": "p-1", "kind": "supersedes", "older": "preferences-1", "newer": "preferences-2",
        "reason": "replaced", "quotes": [], "state": "approved",
        "opened_at": "2026-10-12T06:00:00Z", "settled_at": "2026-10-13T08:00:00Z",
        "settled_by": {"session": SESSION, "message": null, "at": "2026-10-13T08:00:00Z",
                       "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work"}
    }]));
    let report = s.scan();
    assert_eq!(
        report["reapplied"],
        json!([{"proposal": "p-1", "entry": "preferences-1", "status": "superseded"}])
    );
    assert_eq!(
        ids(&report["opened"]),
        ["p-2"],
        "the pair is settled; the ritual is stale"
    );
    let older = s.entry_json("preferences-1");
    assert_eq!(older["status"], "superseded");
    assert_eq!(older["superseded_by"], "preferences-2");
    assert_eq!(s.calls(), 0, "the only pair is settled: no model call");
    let targets: Vec<String> = s
        .audit()
        .iter()
        .map(|l| l["target"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(targets, ["preferences-1", "p-2"]);
    assert!(s.scan()["reapplied"].as_array().unwrap().is_empty(), "once");
}

#[test]
fn the_routine_tools_run_the_commands_against_the_configured_store() {
    let s = Setup::new();
    let root = s.fx.root();
    let transcripts = s.fx.home().join(".claude/projects");
    let config = json!({"root": root, "transcripts": transcripts, "task_workspaces": []});

    let reply = plugin(&s, "scribe.consistency-scan", &config);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(ids(&reply["output"]["opened"]), ["p-1"]);
    assert_eq!(s.proposal("p-1")["state"], "open");
    let audit = s.audit();
    assert_eq!(audit.len(), 1);
    assert!(audit.iter().all(|l| l["command"] == "consistency scan"));

    let reply = plugin(&s, "scribe.session-pass-prepare", &config);
    assert_eq!(reply["ok"], true);
    assert_eq!(reply["output"]["done"], true);
    assert_eq!(reply["output"]["settled"], json!([]));
    assert_eq!(s.audit().len(), 1, "nothing due: no audit line");

    // Unset settings, including the manifest's defaults, are refused.
    for (tool, config) in [
        (
            "scribe.session-pass-prepare",
            json!({"root": root, "transcripts": ""}),
        ),
        (
            "scribe.session-pass-prepare",
            json!({"root": "/var/tmp", "transcripts": transcripts}),
        ),
        ("scribe.consistency-scan", json!({"root": "/var/tmp"})),
        ("scribe.consistency-scan", json!({"root": "relative/path"})),
    ] {
        let reply = plugin(&s, tool, &config);
        assert_eq!(
            reply["error"]["code"], "not_configured",
            "{tool} {config}: {reply}"
        );
    }
}

// S10: the scan deactivates sessions by age.

#[test]
fn a_scan_deactivates_old_sessions_but_never_a_pinned_one() {
    let s = Setup::new();
    for file in &session_files() {
        s.session(file);
    }

    let report = s.scan();

    let deactivated = report["deactivated"].as_array().unwrap();
    assert_eq!(deactivated.len(), 1, "{report}");
    assert_eq!(deactivated[0]["session"], "s-quiet");
    assert_eq!(deactivated[0]["route"], "age");
    assert_eq!(deactivated[0]["cite"], Value::Null);
    assert!(
        deactivated[0]["reason"]
            .as_str()
            .unwrap()
            .ends_with("(limit 120 days)"),
        "{report}"
    );

    let quiet = s.session_json("s-quiet");
    assert_eq!(quiet["active"], false);
    assert_eq!(quiet["active_change"]["route"], "age");
    assert_eq!(quiet["active_change"]["by"]["kind"], "scribe");
    assert_eq!(quiet["active_change"]["by"]["name"], "consistency");
    for id in ["s-pinned", "s-a-digest", "s-b-digest", "s-later"] {
        let record = s.session_json(id);
        assert_eq!(record["active"], true, "{id}");
        assert_eq!(record["active_change"], Value::Null, "{id}");
    }

    let targets: Vec<String> = s
        .audit()
        .iter()
        .filter_map(|l| l["target"].as_str())
        .filter(|t| t.starts_with("session:"))
        .map(str::to_owned)
        .collect();
    assert_eq!(targets, ["session:s-quiet"]);

    // The next scan never deactivates a session already inactive.
    let out = s.run(&["consistency", "scan", "--format", "auto"], b"");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        !text(&out.stdout)
            .lines()
            .any(|l| l.starts_with("deactivated")),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn deactivated_rows_print_in_plain_output() {
    let s = Setup::new();
    s.session(&session_files()[0]);
    let out = s.run(&["consistency", "scan", "--format", "auto"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let rows = text(&out.stdout);
    assert!(
        rows.lines().any(|l| l == "deactivated\ts-1\tage\t-\t-"),
        "{rows}"
    );
}

#[test]
fn a_shorter_configured_limit_deactivates_sooner() {
    let s = Setup::new();
    let [_, _, digest, ..] = session_files();
    let file = SessionFile {
        days_ago: 40,
        updated_ago: 40,
        ..digest
    };
    s.session(&file);
    std::fs::write(
        s.fx.root().join("config.toml"),
        "[sessions]\ninactive_after_days = 30\n",
    )
    .unwrap();
    make_private(&s.fx.root().join("config.toml"));
    let report = s.scan();
    assert_eq!(report["deactivated"][0]["session"], "s-a-digest");
    assert_eq!(report["deactivated"][0]["route"], "age");

    std::fs::write(
        s.fx.root().join("config.toml"),
        "[sessions]\ninactive_after_days = 0\n",
    )
    .unwrap();
    let out = s.run(&["consistency", "scan", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "config_invalid");
}

include!("consistency/claim_reconciliation.rs");
