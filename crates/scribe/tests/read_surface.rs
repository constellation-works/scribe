//! End-to-end tests of S11's read surface through the built binary
//! (docs/design/scribe/2_design.md §1.1, §6.8, §10 S11): `scribe search`
//! ranks without a model or an Orbit call and hides hidden records unless
//! asked; `entry list`, `session list` and `proposal list` take the new
//! filters, and a bad date is a usage error; every command taking an id
//! takes a unique prefix, an exact id wins, and an ambiguous prefix exits 2
//! with `id_ambiguous` and the candidates.
//!
//! Every run is the built binary with a temp `HOME` and data root, and an
//! empty `PATH`, so a model or Orbit call could not even start.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use std::process::Output;
use support::{Fixture, make_private, snapshot, text};

const SESSION: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
const INACTIVE: &str = "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05";
const LEGACY: &str = "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01";

fn run(fx: &Fixture, args: &[&str]) -> Output {
    fx.cmd(args)
        .env("PATH", fx.elsewhere("empty-path"))
        .env("GITHUB_TOKEN", "fixture-token-value-1234")
        .output()
        .unwrap()
}

fn ok(fx: &Fixture, args: &[&str]) -> Output {
    let out = run(fx, args);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{args:?}: {}",
        text(&out.stderr)
    );
    out
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| panic!("{e}: {}", text(bytes)))
}

fn ok_json(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    json(&ok(fx, &all).stdout)
}

fn usage_error(fx: &Fixture, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    let out = run(fx, &all);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{args:?}: {}",
        text(&out.stderr)
    );
    json(&out.stderr)
}

fn ids(list: &Value, key: &str, field: &str) -> Vec<String> {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item[field].as_str().unwrap().to_owned())
        .collect()
}

/// Copy the record of `SESSION` under another id, so two sessions share a
/// prefix.
fn copy_session(fx: &Fixture, id: &str) {
    // Before the first session write the record is still a file; after
    // it, a row.
    let dir = fx.root().join("sessions/2026/11");
    let file = dir.join(format!("{SESSION}.md"));
    if file.exists() {
        let original = std::fs::read_to_string(&file).unwrap();
        let path = dir.join(format!("{id}.md"));
        std::fs::write(
            &path,
            original.replace(&format!("id: {SESSION}"), &format!("id: {id}")),
        )
        .unwrap();
        make_private(&path);
        return;
    }
    support::sql(
        &fx.root(),
        &format!(
            "CREATE TEMP TABLE c AS SELECT * FROM sessions WHERE provider_session = '{SESSION}';
             UPDATE c SET id = NULL, provider_session = '{id}';
             INSERT INTO sessions SELECT * FROM c;
             CREATE TEMP TABLE g AS SELECT * FROM segments WHERE session =
                 (SELECT id FROM sessions WHERE provider_session = '{SESSION}');
             UPDATE g SET session = (SELECT id FROM sessions WHERE provider_session = '{id}');
             INSERT INTO segments SELECT * FROM g;"
        ),
    );
}

#[test]
fn brief_reads_only_and_calls_no_model() {
    let fx = Fixture::new();
    fx.seed();
    let called = fx.elsewhere("claude-called");
    let path = fx.fake_program(
        "claude",
        &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", called.display()),
    );
    let before = snapshot(&fx.root());
    let mut cmd = fx.cmd(&["brief", "--project", "constellation-works/scribe", "--json"]);
    let out = cmd.env("PATH", &path).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(snapshot(&fx.root()), before, "brief wrote to the store");
    assert!(!fx.root().join("audit.jsonl").exists());
    assert!(!called.exists(), "brief ran the model CLI");
    let brief = json(&out.stdout);
    let sessions = brief["sessions"].as_array().unwrap();
    // The fixture's inactive session is left out.
    let ids: Vec<&str> = sessions.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [SESSION, LEGACY]);
    assert!(!ids.contains(&INACTIVE));
    let at: Vec<&str> = sessions.iter().map(|s| s["at"].as_str().unwrap()).collect();
    assert!(
        at.windows(2).all(|w| w[0] >= w[1]),
        "not newest first: {at:?}"
    );
    // Only the open proposal is a claim; the rejected one is not.
    assert_eq!(brief["claim_count"], 1);
    assert_eq!(brief["claims"][0]["tag"], "decisions");
    assert_eq!(brief["claims"][0]["claims"][0]["id"], "p-1");
    assert_eq!(brief["claims"][0]["claims"][0]["kind"], "supersedes");
    assert_eq!(
        brief["claims"][0]["claims"][0]["quote"],
        "merge agent PRs with a merge commit"
    );
}

#[test]
fn brief_sessions_out_of_range_is_a_usage_error() {
    let fx = Fixture::new();
    let error = usage_error(&fx, &["brief", "--sessions", "21"]);
    assert_eq!(error["code"], "usage_error");
}

#[test]
fn search_ranks_the_store_reads_only_and_calls_nothing() {
    let fx = Fixture::new();
    fx.seed();
    let before = snapshot(&fx.root());
    let found = ok_json(&fx, &["search", "merge agent PRs"]);
    assert_eq!(snapshot(&fx.root()), before, "search wrote to the store");
    assert!(!fx.root().join("audit.jsonl").exists());
    let hits = found["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    let scores: Vec<f64> = hits.iter().map(|h| h["score"].as_f64().unwrap()).collect();
    assert!(
        scores.windows(2).all(|w| w[0] >= w[1]),
        "not best first: {scores:?}"
    );
    // Daniel's active entry outranks the session units saying the same.
    assert_eq!(hits[0]["cite"], "decisions-2");
    assert_eq!(hits[0]["kind"], "entry");
    assert_eq!(hits[0]["author"]["kind"], "user");
    assert_eq!(hits[0]["segment"], Value::Null);
    for key in [
        "kind", "cite", "id", "segment", "score", "title", "snippet", "project", "topics",
        "author", "active", "updated", "status",
    ] {
        assert!(hits[0].get(key).is_some(), "hit lacks {key}");
    }
    let segment = hits
        .iter()
        .find(|h| h["segment"].is_number())
        .expect("a segment hit");
    assert_eq!(
        segment["cite"],
        format!(
            "session:{}#{}",
            segment["id"].as_str().unwrap(),
            segment["segment"]
        )
    );
    assert_eq!(found["total"], hits.len());
    assert_eq!(found["truncated"], false);
    assert!(found["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn hidden_entries_and_inactive_sessions_appear_only_on_request() {
    let fx = Fixture::new();
    fx.seed();
    let out = ok(&fx, &["search", "merge commit", "--json"]);
    let cites = ids(&json(&out.stdout), "hits", "cite");
    assert!(!cites.contains(&String::from("decisions-1")), "{cites:?}");
    assert!(!cites.iter().any(|c| c.contains(INACTIVE)), "{cites:?}");
    assert!(text(&out.stderr).contains("--include-hidden"));
    let all = ok_json(&fx, &["search", "merge commit", "--include-hidden"]);
    let hidden: Vec<&Value> = all["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|h| h["cite"] == "decisions-1" || h["id"] == INACTIVE)
        .collect();
    assert!(hidden.len() >= 2, "{all}");
    let entry = hidden.iter().find(|h| h["kind"] == "entry").unwrap();
    assert_eq!(entry["status"], "superseded");
    assert_eq!(entry["active"], Value::Null);
    let session = hidden.iter().find(|h| h["kind"] == "session").unwrap();
    assert_eq!(session["active"], false);
}

#[test]
fn search_filters_narrow_and_the_limit_cuts() {
    let fx = Fixture::new();
    fx.seed();
    let kinds = |args: &[&str]| -> Vec<String> { ids(&ok_json(&fx, args), "hits", "kind") };
    let entries = kinds(&["search", "merge", "--kind", "entry"]);
    assert!(!entries.is_empty() && entries.iter().all(|k| k == "entry"));
    let sessions = kinds(&["search", "merge", "--kind", "session"]);
    assert!(!sessions.is_empty() && sessions.iter().all(|k| k == "session"));
    // The store-layout session ran in October.
    let october = ok_json(&fx, &["search", "store layout"]);
    assert!(ids(&october, "hits", "id").contains(&String::from(LEGACY)));
    let november = ok_json(&fx, &["search", "store layout", "--since", "2026-11-01"]);
    assert!(!ids(&november, "hits", "id").contains(&String::from(LEGACY)));
    let topic = ok_json(
        &fx,
        &[
            "search",
            "merge",
            "--topic",
            "merge-policy",
            "--topic",
            "release",
        ],
    );
    assert_eq!(ids(&topic, "hits", "cite")[0], "decisions-2");
    let none = ok(&fx, &["search", "merge", "--project", "orbit"]);
    assert!(none.stdout.is_empty());
    assert!(text(&none.stderr).contains("no records match"));
    let cut = ok_json(&fx, &["search", "merge", "--limit", "1"]);
    assert_eq!(cut["hits"].as_array().unwrap().len(), 1);
    assert_eq!(cut["truncated"], true);
    assert!(cut["total"].as_u64().unwrap() > 1);
}

#[test]
fn search_refuses_terms_with_no_searchable_word_and_bad_dates() {
    let fx = Fixture::new();
    fx.seed();
    assert_eq!(
        usage_error(&fx, &["search", "the of"])["code"],
        "usage_error"
    );
    for args in [
        &["search", "merge", "--since", "2026-13-01"][..],
        &["search", "merge", "--until", "yesterday"],
        &["entry", "list", "--since", "2026/11/01"],
        &["session", "list", "--until", "11-01-2026"],
        &["proposal", "list", "--since", "soon"],
        &[
            "entry",
            "list",
            "--since",
            "2026-11-05",
            "--until",
            "2026-11-01",
        ],
    ] {
        assert_eq!(usage_error(&fx, args)["code"], "usage_error", "{args:?}");
    }
}

#[test]
fn entry_list_filters_by_topics_related_dates_and_author() {
    let fx = Fixture::new();
    fx.seed();
    let list = |args: &[&str]| ids(&ok_json(&fx, args), "entries", "id");
    assert_eq!(
        list(&[
            "entry",
            "list",
            "--topic",
            "merge-policy",
            "--topic",
            "release"
        ]),
        ["decisions-2"]
    );
    assert_eq!(
        list(&[
            "entry",
            "list",
            "--related",
            "repo:constellation-works/scribe",
            "--related",
            "pr:constellation-works/scribe#7",
        ]),
        ["decisions-1", "decisions-2"]
    );
    assert_eq!(
        list(&[
            "entry",
            "list",
            "--since",
            "2026-10-01",
            "--until",
            "2026-10-20"
        ]),
        ["preferences-1", "decisions-1"]
    );
    assert_eq!(list(&["entry", "list", "--author", "user"]).len(), 4);
    // The name before format 23 still selects the user's entries.
    assert_eq!(list(&["entry", "list", "--author", "daniel"]).len(), 4);
    assert!(list(&["entry", "list", "--author-kind", "agent"]).is_empty());
    ok(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "agent note on merges",
        ],
    );
    let agent = list(&["entry", "list", "--author-kind", "agent"]);
    assert_eq!(agent.len(), 1);
    assert_eq!(list(&["entry", "list", "--author", "opus"]), agent);
    let empty = ok(
        &fx,
        &[
            "entry",
            "list",
            "--status",
            "active",
            "--since",
            "2027-01-01",
        ],
    );
    assert!(text(&empty.stderr).contains("--since 2027-01-01"));
}

#[test]
fn session_list_filters_by_status_dates_topics_and_related() {
    let fx = Fixture::new();
    fx.seed();
    let list = |args: &[&str]| ids(&ok_json(&fx, args), "sessions", "id");
    assert_eq!(list(&["session", "list", "--status", "open"]), [SESSION]);
    assert_eq!(list(&["session", "list", "--status", "final"]), [LEGACY]);
    assert_eq!(
        list(&["session", "list", "--status", "final", "--include-inactive"]),
        [LEGACY, INACTIVE]
    );
    assert_eq!(
        list(&["session", "list", "--since", "2026-11-01"]),
        [SESSION]
    );
    assert_eq!(
        list(&["session", "list", "--until", "2026-10-31"]),
        [LEGACY]
    );
    assert_eq!(
        list(&[
            "session",
            "list",
            "--topic",
            "release",
            "--topic",
            "merge-policy"
        ]),
        [SESSION]
    );
    assert!(list(&["session", "list", "--topic", "release", "--topic", "design"]).is_empty());
    assert_eq!(
        list(&[
            "session",
            "list",
            "--related",
            "task:none-such",
            "--related",
            "repo:constellation-works/scribe",
        ]),
        [LEGACY]
    );
}

#[test]
fn proposal_list_filters_by_state_dates_entry_and_author_kind() {
    let fx = Fixture::new();
    fx.seed();
    let list = |args: &[&str]| ids(&ok_json(&fx, args), "proposals", "id");
    assert_eq!(list(&["proposal", "list", "--state", "open"]), ["p-1"]);
    assert_eq!(list(&["proposal", "list", "--state", "rejected"]), ["p-2"]);
    assert_eq!(
        list(&["proposal", "list", "--since", "2026-11-01"]),
        ["p-1"]
    );
    assert_eq!(
        list(&["proposal", "list", "--until", "2026-10-31"]),
        ["p-2"]
    );
    // As the older or the newer entry, by id or by a unique prefix.
    assert_eq!(
        list(&["proposal", "list", "--entry", "decisions-2"]),
        ["p-1"]
    );
    assert_eq!(list(&["proposal", "list", "--entry", "ritu"]), ["p-2"]);
    assert_eq!(
        list(&["proposal", "list", "--author-kind", "scribe"]).len(),
        2
    );
    assert!(list(&["proposal", "list", "--author-kind", "agent"]).is_empty());
    let err = usage_error(&fx, &["proposal", "list", "--entry", "deci"]);
    assert_eq!(err["code"], "id_ambiguous");
}

#[test]
fn a_unique_prefix_names_a_session_and_an_exact_id_wins() {
    let fx = Fixture::new();
    fx.seed();
    let shown = ok_json(&fx, &["session", "show", "7c2b"]);
    assert_eq!(shown["id"], SESSION);
    let segment = ok_json(&fx, &["session", "show", "session:7c2b9d14#2"]);
    assert_eq!(segment["id"], SESSION);
    assert_eq!(segment["log"].as_array().unwrap().len(), 1);
    // A second session whose id extends the first's.
    let longer = format!("{SESSION}-b");
    copy_session(&fx, &longer);
    assert_eq!(ok_json(&fx, &["session", "show", SESSION])["id"], SESSION);
    assert_eq!(ok_json(&fx, &["session", "show", &longer])["id"], longer);
    let err = usage_error(&fx, &["session", "show", "7c2b"]);
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(&longer),
        "{message}"
    );
    // Fewer than four characters is never a prefix.
    let short = run(&fx, &["session", "show", "0f8", "--json"]);
    assert_eq!(short.status.code(), Some(2));
    assert_eq!(json(&short.stderr)["code"], "usage_error");
}

#[test]
fn an_exact_entry_id_wins_over_a_longer_one_it_prefixes() {
    let fx = Fixture::new();
    fx.seed();
    let path = fx.root().join("entries/preferences/preferences-10.md");
    std::fs::write(&path, support::entry_text(10, "a tenth preference")).unwrap();
    make_private(&path);
    assert_eq!(
        ok_json(&fx, &["entry", "show", "preferences-1"])["id"],
        "preferences-1"
    );
    assert_eq!(
        ok_json(&fx, &["entry", "show", "preferences-10"])["id"],
        "preferences-10"
    );
    assert_eq!(ok_json(&fx, &["entry", "show", "ritu"])["id"], "rituals-1");
    let err = usage_error(&fx, &["entry", "show", "pref"]);
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    for id in ["preferences-1", "preferences-10"] {
        assert!(message.contains(id), "{message}");
    }
}

#[test]
fn session_writes_take_a_prefix_and_audit_the_full_id() {
    let fx = Fixture::new();
    fx.seed();
    ok(
        &fx,
        &[
            "session",
            "deactivate",
            "7c2b9d14",
            "--author",
            "opus",
            "--reason",
            "gone",
        ],
    );
    let shown = ok_json(&fx, &["session", "show", SESSION]);
    assert_eq!(shown["active"], false);
    let audit = std::fs::read_to_string(fx.root().join("audit.jsonl")).unwrap();
    let line: Value = serde_json::from_str(audit.lines().last().unwrap()).unwrap();
    assert_eq!(line["target"], format!("session:{SESSION}"));
    ok(&fx, &["session", "activate", "7c2b", "--author", "opus"]);
    assert_eq!(ok_json(&fx, &["session", "show", SESSION])["active"], true);
    copy_session(&fx, "7c2b0000-0000-4000-8000-000000000000");
    let err = usage_error(
        &fx,
        &[
            "session",
            "deactivate",
            "7c2b",
            "--author",
            "opus",
            "--reason",
            "gone",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let pass = usage_error(&fx, &["session", "pass", "--session", "7c2b"]);
    assert_eq!(pass["code"], "id_ambiguous");
}

#[test]
fn the_record_noun_is_gone_and_search_is_a_top_level_verb() {
    // `scribe search` replaced `record search` with no alias.
    let fx = Fixture::new();
    fx.seed();
    for args in [&["record", "search", "merge"][..], &["record"]] {
        let out = run(&fx, args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    ok(&fx, &["search", "merge agent PRs"]);
}
