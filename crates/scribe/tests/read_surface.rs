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
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/read_surface.rs"]
mod subject;
mod support;

use crate::support::command::Exit::{Capture, Success};
use crate::support::command::Input::Cli;
use serde_json::Value;
use subject::*;
use support::command::with_env;
use support::{Fixture, snapshot, text};

#[test]
fn brief_reads_only_and_calls_no_model() {
    let fx = Fixture::seeded();
    let called = fx.elsewhere("claude-called");
    let path = fx.fake_program(
        "claude",
        &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", called.display()),
    );
    fx.check(Cli(&["session", "list", "--json"]), Success); // the first open creates the WAL
    let before = snapshot(&fx.root());
    let out = fx.check(
        with_env(
            &["brief", "--project", "constellation-works/scribe", "--json"],
            "PATH",
            &path,
        ),
        Success,
    );
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
    let fx = Fixture::seeded();
    fx.check(Cli(&["session", "list", "--json"]), Success); // the first open creates the WAL
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
    let fx = Fixture::seeded();
    let out = fx.check(&["search", "merge commit", "--json"], Success);
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
    let fx = Fixture::seeded();
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
    let none = fx.check(&["search", "merge", "--project", "orbit"], Success);
    assert!(none.stdout.is_empty());
    assert!(text(&none.stderr).contains("no records match"));
    let cut = ok_json(&fx, &["search", "merge", "--limit", "1"]);
    assert_eq!(cut["hits"].as_array().unwrap().len(), 1);
    assert_eq!(cut["truncated"], true);
    assert!(cut["total"].as_u64().unwrap() > 1);
}

#[test]
fn search_tag_keeps_one_tags_entries_combined_with_the_other_filters() {
    let fx = Fixture::seeded();
    // `rituals` is an unknown legacy tag, so its literal value continues to
    // filter the stored entry. The entry is retired and appears only when
    // hidden records are requested.
    let visible = fx.check(&["search", "full gate", "--tag", "rituals"], Success);
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
    let other_project = fx.check(
        &[
            "search",
            "full gate",
            "--tag",
            "rituals",
            "--project",
            "constellation-works/scribe",
            "--include-hidden",
        ],
        Success,
    );
    assert!(other_project.stdout.is_empty());
    // The echoed criteria name the tag, and no message count is offered for
    // a tag: messages carry none.
    let none = fx.check(&["search", "full gate", "--tag", "unknown-tag"], Success);
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
    let fx = Fixture::seeded();
    let canonical = ok_json(&fx, &["search", "commit", "--tag", "preferences"]);
    let alias = ok_json(&fx, &["search", "commit", "--tag", "pref"]);
    assert_eq!(ids(&canonical, "hits", "cite"), ["preferences-1"]);
    assert_eq!(ids(&alias, "hits", "cite"), ["preferences-1"]);
    assert_eq!(alias["total"], 1);
}

#[test]
fn search_tag_refuses_a_session_or_message_kind_in_one_line() {
    let fx = Fixture::seeded();
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
        let out = fx.check(&args, Capture);
        assert_eq!(out.status.code(), Some(2), "{kind}: {}", text(&out.stderr));
        assert!(out.stdout.is_empty());
        let line = text(&out.stderr);
        assert_eq!(line.trim_end().lines().count(), 1, "{line}");
    }
}

#[test]
fn search_refuses_terms_with_no_searchable_word_and_bad_dates() {
    let fx = Fixture::seeded();
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
    let fx = Fixture::seeded();
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
    fx.check(
        &[
            "entry",
            "add",
            "--tag",
            "preferences",
            "--author",
            "opus",
            "agent note on merges",
        ],
        Success,
    );
    let agent = list(&["entry", "list", "--author-kind", "agent"]);
    assert_eq!(agent.len(), 1);
    assert_eq!(list(&["entry", "list", "--author", "opus"]), agent);
    let empty = fx.check(
        &[
            "entry",
            "list",
            "--status",
            "active",
            "--since",
            "2027-01-01",
        ],
        Success,
    );
    assert!(text(&empty.stderr).contains("--since 2027-01-01"));
}

#[test]
fn entry_list_tag_alias_selects_its_configured_tag() {
    let fx = Fixture::seeded();
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
    let fx = Fixture::seeded();
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
    let fx = Fixture::seeded();
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

include!("read_surface/id_resolution.rs");
