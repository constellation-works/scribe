//! End-to-end tests of S4 through the built binary
//! (docs/design/scribe/2_design.md §5, §10 S4): `/scribe on|off|status`
//! switching a session and blocking the prompt, the `Stop` and `SessionEnd`
//! hooks marking a pass due without a model, and `scribe session pass`
//! settling sessions and the prepare/apply tools writing their windows from fixed answers.
//!
//! Every run is the built binary with a temp `HOME` whose
//! `.claude/projects/` holds the test transcripts; nothing touches the real
//! home.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/session_pass.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::Input::Cli;
use crate::support::command::with_env;
use crate::support::command::{Json, Text};
use scribe_core::PassVerbs;
use scribe_core::testing::ViewTestReads;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::process::Output;
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, context_of, make_private, pass, text};

#[test]
fn pass_candidate_waits_for_typed_approval_and_keeps_original_user_cite() {
    let s = Setup::new();
    let quote = "I prefer short progress updates while an agent is working.";
    s.write(ON, &[line("user", "preference-turn", quote)]);
    let mut body = answer();
    body["candidates"] =
        json!([{"quote": quote, "message": "preference-turn", "tag": "preferences"}]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let proposals: Value = s.fx.check(Cli(&["proposal", "list", "--json"]), Json);
    let p = &proposals["proposals"][0];
    assert_eq!(p["kind"], "candidate");
    assert_eq!(p["candidate"]["quote"], quote);
    assert_eq!(p["candidate"]["cite"]["message"], "preference-turn");
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-1.md").is_none());
    let id = p["id"].as_str().unwrap();
    let prompt = format!("#scribe approve {id}");
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!(prompt);
    let first = s.hook("user-prompt-submit", &payload);
    assert!(
        text(&first.stdout).contains("recorded"),
        "{}",
        text(&first.stdout)
    );
    s.hook("user-prompt-submit", &payload);
    let entries: Value = s.fx.check(
        Cli(&["entry", "list", "--tag", "preferences", "--json"]),
        Json,
    );
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    let entry: Value = s.entry_json("preferences-1");
    assert_eq!(entry["content"], context_of(quote));
    assert_eq!(entry["quote"], quote);
    assert_eq!(entry["author"]["kind"], "user");
    assert_eq!(entry["cite"]["message"], "preference-turn");
    assert_eq!(entry["content_from"]["role"], "user");

    // What admitted the claim is printed by `proposal list` and `entry show`
    // in every form.
    let criteria = "A standing rule from the user that applies beyond the work at hand.";
    let admitted = json!({"trigger": "I prefer", "matched": "I prefer", "criteria": criteria});
    assert_eq!(entry["admitted"], admitted);
    assert_eq!(p["candidate"]["admitted"], admitted);
    let plain = s.fx.check(Cli(&["entry", "show", "preferences-1"]), Text);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("admitted\tI prefer\tI prefer\t{criteria}")),
        "{plain}"
    );
    let table = s.fx.check(
        Cli(&["entry", "show", "preferences-1", "--format", "table"]),
        Text,
    );
    assert!(
        table.contains(&format!(
            "admitted    trigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}"
        )),
        "{table}"
    );
    let listed = s.fx.check(Cli(&["proposal", "list"]), Text);
    assert!(
        listed.ends_with(&format!(
            "\ttrigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}\t-\n"
        )),
        "{listed}"
    );
    // An entry not approved from a claim has no admission.
    let typed: Value = s.fx.check(
        Cli(&[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "Keep it short.",
            "--json",
        ]),
        Json,
    );
    let id = typed["id"].as_str().unwrap();
    let shown: Value = s.entry_json(id);
    assert_eq!(shown["admitted"], Value::Null);
}

#[test]
fn a_stored_assistant_finding_candidate_still_approves_into_one_agent_entry() {
    // The pass no longer opens assistant-turn candidates (capture rules read
    // Daniel's turns only), but one opened before, in a format-10 store,
    // still settles: approval writes the agent's entry, never Daniel's.
    let quote = "Measured: the reviewer mix alone predicts a 7-point first-pass gap.";
    let s = setup_stored_assistant_finding_candidate(quote);
    let ps = proposals(&s);
    assert_eq!(ps["proposals"][0]["candidate"]["admitted"], Value::Null);
    assert_eq!(ps["proposals"][0]["closed_reason"], Value::Null);

    let approved = type_settlement(&s, "#scribe approve p-1");
    assert!(
        text(&approved.stdout).contains("recorded finding-1 as the agent's entry"),
        "{}",
        text(&approved.stdout)
    );
    type_settlement(&s, "#scribe approve p-1");
    let entries: Value =
        s.fx.check(Cli(&["entry", "list", "--tag", "finding", "--json"]), Json);
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    let entry = &entries["entries"][0];
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["author"]["kind"], "agent", "never Daniel's");
    assert_eq!(entry["author"]["name"], "assistant");
    assert_eq!(entry["content_from"]["role"], "assistant");
    assert_eq!(entry["evidence"], "exp-5342a4d6");
    assert_eq!(entry["as_of"], "2026-11-03");
}

#[test]
fn forgetting_an_assistant_derived_entry_keeps_it_readable_with_blank_content() {
    let quote = "Measured: the reviewer mix alone predicts a 7-point first-pass gap.";
    // Format 10 allowed free-form evidence, including the finding itself.
    let s = setup_stored_assistant_finding_candidate_with(quote, quote);
    let approved = type_settlement(&s, "#scribe approve p-1");
    assert!(
        text(&approved.stdout).contains("recorded finding-1 as the agent's entry"),
        "{}",
        text(&approved.stdout)
    );

    let forgot = type_settlement(&s, "#scribe forget finding-1");
    assert!(
        text(&forgot.stdout).contains("forgot finding-1"),
        "{}",
        text(&forgot.stdout)
    );

    let listed: Value =
        s.fx.check(Cli(&["entry", "list", "--tag", "finding", "--json"]), Json);
    assert_eq!(listed["warnings"], json!([]));
    assert_eq!(listed["total"], 1);
    let listed_entry = &listed["entries"][0];
    assert_eq!(listed_entry["id"], "finding-1");
    assert_eq!(listed_entry["status"], "forgotten");
    assert_eq!(listed_entry["said"], "");
    assert_eq!(listed_entry["content"], "");
    assert_eq!(listed_entry["author"]["kind"], "agent");
    assert_eq!(listed_entry["author"]["name"], "assistant");

    let shown: Value = s.entry_json("finding-1");
    assert_eq!(shown["id"], "finding-1");
    assert_eq!(shown["status"], "forgotten");
    assert_eq!(shown["said"], "");
    assert_eq!(shown["content"], "");
    assert_eq!(shown["author"]["kind"], "agent");
    assert_eq!(shown["author"]["name"], "assistant");
    assert_eq!(shown["content_from"]["role"], "assistant");
    assert_eq!(shown["cite"]["message"], "a-find");

    let plain_list =
        s.fx.check(Cli(&["entry", "list", "--tag", "finding"]), Text);
    assert!(plain_list.contains("finding-1"), "{plain_list}");
    assert!(plain_list.contains("forgotten"), "{plain_list}");
    let plain_show = s.fx.check(Cli(&["entry", "show", "finding-1"]), Text);
    assert!(plain_show.contains("forgotten"), "{plain_show}");

    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap()
        + &support::exported(&s.fx.root(), "entries/finding/finding-1.md").unwrap();
    assert!(
        !stored.contains(quote),
        "forgotten words must leave the entry and proposals"
    );

    assert_eq!(listed_entry["evidence"], Value::Null);
    assert_eq!(shown["evidence"], Value::Null);
    assert!(!plain_list.contains(quote));
    assert!(!plain_show.contains(quote));
    assert!(text(&forgot.stdout).contains("session records stay"));

    let replay = type_settlement(&s, "#scribe forget finding-1");
    assert!(
        text(&replay.stdout).contains("already forgotten"),
        "{}",
        text(&replay.stdout)
    );
    s.run(&["consistency", "scan", "--json"], b"");
}

#[test]
fn entry_show_refuses_inconsistent_assistant_entry_shapes() {
    let s = setup_stored_assistant_finding_candidate("Measured result.");
    let root = s.fx.root();
    type_settlement(&s, "#scribe approve p-1");
    // The entry is a row of `scribe.db` now: keep its active and forgotten
    // rows, and stage each malformed shape by replacing the row.
    support::sql(
        &root,
        "CREATE TABLE snap_active AS SELECT * FROM entries WHERE id = 'finding-1';",
    );
    type_settlement(&s, "#scribe forget finding-1");
    support::sql(
        &root,
        "CREATE TABLE snap_forgotten AS SELECT * FROM entries WHERE id = 'finding-1';",
    );
    for stage in [
        "DELETE FROM entries WHERE id = 'finding-1';
         INSERT INTO entries SELECT * FROM snap_active;
         UPDATE entries SET said = '' WHERE id = 'finding-1';",
        "DELETE FROM entries WHERE id = 'finding-1';
         INSERT INTO entries SELECT * FROM snap_forgotten;
         UPDATE entries SET said = '#scribe approve p-1' WHERE id = 'finding-1';",
        "DELETE FROM entries WHERE id = 'finding-1';
         INSERT INTO entries SELECT * FROM snap_forgotten;
         UPDATE entries SET content = content || 'leaked words' WHERE id = 'finding-1';",
    ] {
        support::sql(&root, stage);
        let out =
            s.fx.check(Cli(&["entry", "show", "finding-1", "--json"]), Capture);
        assert_eq!(out.status.code(), Some(1), "{stage}");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "store_corrupt", "{stage}");
    }
}

#[test]
fn pass_opens_no_candidate_from_an_assistant_turn() {
    // Daniel's turn fires a trigger, so claims are requested, but a claim
    // on an assistant turn (a measured result or a future consideration) is
    // refused: no tag's capture rules read assistant turns, while Daniel's
    // turns are still read.
    for (tag, quote) in [
        (
            "finding",
            "Measured: the reviewer mix alone predicts a 7-point gap.",
        ),
        (
            "remind",
            "The flaky cache test is not verified yet on this host.",
        ),
        (
            "idea",
            "What if we cache the reviewer mix per crew instead?",
        ),
    ] {
        let s = Setup::new();
        s.write(
            ON,
            &[
                line("user", "u-1", "Always measure before you change the mix."),
                line("assistant", "a-1", quote),
            ],
        );
        s.answer(&candidate_answer(quote, "a-1", tag));
        s.scribe("/scribe on");
        s.pass_session(ON);
        assert_eq!(proposals(&s)["total"], 0, "{tag}");
        assert_eq!(s.record(ON).unwrap()["scribed_through"], "a-1", "{tag}");
    }

    // Daniel's own firing turn in the same conversation still opens a claim.
    let s = Setup::new();
    let daniel_quote = "Always measure before you change the mix.";
    s.write(
        ON,
        &[
            line("user", "u-1", daniel_quote),
            line(
                "assistant",
                "a-1",
                "Measured: the reviewer mix alone predicts a 7-point gap.",
            ),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": "Measured: the reviewer mix alone predicts a 7-point gap.", "message": "a-1", "tag": "finding"},
        {"quote": daniel_quote, "message": "u-1", "tag": "preferences"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["tag"], "preferences");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], daniel_quote);
}

#[test]
fn pass_asks_for_standing_preferences_only_and_never_proposes_a_decision() {
    let s = Setup::new();
    let choice = "For this migration we will go with the blue-green rollout.";
    let standing = "I always want short progress updates while an agent is working.";
    s.write(
        ON,
        &[
            line("user", "u-choice", choice),
            line("user", "u-standing", standing),
        ],
    );
    let mut body = answer();
    // The model offers both; the one-time choice under the retired tag is
    // dropped locally, the standing preference stays.
    body["candidates"] = json!([
        {"quote": choice, "message": "u-choice", "tag": "decisions"},
        {"quote": standing, "message": "u-standing", "tag": "preferences"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.pass_session(ON);
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["tag"], "preferences");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], standing);

    let prompts = s.prompts();
    assert!(!prompts.is_empty());
    for prompt in &prompts {
        assert!(
            !prompt.contains("tag decisions"),
            "the pass prompt never offers decisions: {prompt}"
        );
        assert!(!prompt.contains("message u-choice"), "{prompt}");
        assert!(
            prompt.contains("A standing rule from the user that applies beyond the work at hand."),
            "{prompt}"
        );
    }
    // The admission is recorded with the candidate.
    let admitted = &listed["proposals"][0]["candidate"]["admitted"];
    assert_eq!(admitted["trigger"], "always", "{admitted}");
    assert_eq!(admitted["matched"], "always");
    assert_eq!(
        admitted["criteria"],
        "A standing rule from the user that applies beyond the work at hand."
    );
    // Stored text never keeps the prompt placeholder (2_design.md §15.8).
    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap();
    assert!(
        !stored.contains("{user}") && !stored.contains("{User}"),
        "{stored}"
    );
}

include!("session_pass/message_search.rs");
include!("session_pass/forget.rs");
include!("session_pass/triggers.rs");
include!("session_pass/window_repair.rs");
include!("session_pass/switch_and_pass.rs");
include!("session_pass/links_and_rebuild.rs");
include!("session_pass/candidate_lifecycle.rs");
include!("session_pass/erasure_and_audit.rs");
include!("session_pass/claim_context.rs");
include!("session_pass/ideas.rs");
include!("session_pass/memory.rs");
include!("session_pass/catchers.rs");
