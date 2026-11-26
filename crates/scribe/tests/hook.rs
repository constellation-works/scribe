//! End-to-end tests of `scribe hook`, the Claude Code hook entry points:
//! `#scribe` markers echoed from a fixture `UserPromptSubmit` payload and
//! handed to the agent at `Stop` (docs/design/scribe/2_design.md
//! §14), settling lines (§4.4), the reply shown to Daniel and the agent, the
//! transcript provenance check, the audit lines, the `Stop` hook filling an
//! older entry's cite message uuid (§4.1, §10 S2), and the `SessionStart`
//! reminder to keep recording a continuation (§5.4, §7).
//!
//! Every run is the built binary with a temp `HOME` whose
//! `.claude/projects/` holds the fixture transcript; nothing touches the
//! real home.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "support/hook.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use crate::support::command::Json;
use serde_json::{Value, json};
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, make_private, snapshot, text};

#[test]
fn each_scribe_line_of_the_fixture_prompt_is_a_marker_handed_to_the_agent_at_stop() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "user-prompt-submit", &payload, &path);

    let reply_json = reply(&out);
    assert_eq!(
        reply_json["systemMessage"],
        "scribe: marked 2 times — cataloged after the reply"
    );
    assert_eq!(
        reply_json["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    assert_eq!(
        reply_json["hookSpecificOutput"]["additionalContext"],
        "scribe: marked 2 times — cataloged after the reply (after your reply scribe hands this #scribe marker to you with the command that records it; record nothing for it before then)"
    );
    assert!(
        reply_json.get("decision").is_none(),
        "the prompt is never blocked"
    );
    assert!(claude_calls(&fx).is_empty(), "no model call at submit");
    assert!(!fx.root().exists(), "nothing written at submit");

    // Stop: the turn's two line-start markers go to the agent, each with
    // its tag as the hint. Fenced, quoted, pasted and bare lines are not
    // markers.
    let prompt = payload["prompt"].as_str().unwrap();
    marked_turns(&claude, &[("u-1", prompt)]);
    let first = "#scribe preferences: squash-merge agent PRs; keep merge commits for releases #merge-policy";
    let second = "#scribe add short commit subjects to my preferences";
    assert!(prompt.contains(first) && prompt.contains(second));
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let reply_json = reply(&out);
    assert_eq!(
        reply_json["systemMessage"],
        format!(
            "scribe: #scribe in your message is the agent's to record — \"{first}\"\n\
             scribe: #scribe in your message is the agent's to record — \"{second}\""
        )
    );
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 2, "{context}");
    assert!(lines[0].contains(&format!("\"{first}\"")), "{context}");
    assert!(
        lines[0].contains(
            "scribe entry add --tag preferences --project scribe --topic merge-policy --author <name>"
        ),
        "the tag hint, the cwd's basename outside git, the topic: {context}"
    );
    assert!(
        lines[1].contains("scribe entry add --tag preferences --project scribe --author <name>"),
        "{context}"
    );
    assert!(!context.contains("never recorded"), "{context}");
    assert!(claude_calls(&fx).is_empty(), "no model call at Stop");
    assert!(entries(&fx).is_empty(), "no entry is written at Stop");
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 2, "{marks:?}");
    assert!(
        marks
            .iter()
            .all(|m| m["outcome"] == "refused" && m["code"] == "agent_handoff"),
        "{marks:?}"
    );
}

#[test]
fn a_prompt_without_an_instruction_line_writes_and_prints_nothing() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "```\n#scribe preferences: fenced\n```\n> #scribe preferences: quoted\n\
         <pasted_content>\n#scribe preferences: pasted\n</pasted_content>\n\
         #decision bare marker\n#preferences another\nplain `#scribe` mention\n\
         a \"#scribe\" in quotes"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "no reply: {}", text(&out.stdout));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    assert!(!fx.root().exists(), "the store is not even created");
}

#[test]
fn agent_originated_wrappers_record_and_decide_nothing() {
    let wrappers = [
        "<agent-message from=\"other-session\">\n#scribe preferences: agent wrote this\n</agent-message>",
        "<task-notification>\n<task-id>t-1</task-id>\n#scribe preferences: agent wrote this\n</task-notification>",
        "<side-sessions-event>\n#scribe preferences: agent wrote this\n</side-sessions-event>",
        "<scheduled-loop>\n#scribe preferences: agent wrote this\n</scheduled-loop>",
        "<scheduled-prompt>\n#scribe preferences: agent wrote this\n</scheduled-prompt>",
        "<loop-prompt>\n#scribe preferences: agent wrote this\n</loop-prompt>",
    ];
    for prompt in wrappers {
        let fx = Fixture::new();
        let claude = Claude::new(&fx);
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!(prompt);
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "wrapper {prompt:?}: {}",
            text(&out.stdout)
        );
        assert!(!fx.root().exists(), "wrapper {prompt:?} created the store");
    }
}

#[test]
fn an_agent_wrapper_cannot_forget_an_entry_or_switch_scribe_on() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: squash-merge agent PRs; keep merge commits for releases",
        "an older prompt",
        1,
        Some("u-0"),
    );
    assert_eq!(entries(&fx).len(), 1);
    let audit_before = audit(&fx).len();

    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "<agent-message from=\"other-session\">\n#scribe forget preferences-1\n</agent-message>"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    let entries = entries(&fx);
    assert_eq!(
        entries[0]["content"],
        "squash-merge agent PRs; keep merge commits for releases"
    );
    assert_eq!(
        audit(&fx).len(),
        audit_before,
        "a skipped turn audits nothing"
    );

    // A non-plain payload source refuses a `/scribe on` first line too.
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("/scribe on");
    payload["source"] = json!("agent");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(
        !support::has_control(&fx.root()),
        "agent turn switched scribe on"
    );
}

#[test]
fn a_non_plain_payload_source_records_nothing() {
    for source in ["scheduled", "task-notification", "agent-message"] {
        let fx = Fixture::new();
        let claude = Claude::new(&fx);
        let mut payload = claude.payload("user-prompt-submit.json");
        payload["prompt"] = json!("#scribe preferences: agent wrote this");
        payload["source"] = json!(source);
        let out = hook_ok(&fx, "user-prompt-submit", &payload);
        assert!(
            out.stdout.is_empty(),
            "source {source}: {}",
            text(&out.stdout)
        );
        assert!(!fx.root().exists(), "source {source} created the store");
    }
}

#[test]
fn a_refused_line_tells_the_agent_to_relay_it_to_daniel_not_that_no_action_is_needed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe/all preferences: hello");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: refused"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to the user)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_failed_line_tells_the_agent_to_relay_it_to_daniel_not_that_no_action_is_needed() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe forget preferences-1");
    let elsewhere = fx.elsewhere(&format!("{SESSION}.jsonl"));
    payload["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: could not record"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to the user)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_refused_approve_tells_the_agent_to_relay_it_to_daniel() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-99");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let message = reply["systemMessage"].as_str().unwrap();
    assert!(message.starts_with("scribe: refused"), "{message}");
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        context,
        format!("{message} (not recorded by scribe; relay this to the user)")
    );
    assert!(!context.contains("no action needed"), "{context}");
}

#[test]
fn a_mixed_prompt_notes_each_line_by_whether_it_was_recorded() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: stand-up at nine",
        "an older prompt",
        1,
        Some("u-0"),
    );
    propose_stale(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!(
        "#scribe approve p-1\n#scribe approve p-99\nstand-up moves to ten #scribe preferences"
    );
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let reply = reply(&out);
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = context.lines().collect();
    assert_eq!(lines.len(), 3, "{context}");
    assert!(
        lines[0].ends_with("(recorded by scribe; no action needed)"),
        "{context}"
    );
    assert!(
        lines[1].starts_with("scribe: refused")
            && lines[1].ends_with("(not recorded by scribe; relay this to the user)"),
        "{context}"
    );
    assert_eq!(
        lines[2],
        "scribe: marked — cataloged after the reply (after your reply scribe hands this #scribe marker to you with the command that records it; record nothing for it before then)"
    );
}

#[test]
fn settlement_replies_describe_search_visibility_for_hidden_entries() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: stand-up at nine",
        "an older prompt",
        1,
        Some("u-0"),
    );
    write_verbatim(
        &fx,
        2,
        "#scribe preferences: stand-up at ten",
        "a newer prompt",
        1,
        Some("u-1"),
    );
    fx.check(
        Cli(&[
            "proposal",
            "add",
            "--kind",
            "supersedes",
            "--older",
            "preferences-1",
            "--newer",
            "preferences-2",
            "--reason",
            "The time changed",
            "--author",
            "opus",
        ]),
        Success,
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-1");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: approved p-1 — preferences-1 is superseded by preferences-2; scribe search hides superseded and retired entries by default; --include-hidden shows them"
    );

    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: stand-up at nine",
        "an older prompt",
        1,
        Some("u-0"),
    );
    propose_stale(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve p-1");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: approved p-1 — preferences-1 is retired; scribe search hides superseded and retired entries by default; --include-hidden shows them"
    );

    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_verbatim(
        &fx,
        1,
        "#scribe preferences: stand-up at nine",
        "an older prompt",
        1,
        Some("u-0"),
    );
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe retire preferences-1");
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    assert_eq!(
        reply(&out)["systemMessage"],
        "scribe: retired preferences-1 — scribe search hides retired entries by default; --include-hidden shows them; scribe entry show preferences-1 still has it"
    );
}

#[test]
fn the_audit_log_holds_ids_and_codes_never_what_was_said() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe approve marmalade");
    hook_ok(&fx, "user-prompt-submit", &payload);
    let typed = "the secret phrase is marmalade #scribe preferences";
    marked_turns(&claude, &[("u-1", typed)]);
    hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let log = support::audit_text(&fx.root());
    for needle in [
        "marmalade",
        "secret phrase",
        "#scribe",
        claude.cwd.to_str().unwrap(),
    ] {
        assert!(!log.contains(needle), "{needle} leaked into {log}");
    }
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(
        codes,
        [json!("unparsed_instruction"), json!("agent_handoff")]
    );
    for line in audit(&fx) {
        let mut keys: Vec<&str> = line
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "at",
                "author",
                "command",
                "duration_ms",
                "error_code",
                "format",
                "instruction_message",
                "instruction_session",
                "issue",
                "status",
                "target"
            ]
        );
    }
}

#[test]
fn a_transcript_outside_claudes_projects_fails_the_provenance_check() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let mut payload = claude.payload("user-prompt-submit.json");
    payload["prompt"] = json!("#scribe forget preferences-1\n#scribe approve p-1");
    let elsewhere = fx.elsewhere(&format!("{SESSION}.jsonl"));
    payload["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_ok(&fx, "user-prompt-submit", &payload);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: could not record \"#scribe forget preferences-1\""),
        "{message}"
    );
    assert!(message.contains("transcript check"), "{message}");
    assert!(entries(&fx).is_empty(), "nothing written");
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(
        codes,
        [json!("provenance_failed"), json!("provenance_failed")]
    );

    // A transcript named for another session fails too.
    let mut other = payload.clone();
    other["transcript_path"] = json!(claude.transcript.to_str().unwrap());
    other["session_id"] = json!("another-session");
    let out = hook_ok(&fx, "user-prompt-submit", &other);
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not named for the payload's session_id")
    );

    // Stop checks the transcript before it catalogs anything.
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = fake_claude(&fx, "exit 1");
    let mut stop = claude.payload("stop.json");
    stop["transcript_path"] = json!(elsewhere.to_str().unwrap());
    let out = hook_path(&fx, "stop", &stop, &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.starts_with("scribe: could not check for #scribe markers — "),
        "{message}"
    );
    assert!(claude_calls(&fx).is_empty());
    assert_eq!(
        audit(&fx).last().unwrap()["error_code"],
        "provenance_failed"
    );
    assert!(entries(&fx).is_empty());
}

#[cfg(unix)]
#[test]
fn a_linked_transcript_fails_the_provenance_check() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let target = fx.elsewhere("real.jsonl");
    std::fs::write(&target, "").unwrap();
    std::os::unix::fs::symlink(&target, &claude.transcript).unwrap();
    let out = hook_ok(
        &fx,
        "user-prompt-submit",
        &prompt_payload(&claude, "#scribe forget preferences-1"),
    );
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not a regular file")
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    assert!(
        reply(&out)["systemMessage"]
            .as_str()
            .unwrap()
            .contains("not a regular file")
    );
    assert!(entries(&fx).is_empty());
}

#[test]
fn a_bad_config_is_reported_in_the_reply_and_nothing_is_written() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    write_config(&fx, "[tags.preferences]\nalias = [\"typo\"]\n");
    marked_turns(&claude, &[("u-1", MARKED)]);
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    let message = reply(&out)["systemMessage"].as_str().unwrap().to_owned();
    assert!(
        message.contains("is not a valid scribe config")
            && message.ends_with("; will retry at the next reply"),
        "{message}"
    );
    assert!(claude_calls(&fx).is_empty());
    // A bad config fails `entry list` itself, so look at the export.
    let export = tempfile::tempdir().unwrap();
    scribe_core::Store::open(fx.root())
        .export(
            &export.path().join("x"),
            scribe_core::ExportFormat::Markdown,
        )
        .unwrap();
    assert!(
        std::fs::read_dir(export.path().join("x/entries")).map_or(true, |mut d| d.next().is_none()),
        "no entry written"
    );
    assert!(marks_json(&fx).is_empty(), "the marker stays unsettled");
    let codes: Vec<Value> = audit(&fx).iter().map(|l| l["error_code"].clone()).collect();
    assert_eq!(codes, [json!("config_invalid")]);
}

#[test]
fn a_payload_that_is_not_hook_json_fails_with_a_stable_code() {
    let fx = Fixture::new();
    let out = run_with_stdin(
        &fx,
        &["hook", "user-prompt-submit", "--json"],
        b"{\"prompt\": 3}",
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "hook_payload_invalid");
}

#[test]
fn the_stop_hook_fills_the_cite_message_uuid_from_the_transcript() {
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let payload = claude.payload("user-prompt-submit.json");
    let prompt = payload["prompt"].as_str().unwrap();
    write_fixture_entries(&fx, prompt);
    claude.write_transcript(prompt, "u-prompt");

    // The lines already hold entries, so they are not cataloged again.
    let path = fake_claude(&fx, "exit 1");
    let out = hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert!(out.stdout.is_empty(), "{}", text(&out.stdout));
    assert!(claude_calls(&fx).is_empty(), "no line cataloged twice");
    let entries = entries(&fx);
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry["cite"]["message"], "u-prompt", "{}", entry["id"]);
    }
    let lines = audit(&fx);
    let filled: Vec<&Value> = lines.iter().map(|l| &l["target"]).collect();
    assert_eq!(filled, [&json!("preferences-1"), &json!("preferences-2")]);
    assert!(lines.iter().all(|l| l["command"] == "hook stop"));

    // Nothing waits any more: the next Stop changes nothing at all.
    let before = snapshot(&fx.root());
    hook_path(&fx, "stop", &claude.payload("stop.json"), &path);
    assert_eq!(snapshot(&fx.root()), before);
}

#[test]
fn a_line_recorded_verbatim_in_one_block_of_a_message_is_not_cataloged_again() {
    // The prompt hook may have had one text block, not the joined message:
    // its cite lines count from that block.
    let fx = Fixture::new();
    let claude = Claude::new(&fx);
    let block = "#scribe preferences: use rg\nuse fd for finding files #scribe preferences";
    write_verbatim(&fx, 1, "#scribe preferences: use rg", block, 1, Some("u-1"));
    write_lines(
        &claude,
        &[
            json!({"type": "session-owner", "sessionId": SESSION}),
            json!({"type": "user", "uuid": "u-1", "sessionId": SESSION, "timestamp": now_rfc3339(),
                   "message": {"role": "user", "content": [
                       {"type": "text", "text": "Two tools."},
                       {"type": "text", "text": block}]}}),
            json!({"type": "assistant", "uuid": "a-1", "sessionId": SESSION, "timestamp": now_rfc3339(),
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "Noted."}]}}),
        ],
    );
    let out = hook_ok(&fx, "stop", &claude.payload("stop.json"));
    let reply_json = reply(&out);
    let echo = reply_json["systemMessage"].as_str().unwrap().to_owned();
    assert_eq!(
        echo,
        "scribe: #scribe in your message is the agent's to record — \"use fd for finding files #scribe preferences\""
    );
    let context = reply_json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(!context.contains("use rg"), "{context}");
    assert!(context.contains("--tag preferences"), "{context}");
    let marks = marks_json(&fx);
    assert_eq!(marks.len(), 1, "{marks:?}");
    assert_eq!(marks[0]["marker"], 2);
    assert_eq!(marks[0]["code"], "agent_handoff");
    assert_eq!(entries(&fx).len(), 1);
}

include!("hook/stop.rs");
include!("hook/session_start.rs");
include!("hook/desktop.rs");
