//! End-to-end tests of `scribe plugin call`, the Orbit exec backend
//! (docs/design/scribe/2_design.md §1, §10 S5): every outcome other than a
//! routine tool's report is one error envelope with exit 0. The manifest's
//! contract tests are compiled in from the plugin tree.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/plugin.rs"]
mod subject;
mod support;

use subject::*;

#[path = "../../../plugin/.orbit-plugin/tests/conformance.rs"]
mod conformance;
use serde_json::json;

#[test]
fn unknown_tools_are_refused() {
    let s = Setup::new();
    let reply = s.reply("scribe.delete-everything", json!({}), s.root_config());
    assert_eq!(error_code(&reply), "unknown_tool");
}

#[test]
fn the_routine_tools_refuse_any_input_key() {
    let s = Setup::new();
    for tool in [
        "scribe.consistency-scan",
        "scribe.memory-refresh-prepare",
        "scribe.memory-refresh-apply",
    ] {
        let reply = s.reply(tool, json!({"limit": 1}), s.root_config());
        assert_eq!(error_code(&reply), "invalid_input", "{tool}");
    }
    let pass_cfg =
        json!({"root": s.fx.root(), "transcripts": s.fx.home().join(".claude/projects")});
    let invalid = s.reply("scribe.session-pass-prepare", json!({"limit": 1}), pass_cfg);
    assert_eq!(error_code(&invalid), "invalid_input");
    // Store-backed memory-refresh-prepare -> retry -> apply -> nothing_new round-trip.
    let cfg = json!({"root": s.fx.root().to_str().unwrap(), "profile_refresh_crew": "haiku"});
    let prepared = s.reply("scribe.memory-refresh-prepare", json!({}), cfg.clone());
    assert_eq!(prepared["ok"], true, "{prepared}");
    assert_eq!(prepared["output"]["nothing_new"], false, "{prepared}");
    assert_eq!(prepared["output"]["crew"], "haiku", "{prepared}");
    let out = &prepared["output"];
    // Refused shape with allow_retry: true returns {status: "retry", correction}.
    let retry = s.reply(
        "scribe.memory-refresh-apply",
        json!({
            "basis": out["basis"],
            "cursor_to": out["cursor_to"],
            "session_ids": out["session_ids"],
            "backlog": out["backlog"],
            "passed_stems": out["passed_stems"],
            "answer": {"profile": "not valid headings", "milestones": []},
            "allow_retry": true
        }),
        cfg.clone(),
    );
    assert_eq!(retry["ok"], true, "{retry}");
    assert_eq!(retry["output"]["status"], "retry", "{retry}");
    assert_eq!(retry["output"]["code"], "profile_shape", "{retry}");
    assert!(retry["output"]["correction"].is_string(), "{retry}");
    // Valid fixed answer commits revision 1 and advances the cursor.
    let applied = s.reply(
        "scribe.memory-refresh-apply",
        json!({
            "basis": out["basis"],
            "cursor_to": out["cursor_to"],
            "session_ids": out["session_ids"],
            "backlog": out["backlog"],
            "passed_stems": out["passed_stems"],
            "answer": {
                "profile": "## Role and work\n\n## Current focus\n\n## Projects\n- scribe.\n\n## Goals\n\n## Environment\n\n## How they work\n\n## Pinned\n\n",
                "milestones": []
            },
            "allow_retry": false
        }),
        cfg.clone(),
    );
    assert_eq!(applied["ok"], true, "{applied}");
    assert_eq!(applied["output"]["status"], "written", "{applied}");
    assert_eq!(applied["output"]["revision"], 1, "{applied}");
    // Second prepare sees nothing new.
    let again = s.reply("scribe.memory-refresh-prepare", json!({}), cfg);
    assert_eq!(again["ok"], true, "{again}");
    assert_eq!(again["output"]["nothing_new"], true, "{again}");
    assert_eq!(again["output"]["crew"], "haiku", "{again}");
}

#[test]
fn a_malformed_envelope_is_one_error_envelope() {
    let s = Setup::new();
    assert_eq!(error_code(&s.reply_raw(b"not json")), "invalid_envelope");
    assert_eq!(
        error_code(
            &s.reply_raw(
                br#"{"schema_version":2,"tool":"scribe.session-pass-prepare","input":{}}"#
            )
        ),
        "invalid_envelope"
    );
}
