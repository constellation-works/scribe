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
    fx.ok(&["session", "list", "--json"]); // the first open creates the WAL
    let before = snapshot(&fx.root());
    let mut cmd = fx.cmd(&["brief", "--project", "constellation-works/scribe", "--json"]);
    let out = cmd.env("PATH", &path).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(snapshot(&fx.root()), before, "brief wrote to the store");
    assert!(support::audit_lines(&fx.root()).is_empty());
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
    fx.ok(&["session", "list", "--json"]); // the first open creates the WAL
    let before = snapshot(&fx.root());
    let found = ok_json(&fx, &["search", "merge agent PRs"]);
    assert_eq!(snapshot(&fx.root()), before, "search wrote to the store");
    assert!(support::audit_lines(&fx.root()).is_empty());
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
fn search_tag_keeps_one_tags_entries_combined_with_the_other_filters() {
    let fx = Fixture::new();
    fx.seed();
    // `rituals` is an unknown legacy tag, so its literal value continues to
    // filter the stored entry. The entry is retired and appears only when
    // hidden records are requested.
    let visible = ok(&fx, &["search", "full gate", "--tag", "rituals"]);
    assert!(visible.stdout.is_empty());
    assert!(text(&visible.stderr).contains("--include-hidden"));
    let tagged = ok_json(
        &fx,
        &[
            "search",
            "full gate",
            "--tag",
            "rituals",
            "--include-hidden",
        ],
    );
    assert_eq!(ids(&tagged, "hits", "cite"), ["rituals-1"]);
    assert_eq!(tagged["hits"][0]["kind"], "entry");
    assert_eq!(tagged["total"], 1);
    // The date and project filters still apply under the tag.
    let before = ok_json(
        &fx,
        &[
            "search",
            "full gate",
            "--tag",
            "rituals",
            "--until",
            "2026-09-14",
            "--include-hidden",
        ],
    );
    assert!(ids(&before, "hits", "cite").is_empty());
    let other_project = ok(
        &fx,
        &[
            "search",
            "full gate",
            "--tag",
            "rituals",
            "--project",
            "constellation-works/scribe",
            "--include-hidden",
        ],
    );
    assert!(other_project.stdout.is_empty());
    // The echoed criteria name the tag, and no message count is offered for
    // a tag: messages carry none.
    let none = ok(&fx, &["search", "full gate", "--tag", "unknown-tag"]);
    assert!(none.stdout.is_empty());
    let notice = text(&none.stderr);
    assert!(
        notice.contains("no records match \"full gate\" --tag unknown-tag"),
        "{notice}"
    );
    assert!(!notice.contains("stored message"), "{notice}");
}

#[test]
fn search_tag_alias_selects_its_configured_tag() {
    let fx = Fixture::new();
    fx.seed();
    let canonical = ok_json(&fx, &["search", "commit", "--tag", "preferences"]);
    let alias = ok_json(&fx, &["search", "commit", "--tag", "pref"]);
    assert_eq!(ids(&canonical, "hits", "cite"), ["preferences-1"]);
    assert_eq!(ids(&alias, "hits", "cite"), ["preferences-1"]);
    assert_eq!(alias["total"], 1);
}

#[test]
fn search_tag_refuses_a_session_or_message_kind_in_one_line() {
    let fx = Fixture::new();
    fx.seed();
    for kind in ["session", "message"] {
        let args = ["search", "merge", "--tag", "decisions", "--kind", kind];
        let error = usage_error(&fx, &args);
        assert_eq!(error["code"], "usage_error", "{kind}: {error}");
        let message = error["error"].as_str().unwrap();
        assert!(
            message.starts_with("--tag filters entries only"),
            "{message}"
        );
        // The same refusal in text mode is one line on stderr, nothing on stdout.
        let out = run(&fx, &args);
        assert_eq!(out.status.code(), Some(2), "{kind}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty());
        let line = text(&out.stderr);
        assert_eq!(line.trim_end().lines().count(), 1, "{line}");
    }
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
fn entry_list_tag_alias_selects_its_configured_tag() {
    let fx = Fixture::new();
    fx.seed();
    let canonical = ok_json(&fx, &["entry", "list", "--tag", "preferences"]);
    let alias = ok_json(&fx, &["entry", "list", "--tag", "pref"]);
    assert_eq!(ids(&canonical, "entries", "id"), ["preferences-1"]);
    assert_eq!(ids(&alias, "entries", "id"), ["preferences-1"]);
    assert!(
        ids(
            &ok_json(&fx, &["entry", "list", "--tag", "unknown-tag"]),
            "entries",
            "id"
        )
        .is_empty()
    );
}

#[test]
fn session_list_filters_by_status_dates_topics_and_related() {
    let fx = Fixture::new();
    fx.seed();
    let list = |args: &[&str]| ids(&ok_json(&fx, args), "sessions", "id");
    assert_eq!(list(&["session", "list", "--status", "open"]), [SESSION]);
    assert_eq!(list(&["session", "list", "--status", "final"]), [LEGACY]);
    // Newest updated first: INACTIVE (updated 10-25) before LEGACY (10-20).
    assert_eq!(
        list(&["session", "list", "--status", "final", "--include-inactive"]),
        [INACTIVE, LEGACY]
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
    support::seed_records(
        &fx.root(),
        &[&support::entry_text(10, "a tenth preference")],
    );
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
    let lines = support::audit_lines(&fx.root());
    let line = lines.last().unwrap();
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

#[test]
fn a_table_prints_unique_prefixes_and_piped_output_aliases() {
    let fx = Fixture::new();
    fx.seed();
    let table = |fx: &Fixture| text(&ok(fx, &["session", "list", "--format", "table"]).stdout);
    // Unique at eight characters, the first group of a UUID.
    let listed = table(&fx);
    assert!(listed.contains("7c2b9d14 "), "{listed}");
    assert!(!listed.contains(SESSION), "{listed}");
    // A printed prefix is accepted back.
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14"])["id"],
        SESSION
    );
    // Piped output uses an alias that resolves; JSON keeps the provider id.
    let plain = text(&ok(&fx, &["session", "list", "--format", "auto"]).stdout);
    assert!(
        plain.lines().any(|line| line.starts_with("s-3\t")),
        "{plain}"
    );
    assert_eq!(ok_json(&fx, &["session", "show", "s-3"])["id"], SESSION);
    let listed_json = ok_json(&fx, &["session", "list"]);
    assert!(ids(&listed_json, "sessions", "id").contains(&SESSION.to_owned()));
    // A session sharing the first eight characters (a UUIDv7's timestamp)
    // lengthens both, past the shared run, and each is accepted back.
    let sibling = "7c2b9d14-ffff-4a3b-b1c6-2f9e8d7a6b02";
    copy_session(&fx, sibling);
    let listed = table(&fx);
    assert!(listed.contains("7c2b9d14-8 "), "{listed}");
    assert!(listed.contains("7c2b9d14-f "), "{listed}");
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14-8"])["id"],
        SESSION
    );
    assert_eq!(
        ok_json(&fx, &["session", "show", "7c2b9d14-f"])["id"],
        sibling
    );
}

#[test]
fn cursor_only_codex_sessions_disambiguate_cataloged_session_prefixes() {
    let fx = Fixture::new();
    fx.seed();
    let cursor_only = "7c2b9d14-ffff-4a3b-b1c6-2f9e8d7a6b02";
    // The first store write migrates the checked-in document fixture into
    // SQLite, where cursor-only ids are persisted independently of sessions.
    ok(
        &fx,
        &[
            "session",
            "deactivate",
            SESSION,
            "--author",
            "opus",
            "--reason",
            "fixture migration",
        ],
    );
    ok(&fx, &["session", "activate", SESSION, "--author", "opus"]);
    support::sql(
        &fx.root(),
        &format!(
            "INSERT INTO codex_cursors (session, message) VALUES ('{cursor_only}', 'cursor-1')"
        ),
    );

    let table = text(&ok(&fx, &["session", "list", "--format", "table"]).stdout);
    assert!(table.contains("7c2b9d14-8 "), "{table}");

    let err = usage_error(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Cache the manifest between runs",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(cursor_only),
        "{message}"
    );
}

#[test]
fn a_session_reference_given_as_a_prefix_is_stored_in_full() {
    let fx = Fixture::new();
    fx.seed();
    let added = ok_json(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Cache the manifest between runs",
        ],
    );
    let related: Vec<&str> = added["related"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(related, [format!("session:{SESSION}")]);
    // The same prefix filters a list.
    let listed = ok_json(&fx, &["entry", "list", "--related", "session:7c2b9d14"]);
    assert_eq!(listed["total"], 1);
    // An ambiguous prefix is refused, names the sessions and writes nothing.
    let longer = format!("{SESSION}-b");
    copy_session(&fx, &longer);
    let before = ok_json(&fx, &["entry", "list"])["total"].clone();
    let err = usage_error(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:7c2b9d14",
            "Another idea about caching",
        ],
    );
    assert_eq!(err["code"], "id_ambiguous");
    let message = err["error"].as_str().unwrap();
    assert!(
        message.contains(SESSION) && message.contains(&longer),
        "{message}"
    );
    assert_eq!(ok_json(&fx, &["entry", "list"])["total"], before);
}

#[test]
fn literal_session_references_remain_filterable_and_removable_after_cataloging() {
    let fx = Fixture::new();
    fx.seed();
    let literal = ok_json(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "--related",
            "session:abcd",
            "Keep this reference until the session is known",
        ],
    );
    let id = literal["id"].as_str().unwrap();
    let expected = support::session_id_outcome("related_literal");
    assert_eq!(expected["exit"], 0); // ok_json observed exit 0
    assert!(expected["code"].is_null());
    assert_eq!(literal["related"][0], expected["value"]);

    // Make a longer matching session known only after the reference was
    // written. This is the ordering that turns resolution into a regression.
    copy_session(&fx, "abcd1234");
    let filtered = ok_json(&fx, &["entry", "list", "--related", "session:abcd"]);
    assert!(ids(&filtered, "entries", "id").contains(&id.to_owned()));

    let removed = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--remove-related",
            "session:abcd",
        ],
    );
    assert!(removed["related"].as_array().unwrap().is_empty());
    assert!(
        !ids(
            &ok_json(&fx, &["entry", "list", "--related", "session:abcd"]),
            "entries",
            "id"
        )
        .contains(&id.to_owned())
    );

    // Fully canonical references keep the established add, filter, and
    // removal behavior.
    let canonical = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--add-related",
            "session:abcd1234",
        ],
    );
    assert_eq!(canonical["related"][0], "session:abcd1234");
    assert!(
        ids(
            &ok_json(&fx, &["entry", "list", "--related", "session:abcd1234"]),
            "entries",
            "id"
        )
        .contains(&id.to_owned())
    );
    let removed = ok_json(
        &fx,
        &[
            "entry",
            "update",
            id,
            "--author",
            "opus",
            "--remove-related",
            "session:abcd1234",
        ],
    );
    assert!(removed["related"].as_array().unwrap().is_empty());
}

#[test]
fn session_id_matrix_rows_have_boundary_cases() {
    let rows: Vec<Value> =
        serde_json::from_str(include_str!("../../../scripts/session-id-outcomes.json")).unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "related_literal",
            "manual_link",
            "session_pass",
            "instruction_missing",
            "instruction_prefix",
            "instruction_ambiguous"
        ]
    );
}
