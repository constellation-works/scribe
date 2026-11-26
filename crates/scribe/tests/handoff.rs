//! End-to-end tests of agent-written session handoffs
//! (docs/design/scribe/2_design.md §16): the shipped `handoff` tag,
//! verbatim quote check against a human-typed turn in the target session,
//! fixed six headings in order and size limit, off/unrecorded sessions,
//! `session show` and `brief` rendering, `#scribe retire`, `search`
//! agent-note attribution, and pass isolation (no memory
//! item or proposed claim from handoff text).
//!
//! Every run is the built binary with a temp `HOME` and data root.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/handoff.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::Input::Cli;
use crate::support::command::Text;
use crate::support::command::with_env;
use serde_json::{Value, json};
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, text};

#[test]
fn handoff_requires_verbatim_user_quote_and_session_ref_and_works_for_off_unrecorded_session() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Please write a handoff for this session before we pause.";
    tr.write(&[
        ("u-1", "user", "Let's check the store schema."),
        (
            "a-1",
            "assistant",
            "I can write a handoff for you right now.",
        ),
        ("u-2", "user", request),
    ]);

    // Shipped `handoff` tag is visible in `tag show` as a non-action tag with no capture rules.
    let shown_tag = json_of(&fx.check(Cli(&["tag", "show", "handoff", "--json"]), Success));
    assert_eq!(shown_tag["tag"], "handoff");
    assert_eq!(shown_tag["action"], Value::Null);
    assert_eq!(shown_tag["capture"]["route"], Value::Null);

    let body = valid_handoff("Schema check done.", "Run the integration suite.");
    let rel = format!("session:{OFF_SESSION}");

    // Refused: missing --quote.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            &body,
        ],
    );
    assert_eq!(code, "quote_required");

    // Refused: missing --related session:<id>.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--quote",
            request,
            &body,
        ],
    );
    assert_eq!(code, "quote_required");

    // Refused: non-verbatim quote.
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            "Write a handoff before pausing please.",
            &body,
        ],
    );
    assert_eq!(code, "quote_not_found");

    // Refused: quote from an assistant turn (not a human-typed turn).
    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            "I can write a handoff for you right now.",
            &body,
        ],
    );
    assert_eq!(code, "quote_not_found");

    // Accepted: verbatim quote from user's turn in an off, unrecorded session.
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &body,
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");
    assert_eq!(added["tag"], "handoff");
    assert_eq!(added["author"]["kind"], "agent");
    assert_eq!(added["author"]["name"], "opus");
    assert_eq!(added["quote"], request);
    assert_eq!(added["related"], json!([rel]));

    // Writing a handoff did NOT switch the session on or create a session record.
    assert!(
        !support::has_control(&fx.root()),
        "writing a handoff must not add a session switch"
    );
    assert!(
        support::record_text(&fx.root(), OFF_SESSION).is_none(),
        "writing a handoff must not create a session record"
    );
    let listed_sessions = json_of(&fx.check(
        Cli(&["session", "list", "--include-inactive", "--json"]),
        Success,
    ));
    assert!(listed_sessions["sessions"].as_array().unwrap().is_empty());

    // `session pass` has nothing due for an off session with only a handoff.
    let pass_out = json_of(&fx.check(Cli(&["session", "pass", "--json"]), Success));
    assert!(pass_out["passes"].as_array().unwrap().is_empty());
    assert_eq!(pass_out["remaining"], 0);
}

#[test]
fn handoff_enforces_fixed_headings_order_and_size_limit_naming_the_fault() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Write a handoff now so the next session can continue.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");

    // Missing heading(s).
    let missing_body = "\
## State
Done.

## In flight
WIP.

## Next
Tests.

## Do not touch
Goldens.

## Pointers
README.md
";
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            missing_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("Waiting on you"), "{msg}");

    // Out-of-order headings.
    let unordered_body = "\
## State
Done.

## Next
Tests.

## In flight
WIP.

## Waiting on you
Nothing.

## Do not touch
Goldens.

## Pointers
README.md
";
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            unordered_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("out of order"), "{msg}");

    // Exceeds 3000-character limit.
    let huge = valid_handoff(&"x".repeat(3100), "Continue.");
    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &huge,
        ],
    );
    assert_eq!(code, "handoff_too_long");
    assert!(msg.contains("3000"), "{msg}");

    // Write a valid one, then verify `entry update --content` enforces the same contract.
    let valid = valid_handoff("Initial state.", "Next step.");
    let added = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--quote",
            request,
            &valid,
            "--json",
        ],
        Success,
    ));
    assert_eq!(added["id"], "handoff-1");

    let (code, msg) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--content",
            missing_body,
        ],
    );
    assert_eq!(code, "handoff_shape");
    assert!(msg.contains("Waiting on you"), "{msg}");

    let (code, _) = refused(
        &fx,
        &[
            "entry",
            "update",
            "handoff-1",
            "--author",
            "opus",
            "--content",
            &huge,
        ],
    );
    assert_eq!(code, "handoff_too_long");
}

#[test]
fn session_show_brief_and_retire_handle_multiple_handoffs_on_recorded_and_off_sessions() {
    let fx = Fixture::new();

    // 1. An off session with no record gets two handoffs (different timestamps).
    let off_tr = Transcript::new(&fx, OFF_SESSION);
    let req1 = "Please write a first handoff for this off session.";
    let req2 = "Please write a second updated handoff for this off session.";
    off_tr.write(&[("u-1", "user", req1), ("u-2", "user", req2)]);
    let off_rel = format!("session:{OFF_SESSION}");

    let h1 = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &off_rel,
            "--quote",
            req1,
            &valid_handoff("First state.", "First next."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(h1["id"], "handoff-1");
    // Backdate handoff-1 so newest-first ordering is deterministic.
    support::sql(
        &fx.root(),
        "UPDATE entries SET created = '2026-11-01T10:00:00Z', updated = '2026-11-01T10:00:00Z' WHERE id = 'handoff-1'",
    );

    let h2 = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "sonnet",
            "--project",
            "scribe",
            "--related",
            &off_rel,
            "--quote",
            req2,
            &valid_handoff("Second state.", "Second next."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(h2["id"], "handoff-2");
    support::sql(
        &fx.root(),
        "UPDATE entries SET created = '2026-11-01T11:00:00Z', updated = '2026-11-01T11:00:00Z' WHERE id = 'handoff-2'",
    );

    // `session show` works for the off session (by full id and unique prefix) and lists newest first.
    let shown_off = json_of(&fx.check(Cli(&["session", "show", OFF_SESSION, "--json"]), Success));
    assert_eq!(shown_off["id"], OFF_SESSION);
    assert_eq!(shown_off["has_record"], false);
    assert_eq!(shown_off["on"], false);
    let off_handoffs = shown_off["handoffs"].as_array().unwrap();
    assert_eq!(off_handoffs.len(), 1);
    assert_eq!(off_handoffs[0]["id"], "handoff-2");
    assert_eq!(off_handoffs[0]["author"]["name"], "sonnet");
    assert_eq!(off_handoffs[0]["created"], "2026-11-01T11:00:00Z");
    let old = json_of(&fx.check(&["show", "handoff-1", "--json"], Success));
    assert_eq!(old["status"], "superseded");
    assert_eq!(old["superseded_by"], "handoff-2");
    let superseded = json_of(&fx.check(
        &[
            "list",
            "--tag",
            "handoff",
            "--status",
            "superseded",
            "--json",
        ],
        Success,
    ));
    assert_eq!(superseded["total"], 1);
    let active = json_of(&fx.check(
        &["list", "--tag", "handoff", "--status", "active", "--json"],
        Success,
    ));
    assert_eq!(active["total"], 1);

    let shown_prefix = json_of(&fx.check(
        Cli(&["session", "show", &OFF_SESSION[..8], "--json"]),
        Success,
    ));
    assert_eq!(shown_prefix["id"], OFF_SESSION);

    // Table and plain rendering include author, timestamp, and newest-first order.
    let table_off = fx.check(
        Cli(&["session", "show", OFF_SESSION, "--format", "table"]),
        Text,
    );
    assert!(table_off.contains("handoff-2"));
    assert!(!table_off.contains("handoff-1"));
    assert!(
        table_off.contains("sonnet") && table_off.contains("2026-11-01T11:00:00Z"),
        "{table_off}"
    );

    // `--segment` on a handoff-only session is refused with segment_not_found.
    let (code, _) = refused(&fx, &["session", "show", OFF_SESSION, "--segment", "1"]);
    assert_eq!(code, "segment_not_found");

    // 2. A recorded session with a handoff.
    let on_tr = Transcript::new(&fx, ON_SESSION);
    let on_req = "Please save a handoff for this recorded session too.";
    on_tr.write(&[
        ("u-1", "user", "Let's refactor the brief builder."),
        ("a-1", "assistant", "Done refactoring."),
        ("u-2", "user", on_req),
    ]);
    on_tr.prompt(&fx, "/scribe on");
    let answer = json!({
        "title": "Brief refactor",
        "summary": "Refactored brief builder.",
        "goals": "Surface handoffs in brief.",
        "open": null,
        "new": [{"starts_at": "u-1", "heading": "Brief work", "text": "Refactored brief builder.", "topics": [], "related": []}],
        "next": ["Write tests."],
        "topics": ["brief"],
        "related": [],
        "terms": [],
        "candidates": [],
        "memory": []
    });
    let pass = support::pass::run(
        |step, input| support::pass::cli_step(&fx, &[], step, input),
        json!({"sessions": [ON_SESSION]}),
        |_, _| Ok(answer.clone()),
    )
    .unwrap();
    assert_eq!(pass["passes"][0]["result"], "written");

    let on_rel = format!("session:{ON_SESSION}");
    let h3 = json_of(&fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &on_rel,
            "--quote",
            on_req,
            &valid_handoff("Recorded session state.", "Recorded next."),
            "--json",
        ],
        Success,
    ));
    assert_eq!(h3["id"], "handoff-3");

    // `session show` on the recorded session shows both scribe's header and the handoff.
    let shown_on = json_of(&fx.check(Cli(&["session", "show", ON_SESSION, "--json"]), Success));
    assert_eq!(shown_on["title"], "Brief refactor");
    assert_eq!(shown_on["summary"], "Refactored brief builder.");
    assert_eq!(shown_on["handoffs"][0]["id"], "handoff-3");
    assert_eq!(shown_on["handoffs"][0]["author"]["name"], "opus");

    // `scribe brief` marks the recorded session AND includes the unrecorded off session.
    let brief_json = json_of(&fx.check(Cli(&["brief", "--project", "scribe", "--json"]), Success));
    let sessions = brief_json["sessions"].as_array().unwrap();
    let rec = sessions
        .iter()
        .find(|s| s["id"] == ON_SESSION)
        .expect("recorded session in brief");
    assert_eq!(rec["has_record"], true);
    assert_eq!(rec["handoff"], "handoff-3");
    assert_eq!(rec["handoffs"], 1);
    let off = sessions
        .iter()
        .find(|s| s["id"] == OFF_SESSION)
        .expect("off session in brief");
    assert_eq!(off["has_record"], false);
    assert_eq!(off["handoff"], "handoff-2");
    assert_eq!(off["handoffs"], 1);

    let brief_text = fx.check(Cli(&["brief", "--project", "scribe"]), Text);
    assert!(brief_text.contains("[handoff: handoff-3]"), "{brief_text}");
    assert!(
        brief_text.contains("no session record [handoff: handoff-2]"),
        "{brief_text}"
    );

    // `hook session-start` also surfaces the handoff marker in its brief context.
    let start_tr = Transcript::new(&fx, "7f3e2d90-4b6c-4e51-8a83-2c1d0e9f8a71");
    let start_reply = start_tr.session_start(&fx);
    let ctx = start_reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(ctx.contains("[handoff: handoff-3]"), "{ctx}");
    assert!(ctx.contains("[handoff: handoff-2]"), "{ctx}");

    // `#scribe retire handoff-2` removes the newest handoff from `session show` and updates `brief`.
    let retire_reply = off_tr.prompt(&fx, "#scribe retire handoff-2");
    assert!(
        retire_reply["systemMessage"]
            .as_str()
            .unwrap_or_default()
            .contains("retired"),
        "{retire_reply}"
    );
    let (code, _) = refused(&fx, &["session", "show", OFF_SESSION]);
    assert_eq!(code, "session_not_found");

    let brief_after = json_of(&fx.check(Cli(&["brief", "--project", "scribe", "--json"]), Success));
    assert!(
        !brief_after["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == OFF_SESSION)
    );
}

#[test]
fn search_and_ask_treat_handoff_as_agent_note_never_users_words() {
    let fx = Fixture::new();
    let tr = Transcript::new(&fx, OFF_SESSION);
    let request = "Write a handoff about the widget migration before stopping.";
    tr.write(&[("u-1", "user", request)]);
    let rel = format!("session:{OFF_SESSION}");

    let body = valid_handoff(
        "Widget migration phase one is complete on branch widget-v2.",
        "Run the widget smoke test and merge.",
    );
    fx.check(
        &[
            "add",
            "--tag",
            "handoff",
            "--author",
            "opus",
            "--project",
            "scribe",
            "--related",
            &rel,
            "--mode",
            "new",
            "--quote",
            request,
            &body,
            "--json",
        ],
        Success,
    );

    // `scribe search` finds the handoff as an agent-authored entry.
    let found = json_of(&fx.check(Cli(&["search", "widget-v2", "--json"]), Success));
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["id"], "handoff-1");
    assert_eq!(hits[0]["author"]["kind"], "agent");
    assert_eq!(hits[0]["author"]["name"], "opus");
    assert!(
        hits[0]["snippet"]
            .as_str()
            .unwrap()
            .contains("handoff mode: new")
    );
}

include!("handoff/session_references.rs");
include!("handoff/refresh.rs");
include!("handoff/prefixes.rs");
