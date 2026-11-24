//! End-to-end tests of S4 through the built binary
//! (docs/design/scribe/2_design.md §5, §10 S4): `/scribe on|off|status`
//! switching a session and blocking the prompt, the `Stop` and `SessionEnd`
//! hooks marking a pass due without a model, and `scribe session pass`
//! writing one record per due session through a fake `claude`.
//!
//! Every run is the built binary with a temp `HOME` whose
//! `.claude/projects/` holds the test transcripts; nothing touches the real
//! home.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use support::{Fixture, context_of, make_private, text};

const ON: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const NEVER: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e60";
const DEFINED: &str = "by graduate to level 4 I mean a crew that merges without review";
const NEVER_SAID: &str = "a session never switched on, which scribe must never read";

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
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let proposals: Value =
        serde_json::from_slice(&s.fx.ok(&["proposal", "list", "--json"]).stdout).unwrap();
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
    let entries: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "list", "--tag", "preferences", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entries["entries"].as_array().unwrap().len(), 1);
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
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
    let plain = text(&s.fx.ok(&["entry", "show", "preferences-1"]).stdout);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("admitted\tI prefer\tI prefer\t{criteria}")),
        "{plain}"
    );
    let table = text(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains(&format!(
            "admitted    trigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}"
        )),
        "{table}"
    );
    let listed = text(&s.fx.ok(&["proposal", "list"]).stdout);
    assert!(
        listed.ends_with(&format!(
            "\ttrigger \"I prefer\" matched \"I prefer\"; criteria: {criteria}\t-\n"
        )),
        "{listed}"
    );
    // An entry not approved from a claim has no admission.
    let typed: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "Keep it short.",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    let id = typed["id"].as_str().unwrap();
    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", id, "--json"]).stdout).unwrap();
    assert_eq!(shown["admitted"], Value::Null);
}

/// Marks claim `n` approved by Daniel without writing its entry: the
/// journal an approval leaves durable before the entry is written.
fn approve_claim_in_db(root: &std::path::Path, n: i64) {
    support::sql(
        root,
        &format!(
            "UPDATE claims SET state = 'approved', settled_at = '2026-11-15T23:00:00Z',
                 settled_author = '{{\"kind\":\"daniel\",\"name\":\"daniel\"}}',
                 settled_by = c_cite WHERE n = {n};"
        ),
    );
}

/// Stages claims in the shape `proposals.json` held them (`id: "p-N"`,
/// `candidate: {..}`) as rows of `claims`. The store must already have had
/// a write: a `proposals.json` seeded after that is ignored, the documents
/// having moved into `scribe.db`. Before the db exists, the claims are
/// seeded as the `proposals.json` the first write then imports.
fn seed_claims(root: &std::path::Path, proposals: &[Value]) {
    if !root.join("scribe.db").exists() {
        if !root.exists() {
            std::fs::create_dir(root).unwrap();
            make_private(root);
        }
        let file = root.join("proposals.json");
        std::fs::write(
            &file,
            json!({"format": 11, "proposals": proposals}).to_string(),
        )
        .unwrap();
        make_private(&file);
        return;
    }
    let conn = rusqlite::Connection::open(root.join("scribe.db")).expect("open scribe.db");
    let json_text = |v: &Value| -> Option<String> { (!v.is_null()).then(|| v.to_string()) };
    let plain = |v: &Value| -> Option<String> { v.as_str().map(str::to_owned) };
    for p in proposals {
        let n: i64 = p["id"]
            .as_str()
            .unwrap()
            .trim_start_matches("p-")
            .parse()
            .unwrap();
        let c = &p["candidate"];
        let has = i64::from(c.is_object());
        conn.execute(
            "INSERT INTO claims (n, kind, older, newer, reason, quotes, state, opened_at,
                 settled_at, settled_by, author, updated_by, settled_author, closed_reason,
                 instruction, has_candidate, c_tag, c_role, c_quote, c_context, c_statement,
                 c_subject, c_evidence, c_cite, c_project, c_provider, c_host, c_admitted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                 ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)",
            rusqlite::params![
                n,
                plain(&p["kind"]),
                plain(&p["older"]),
                plain(&p["newer"]),
                plain(&p["reason"]),
                p.get("quotes")
                    .map_or_else(|| "[]".to_owned(), Value::to_string),
                plain(&p["state"]),
                plain(&p["opened_at"]),
                plain(&p["settled_at"]),
                json_text(&p["settled_by"]),
                p["author"].to_string(),
                json_text(&p["updated_by"]),
                json_text(&p["settled_author"]),
                plain(&p["closed_reason"]),
                json_text(&p["instruction"]),
                has,
                plain(&c["tag"]),
                plain(&c["role"]),
                plain(&c["quote"]),
                plain(&c["context"]),
                plain(&c["statement"]),
                plain(&c["subject"]),
                plain(&c["evidence"]),
                json_text(&c["cite"]),
                plain(&c["project"]),
                plain(&c["provider"]),
                plain(&c["host"]),
                json_text(&c["admitted"]),
            ],
        )
        .expect("insert claim");
    }
}

fn setup_stored_assistant_finding_candidate(quote: &str) -> Setup {
    setup_stored_assistant_finding_candidate_with(quote, "exp-5342a4d6")
}

/// The pass has run; the stored claim is then a row of `claims` (a
/// `proposals.json` seeded now would be ignored, the documents having moved
/// into `scribe.db`).
fn setup_stored_assistant_finding_candidate_with(quote: &str, evidence: &str) -> Setup {
    let s = Setup::new();
    s.write(
        ON,
        &[
            line("user", "u-1", "Please measure the reviewer mix effect."),
            line("assistant", "a-find", quote),
        ],
    );
    s.answer(&answer());
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let cite = json!({"session": ON, "message": "a-find", "at": "2026-11-03T06:00:00Z",
        "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/nonexistent/work/scribe"});
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    conn.execute(
        "INSERT INTO claims (n, kind, older, newer, reason, quotes, state, opened_at, author,
             has_candidate, c_tag, c_role, c_quote, c_evidence, c_cite, c_project, c_provider, c_host)
         VALUES (1, 'candidate', 'finding-1', NULL, 'Assistant finding statement', '[]', 'open',
             '2026-11-03T06:01:00Z', ?1, 1, 'finding', 'assistant', ?2, ?3, ?4, 'scribe',
             'claude-code', 'host')",
        rusqlite::params![
            json!({"kind": "scribe", "name": "session-pass"}).to_string(),
            quote,
            evidence,
            cite.to_string()
        ],
    )
    .unwrap();
    s
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
    let entries: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "list", "--tag", "finding", "--json"])
            .stdout,
    )
    .unwrap();
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

    let listed: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "list", "--tag", "finding", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(listed["warnings"], json!([]));
    assert_eq!(listed["total"], 1);
    let listed_entry = &listed["entries"][0];
    assert_eq!(listed_entry["id"], "finding-1");
    assert_eq!(listed_entry["status"], "forgotten");
    assert_eq!(listed_entry["said"], "");
    assert_eq!(listed_entry["content"], "");
    assert_eq!(listed_entry["author"]["kind"], "agent");
    assert_eq!(listed_entry["author"]["name"], "assistant");

    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "finding-1", "--json"]).stdout).unwrap();
    assert_eq!(shown["id"], "finding-1");
    assert_eq!(shown["status"], "forgotten");
    assert_eq!(shown["said"], "");
    assert_eq!(shown["content"], "");
    assert_eq!(shown["author"]["kind"], "agent");
    assert_eq!(shown["author"]["name"], "assistant");
    assert_eq!(shown["content_from"]["role"], "assistant");
    assert_eq!(shown["cite"]["message"], "a-find");

    let plain_list = text(&s.fx.ok(&["entry", "list", "--tag", "finding"]).stdout);
    assert!(plain_list.contains("finding-1"), "{plain_list}");
    assert!(plain_list.contains("forgotten"), "{plain_list}");
    let plain_show = text(&s.fx.ok(&["entry", "show", "finding-1"]).stdout);
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
        let out = s.fx.run(&["entry", "show", "finding-1", "--json"]);
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
        s.run(&["session", "pass", "--session", ON, "--json"], b"");
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
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
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
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
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

#[test]
fn a_pass_keeps_the_windows_typed_turns_for_search_kind_message() {
    let s = Setup::new();
    let quote = "Let us keep the merge queue for agent pull requests.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            // An assistant turn no claim cites is not kept.
            line("assistant", "a-1", "Noted: the merge queue stays."),
            // Fenced code is not the user's typed words.
            line(
                "user",
                "u-2",
                "Here is the old helper:\n```\nfn merge_all() {}\n```\nIt can go.",
            ),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let db = s.fx.root().join("scribe.db");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let search = |terms: &str| -> Value {
        serde_json::from_slice(
            &s.fx
                .ok(&["search", terms, "--kind", "message", "--json"])
                .stdout,
        )
        .unwrap()
    };
    let found = search("merge");
    assert_eq!(found["total"], 1, "{found}");
    let hit = &found["hits"][0];
    assert_eq!(hit["kind"], "message");
    assert_eq!(hit["message"], "u-1");
    assert_eq!(hit["cite"], format!("session:{ON}"));
    assert_eq!(hit["snippet"], quote);
    assert_eq!(hit["project"], "scribe");
    assert_eq!(
        search("helper")["hits"][0]["snippet"],
        "Here is the old helper: It can go."
    );
    // The default kinds are entries and sessions: no message hit.
    let default: Value =
        serde_json::from_slice(&s.fx.ok(&["search", "merge", "--json"]).stdout).unwrap();
    assert!(
        default["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["kind"] != "message"),
        "{default}"
    );
    let plain = text(&s.fx.ok(&["search", "merge", "--kind", "message"]).stdout);
    assert!(plain.contains("session:s-1"), "{plain}");

    // A read-only copy of the store, without the -wal and -shm files, is
    // opened immutable and says so.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let copy = s.fx.elsewhere("copy");
        let copied = std::process::Command::new("cp")
            .arg("-Rp")
            .arg(s.fx.root())
            .arg(&copy)
            .status()
            .unwrap();
        assert!(copied.success());
        std::fs::remove_file(copy.join("scribe.db-wal")).unwrap();
        std::fs::remove_file(copy.join("scribe.db-shm")).unwrap();
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o500)).unwrap();
        let copy_arg = copy.display().to_string();
        let out =
            s.fx.ok(&["--root", &copy_arg, "search", "merge", "--kind", "message"]);
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
        assert!(
            stderr.contains("read-only copy") || stdout.contains("read-only copy"),
            "{stdout}{stderr}"
        );
        assert!(stdout.contains("merge queue"), "{stdout}");
    }
}

#[test]
fn an_empty_default_search_names_the_stored_messages_that_match() {
    let s = Setup::new();
    // The word is only in a turn he typed: no entry or session holds it.
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let json = |out: &Output| -> Value { serde_json::from_slice(&out.stdout).unwrap() };

    // The default search finds no entry or session, and its last stderr line
    // names the messages and the command that lists them.
    let found = s.fx.ok(&["search", "quartzlark"]);
    assert!(found.stdout.is_empty(), "{}", text(&found.stdout));
    let default_root = s.fx.root().display().to_string();
    let hint = format!(
        "1 stored message(s) match, and no entry or session does: scribe --root {default_root} search --kind message -- quartzlark lists them"
    );
    assert_eq!(
        text(&found.stderr).lines().last(),
        Some(hint.as_str()),
        "{}",
        text(&found.stderr)
    );
    let listed = json(
        &s.fx
            .ok(&["search", "quartzlark", "--kind", "message", "--json"]),
    );
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");
    let default_json = json(&s.fx.ok(&["search", "quartzlark", "--json"]));
    assert_eq!(default_json["hits"], json!([]), "{default_json}");
    assert_eq!(default_json["message_matches"], 1, "{default_json}");

    // The count takes the same filters, so a project with no such message
    // gets no hint.
    let other = s.fx.ok(&["search", "quartzlark", "--project", "orbit"]);
    assert_eq!(
        text(&other.stderr),
        "no records match \"quartzlark\" --project orbit\n"
    );

    // With no matching message, the output and the JSON are unchanged.
    let none = s.fx.ok(&["search", "zzznomatch"]);
    assert_eq!(text(&none.stderr), "no records match \"zzznomatch\"\n");
    let none_json = json(&s.fx.ok(&["search", "zzznomatch", "--json"]));
    assert!(none_json.get("message_matches").is_none(), "{none_json}");

    // An entry that holds the word answers the default search, so no hint.
    s.fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--author",
        "opus",
        "Quartzlark is the rollout code name.",
    ]);
    let answered = json(&s.fx.ok(&["search", "quartzlark", "--json"]));
    assert_eq!(answered["total"], 1, "{answered}");
    assert!(answered.get("message_matches").is_none(), "{answered}");
    assert!(!text(&s.fx.ok(&["search", "quartzlark"]).stderr).contains("stored message"));
}

#[test]
fn a_message_hint_beside_a_hidden_match_names_only_the_visible_entries_and_sessions() {
    let s = Setup::new();
    // The word is in a turn he typed, and in a retired entry: the default
    // search finds no visible entry or session, so the hint names the message.
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let added: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "opus",
                "Quartzlark was the old rollout code name.",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    let id = added["id"].as_str().unwrap();
    s.fx.ok(&[
        "entry",
        "retire",
        id,
        "--author",
        "opus",
        "--reason",
        "the code name changed",
    ]);

    let found = s.fx.ok(&["search", "quartzlark"]);
    assert!(found.stdout.is_empty(), "{}", text(&found.stdout));
    let stderr = text(&found.stderr);
    let default_root = s.fx.root().display().to_string();
    let hidden = "1 hidden record(s) also match: superseded or retired entries, or inactive sessions; --include-hidden shows them";
    let hint = format!(
        "1 stored message(s) match, and no visible entry or session does: scribe --root {default_root} search --kind message -- quartzlark lists them"
    );
    assert!(stderr.lines().any(|l| l == hidden), "{stderr}");
    assert_eq!(stderr.lines().last(), Some(hint.as_str()), "{stderr}");
    assert!(!stderr.contains("and no entry or session does"), "{stderr}");

    // The count is the same JSON key, and it names the visible message.
    let json: Value =
        serde_json::from_slice(&s.fx.ok(&["search", "quartzlark", "--json"]).stdout).unwrap();
    assert_eq!(json["total"], 0, "{json}");
    assert_eq!(json["hits"], json!([]), "{json}");
    assert_eq!(json["message_matches"], 1, "{json}");

    // Showing the hidden entry answers the search, so the hint goes.
    let shown =
        s.fx.ok(&["search", "quartzlark", "--include-hidden", "--json"]);
    let shown_json: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown_json["total"], 1, "{shown_json}");
    assert!(shown_json.get("message_matches").is_none(), "{shown_json}");
    assert!(
        !text(&shown.stderr).contains("stored message"),
        "{}",
        text(&shown.stderr)
    );
}

#[test]
fn a_message_search_hint_keeps_its_root_filters_and_dash_leading_terms() {
    let s = Setup::new();
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    // Move the populated fixture store to a nondefault root with spaces so
    // the suggested command must preserve both the selected root and quote it.
    let root = s.fx.elsewhere("selected store");
    std::fs::rename(s.fx.root(), &root).unwrap();
    let root_arg = root.display().to_string();
    let search = s.fx.ok(&[
        "--root",
        &root_arg,
        "search",
        "--project",
        "scribe",
        "--since",
        "2026-11-03",
        "--until",
        "2026-11-03",
        "--",
        "-quartzlark",
    ]);
    let hint = format!(
        "1 stored message(s) match, and no entry or session does: scribe --root '{}' search --kind message --project scribe --since 2026-11-03 --until 2026-11-03 -- -quartzlark lists them",
        root.display()
    );
    assert_eq!(
        text(&search.stderr).lines().last(),
        Some(hint.as_str()),
        "{}",
        text(&search.stderr)
    );

    // Execute the hinted command with JSON inserted before its terminator.
    let listed: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "--root",
                &root_arg,
                "search",
                "--kind",
                "message",
                "--project",
                "scribe",
                "--since",
                "2026-11-03",
                "--until",
                "2026-11-03",
                "--json",
                "--",
                "-quartzlark",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["hits"][0]["message"], "u-1", "{listed}");

    // On Unix, run the actual shell command from the hint, adding JSON before
    // its terminator so the output can be checked.
    #[cfg(unix)]
    {
        let stderr = text(&search.stderr);
        let line = stderr.lines().last().unwrap();
        let command = line
            .strip_prefix("1 stored message(s) match, and no entry or session does: ")
            .unwrap()
            .strip_suffix(" lists them")
            .unwrap()
            .replacen(" -- ", " --json -- ", 1);
        let binary_dir = std::path::Path::new(support::BIN)
            .parent()
            .unwrap()
            .display();
        let path = format!("{binary_dir}:/usr/bin:/bin");
        let out = Command::new("sh")
            .arg("-c")
            .arg(command)
            .env_clear()
            .env("HOME", s.fx.home())
            .env("SCRIBE_ROOT", s.fx.root())
            .env("PATH", path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let shell_listed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(shell_listed["total"], 1, "{shell_listed}");
        assert_eq!(shell_listed["hits"][0]["message"], "u-1", "{shell_listed}");
    }
}

#[test]
fn a_message_search_hint_replays_a_project_with_an_apostrophe() {
    let json = |out: &Output| -> Value { serde_json::from_slice(&out.stdout).unwrap() };
    let s = Setup::new();
    // A project is the cwd's basename, so this transcript names it with an
    // apostrophe, which a POSIX shell reads as an opening quote.
    let project = "editor's-project";
    let quote = "Keep the quartzlark rollout note out of the release.";
    s.write(
        ON,
        &[
            json!({"type": "user", "uuid": "u-1", "timestamp": "2026-11-03T06:00:00Z",
                 "cwd": format!("/nonexistent/work/{project}"),
                 "message": {"role": "user", "content": quote}}),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let search = s.fx.ok(&["search", "quartzlark", "--project", project]);
    let stderr = text(&search.stderr);
    let hint = stderr.lines().last().unwrap();
    assert!(
        hint.contains(r"--project 'editor'\''s-project'"),
        "the project is one shell word: {hint}"
    );

    // The direct argv search is the reference the replayed hint must match.
    let direct = json(&s.fx.ok(&[
        "search",
        "quartzlark",
        "--kind",
        "message",
        "--project",
        project,
        "--json",
    ]));
    assert_eq!(direct["total"], 1, "{direct}");
    assert_eq!(direct["hits"][0]["message"], "u-1", "{direct}");

    // On Unix, replay the hinted command in sh, adding JSON before its
    // terminator, and compare the hits with the direct argv search.
    #[cfg(unix)]
    {
        let command = hint
            .strip_prefix("1 stored message(s) match, and no entry or session does: ")
            .unwrap()
            .strip_suffix(" lists them")
            .unwrap()
            .replacen(" -- ", " --json -- ", 1);
        let binary_dir = std::path::Path::new(support::BIN)
            .parent()
            .unwrap()
            .display();
        let path = format!("{binary_dir}:/usr/bin:/bin");
        let out = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env_clear()
            .env("HOME", s.fx.home())
            .env("SCRIBE_ROOT", s.fx.root())
            .env("PATH", path)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{command}\n{}",
            text(&out.stderr)
        );
        let replayed: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(replayed["total"], direct["total"], "{replayed}");
        assert_eq!(replayed["hits"], direct["hits"], "{command}");
    }
}

#[test]
fn forgetting_an_entry_deletes_the_stored_message_it_cites() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line("user", "u-2", "Short explanations help reviewers too."),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let messages = |terms: &str| -> Vec<String> {
        let found: Value = serde_json::from_slice(
            &s.fx
                .ok(&["search", terms, "--kind", "message", "--json"])
                .stdout,
        )
        .unwrap();
        let mut ids: Vec<String> = found["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["message"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(messages("explanations"), ["u-1", "u-2"]);
    type_settlement(&s, "#scribe approve p-1");
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(messages("explanations"), ["u-2"]);
}

#[test]
fn forget_first_write_and_repeat_erase_legacy_session_archives() {
    let s = Setup::new();
    s.fx.seed();
    let root = s.fx.root();
    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["session", "show", "s-1", "--json"]).stdout).unwrap();
    let id = shown["id"].as_str().unwrap();
    let files = |base: &std::path::Path| -> Vec<PathBuf> {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(base, &mut out);
        out
    };
    let original = files(&root.join("sessions"))
        .into_iter()
        .find(|p| p.file_name().unwrap() == format!("{id}.md").as_str())
        .unwrap();
    let bytes = std::fs::read(&original).unwrap();
    let checkpoint = original.with_file_name(format!(".{id}.rebuild.md"));
    std::fs::write(&checkpoint, &bytes).unwrap();
    make_private(&checkpoint);
    assert!(!root.join("scribe.db").exists());
    let out = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(out.contains("forgot s-1"), "{out}");
    let assert_absent = || {
        for path in files(&root) {
            let name = path.file_name().unwrap().to_string_lossy();
            assert!(
                name != format!("{id}.md") && name != format!(".{id}.rebuild.md"),
                "{}",
                path.display()
            );
        }
    };
    assert_absent();
    let other: Value =
        serde_json::from_slice(&s.fx.ok(&["session", "show", "s-2", "--json"]).stdout).unwrap();
    assert!(other["id"].is_string());
    // Model the residue left by an older build after a successful forget.
    let stale = root.join("pre-sqlite-stale/sessions/2026/11");
    s.fx.create_private_dir_all(&stale);
    for name in [format!("{id}.md"), format!(".{id}.rebuild.md")] {
        let path = stale.join(name);
        std::fs::write(&path, &bytes).unwrap();
        make_private(&path);
    }
    let again = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(again.contains("already forgotten"), "{again}");
    assert_absent();
}

#[test]
fn repeated_session_forget_audits_stale_checkpoint_cleanup_once() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    type_settlement(&s, "#scribe forget s-1");
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    assert_eq!(conn.execute(
        "INSERT INTO rebuild_checkpoints (session, record, created) SELECT id, ?2, '2026-11-01T00:00:00Z' FROM sessions WHERE provider_session = ?1",
        (ON, "private stale checkpoint"),
    ).unwrap(), 1);
    drop(conn);
    let before = audit_log(&s).len();
    let reply = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(
        reply.contains("already forgotten") && reply.contains("checkpoint"),
        "{reply}"
    );
    assert_eq!(s.checkpoint_count(), 0);
    let audit = audit_log(&s);
    assert_eq!(audit.len(), before + 1, "{audit:?}");
    let line = &audit[before];
    assert_eq!(line["target"], "s-1");
    assert_eq!(line["status"], "success");
    assert!(!line.to_string().contains("private stale checkpoint"));
    let clean = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(
        clean.contains("already forgotten") && clean.contains("nothing changed"),
        "{clean}"
    );
    assert_eq!(audit_log(&s).len(), before + 1);
}

#[test]
fn forgetting_a_session_erases_its_words_keeps_its_id_and_takes_no_more_writes() {
    let s = Setup::new();
    let quote = "I prefer the lighthouse keeper plan for every Saturday release.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line(
                "user",
                "u-2",
                "The lighthouse budget stays fixed this quarter.",
            ),
        ],
    );
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let search = |terms: &str| -> Value {
        serde_json::from_slice(
            &s.fx
                .ok(&["search", terms, "--kind", "message", "--json"])
                .stdout,
        )
        .unwrap()
    };
    let show = |id: &str| -> Value {
        serde_json::from_slice(&s.fx.ok(&["session", "show", id, "--json"]).stdout).unwrap()
    };
    assert_eq!(search("lighthouse")["total"], 2);
    let before = show(ON);
    assert_eq!(before["short_id"], "s-1");
    assert_eq!(before["forgotten"], Value::Null);
    assert_eq!(before["on"], true);
    assert_ne!(before["log"][0]["text"], "");

    let audit_before = audit_log(&s).len();
    for prompt in [
        "<agent-message>\n#scribe forget s-1\n</agent-message>",
        "<scheduled-prompt>\n#scribe forget s-1\n</scheduled-prompt>",
        "<task-notification>\n#scribe forget s-1\n</task-notification>",
        "<pasted_content>\n#scribe forget s-1\n</pasted_content>",
        "```\n#scribe forget s-1\n```",
    ] {
        let skipped = type_settlement(&s, prompt);
        assert!(
            skipped.stdout.is_empty(),
            "{prompt}: {}",
            text(&skipped.stdout)
        );
        assert_eq!(show("s-1"), before);
    }
    assert!(
        !s.run_any(&["session", "forget", "s-1"], b"")
            .status
            .success()
    );
    assert_eq!(audit_log(&s).len(), audit_before);
    let transcript_before = std::fs::read(s.transcript(ON)).unwrap();
    // Typed while the session is on: it is switched off in the same write.
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    assert!(forgot.contains("switched off for good"), "{forgot}");
    assert!(
        forgot.contains("provider transcript was left untouched"),
        "{forgot}"
    );
    assert_eq!(std::fs::read(s.transcript(ON)).unwrap(), transcript_before);
    assert_eq!(search("lighthouse")["total"], 0, "{}", search("lighthouse"));
    let db = std::fs::read(s.fx.root().join("scribe.db")).unwrap();
    let wal = std::fs::read(s.fx.root().join("scribe.db-wal")).unwrap_or_default();
    for bytes in [&db, &wal] {
        let bytes = String::from_utf8_lossy(bytes);
        for words in ["lighthouse budget", "Level 4 is defined."] {
            assert!(!bytes.contains(words), "{words:?} is left in the database");
        }
    }
    let after = show("s-1");
    assert_eq!(after["id"], ON);
    assert_eq!(after["short_id"], "s-1");
    assert!(after["forgotten"].is_string(), "{after}");
    assert_eq!(after["on"], false);
    assert_eq!(after["summary"], "");
    assert_eq!(after["terms"], json!([]));
    assert_eq!(after["log"], json!([]));
    for field in ["title", "cwd", "summary", "goals", "next"] {
        assert_eq!(after[field], "", "{field}: {after}");
    }
    assert_eq!(after["topics"], json!([]));
    let listing: Value = serde_json::from_slice(
        &s.fx
            .ok(&["session", "list", "--include-inactive", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(listing["total"], 0);
    let pass = s.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert!(!pass.status.success());
    assert!(text(&pass.stderr).contains("session_forgotten"));
    let table = text(&s.fx.ok(&["session", "show", "s-1"]).stdout);
    assert!(table.contains("forgotten"), "{table}");
    let claim: Value =
        serde_json::from_slice(&s.fx.ok(&["proposal", "show", "p-1", "--json"]).stdout).unwrap();
    assert_eq!(claim["state"], "rejected", "{claim}");
    assert_eq!(claim["closed_reason"], "withdrawn");
    assert_eq!(claim["candidate"]["quote"], "");
    assert_eq!(claim["candidate"]["cite"]["session"], "deleted");
    let audit = audit_log(&s);
    let forgets: Vec<_> = audit[audit_before..]
        .iter()
        .filter(|line| line["command"] == "hook user-prompt-submit")
        .collect();
    assert_eq!(forgets.len(), 1, "{audit:?}");
    assert_eq!(forgets[0]["target"], "s-1");
    assert!(!forgets[0].to_string().contains("lighthouse"));
    let export = s.fx.home().join("jsonl-export");
    s.fx.ok(&["export", "--as", "jsonl", export.to_str().unwrap()]);
    let rows = std::fs::read_to_string(export.join("sessions.jsonl")).unwrap();
    let row: Value = serde_json::from_str(rows.trim()).unwrap();
    assert_eq!(row["has_record"], 0);
    assert_eq!(row["has_switch"], 0);
    assert_eq!(row["title"], Value::Null);
    assert_eq!(row["cwd"], Value::Null);
    assert!(!export.join("segments.jsonl").exists());
    // Replayed: nothing changes.
    let again = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(again.contains("already forgotten"), "{again}");
    // It cannot be switched on again, and a pass writes nothing to it.
    let on = s.scribe("/scribe on");
    assert!(on.contains("forgotten"), "{on}");
    assert_eq!(show("s-1")["on"], false);

    // The provider-id form, from another session; a new session after it
    // never takes a forgotten number.
    const OTHER: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    const THIRD: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e62";
    let switch_on = |session: &str| {
        s.write(
            session,
            &[line("user", "o-1", "The harbour crane needs a new cable.")],
        );
        let mut payload = s.payload("UserPromptSubmit");
        payload["session_id"] = json!(session);
        payload["transcript_path"] = json!(s.transcript(session));
        payload["prompt"] = json!("/scribe on");
        s.hook("user-prompt-submit", &payload);
    };
    switch_on(OTHER);
    s.answer(&answer());
    s.run(&["session", "pass", "--session", OTHER, "--json"], b"");
    assert_eq!(show(OTHER)["short_id"], "s-2");
    assert_eq!(search("harbour")["total"], 1);
    let forgot = text(&type_settlement(&s, &format!("#scribe forget {OTHER}")).stdout);
    assert!(forgot.contains("scribe: forgot s-2"), "{forgot}");
    assert_eq!(search("harbour")["total"], 0);
    assert!(show(OTHER)["forgotten"].is_string());
    assert_eq!(audit_log(&s).pop().unwrap()["target"], "s-2");
    switch_on(THIRD);
    s.run(&["session", "pass", "--session", THIRD, "--json"], b"");
    assert_eq!(show(THIRD)["short_id"], "s-3");
}

#[test]
fn typed_forget_takes_a_printed_session_prefix_and_refuses_an_ambiguous_one() {
    const SIBLING: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    const OTHER: &str = "9e3a4f10-7b2c-4d58-a6f1-3c8d9e0b1a24";
    let s = Setup::new();
    let show = |id: &str| -> Value {
        serde_json::from_slice(&s.fx.ok(&["session", "show", id, "--json"]).stdout).unwrap()
    };
    let search = |word: &str| -> Value {
        serde_json::from_slice(
            &s.fx
                .ok(&["search", word, "--kind", "message", "--json"])
                .stdout,
        )
        .unwrap()
    };
    let switch_on = |session: &str, said: &str| {
        s.write(session, &[line("user", "o-1", said)]);
        let mut payload = s.payload("UserPromptSubmit");
        payload["session_id"] = json!(session);
        payload["transcript_path"] = json!(s.transcript(session));
        payload["prompt"] = json!("/scribe on");
        s.hook("user-prompt-submit", &payload);
        s.answer(&answer());
        s.run(&["session", "pass", "--session", session, "--json"], b"");
    };
    s.write(ON, &[line("user", "u-1", "The quarry needs a new fence.")]);
    s.scribe("/scribe on");
    s.answer(&answer());
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    switch_on(SIBLING, "The harbour crane needs a new cable.");
    switch_on(OTHER, "The lighthouse needs a new lamp.");
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);
    assert_eq!(search("lighthouse")["total"], 1);

    // The first group is shared by ON and SIBLING: ambiguous, so nothing
    // changes and the refusal is audited with its code.
    let audited = audit_log(&s).len();
    let refused = text(&type_settlement(&s, "#scribe forget 5d1c0b7e").stdout);
    assert!(refused.contains("could not apply"), "{refused}");
    assert!(refused.contains("ambiguous"), "{refused}");
    assert_eq!(audit_log(&s).pop().unwrap()["error_code"], "id_ambiguous");
    assert_eq!(audit_log(&s).len(), audited + 1);
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);
    assert!(show(ON)["forgotten"].is_null());
    assert!(show(SIBLING)["forgotten"].is_null());

    // A unique printed prefix erases the same session as the full id would,
    // and the echo names the full id.
    let forgot = text(&type_settlement(&s, "#scribe forget 9e3a4f10").stdout);
    assert!(
        forgot.contains(&format!("scribe: forgot s-3 ({OTHER})")),
        "{forgot}"
    );
    assert_eq!(search("lighthouse")["total"], 0);
    assert!(show(OTHER)["forgotten"].is_string());
    assert_eq!(search("quarry")["total"], 1);
    assert_eq!(search("harbour")["total"], 1);

    // A longer prefix both siblings still share is refused; the full id of one erases it.
    let forgot =
        text(&type_settlement(&s, "#scribe forget 5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e").stdout);
    assert!(forgot.contains("could not apply"), "{forgot}");
    let forgot =
        text(&type_settlement(&s, "#scribe forget 5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61").stdout);
    assert!(
        forgot.contains(&format!("forgot s-2 ({SIBLING})")),
        "{forgot}"
    );
    assert_eq!(search("harbour")["total"], 0);
    assert_eq!(search("quarry")["total"], 1);

    // A prefix that matches nothing erases nothing.
    let none = text(&type_settlement(&s, "#scribe forget abcdef12").stdout);
    assert!(none.contains("could not apply"), "{none}");
    assert_eq!(search("quarry")["total"], 1);
}

#[test]
fn session_forget_preserves_an_approved_entry_with_a_deleted_cite() {
    let s = Setup::new();
    let quote = "I always prefer the approved bluebird release process.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let approved = type_settlement(&s, "#scribe approve p-1");
    assert!(text(&approved.stdout).contains("recorded preferences-1"));
    let show = || -> Value {
        serde_json::from_slice(
            &s.fx
                .ok(&["entry", "show", "preferences-1", "--json"])
                .stdout,
        )
        .unwrap()
    };
    let original = show();
    let forgot = type_settlement(&s, "#scribe forget s-1");
    assert!(text(&forgot.stdout).contains("forgot s-1"));
    let retained = show();
    assert_eq!(retained["content"], original["content"]);
    assert_eq!(retained["quote"], quote);
    assert_eq!(retained["status"], "active");
    assert_eq!(retained["cite"]["session"], "deleted");
    assert_eq!(retained["cite"]["message"], Value::Null);
    let after = type_settlement(&s, "#scribe approve p-1");
    assert!(text(&after.stdout).contains("already approved"));
    assert_eq!(show(), retained);
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(show()["status"], "forgotten");
    assert_eq!(show()["content"], "");
}

#[test]
fn a_window_where_no_daniel_turn_fires_a_trigger_requests_no_claims() {
    let s = Setup::new();
    let quote = "Let us use the blue-green rollout for this migration.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            // An assistant turn's trigger words never count.
            line("assistant", "a-1", "I always prefer to check first."),
        ],
    );
    // A model that ignores the request still opens nothing.
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0);
    let prompts = s.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0].contains("Return candidates: [] (none are requested in this window)."),
        "{}",
        prompts[0]
    );
    assert!(!prompts[0].contains("Already recorded or proposed"));
}

#[test]
fn a_firing_turn_is_named_with_its_tag_and_trigger_and_idea_configured_to_propose_proposes() {
    let s = Setup::new();
    // Config can put `idea` back on the candidate route.
    s.config(&support::idea_catcher_on("propose"));
    let idea = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-plain", "Please look at the routing table."),
            line("user", "u-idea", idea),
        ],
    );
    s.answer(&candidate_answer(idea, "u-idea", "idea"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "- message u-idea (the user's turn): tag idea (proposed; trigger \"what if we\", matched \"What if we\")"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    assert!(
        prompt.contains("- idea: qualifies: The user proposes something that could be built"),
        "{prompt}"
    );
    assert!(!prompt.contains("under written tags:"), "{prompt}");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let p = &listed["proposals"][0];
    assert_eq!(p["kind"], "candidate", "the propose route");
    assert_eq!(p["state"], "open");
    assert_eq!(p["candidate"]["tag"], "idea");
    assert_eq!(p["candidate"]["admitted"]["trigger"], "what if we");
    assert_eq!(p["candidate"]["admitted"]["matched"], "What if we");
}

#[test]
fn a_marked_turn_supplies_no_written_idea_and_an_unmarked_one_is_written() {
    // §14.7: a turn Daniel marked with `#scribe` (inside a sentence, or a
    // line that opens with it) is cataloged at Stop, so the pass skips the
    // whole turn, even its unmarked lines. His unmarked turn is captured.
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let inline = "What if we sharded the audit log by month? #scribe";
    let opened = "What if we dropped the popup preview?";
    let opened_turn = format!("{opened}\n#scribe idea: drop the popup preview");
    let plain = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-inline", inline),
            line("user", "u-opened", &opened_turn),
            line("user", "u-plain", plain),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            inline,
            "u-inline",
            "Daniel suggests sharding the audit log by month."
        ),
        idea_claim(
            opened,
            "u-opened",
            "Daniel suggests dropping the popup preview."
        ),
        idea_claim(
            plain,
            "u-plain",
            "Daniel suggests caching the routing table per host."
        ),
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains("- message u-plain (the user's turn): tag idea"),
        "{prompt}"
    );
    assert!(!prompt.contains("- message u-inline:"), "{prompt}");
    assert!(!prompt.contains("- message u-opened:"), "{prompt}");
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["entries"][0]["quote"], plain);
}

#[test]
fn a_marked_turn_supplies_no_candidate_and_an_unmarked_one_proposes() {
    let s = Setup::new();
    s.config(&support::idea_catcher_on("propose"));
    let marked = "What if we sharded the audit log by month? #scribe/global";
    let plain = "What if we cached the routing table per host?";
    s.write(
        ON,
        &[
            line("user", "u-marked", marked),
            line("user", "u-plain", plain),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": marked, "message": "u-marked", "tag": "idea", "subject": "the audit log",
         "statement": "Daniel suggests sharding the audit log by month."},
        {"quote": plain, "message": "u-plain", "tag": "idea", "subject": "the routing table",
         "statement": "Daniel suggests caching the routing table per host."}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["candidate"]["quote"], plain);
}

#[test]
fn a_claim_whose_turn_fires_no_trigger_of_its_tag_is_refused() {
    // The model names a firing turn under another tag than the one fired.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "idea"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0);
}

#[test]
fn the_trigger_is_checked_again_under_the_lock_against_the_config_then() {
    // The config changes while the model runs: the turn fired `always` when
    // the request was built, but under the lock `preferences` no longer has
    // that trigger, so the claim is refused and nothing is opened.
    let s = Setup::new();
    let quote = "I always keep the release notes short.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    let mut script = format!(
        "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\numask 077\nprintf '%s\\n' '[tags.preferences.capture]' 'triggers = [\"from now on\"]' > '{}'\n",
        config.display()
    );
    script.push_str(&response(&candidate_answer(quote, "u-1", "preferences")));
    s.fx.fake_program("claude", &script);
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert!(s.prompts()[0].contains("message u-1 (the user's turn): tag preferences"));
    assert!(config.exists(), "the fake model rewrote the config");
    assert_eq!(proposals(&s)["total"], 0);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-1");
}

#[test]
fn pass_proposes_no_candidate_under_task_without_a_config_declaring_it() {
    let s = Setup::new();
    let quote = "The migration still has to be filed as work.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please plan the migration."),
            line("assistant", "a-work", quote),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": "a-work", "tag": "task"}]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0, "task is not a shipped tag");
}

#[test]
fn no_candidate_is_opened_from_a_tool_payload() {
    let s = Setup::new();
    let payload_text = "Measured: the cache holds 42 entries after warmup.";
    let tool_line = json!({
        "type": "assistant", "uuid": "a-tool", "timestamp": "2026-11-03T06:00:00Z",
        "cwd": "/nonexistent/work/scribe",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": "Running the check now."},
            {"type": "tool_use", "name": "Bash", "input": {"command": payload_text}}
        ]}
    });
    s.write(
        ON,
        &[line("user", "u-1", "Please check the cache."), tool_line],
    );
    let mut body = answer();
    body["candidates"] = json!([{
        "quote": payload_text, "message": "a-tool", "tag": "preferences"
    }]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0, "tool payloads are not turns");
}

#[test]
fn session_start_output_is_the_same_with_many_findings() {
    let s = Setup::new();
    s.fx.create_private_dir_all(&s.fx.root());
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.finding]\nrequire_evidence = true\n").unwrap();
    make_private(&config);
    let before = s.hook("session-start", &s.payload("SessionStart")).stdout;
    for n in 0..6 {
        s.fx.ok(&[
            "entry",
            "add",
            "--tag",
            "finding",
            "--author",
            "opus",
            "--related",
            &format!("session:s-{n}"),
            "--evidence",
            "exp-1",
            &format!("Measured widget result {n} holds under load"),
            "--json",
        ]);
    }
    let after = s.hook("session-start", &s.payload("SessionStart")).stdout;
    assert_eq!(before, after, "findings never grow the start line");

    // An intake's pickup refuses a finding: it has no worker.
    let out =
        s.fx.run(&["entry", "pickup", "finding-1", "--author", "opus", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_action_tag", "{err}");
}

/// A Claude Code layout under the fixture's home, a fake `claude` on PATH
/// and helpers to run the binary with them.
struct Setup {
    fx: Fixture,
    project: PathBuf,
    cwd: PathBuf,
    path: OsString,
}

fn line(kind: &str, uuid: &str, text: &str) -> Value {
    json!({"type": kind, "uuid": uuid, "timestamp": "2026-11-03T06:00:00Z",
           "cwd": "/nonexistent/work/scribe",
           "message": {"role": kind, "content": text}})
}

impl Setup {
    fn new() -> Self {
        let fx = Fixture::new();
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        let bin = fx.elsewhere("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut path = bin.into_os_string();
        path.push(":/usr/bin:/bin");
        let setup = Self {
            fx,
            project,
            cwd,
            path,
        };
        setup.answer(&answer());
        setup.write(ON, &[line("user", "u-1", &format!("Crews. {DEFINED}."))]);
        setup.append(
            ON,
            &[line(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            )],
        );
        setup.write(NEVER, &[line("user", "n-1", NEVER_SAID)]);
        setup
    }

    fn transcript(&self, session: &str) -> PathBuf {
        self.project.join(format!("{session}.jsonl"))
    }

    fn write(&self, session: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let path = self.transcript(session);
        std::fs::write(&path, text).unwrap();
        make_private(&path);
    }

    fn append(&self, session: &str, lines: &[Value]) {
        let mut text = std::fs::read_to_string(self.transcript(session)).unwrap_or_default();
        text.extend(lines.iter().map(|l| format!("{l}\n")));
        let path = self.transcript(session);
        std::fs::write(&path, text).unwrap();
        make_private(&path);
    }

    /// The fake `claude`: saves its stdin (the prompt) and answers `body`
    /// as `claude -p --output-format json` does.
    fn answer(&self, body: &Value) {
        let script = format!(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\n{}",
            response(body)
        );
        self.fx.fake_program("claude", &script);
    }

    /// Every prompt the fake `claude` received, oldest first.
    fn prompts(&self) -> Vec<String> {
        (0..)
            .map(|n| self.fx.home().join(format!("claude-stdin-{n}")))
            .take_while(|p| p.exists())
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    }

    /// Scripted answers across process invocations; `None` fails that call.
    fn answers(&self, answers: &[Option<Value>]) {
        let mut script = String::from(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\ncase $n in\n",
        );
        for (n, answer) in answers.iter().enumerate() {
            match answer {
                Some(body) => {
                    script.push_str(&format!("{n}) {} ;;\n", response(body)));
                }
                None => script.push_str(&format!("{n}) exit 3 ;;\n")),
            }
        }
        script.push_str("*) exit 4 ;;\nesac\n");
        self.fx.fake_program("claude", &script);
    }

    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let out = self.run_any(args, input);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    }

    /// [`Setup::run`] without asserting the exit code.
    fn run_any(&self, args: &[&str], input: &[u8]) -> Output {
        let mut cmd = self.fx.cmd(args);
        cmd.env("PATH", &self.path);
        self.run_with(cmd, input)
    }

    /// Run `cmd` with `input` on stdin; the exit code is not checked.
    fn run_with(&self, mut cmd: std::process::Command, input: &[u8]) -> Output {
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
            .write_all(input)
            .unwrap();
        guard.finish()
    }

    fn hook(&self, event: &str, payload: &Value) -> Output {
        self.run(&["hook", event], payload.to_string().as_bytes())
    }

    fn payload(&self, event: &str) -> Value {
        json!({"session_id": ON,
               "transcript_path": self.transcript(ON),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": event})
    }

    /// `/scribe …` typed in the `ON` session: the hook's block reason.
    fn scribe(&self, prompt: &str) -> String {
        let mut payload = self.payload("UserPromptSubmit");
        payload["prompt"] = json!(prompt);
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    fn pass(&self) -> Value {
        let out = self.run(&["session", "pass", "--json"], b"");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn control(&self) -> Value {
        support::control(&self.fx.root())["sessions"][ON].clone()
    }

    fn expire_retry(&self) {
        support::set_switch(&self.fx.root(), ON, "retry_after", "'2026-01-01T00:00:00Z'");
    }

    /// The switches and cursors, serialized: equal bytes mean no change.
    fn control_bytes(&self) -> Vec<u8> {
        support::control(&self.fx.root()).to_string().into_bytes()
    }

    /// The `ON` record as its file read (`scribe export`).
    fn record_text(&self) -> String {
        support::record_text(&self.fx.root(), ON).expect("the ON record")
    }

    fn record(&self, session: &str) -> Option<Value> {
        let out = self.fx.run(&["session", "show", session, "--json"]);
        (out.status.code() == Some(0)).then(|| serde_json::from_slice(&out.stdout).unwrap())
    }

    /// How many entries `scribe.db` holds (`0` before the db exists).
    fn entry_count(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.fx.root().join("scribe.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|c| c.query_row("SELECT count(*) FROM entries", [], |r| r.get(0)))
        .unwrap_or(0)
    }

    /// How many rebuild checkpoints `scribe.db` holds.
    fn checkpoint_count(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.fx.root().join("scribe.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|c| c.query_row("SELECT count(*) FROM rebuild_checkpoints", [], |r| r.get(0)))
        .expect("the store's scribe.db")
    }

    fn audit_lines(&self) -> usize {
        support::audit_lines(&self.fx.root()).len()
    }

    fn config(&self, toml: &str) {
        let root = self.fx.root();
        if !root.exists() {
            std::fs::create_dir(&root).unwrap();
            make_private(&root);
        }
        let file = root.join("config.toml");
        std::fs::write(&file, toml).unwrap();
        make_private(&file);
    }
}

/// Generate an answer for the actual request. Compression returns only the
/// prose header. Explicit S9 window answers are passed through unchanged;
/// the generic fixture rewrites an open segment or starts a new one at the
/// first message when scribe has sealed the preceding one.
/// A session answer as the current schema returns it: a candidate a test
/// wrote without `context` gets one, [`context_of`] its `statement` (a
/// test's short form for the claim) or else of its quote, and a fixed
/// subject when it has none, so only a test that sets them (or sets one
/// empty) exercises them.
fn with_claim_contexts(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(candidates) = body.get_mut("candidates").and_then(Value::as_array_mut) {
        for candidate in candidates.iter_mut().filter_map(Value::as_object_mut) {
            let said = candidate
                .remove("statement")
                .or_else(|| candidate.get("quote").cloned())
                .and_then(|v| v.as_str().map(context_of))
                .unwrap_or_default();
            candidate.entry("context").or_insert(json!(said));
            candidate
                .entry("subject")
                .or_insert(json!("the work at hand"));
        }
    }
    body
}

fn response(body: &Value) -> String {
    let body = &with_claim_contexts(body);
    let envelope = |body: &Value| {
        // Single-quoted in the fake's shell script: an apostrophe closes
        // the quote, escapes itself and reopens it.
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body})
            .to_string()
            .replace('\'', "'\\''")
    };
    if body.get("new").is_some() {
        let header = json!({"title": body["title"], "summary": body["summary"],
                            "goals": body["goals"], "next": body["next"]});
        let mut script = format!(
            "if grep -q '^Compress only' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\n",
            envelope(&header)
        );
        script.push_str(&session_response(body));
        script.push_str("\nfi");
        script
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

fn session_response(body: &Value) -> String {
    let envelope = |body: &Value| {
        // Single-quoted in the fake's shell script: an apostrophe closes
        // the quote, escapes itself and reopens it.
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body})
            .to_string()
            .replace('\'', "'\\''")
    };
    if body["new"][0]["starts_at"] == "__window_first__" {
        let mut open = body.clone();
        let mut segment = open["new"][0].clone();
        segment.as_object_mut().unwrap().remove("starts_at");
        open["open"] = segment;
        open["new"] = json!([]);
        format!(
            "if grep -q '^Open segment ' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\nfirst=$(sed -n 's/^--- .* · \\(.*\\) ---$/\\1/p' \"$HOME/claude-stdin-$n\" | head -1)\nprintf '%s\\n' '{}' | sed \"s/__window_first__/$first/g\"\nfi",
            envelope(&open),
            envelope(body)
        )
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

fn answer() -> Value {
    json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "open": null,
        "new": [{"starts_at": "__window_first__", "heading": "Crew levels", "text": "Level 4 is defined.", "topics": [], "related": []}],
        "next": ["write it into the routing doc"],
        "topics": ["crew-routing"],
        "related": [],
        "terms": [
            {"term": "graduate to level 4", "said": DEFINED, "message": "u-1"},
            {"term": "level 4 crew", "said": "Level 4 crews merge their own pull requests",
             "message": "a-1"}
        ]
    })
}

#[test]
fn header_overruns_retry_with_only_the_header_and_preserve_the_segments() {
    let setup = Setup::new();
    let mut over = answer();
    over["summary"] = json!("é".repeat(1201));
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "A compact summary.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    setup.answers(&[Some(over), Some(compressed)]);
    setup.scribe("/scribe on");

    let report = setup.pass();

    assert_eq!(report["passes"][0]["windows"], 1);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains(DEFINED));
    assert!(
        prompts[0].contains("summary: at most 900 characters"),
        "ORB-23950: leave summary headroom"
    );
    assert!(
        prompts[1].contains("Compress only"),
        "ORB-23950: compress rather than resend the window"
    );
    assert!(
        prompts[1].contains("Current summary length: 1201 characters"),
        "ORB-23950: count Unicode characters for the model"
    );
    assert!(prompts[1].contains(&"é".repeat(1201)));
    for excluded in [
        DEFINED,
        "Level 4 is defined.",
        "--- user",
        "--- assistant",
        "terms:",
    ] {
        assert!(!prompts[1].contains(excluded), "{excluded} resent");
    }
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "A compact summary.");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["terms"].as_array().unwrap().len(), 1);
    assert_eq!(setup.control()["due"], false);
}

/// A line Claude Code wrote again on resume: the same uuid, timestamp and
/// text as the first copy, from another working directory.
fn replayed(kind: &str, uuid: &str, text: &str) -> Value {
    let mut line = line(kind, uuid, text);
    line["cwd"] = json!("/nonexistent/work/elsewhere");
    line
}

/// The fake `claude` answers `body` with every `__window_first__` replaced
/// by the id of the first message in the window it was sent, as a model
/// opening a new segment at the window's first message does.
fn answer_at_window_first(setup: &Setup, body: &Value) {
    let envelope = json!({"type":"result", "subtype":"success", "is_error":false,
                          "result":"", "structured_output":body});
    setup.fx.fake_program(
        "claude",
        &format!(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\nfirst=$(sed -n 's/^--- .* · \\(.*\\) ---$/\\1/p' \"$HOME/claude-stdin-$n\" | head -1)\nprintf '%s\\n' '{envelope}' | sed \"s/__window_first__/$first/g\"\n"
        ),
    );
}

#[test]
fn a_replayed_transcript_is_read_once_and_a_segment_after_the_replay_is_accepted() {
    // ORB-24918: Claude Code wrote earlier lines into the transcript again on
    // resume. The pass resumed after the first copy of `scribed_through`,
    // re-read the replay, and refused every start in it as not following the
    // open segment.
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "a-1");
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            ),
            line("user", "u-2", "Fresh work after the resume."),
        ],
    );
    let mut body = answer();
    body["terms"] = json!([]);
    body["new"][0]["heading"] = json!("After the resume");
    body["new"][0]["text"] = json!("Fresh work.");
    answer_at_window_first(&setup, &body);

    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written", "{report}");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["warnings"], json!([]), "{report}");
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 2, "one call per pass, no retry");
    assert!(prompts[1].contains("· u-2 ---"));
    for replay in ["· u-1 ---", "· a-1 ---"] {
        assert!(!prompts[1].contains(replay), "{replay} read again");
    }
    let record = setup.record(ON).unwrap();
    let log = record["log"].as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["state"], "sealed");
    assert_eq!(log[0]["last_message"], "a-1");
    assert_eq!(log[1]["first_message"], "u-2");
    assert_eq!(log[1]["heading"], "After the resume");
    assert_eq!(record["scribed_through"], "u-2");

    // A second replay with nothing new needs no model call.
    setup.append(
        ON,
        &[
            replayed("user", "u-1", &format!("Crews. {DEFINED}.")),
            replayed("user", "u-2", "Fresh work after the resume."),
        ],
    );
    setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(setup.prompts().len(), 2);
    assert_eq!(setup.record(ON).unwrap()["passes"], record["passes"]);
}

#[test]
fn next_and_segment_text_get_a_target_and_their_current_length() {
    // ORB-24918: `next` and segment text had only their hard limits, so a
    // record carrying ~900 characters of `next` forward overshot 1000 when
    // the model added an item.
    let setup = Setup::new();
    let mut first = answer();
    let items: Vec<String> = (0..9)
        .map(|n| format!("item {n} {}", "x".repeat(100)))
        .collect();
    first["next"] = json!(items);
    first["new"][0]["text"] = json!("é".repeat(650));
    setup.answer(&first);
    setup.scribe("/scribe on");
    setup.pass();
    let record = setup.record(ON).unwrap();
    let next_chars = record["next"].as_str().unwrap().chars().count();
    assert!(next_chars > 900, "{next_chars}");

    setup.append(ON, &[line("user", "u-2", "One more step.")]);
    let mut over = answer();
    over["new"] = json!([]);
    over["open"] = json!(null);
    let mut more = items.clone();
    more.push(format!("item 9 {}", "x".repeat(100)));
    over["next"] = json!(more);
    let compressed = json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "next": ["write it into the routing doc"]
    });
    // The fake numbers calls across runs: the first pass was call 0.
    setup.answers(&[None, Some(over), Some(compressed)]);

    let out = setup.run(&["session", "pass", "--session", ON], b"");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 3);
    let window = &prompts[1];
    assert!(
        window.contains(
            "next: the open next steps, one short item each, at most 750 characters in all \
             (hard limit 1000), counting a `- ` prefix per item and a newline between items"
        ),
        "{window}"
    );
    assert!(window.contains(&format!("Current next length: {next_chars} characters.")));
    assert!(window.contains("Segment text is at most 700 characters (hard limit 900)"));
    assert!(window.contains("text length: 650 characters\n"));
    let compress = &prompts[2];
    assert!(compress.starts_with("Compress only"));
    assert!(compress.contains("next: at most 750 characters total (hard limit 1000)"));
    assert!(compress.contains("Current next length: 1099 characters."));
    // The retry resolved the refusal: nothing at warn level.
    assert!(
        !text(&out.stderr).contains("refused"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        setup.record(ON).unwrap()["next"],
        "- write it into the routing doc"
    );
}

#[test]
fn omitted_or_null_open_keeps_its_content_and_still_advances_and_seals() {
    for omitted in [true, false] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = setup.record(ON).unwrap()["log"][0].clone();
        setup.append(ON, &[line("user", "u-2", "Continue the thread.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut body = answer();
        body["new"] = json!([]);
        if omitted {
            body.as_object_mut().unwrap().remove("open");
        }
        setup.answer(&body);

        setup.pass();

        let record = setup.record(ON).unwrap();
        let segment = &record["log"][0];
        for field in [
            "heading",
            "text",
            "topics",
            "related",
            "first_message",
            "from",
        ] {
            assert_eq!(segment[field], before[field], "{field}");
        }
        assert_eq!(segment["last_message"], "u-2");
        assert_eq!(segment["windows"], 2);
        assert_eq!(segment["state"], "sealed");
        assert_eq!(record["scribed_through"], "u-2");
        assert_eq!(setup.prompts().len(), 2, "no retry needed");
        assert!(
            setup.prompts()[1].contains(&format!(
                "Current summary length: {} characters",
                answer()["summary"].as_str().unwrap().chars().count()
            )),
            "ORB-23950: tell the model the previous summary length"
        );
    }
}

#[test]
fn unexpected_open_becomes_the_first_new_segment() {
    let setup = Setup::new();
    let mut body = answer();
    body["open"] = json!({"heading": "Opening thread", "text": "Opening details.", "topics": [], "related": []});
    body["new"][0]["starts_at"] = json!("a-1");
    setup.answer(&body);
    setup.scribe("/scribe on");

    setup.pass();

    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 2);
    assert_eq!(record["log"][0]["text"], "Opening details.");
    assert_eq!(record["log"][0]["first_message"], "u-1");
    assert_eq!(record["log"][0]["last_message"], "u-1");
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][1]["first_message"], "a-1");
    assert_eq!(setup.prompts().len(), 1);
}

#[test]
fn repeated_header_budget_misses_are_repaired_after_retry_and_reported() {
    for (field, value, expected, boundary) in [
        (
            "summary",
            json!(format!("Keep this sentence. {}", "é".repeat(1200))),
            "Keep this sentence.".to_owned(),
            "sentence",
        ),
        (
            "goals",
            json!(format!("Keep these words {}", "é".repeat(600))),
            "Keep these words…".to_owned(),
            "word",
        ),
        (
            "summary",
            json!(format!("Keep this sentence.\" {}", "é".repeat(1200))),
            "Keep this sentence.\"".to_owned(),
            "sentence",
        ),
        (
            "title",
            json!("é".repeat(121)),
            format!("{}…", "é".repeat(119)),
            "word",
        ),
        (
            "next",
            json!(["Keep this bullet", "é".repeat(1000)]),
            "- Keep this bullet".to_owned(),
            "bullet",
        ),
        (
            "next",
            json!([format!("Keep these words {}", "é".repeat(1000))]),
            "- Keep these words…".to_owned(),
            "word",
        ),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        setup.answers(&[Some(over.clone()), Some(over)]);
        setup.scribe("/scribe on");

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(0), "{field}: {}", text(&out.stderr));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["windows"], 1);
        let warning = report["passes"][0]["warnings"][0].as_str().unwrap();
        assert!(
            warning.contains(field) && warning.contains(boundary),
            "{warning}"
        );
        assert!(text(&out.stderr).contains(warning));
        assert_eq!(setup.prompts().len(), 2, "repair follows the retry");
        assert!(!setup.prompts()[1].contains(DEFINED), "header-only retry");
        let record = setup.record(ON).unwrap();
        assert_eq!(record[field], expected, "{field}");
        assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
        assert_eq!(record["scribed_through"], "a-1");
        assert_eq!(setup.control()["due"], false);
    }
}

#[test]
fn segment_text_repairs_keep_new_segments_and_redact_before_writing() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second."),
            line("assistant", "a-2", "Third."),
        ],
    );
    let mut over = answer();
    over["open"] = json!({"heading": "First", "text": format!("Keep first. {}", "é".repeat(900)), "topics": [], "related": []});
    over["new"] = json!([
        {"starts_at": "u-2", "heading": "Second", "text": format!("Keep sensitive-value-123. {}", "é".repeat(900)), "topics": [], "related": []},
        {"starts_at": "a-2", "heading": "Third", "text": format!("Keep third. {}", "é".repeat(900)), "topics": [], "related": []}
    ]);
    setup.answer(&over);
    setup.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    let mut cmd = setup
        .fx
        .cmd(&["session", "pass", "--session", ON, "--json"]);
    cmd.env("PATH", &setup.path)
        .env("TEST_SECRET", "sensitive-value-123");

    let out = setup.run_with(cmd, b"");

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["warnings"].as_array().unwrap().len(), 3);
    assert_eq!(report["passes"][0]["segments_sealed"], 2);
    assert_eq!(setup.prompts().len(), 3, "initial call, miss and one retry");
    // Reading the stored record does not require loading the model config.
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["text"], "Keep first.");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["text"], "Keep [redacted:TEST_SECRET].");
    assert_eq!(record["log"][1]["first_message"], "u-2");
    assert_eq!(record["log"][2]["text"], "Keep third.");
    assert_eq!(record["log"][2]["state"], "open");
    assert!(!setup.record_text().contains("sensitive-value-123"));
}

#[test]
fn starts_at_repairs_warn_in_plain_output_and_preserve_sealed_segments() {
    for overflow in [false, true] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        let mut initial = answer();
        initial["new"] = json!([
            {"starts_at": "u-1", "heading": "Sealed", "text": "Sealed history.", "topics": [], "related": []},
            {"starts_at": "a-1", "heading": "Open", "text": "Open history.", "topics": [], "related": []}
        ]);
        setup.answer(&initial);
        setup.pass();
        let before = setup.record(ON).unwrap();
        setup.append(ON, &[line("user", "u-2", "More.")]);
        let mut bad = answer();
        bad["new"][0]["starts_at"] = json!("a-1");
        bad["new"][0]["text"] = json!(if overflow {
            "é".repeat(900)
        } else {
            "Continued history.".to_owned()
        });
        setup.answer(&bad);

        let out = setup.run(&["session", "pass", "--session", ON], b"");

        assert!(text(&out.stderr).contains("window 1: repaired new segment 1 starts_at"));
        assert!(text(&out.stdout).contains("written"));
        assert_eq!(setup.prompts().len(), 3);
        let record = setup.record(ON).unwrap();
        assert_eq!(record["log"][0], before["log"][0]);
        assert_eq!(record["scribed_through"], "u-2");
        if overflow {
            assert_eq!(record["log"].as_array().unwrap().len(), 3);
            assert_eq!(record["log"][2]["first_message"], "u-2");
            assert_eq!(record["log"][2]["text"], "é".repeat(900));
        } else {
            assert_eq!(record["log"].as_array().unwrap().len(), 2);
            assert_eq!(
                record["log"][1]["text"],
                "Open history.\n\nContinued history."
            );
        }
    }
}

#[test]
fn header_length_retries_cover_every_prose_field_and_recheck_the_result() {
    for (field, value) in [
        ("title", json!("t".repeat(121))),
        ("goals", json!("g".repeat(601))),
        ("next", json!(["n".repeat(999)])),
    ] {
        let setup = Setup::new();
        let mut over = answer();
        over[field] = value;
        over["summary"] = json!("é".repeat(1201));
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written", "{field}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(
            !setup.prompts()[1].contains(DEFINED),
            "window resent for {field}"
        );
        assert_eq!(
            setup.record(ON).unwrap()[field],
            match field {
                "next" => json!("- write it into the routing doc"),
                _ => answer()[field].clone(),
            }
        );
    }

    // A shorter header can still be invalid; it receives no third attempt.
    for (field, value, reason) in [
        ("summary", json!(""), "summary is empty"),
        (
            "goals",
            json!("## Injected"),
            "Goals holds a section heading",
        ),
        (
            "title",
            json!("two\nlines"),
            "title must be one non-empty line",
        ),
        ("model_failure", Value::Null, "the model failed"),
    ] {
        let setup = Setup::new();
        setup.scribe("/scribe on");
        setup.pass();
        let before = setup.record_text().into_bytes();
        setup.append(ON, &[line("user", "u-2", "A new turn.")]);
        setup.hook("session-end", &setup.payload("SessionEnd"));
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        let mut invalid = answer();
        invalid[field] = value;
        // Script indices include the already successful initial pass.
        setup.answers(&[
            Some(answer()),
            Some(over),
            (field != "model_failure").then_some(invalid),
        ]);

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            report["passes"][0]["error_code"],
            if field == "model_failure" {
                "model_failed"
            } else {
                "session_answer_invalid"
            }
        );
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains("Summary has 1201 characters"), "{error}");
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 3, "only one retry");
        assert_eq!(setup.record_text().into_bytes(), before);
        assert_eq!(setup.control()["due"], true);
    }
}

#[test]
fn header_overruns_with_other_defects_use_the_full_window_retry() {
    for defect in ["segment", "topics", "heading"] {
        let setup = Setup::new();
        let mut over = answer();
        over["summary"] = json!("s".repeat(1201));
        match defect {
            "segment" => over["new"][0]["text"] = json!("s".repeat(901)),
            "topics" => over["topics"] = json!(["Invalid Topic"]),
            "heading" => over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201))),
            _ => unreachable!(),
        }
        setup.answers(&[Some(over), Some(answer())]);
        setup.scribe("/scribe on");

        assert_eq!(setup.pass()["passes"][0]["result"], "written");

        let prompts = setup.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(
            prompts[1].contains(DEFINED),
            "{defect}: full window required"
        );
        assert!(prompts[1].contains("Summary has 1201 characters") || defect == "heading");
        assert!(!prompts[1].starts_with("Compress only"), "{defect}");
    }
}

#[test]
fn impossible_unexpected_open_conversions_are_refused_with_a_reason() {
    for (starts, reason) in [
        (vec!["u-1"], "distinct and in transcript order"),
        (vec!["a-1", "u-2", "a-2"], "at most 3 segments"),
    ] {
        let setup = Setup::new();
        setup.append(
            ON,
            &[
                line("user", "u-2", "More."),
                line("assistant", "a-2", "Done."),
            ],
        );
        let mut body = answer();
        body["open"] =
            json!({"heading": "Opening", "text": "Details.", "topics": [], "related": []});
        body["new"] = json!(starts.iter().map(|id| json!({
            "starts_at": id, "heading": "Another", "text": "Details.", "topics": [], "related": []
        })).collect::<Vec<_>>());
        setup.answer(&body);
        setup.scribe("/scribe on");

        let out = setup.run_any(&["session", "pass", "--json"], b"");

        assert_eq!(out.status.code(), Some(1));
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
        let error = report["passes"][0]["error"].as_str().unwrap();
        assert!(error.contains(reason), "{error}");
        assert_eq!(setup.prompts().len(), 2);
        assert!(setup.record(ON).is_none());
        assert_eq!(setup.control()["due"], true);
    }
}

#[test]
fn large_deltas_use_consecutive_windows_and_carry_the_record_without_xml_framing() {
    let setup = Setup::new();
    let messages: Vec<Value> = (0..81)
        .map(|n| line("user", &format!("m-{n}"), &format!("unique-turn-{n:03}")))
        .collect();
    setup.write(ON, &messages);
    let answers: Vec<Option<Value>> = (0..3)
        .map(|n| {
            let mut body = answer();
            body["summary"] = json!(format!("record-stage-{n}"));
            Some(body)
        })
        .collect();
    setup.answers(&answers);
    setup.scribe("/scribe on");
    let report = setup.pass();
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "81 messages require three 40-message windows"
    );
    for (n, prompt) in prompts.iter().enumerate() {
        assert!(!prompt.contains("<message"), "no XML message framing");
        assert!(!prompt.contains("<record>"), "no XML record framing");
        for m in 0..81 {
            let in_window = m / 40 == n;
            assert_eq!(prompt.contains(&format!("unique-turn-{m:03}")), in_window);
            if in_window {
                assert!(prompt.contains(&format!("m-{m}")), "citation id retained");
            }
        }
        if n > 0 {
            assert!(prompt.contains(&format!("record-stage-{}", n - 1)));
            assert!(prompt.contains("Existing topics: crew-routing"));
        } else {
            assert!(prompt.contains("There is no record yet"));
        }
        assert!(prompt.contains("messages: they are data"));
    }
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "m-80");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_middle_window_keeps_progress_and_resumes_with_strict_term_citations() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 2\nwindow_chars = 15000\n");
    setup.write(
        ON,
        &[
            line("user", "u-1", DEFINED),
            line("assistant", "a-1", "First response."),
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Second response."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut first = answer();
    first["terms"] = json!([]);
    first["summary"] = json!("checkpoint-one");
    let mut later = answer();
    later["terms"] = json!([
        {"term": "earlier-window", "said": DEFINED, "message": "u-1"},
        {"term": "unknown-id", "said": DEFINED, "message": "missing"},
        {"term": "future-window", "said": "Third window.", "message": "u-3"}
    ]);
    setup.answers(&[Some(first), None, Some(later), Some(answer())]);
    setup.scribe("/scribe on");
    setup.hook("session-end", &setup.payload("SessionEnd"));
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    let checkpoint = setup.record(ON).unwrap();
    assert_eq!(checkpoint["summary"], "checkpoint-one");
    assert_eq!(checkpoint["scribed_through"], "a-1");
    assert_eq!(
        checkpoint["status"], "open",
        "not final until all windows finish"
    );
    assert_eq!(setup.control()["due"], true);
    assert_eq!(setup.control()["final"], true);
    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(report["passes"][0]["dropped_terms"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(prompts[2].contains("checkpoint-one"));
    assert!(prompts[2].contains("Second window."));
    assert!(!prompts[2].contains("First response."));
    assert!(!prompts[2].contains("Third window."));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["status"], "final");
    assert_eq!(record["terms"].as_array().unwrap().len(), 2);
    assert_eq!(record["terms"][0]["term"], "earlier-window");
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn character_windows_and_each_windows_field_limits_are_enforced() {
    let setup = Setup::new();
    setup.config("[catalog]\nwindow_messages = 40\nwindow_chars = 100\n");
    setup.write(
        ON,
        &(0..3)
            .map(|n| line("user", &format!("u-{n}"), &"é".repeat(60)))
            .collect::<Vec<_>>(),
    );
    let mut over = answer();
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
    setup.answers(&[Some(answer()), Some(over.clone()), Some(over)]);
    setup.scribe("/scribe on");
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(setup.record(ON).unwrap()["scribed_through"], "u-0");
    let prompts = setup.prompts();
    assert_eq!(
        prompts.len(),
        3,
        "one call then an invalid window and its fresh retry"
    );
    assert!(prompts[1].contains("u-1"));
    assert!(!prompts[1].contains("u-2"));
}

#[test]
fn failures_back_off_while_due_and_explicit_sessions_ignore_the_delay() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.answers(&[None, None, Some(answer())]);
    for failures in 1..=2 {
        let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
        assert_eq!(out.status.code(), Some(1));
        let control = setup.control();
        assert_eq!(control["due"], true);
        assert_eq!(control["consecutive_failures"], failures);
        assert!(control["retry_after"].is_string());
        let calls = setup.prompts().len();
        let report = setup.pass();
        assert_eq!(report["passes"], json!([]));
        assert_eq!(report["remaining"], 1, "backed-off session is still owed");
        assert_eq!(setup.prompts().len(), calls);
    }
    setup.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["consecutive_failures"], 0);
    assert_eq!(setup.control()["retry_after"], Value::Null);
}

#[test]
fn legacy_control_is_read_without_writes_and_new_retry_state_is_strict() {
    let setup = Setup::new();
    setup.config("");
    let file = setup.fx.root().join("control.json");
    let legacy = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-11-03T05:00:00Z", "due": true, "final": false}}});
    std::fs::write(&file, legacy.to_string()).unwrap();
    make_private(&file);
    let before = setup.control_bytes();
    setup.run(&["session", "list", "--json"], b"");
    assert_eq!(
        setup.control_bytes(),
        before,
        "migration on read is in memory only"
    );
    setup.pass();
    let current = support::control(&setup.fx.root());
    assert_eq!(current["format"], 31);
    assert_eq!(current["sessions"][ON]["consecutive_failures"], 0);
    assert_eq!(current["sessions"][ON]["retry_after"], Value::Null);
    assert_eq!(current["sessions"][ON]["pending_links"], json!([]));
    // A current-format file that has not moved into `scribe.db` yet is read
    // strictly; once moved, the table's CHECKs hold these fields.
    for missing in ["consecutive_failures", "retry_after", "pending_links"] {
        let fresh = Setup::new();
        fresh.config("");
        let mut broken = current.clone();
        broken["format"] = json!(25);
        broken["sessions"][ON]
            .as_object_mut()
            .unwrap()
            .remove(missing);
        let file = fresh.fx.root().join("control.json");
        std::fs::write(&file, broken.to_string()).unwrap();
        make_private(&file);
        let out = fresh.run_any(&["session", "list", "--json"], b"");
        assert_eq!(out.status.code(), Some(1), "{missing}");
        let error: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(error["code"], "store_corrupt");
    }
}

#[test]
fn scribe_on_off_and_status_switch_the_session_and_block_the_prompt() {
    let setup = Setup::new();
    let reason = setup.scribe("/scribe status");
    assert!(reason.contains("never switched on"), "{reason}");
    assert!(!support::has_control(&setup.fx.root()));

    let reason = setup.scribe("/scribe on");
    assert!(reason.starts_with("scribe: on"), "{reason}");
    let switch = setup.control();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(setup.audit_lines(), 1);

    // On twice: a no-op that says so.
    let before = setup.control_bytes();
    let reason = setup.scribe("/scribe ON");
    assert!(reason.starts_with("scribe: already on since "), "{reason}");
    assert!(reason.ends_with("nothing changed"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), 1);

    let reason = setup.scribe("/scribe");
    assert!(reason.starts_with("scribe: on since "), "{reason}");
    assert!(reason.contains("a pass is due; no record yet"), "{reason}");

    let reason = setup.scribe("/scribe off");
    assert!(reason.starts_with("scribe: off"), "{reason}");
    assert_eq!(setup.control()["on"], false);
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.audit_lines(), 2);

    let reason = setup.scribe("/scribe maybe");
    assert!(reason.contains("unknown command"), "{reason}");
    assert_eq!(setup.control()["on"], false);

    // No pass runs for a session switched off.
    assert_eq!(setup.pass()["passes"], json!([]));
    assert!(setup.prompts().is_empty());
}

/// The audit log's lines, oldest first.
fn audit_log(setup: &Setup) -> Vec<Value> {
    support::audit_lines(&setup.fx.root())
}

#[test]
fn a_failed_session_catalog_writes_one_failure_audit_line() {
    let setup = Setup::new();
    setup.fx.fake_program("claude", BROKEN);
    let out = setup.run_any(&["session", "catalog", NEVER, "--author", "opus"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    // The switch was written and stays due, yet the command is one outcome.
    assert!(support::has_control(&setup.fx.root()));
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["command"], "session catalog");
    assert_eq!(audit[0]["status"], "failure");
    assert_eq!(audit[0]["error_code"], "model_failed");
    assert_eq!(audit[0]["target"], format!("session:{NEVER}"));
    assert_eq!(audit[0]["author"], "opus");

    // A catalog that succeeds still logs its switch, then its window.
    setup.answer(&answer());
    let out = setup.run_any(&["session", "catalog", NEVER, "--author", "opus"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let audit = audit_log(&setup);
    assert_eq!(audit.len(), 2, "a resume wrote no switch: {audit:?}");
    assert_eq!(audit[1]["status"], "success");
}

#[test]
fn scribe_catalog_in_a_typed_prompt_catalogs_a_past_session_as_daniel() {
    let setup = Setup::new();
    // An id that names no transcript is refused, audited by code, and
    // writes no switch.
    let reason = setup.scribe("/scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead");
    assert!(
        reason.starts_with("scribe: /scribe catalog 0a1b2c3d-0000-4000-8000-00000000dead refused"),
        "{reason}"
    );
    assert!(!support::has_control(&setup.fx.root()));

    // Typed by Daniel in another session: the switch is written as his,
    // and no window runs in the hook.
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(
        reason.starts_with(&format!("scribe: cataloging {NEVER} as yours")),
        "{reason}"
    );
    let switch = support::control(&setup.fx.root())["sessions"][NEVER].clone();
    assert_eq!(switch["on"], true);
    assert_eq!(switch["due"], true);
    assert_eq!(switch["cataloged_by"]["by"]["kind"], "user");
    assert!(setup.prompts().is_empty());
    assert!(setup.record(NEVER).is_none());
    let audited = setup.audit_lines();

    // Again: nothing changes and nothing is audited.
    let before = setup.control_bytes();
    let reason = setup.scribe(&format!("/scribe catalog {NEVER}"));
    assert!(reason.contains("already being cataloged"), "{reason}");
    assert_eq!(setup.control_bytes(), before);
    assert_eq!(setup.audit_lines(), audited);

    // The next session pass runs its windows as a catalog and keeps
    // Daniel's attribution; the transcript is days old, so it ends final
    // and off.
    let pass = setup.pass();
    assert_eq!(pass["passes"][0]["result"], "written", "{pass}");
    let record = setup.record(NEVER).unwrap();
    assert_eq!(record["status"], "final", "{record}");
    assert_eq!(record["cataloged_by"]["by"]["kind"], "user", "{record}");
    let switch = support::control(&setup.fx.root())["sessions"][NEVER].clone();
    assert_eq!(switch["on"], false);
}

#[test]
fn stop_marks_due_by_turns_and_minutes_and_session_end_marks_final_without_a_model() {
    let setup = Setup::new();
    setup.config("[catalog]\nevery_turns = 2\nevery_minutes = 600\n");
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    assert_eq!(setup.control()["due"], false);
    let calls = setup.prompts().len();

    setup.append(ON, &[line("user", "u-2", "One more turn.")]);
    setup.hook("stop", &setup.payload("Stop"));
    assert_eq!(setup.control()["due"], false, "one turn of two");
    setup.append(ON, &[line("user", "u-3", "And another.")]);
    let out = setup.hook("stop", &setup.payload("Stop"));
    assert!(out.stdout.is_empty());
    assert_eq!(setup.control()["due"], true, "two turns since the pass");

    setup.hook("session-end", &setup.payload("SessionEnd"));
    assert_eq!(setup.control()["final"], true);
    assert_eq!(setup.prompts().len(), calls, "the hooks call no model");

    // The final pass writes a final record and settles the switch.
    let report = setup.pass();
    assert_eq!(report["passes"][0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["status"], "final");
    assert_eq!(setup.control()["due"], false);
    assert_eq!(setup.control()["final"], false);

    // By minutes: on since long ago, never passed, no new turns needed.
    let minutes = Setup::new();
    minutes.config("[catalog]\nevery_turns = 50\nevery_minutes = 5\n");
    let control = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-01-01T00:00:00Z", "due": false, "final": false}}});
    let file = minutes.fx.root().join("control.json");
    std::fs::write(&file, format!("{control}\n")).unwrap();
    make_private(&file);
    minutes.hook("stop", &minutes.payload("Stop"));
    assert_eq!(minutes.control()["due"], true);
    assert!(minutes.prompts().is_empty());
}

#[test]
fn a_pass_refuses_when_a_configured_redact_name_is_unset() {
    // STD-05 R13: [redact] env names a value scribe must mask, but the
    // routine's environment (the plugin's env_pass, here the cleared test
    // environment) does not carry it. The pass refuses before any model
    // call or write, rather than storing text unmasked without warning.
    let setup = Setup::new();
    setup.config("[redact]\nenv = [\"SCRIBE_UNSET_REDACT_TOKEN\"]\n");
    setup.scribe("/scribe on");
    let out = setup
        .fx
        .cmd(&["session", "pass", "--json"])
        .env("PATH", &setup.path)
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(
        text(&out.stderr).contains("SCRIBE_UNSET_REDACT_TOKEN"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stderr).contains("redact_env_unset")
            || text(&out.stderr).contains("[redact] env"),
        "{}",
        text(&out.stderr)
    );
    assert!(setup.record(ON).is_none(), "nothing was written");
    assert_eq!(setup.control()["due"], true, "the pass stays due");
    assert!(setup.prompts().is_empty(), "no model call before refusal");

    // With the name supplied, the same pass writes as usual.
    let out = setup
        .fx
        .cmd(&["session", "pass", "--json"])
        .env("PATH", &setup.path)
        .env("SCRIBE_UNSET_REDACT_TOKEN", "a-value-that-must-be-masked")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(setup.record(ON).is_some());
}

#[test]
fn a_pass_writes_one_record_of_daniels_terms_and_never_reads_a_session_never_switched_on() {
    let setup = Setup::new();
    let never_before = std::fs::read(setup.transcript(NEVER)).unwrap();
    // A Stop and a SessionEnd for the never-on session touch nothing.
    let mut never = setup.payload("Stop");
    never["session_id"] = json!(NEVER);
    never["transcript_path"] = json!(setup.transcript(NEVER));
    setup.hook("stop", &never);
    never["hook_event_name"] = json!("SessionEnd");
    setup.hook("session-end", &never);
    assert!(!support::has_control(&setup.fx.root()));

    setup.scribe("/scribe on");
    let report = setup.pass();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1);
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(passes[0]["dropped_terms"], 1, "the assistant's term");

    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "a-1");
    assert_eq!(record["passes"], 1);
    assert_eq!(
        record["terms"],
        json!([{"term": "graduate to level 4", "said": DEFINED, "message": "u-1"}])
    );
    assert!(
        support::exported(&setup.fx.root(), "entries/preferences/preferences-1.md").is_none()
            && setup.entry_count() == 0,
        "no entries"
    );
    // The record is written at format 23: a header and one open segment
    // holding this pass's segment text.
    let file = setup.record_text();
    assert!(file.starts_with("---\nformat: 31\n"), "{file}");
    assert!(file.contains("\n## Log\n### 1. "), "{file}");
    assert!(!file.contains("## Outcome"), "{file}");
    assert_eq!(record["active"], true);
    assert_eq!(record["outcome"], "");
    assert_eq!(record["log"][0]["state"], "open");
    assert_eq!(record["log"][0]["origin"], "pass");
    assert_eq!(record["log"][0]["last_message"], "a-1");

    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(DEFINED));
    assert!(!prompts[0].contains(NEVER_SAID));
    assert!(setup.record(NEVER).is_none());
    assert_eq!(
        std::fs::read(setup.transcript(NEVER)).unwrap(),
        never_before
    );

    // An invalid answer keeps the previous record and leaves the pass due.
    let before = setup.record_text().into_bytes();
    setup.append(ON, &[line("user", "u-2", "More on levels.")]);
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let mut over = answer();
    over["summary"] = json!(format!("{}\n## Injected", "s".repeat(1201)));
    setup.answer(&over);
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "session_answer_invalid");
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(setup.control()["due"], true);
}

/// A `claude` that cannot answer: exits 3 without output (`model_failed`).
const BROKEN: &str = "#!/bin/sh\ncat >/dev/null\nexit 3\n";

#[test]
fn a_failed_pass_stays_due_exits_1_naming_it_and_the_next_run_retries_it() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.fx.fake_program("claude", BROKEN);
    // More failures in a row than the old give-up bound of three: each one
    // leaves the session due, whatever its code.
    for run in 1..=4 {
        let out = setup.run_any(&["session", "pass", "--session", ON], b"");
        assert_eq!(out.status.code(), Some(1), "run {run}");
        let stderr = text(&out.stderr);
        assert!(
            stderr.contains(&format!(
                "error: 1 session pass(es) failed: {ON} model_failed"
            )),
            "{stderr}"
        );
        assert!(
            text(&out.stdout).contains("failed"),
            "the report is printed"
        );
        assert_eq!(setup.control()["due"], true, "run {run} keeps it due");
        assert!(setup.record(ON).is_none());
    }
    // --json still prints the report, and the error object on stderr.
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    assert_eq!(report["passes"][0]["gave_up"], false);
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(reason.contains("model failed"), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    // The model's failure is logged first; the error object is the last line.
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
    let audit = support::audit_text(&setup.fx.root());
    assert!(
        !audit.contains(reason),
        "audit exposed the failure reason: {audit}"
    );

    // No claude at all (model_unavailable) is no different.
    std::fs::remove_file(setup.fx.elsewhere("bin/claude")).unwrap();
    let out = setup.run_any(&["session", "pass", "--session", ON], b"");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("model_unavailable"));
    assert_eq!(setup.control()["due"], true);

    // Once the model answers, the next run writes the record.
    setup.answer(&answer());
    let out = setup.run(&["session", "pass", "--session", ON, "--json"], b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "written");
    assert!(report["passes"][0].get("error").is_none());
    assert!(setup.record(ON).is_some());
    assert_eq!(setup.control()["due"], false);
}

#[test]
fn a_failed_pass_redacts_the_model_reason_in_json_and_the_aggregate_error() {
    const SECRET: &str = "pass-error-secret-value-123";
    const SECRET_NAME: &str = "SCRIBE_PASS_ERROR_SECRET";
    let setup = Setup::new();
    setup.config(&format!("[redact]\nenv = [\"{SECRET_NAME}\"]\n"));
    setup.scribe("/scribe on");
    let script =
        format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' 'model error: {SECRET}' >&2\nexit 3\n");
    setup.fx.fake_program("claude", &script);
    let mut command = setup
        .fx
        .cmd(&["session", "pass", "--session", ON, "--json"]);
    command.env("PATH", &setup.path).env(SECRET_NAME, SECRET);

    let out = setup.run_with(command, b"");

    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = report["passes"][0]["error"].as_str().unwrap();
    assert!(
        reason.contains("[redacted:SCRIBE_PASS_ERROR_SECRET]"),
        "{reason}"
    );
    assert!(!reason.contains(SECRET), "{reason}");
    assert!(reason.chars().count() <= 300, "{reason}");
    let stderr = text(&out.stderr);
    let error: Value = serde_json::from_str(stderr.lines().last().unwrap()).unwrap();
    assert_eq!(error["code"], "session_pass_failed", "{stderr}");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains(&format!("{ON} model_failed: {reason}")),
        "{}",
        error["error"]
    );
}

#[test]
fn session_names_a_switched_on_session_whose_due_was_cleared_and_passes_it() {
    let setup = Setup::new();
    // A session stranded as an earlier give-up rule left it: on, not due,
    // no record.
    let control = json!({"format": 1, "sessions": {ON: {
        "on": true, "since": "2026-11-03T05:00:00Z", "due": false, "final": false}}});
    setup.config("");
    let file = setup.fx.root().join("control.json");
    std::fs::write(&file, format!("{control}\n")).unwrap();
    make_private(&file);
    assert_eq!(setup.pass()["passes"], json!([]), "nothing is due");
    assert!(setup.prompts().is_empty());

    let out = setup.run(
        &[
            "session",
            "pass",
            "--session",
            ON,
            "--session",
            ON,
            "--json",
        ],
        b"",
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let passes = report["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 1, "named twice, passed once");
    assert_eq!(passes[0]["session"], ON);
    assert_eq!(passes[0]["result"], "written");
    assert_eq!(setup.record(ON).unwrap()["passes"], 1);
    assert_eq!(setup.control()["due"], false);

    // A session never switched on, or switched off, is refused and not read.
    let calls = setup.prompts().len();
    let out = setup.run_any(&["session", "pass", "--session", NEVER], b"");
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains(&format!("session {NEVER} is not switched on")),
        "{stderr}"
    );
    setup.scribe("/scribe off");
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_not_on");
    assert_eq!(setup.prompts().len(), calls, "no model call");
    assert!(setup.record(NEVER).is_none());
}

/// `scribe.session-pass` as Orbit calls it: the reply envelope.
fn plugin_pass(setup: &Setup) -> Value {
    let config = json!({
        "root": setup.fx.root(),
        "transcripts": setup.fx.home().join(".claude/projects"),
    });
    let envelope = json!({"schema_version": 1, "tool": "scribe.session-pass", "input": {},
                          "context": {"config": config}});
    let mut cmd = setup.fx.cmd(&["plugin", "call"]);
    cmd.env_remove("SCRIBE_ROOT").env("PATH", &setup.path);
    let out = setup.run_with(cmd, envelope.to_string().as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_plugin_tool_answers_session_pass_failed_and_keeps_the_session_due() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.fx.fake_program("claude", BROKEN);
    let reply = plugin_pass(&setup);
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "session_pass_failed", "{reply}");
    assert_eq!(reply["error"]["retryable"], false, "{reply}");
    let message = reply["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("{ON} model_failed: the model failed")),
        "{message}"
    );
    assert_eq!(setup.control()["due"], true);

    setup.answer(&answer());
    let reply = plugin_pass(&setup);
    assert_eq!(reply["output"]["passes"], json!([]));
    assert_eq!(reply["output"]["remaining"], 1);
    setup.expire_retry();
    let reply = plugin_pass(&setup);
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["output"]["passes"][0]["result"], "written", "{reply}");
    assert_eq!(setup.control()["due"], false);
}

/// A second session, spawned from `ON`.
const CHILD: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
/// A well-formed id scribe has never seen.
const UNKNOWN: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e62";

impl Setup {
    /// `/scribe …` typed in `session`: the hook's block reason.
    fn scribe_in(&self, session: &str, prompt: &str) -> String {
        let payload = json!({"session_id": session,
               "transcript_path": self.transcript(session),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": "UserPromptSubmit",
               "prompt": prompt});
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    /// The `error_code` of every audit line, oldest first.
    fn audit_codes(&self) -> Vec<Value> {
        support::audit_lines(&self.fx.root())
            .iter()
            .map(|l| l["error_code"].clone())
            .collect()
    }

    fn switch_of(&self, session: &str) -> Value {
        support::control(&self.fx.root())["sessions"][session].clone()
    }
}

#[test]
fn a_session_links_to_its_parent_and_its_pass_reads_the_parent_as_context() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    assert_eq!(setup.pass()["passes"][0]["result"], "written");
    setup.write(
        CHILD,
        &[line(
            "user",
            "c-1",
            "Side session: fix the flaky test only.",
        )],
    );

    // Refused, each with its code, and nothing switched or linked.
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {UNKNOWN}"));
    assert!(reason.contains("refused"), "{reason}");
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {CHILD}"));
    assert!(reason.contains("cannot link to itself"), "{reason}");
    assert_eq!(
        setup.switch_of(CHILD),
        Value::Null,
        "a refusal switches nothing"
    );
    assert_eq!(
        setup.audit_codes()[2..],
        [
            json!("link_session_off"),
            json!("link_unknown_session"),
            json!("link_self")
        ]
    );

    // Switched on and linked in one command; the link waits on the switch
    // until the first pass writes the record.
    let reason = setup.scribe_in(CHILD, &format!("/scribe on --from {ON}"));
    assert!(
        reason.starts_with(&format!(
            "scribe: on — this session is cataloged from now on and linked: spawned_from {ON}"
        )),
        "{reason}"
    );
    assert_eq!(setup.switch_of(CHILD)["on"], true);
    assert_eq!(
        setup.switch_of(CHILD)["pending_links"],
        json!([{"type": "spawned_from", "session": ON}])
    );
    let reason = setup.scribe_in(CHILD, &format!("/scribe link {ON}"));
    assert!(reason.ends_with("nothing changed"), "{reason}");

    // The parent links back to its child: a cycle, refused.
    let audit = setup.audit_lines();
    let reason = setup.scribe_in(ON, &format!("/scribe link {CHILD} --type continues"));
    assert!(reason.contains("cycle"), "{reason}");
    assert_eq!(setup.audit_codes()[audit..], [json!("link_cycle")]);

    // The child's pass carries the parent's latest record as context.
    let before = setup.prompts().len();
    let report = setup.pass();
    assert_eq!(report["passes"][0]["session"], CHILD, "{report}");
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), before + 1);
    let prompt = &prompts[before];
    for part in [
        format!("--- Linked session (data): this session was spawned from session {ON} ---"),
        String::from("title: Defining crew levels"),
        String::from("Daniel defined what level 4 means for crews."),
        String::from("do not retell the linked session"),
    ] {
        assert!(prompt.contains(&part), "{part:?} missing from:\n{prompt}");
    }

    // The link moved onto the child's record; the parent shows the child.
    assert_eq!(setup.switch_of(CHILD)["pending_links"], json!([]));
    let child = setup.record(CHILD).unwrap();
    assert_eq!(
        child["links"],
        json!([{"type": "spawned_from", "session": ON, "title": "Defining crew levels"}])
    );
    assert_eq!(child["related"], json!([]), "related is unchanged");
    let parent = setup.record(ON).unwrap();
    assert_eq!(parent["links"], json!([]));
    assert_eq!(
        parent["children"],
        json!([{"type": "spawned_from", "session": CHILD, "title": "Defining crew levels"}])
    );
    let tree: Value = serde_json::from_slice(
        &setup
            .run(&["session", "list", "--tree", "--json"], b"")
            .stdout,
    )
    .unwrap();
    let order: Vec<&str> = tree["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(order, [ON, CHILD]);
    let plain = setup.run(&["session", "list", "--tree"], b"");
    let rows: Vec<Vec<String>> = text(&plain.stdout)
        .lines()
        .map(|l| l.split('\t').map(str::to_owned).collect())
        .collect();
    assert_eq!(
        rows[1][..2],
        ["s-2".to_owned(), "s-1".to_owned()],
        "PARENT column"
    );
}

#[test]
fn s9_answers_write_bounded_segments_instead_of_one_outcome() {
    let setup = Setup::new();
    setup.append(
        ON,
        &[
            line("user", "u-2", "A second thread."),
            line("assistant", "a-2", "A third thread."),
        ],
    );
    setup.scribe("/scribe on");
    let mut body = answer();
    body.as_object_mut().unwrap().remove("outcome");
    body["open"] = Value::Null;
    body["new"] = json!(["u-1", "u-2", "a-2"].map(|id| json!({
        "starts_at": id, "heading": format!("Thread {id}"), "text": "é".repeat(900),
        "topics": [], "related": []
    })));
    setup.answer(&body);
    let out = setup.run_any(&["session", "pass", "--json"], b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let record = setup.record(ON).unwrap();
    assert_eq!(record["log"].as_array().unwrap().len(), 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["last_message"], "a-1");
    assert_eq!(record["log"][1]["last_message"], "u-2");
    assert_eq!(record["log"][2]["state"], "open");
}

#[test]
fn a_pass_refuses_when_the_record_changed_since_it_was_loaded() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.append(
        ON,
        &[
            line("user", "u-2", "More work."),
            line("assistant", "a-2", "Done."),
        ],
    );
    // The fake model waits while the test publishes a rebuilt record (a
    // new `updated` revision and summary), as a rebuild that finished
    // while the pass was running would. No timing race is involved.
    let root = setup.fx.root();
    let hold = support::Hold::new(&setup.fx.home(), move || {
        support::sql(
            &root,
            &format!(
                "UPDATE sessions SET updated = '2030-01-01T00:00:00Z',
                 summary = 'Rebuilt summary.' WHERE provider_session = '{ON}'"
            ),
        );
    });
    let script = format!(
        "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\nif [ \"$n\" = 1 ]; then\n{}fi\n{}",
        hold.script(),
        response(&answer())
    );
    setup.fx.fake_program("claude", &script);
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    hold.join();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(report["passes"][0]["error_code"], "record_changed");
    assert!(
        report["passes"][0]["error"]
            .as_str()
            .unwrap()
            .contains("changed since it was loaded"),
        "{}",
        report["passes"][0]["error"]
    );
    let record = setup.record(ON).unwrap();
    assert_eq!(
        record["updated"], "2030-01-01T00:00:00Z",
        "the rebuilt record is kept"
    );
    assert_eq!(record["summary"], "Rebuilt summary.");
    assert_eq!(record["scribed_through"], "a-1", "{record}");
    assert_eq!(setup.control()["due"], true, "the session stays due");
}

fn rebuild_args(confirm: bool) -> Vec<&'static str> {
    let mut args = vec!["session", "rebuild", ON, "--author", "test-agent", "--json"];
    if confirm {
        args.push("--confirm");
    }
    args
}

#[test]
fn rebuild_refusals_are_audited_and_leave_the_record_unchanged() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let before = setup.record_text().into_bytes();
    let out = setup.run_any(&rebuild_args(false), b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "confirm_required");
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("1 segment(s) and 1 pass(es)")
    );
    setup.scribe("/scribe off");
    setup.scribe("/scribe on");
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "session_busy"
    );
    setup.scribe("/scribe off");
    std::fs::remove_file(setup.transcript(ON)).unwrap();
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stderr).unwrap()["code"],
        "transcript_missing"
    );
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(setup.prompts().len(), 1, "no model calls for refusals");
    let rebuilds: Vec<Value> = support::audit_lines(&setup.fx.root())
        .into_iter()
        .filter(|l| l["command"] == "session rebuild")
        .collect();
    assert_eq!(rebuilds.len(), 3);
    for line in rebuilds {
        assert_eq!(line["target"], format!("session:{ON}"));
        assert_eq!(line["status"], "failure");
    }
}

#[test]
fn a_rebuild_of_a_forgotten_session_is_refused_before_any_model_call_or_checkpoint() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    // The transcript is still on disk, so only the refusal stands between
    // the rebuild and the model.
    assert!(s.transcript(ON).exists());
    let calls = s.prompts().len();
    let out = s.run_any(&rebuild_args(true), b"");
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0, "and keeps no rebuild checkpoint");
}

// Linux exposes the child's open lock descriptor, giving the test a handshake
// after the pre-lock tombstone check and before the held lock can be acquired.
#[cfg(target_os = "linux")]
#[test]
fn rebuild_waiting_for_lock_refuses_a_session_forgotten_during_the_wait() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::{Duration, Instant};

    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let calls = s.prompts().len();
    let path = s.fx.root().join("locks").join(format!("rebuild-{ON}.lock"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    make_private(path.parent().unwrap());
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    lock.lock().unwrap();
    let mut cmd = s.fx.cmd(&rebuild_args(true));
    let child = cmd
        .env("PATH", &s.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = support::ChildGuard::new(child);
    let fds = format!("/proc/{}/fd", guard.child().id());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let admitted = std::fs::read_dir(&fds)
            .unwrap()
            .flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|target| target == path));
        if admitted {
            break;
        }
        assert!(
            guard.child().try_wait().unwrap().is_none(),
            "rebuild exited before lock admission"
        );
        assert!(Instant::now() < deadline, "rebuild never opened its lock");
        std::thread::sleep(Duration::from_millis(5));
    }
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    lock.unlock().unwrap();
    let out = guard.finish();
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(s.prompts().len(), calls, "no rebuild model invocation");
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_catalog_of_a_forgotten_session_with_a_record_is_refused_before_any_model_call() {
    let s = Setup::new();
    s.scribe("/scribe on");
    s.pass();
    let forgot = text(&type_settlement(&s, "#scribe forget s-1").stdout);
    assert!(forgot.contains("scribe: forgot s-1"), "{forgot}");
    let calls = s.prompts().len();
    let out = s.run_any(
        &["session", "catalog", ON, "--author", "opus", "--json"],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_catalog_of_a_forgotten_session_never_passed_is_refused_before_any_model_call() {
    let s = Setup::new();
    // Switched on, never passed: a switch row with no record.
    let mut payload = s.payload("UserPromptSubmit");
    payload["session_id"] = json!(NEVER);
    payload["transcript_path"] = json!(s.transcript(NEVER));
    payload["prompt"] = json!("/scribe on");
    s.hook("user-prompt-submit", &payload);
    let forgot = text(&type_settlement(&s, &format!("#scribe forget {NEVER}")).stdout);
    assert!(forgot.contains("scribe: forgot s-"), "{forgot}");
    let calls = s.prompts().len();
    let out = s.run_any(
        &["session", "catalog", NEVER, "--author", "opus", "--json"],
        b"",
    );
    assert_eq!(out.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], "session_forgotten", "{error}");
    assert_eq!(
        s.prompts().len(),
        calls,
        "a forgotten session takes no model call"
    );
    assert_eq!(s.checkpoint_count(), 0);
}

#[test]
fn a_pass_window_that_fails_rolls_its_closures_back_and_audits_none() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let proposal = json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "decisions-1",
        "newer": null,
        "candidate": {
            "tag": "decisions",
            "role": "user",
            "quote": "I always keep decisions item 1 documented.",
            "evidence": null,
            "cite": {
                "session": ON,
                "message": "u-1",
                "at": "2026-11-03T06:00:00Z",
                "prompt_sha256": "0".repeat(64),
                "line": 1,
                "cwd": "/nonexistent/work/scribe"
            },
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": null
        },
        "reason": "Unmarked decisions statement.",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-03T06:00:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null
    });
    seed_claims(&setup.fx.root(), &[proposal]);

    // A trigger refuses the record's row, so the window's transaction fails
    // after it closed the claim (phase 7: the closure and the record commit
    // together, so nothing persisted is left to audit).
    support::sql(
        &setup.fx.root(),
        "CREATE TRIGGER staged_failure BEFORE UPDATE ON sessions WHEN NEW.has_record = 1
         BEGIN SELECT RAISE(ABORT, 'staged failure'); END;",
    );
    let out = setup.run_any(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["result"], "failed");
    assert_eq!(
        report["passes"][0]["error_code"], "store_database",
        "{report}"
    );

    let stored = support::proposals_doc(&setup.fx.root());
    assert_eq!(stored["proposals"][0]["state"], "open");
    assert_eq!(stored["proposals"][0]["closed_reason"], Value::Null);

    let pass_lines: Vec<Value> = support::audit_lines(&setup.fx.root())
        .into_iter()
        .filter(|line| line["command"] == "session pass")
        .collect();
    let closures: Vec<&Value> = pass_lines
        .iter()
        .filter(|line| line["status"] == "success" && line["target"] == "p-1")
        .collect();
    assert!(closures.is_empty(), "{pass_lines:?}");
    let failures: Vec<&Value> = pass_lines
        .iter()
        .filter(|line| line["status"] == "failure" && line["error_code"] == "store_database")
        .collect();
    assert_eq!(failures.len(), 1, "{pass_lines:?}");
}

#[test]
fn rebuild_resumes_hidden_checkpoints_and_preserves_identity_links_and_activity() {
    use scribe_core::activity::CatalogedBy;
    use scribe_core::{Author, AuthorKind, LinkType, Redactor, SessionLink, Store};
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let store = Store::open(setup.fx.root());
    let id = ON.parse().unwrap();
    let mut original = store.session(&id).unwrap().record;
    original.active = false;
    original.keep_active = true;
    original.active_change = Some(scribe_core::activity::ActiveChange {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "curator".parse().unwrap(),
        },
        route: scribe_core::activity::ActiveRoute::Manual,
        reason: String::from("No longer current"),
        cite: Some(String::from("session:source#1")),
    });
    original.links = vec![SessionLink {
        kind: LinkType::Continues,
        session: NEVER.parse().unwrap(),
    }];
    original.cataloged_by = Some(CatalogedBy {
        at: original.created,
        by: Author {
            kind: AuthorKind::Agent,
            name: "cataloger".parse().unwrap(),
        },
    });
    original.status = scribe_core::SessionStatus::Final;
    original.log[0].state = scribe_core::SegmentState::Sealed;
    original.log[0].origin = scribe_core::SegmentOrigin::Upgrade;
    let expected = original.updated;
    store
        .settle_pass(
            &id,
            Some(original.clone()),
            false,
            Some(expected),
            &Redactor::new([]),
        )
        .unwrap();
    let before = setup.record_text().into_bytes();
    setup.config("[catalog]\nwindow_messages = 2\nsegment_windows = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "New work."),
            line("assistant", "a-2", "Built it."),
            line("user", "u-3", "Finish."),
        ],
    );
    setup.answers(&[None, Some(answer()), None, Some(answer()), Some(answer())]);
    let out = setup.run_any(&rebuild_args(true), b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 1);
    assert_eq!(report["passes"][0]["error_code"], "model_failed");
    assert!(support::has_checkpoint(&setup.fx.root(), ON));
    assert_eq!(setup.record_text().into_bytes(), before);
    assert_eq!(
        store
            .list_sessions(&scribe_core::SessionFilter {
                include_inactive: true,
                ..Default::default()
            })
            .unwrap()
            .listing
            .items
            .len(),
        1
    );
    let out = setup.run(&rebuild_args(true), b"");
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    assert_eq!(
        report["passes"][0]["author"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let shown: Value =
        serde_json::from_slice(&setup.run(&["session", "show", ON, "--json"], b"").stdout).unwrap();
    assert_eq!(
        shown["rebuilt_by"],
        json!({"kind": "agent", "name": "test-agent"})
    );
    let rebuild_line = support::audit_lines(&setup.fx.root())
        .into_iter()
        .rfind(|l| l["command"] == "session rebuild")
        .unwrap();
    assert_eq!(rebuild_line["status"], "success");
    assert_eq!(rebuild_line["author"], "test-agent");
    let rebuilt = store.session(&id).unwrap().record;
    assert_eq!(rebuilt.id, original.id);
    assert_eq!(rebuilt.created, original.created);
    assert_eq!(rebuilt.links, original.links);
    assert_eq!(rebuilt.active, original.active);
    assert_eq!(rebuilt.keep_active, original.keep_active);
    assert_eq!(rebuilt.active_change, original.active_change);
    assert_eq!(rebuilt.cataloged_by, original.cataloged_by);
    assert_eq!(rebuilt.scribed_through.as_deref(), Some("u-3"));
    assert_eq!(rebuilt.passes, 3);
    assert_eq!(rebuilt.status, scribe_core::SessionStatus::Final);
    assert!(
        rebuilt
            .log
            .iter()
            .all(|s| s.origin == scribe_core::SegmentOrigin::Rebuild
                && s.state == scribe_core::SegmentState::Sealed)
    );
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
    let prompts = setup.prompts();
    assert!(
        !prompts[3].contains("--- user · u-1 ---"),
        "resume starts at failed window"
    );
    assert!(prompts[3].contains("--- user · u-2 ---"));
    assert!(
        prompts[1].contains("There is no record yet"),
        "first rebuild window has no previous record"
    );
}

#[test]
fn rebuild_folds_newer_transcript_messages_after_a_visible_revision_changes() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    let extra = line("user", "u-later", "Work arriving during the rebuild.");
    // The fake model waits while the test performs the competing writer's
    // publication before its first rebuild answer is returned. No timing
    // race, sleeps or concurrent test load is involved.
    let root = setup.fx.root();
    let transcript = setup.transcript(ON);
    let hold = support::Hold::new(&setup.fx.home(), move || {
        support::sql(
            &root,
            &format!(
                "UPDATE sessions SET updated = '2030-01-01T00:00:00Z'
                 WHERE provider_session = '{ON}'"
            ),
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&transcript)
            .unwrap();
        writeln!(file, "{extra}").unwrap();
    });
    let script = format!(
        r#"#!/bin/sh
n=$(ls "$HOME" | grep -c '^claude-stdin')
cat > "$HOME/claude-stdin-$n"
if [ "$n" = 1 ]; then
{hold}fi
{response}
"#,
        hold = hold.script(),
        response = response(&answer())
    );
    setup.fx.fake_program("claude", &script);
    let control_before = setup.control_bytes();
    let out = setup.run(&rebuild_args(true), b"");
    hold.join();
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 2);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["scribed_through"], "u-later");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert_eq!(setup.control_bytes(), control_before);
    let prompts = setup.prompts();
    assert!(prompts[2].contains("--- user · u-later ---"));
    assert!(!prompts[2].contains("--- user · u-1 ---"));
}

#[test]
fn rebuild_repairs_a_repeated_third_window_budget_miss_and_reports_its_window() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over["open"] = Value::Null;
    over["new"] = json!([]);
    over["next"] = json!(["Keep this bullet", "é".repeat(1000)]);
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over.clone()),
        Some(over),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    assert_eq!(
        report["passes"][0]["warnings"],
        json!(["window 3: repaired next: cut at bullet boundary (budget 1000 characters)"])
    );
    assert!(text(&out.stderr).contains("window 3: repaired next"));
    assert_eq!(setup.prompts().len(), 5);
    let record = setup.record(ON).unwrap();
    assert_eq!(record["next"], "- Keep this bullet");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
}

#[test]
fn rebuild_compresses_an_overlong_third_window_header_even_when_open_is_omitted() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    setup.pass();
    setup.config("[catalog]\nwindow_messages = 2\n");
    setup.append(
        ON,
        &[
            line("user", "u-2", "Second window."),
            line("assistant", "a-2", "Continue."),
            line("user", "u-3", "Third window."),
        ],
    );
    let mut over = answer();
    over.as_object_mut().unwrap().remove("open");
    over["new"] = json!([]);
    over["summary"] = json!("s".repeat(1201));
    let compressed = json!({"title": "Rebuilt session", "summary": "All three windows.",
                            "goals": "Record the session.", "next": []});
    setup.answers(&[
        None,
        Some(answer()),
        Some(answer()),
        Some(over),
        Some(compressed),
    ]);

    let out = setup.run(&rebuild_args(true), b"");

    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["passes"][0]["windows"], 3);
    let prompts = setup.prompts();
    assert_eq!(prompts.len(), 5);
    assert!(prompts[3].contains("Third window."));
    assert!(!prompts[4].contains("Third window."), "window not resent");
    assert!(
        !prompts[4].contains("Level 4 is defined."),
        "segment not resent"
    );
    let record = setup.record(ON).unwrap();
    assert_eq!(record["summary"], "All three windows.");
    assert_eq!(record["scribed_through"], "u-3");
    assert_eq!(record["log"][0]["text"], "Level 4 is defined.");
    assert_eq!(record["log"][0]["windows"], 3);
    assert_eq!(record["log"][0]["state"], "sealed");
    assert_eq!(record["log"][0]["origin"], "rebuild");
    assert!(!support::has_checkpoint(&setup.fx.root(), ON));
}

fn candidate_answer(quote: &str, message: &str, tag: &str) -> Value {
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": message, "tag": tag}]);
    body
}

fn proposals(s: &Setup) -> Value {
    serde_json::from_slice(&s.fx.ok(&["proposal", "list", "--json"]).stdout).unwrap()
}

fn type_settlement(s: &Setup, prompt: &str) -> Output {
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!(prompt);
    s.hook("user-prompt-submit", &payload)
}

/// Daniel types `prompt`, holding one `#scribe` marker, as turn `uuid` of
/// the `ON` session, and the Stop after the reply catalogs it
/// (2_design.md §14.6): the fake `claude` answers the annotation call with
/// an entry under `tag` whose context is [`context_of`] `said`, with
/// `quote` as his words. The default pass answer is restored after.
/// Returns Stop's echo.
fn mark(s: &Setup, uuid: &str, prompt: &str, tag: &str, said: &str, quote: &str) -> String {
    let submitted = type_settlement(s, prompt);
    let echo: Value = serde_json::from_slice(&submitted.stdout).unwrap();
    assert_eq!(
        echo["systemMessage"], "scribe: marked — cataloged after the reply",
        "{echo}"
    );
    s.append(
        ON,
        &[
            line("user", uuid, prompt),
            line("assistant", &format!("a-{uuid}"), "Noted."),
        ],
    );
    s.answer(&json!({"marks": [{"marker": 1, "kind": "entry", "tag": tag,
                                "context": context_of(said), "subject": "the work at hand",
                                "quote": quote, "message": uuid, "reason": ""}]}));
    let stopped = s.hook("stop", &s.payload("Stop"));
    s.answer(&answer());
    let reply: Value = serde_json::from_slice(&stopped.stdout).unwrap();
    let said = reply["systemMessage"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        said.starts_with("scribe: added "),
        "{reply}\n{:?}",
        s.prompts().last()
    );
    said
}

#[test]
fn candidate_sources_and_existing_statements_are_checked_at_the_binary_boundary() {
    let quote = "I prefer concise messages during lengthy builds.";
    let cases = [
        ("assistant", quote.to_owned(), "u-1", "preferences", false),
        (
            "user",
            format!("Read this: <pasted_content>{quote}</pasted_content>"),
            "u-1",
            "preferences",
            false,
        ),
        (
            "user",
            format!("```text\n{quote}\n```"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", format!("> {quote}"), "u-1", "preferences", false),
        (
            "user",
            quote.to_owned(),
            "missing-turn",
            "preferences",
            false,
        ),
        (
            "user",
            "I never said the model's proposed sentence.".to_owned(),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "task", false),
        (
            "user",
            format!("#scribe preferences: {quote}"),
            "u-1",
            "preferences",
            false,
        ),
        ("user", quote.to_owned(), "u-1", "preferences", true),
    ];
    for (role, text, message, tag, existing) in cases {
        let s = Setup::new();
        s.write(ON, &[line(role, "u-1", &text)]);
        if existing {
            let root = s.fx.root();
            let dir = root.join("entries/preferences");
            std::fs::create_dir_all(&dir).unwrap();
            for path in [&root, &root.join("entries"), &dir] {
                make_private(path);
            }
            let path = dir.join("preferences-1.md");
            std::fs::write(&path, support::entry_text(1, quote)).unwrap();
            make_private(&path);
        }
        s.answer(&candidate_answer(quote, message, tag));
        s.scribe("/scribe on");
        s.run(&["session", "pass", "--session", ON, "--json"], b"");
        assert_eq!(
            proposals(&s)["total"],
            0,
            "unsafe source or duplicate: {role} {text}"
        );
    }
}

#[test]
fn rejected_candidates_are_never_repeated_and_are_counted_at_session_start() {
    let s = Setup::new();
    let quote = "I prefer concise messages during lengthy builds.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let start = s.hook("session-start", &s.payload("SessionStart"));
    assert!(text(&start.stdout).contains("1 proposal to review"));
    type_settlement(&s, "#scribe reject p-1");
    type_settlement(&s, "#scribe reject p-1");
    // New turn and punctuation/case changes still name the same statement.
    let repeated = "I prefer concise messages during lengthy builds!";
    s.append(ON, &[line("user", "u-2", repeated)]);
    s.answer(&candidate_answer(repeated, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    let ps = proposals(&s);
    assert_eq!(ps["total"], 1);
    assert_eq!(ps["proposals"][0]["state"], "rejected");
    let es: Value = serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(es["total"], 0);
    assert_eq!(
        s.hook("session-start", &s.payload("SessionStart")).stdout,
        b""
    );
}

#[test]
fn candidates_share_a_three_slot_budget_across_windows_and_keep_cataloging() {
    let s = Setup::new();
    let lines: Vec<_> = (0..5)
        .map(|n| {
            line(
                "user",
                &format!("u-{n}"),
                &format!("I always choose option {n}."),
            )
        })
        .collect();
    s.write(ON, &lines);
    let answers: Vec<_> = (0..5)
        .map(|n| {
            Some(candidate_answer(
                &format!("I always choose option {n}."),
                &format!("u-{n}"),
                "preferences",
            ))
        })
        .collect();
    s.answers(&answers);
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[catalog]\nwindow_messages = 1\n").unwrap();
    make_private(&config);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 3);
    assert_eq!(s.record(ON).unwrap()["passes"], 5);
    assert!(!s.control()["due"].as_bool().unwrap());
}

#[test]
fn candidate_open_cap_is_enforced_even_when_the_model_ignores_it() {
    let s = Setup::new();
    // The store-wide cap, not the per-tag one.
    s.config("[tags.preferences.capture]\nmax_open = 20\n");
    s.write(ON, &[line("user", "u-1", "I always choose option zero.")]);
    s.answer(&candidate_answer(
        "I always choose option zero.",
        "u-1",
        "preferences",
    ));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let template = proposals(&s)["proposals"][0].clone();
    let full: Vec<_> = (1..=19)
        .map(|n| {
            let mut p = template.clone();
            p["id"] = json!(format!("p-{n}"));
            p["older"] = json!(format!("preferences-{n}"));
            p["candidate"]["quote"] = json!(format!("Stored preference {n}."));
            p["candidate"]["statement"] = json!(format!("Stored preference {n}."));
            // A format-14 claim has no `context` (format 24).
            p["candidate"].as_object_mut().unwrap().remove("context");
            p
        })
        .collect();
    // The stored claims replace the pass's own: p-1 .. p-19.
    support::sql(&s.fx.root(), "DELETE FROM claims;");
    seed_claims(&s.fx.root(), &full);
    let quotes = [
        "I prefer faster incremental compilation.",
        "I always choose a blue interface.",
        "Always retain descriptive error messages.",
    ];
    let turns: Vec<_> = quotes
        .iter()
        .enumerate()
        .map(|(n, quote)| line("user", &format!("cap-{n}"), quote))
        .collect();
    s.append(ON, &turns);
    let mut body = answer();
    body["candidates"] = json!(quotes.iter().enumerate().map(|(n, quote)| json!({"quote":quote, "message":format!("cap-{n}"), "tag":"preferences"})).collect::<Vec<_>>());
    s.answer(&body);
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    s.append(
        ON,
        &[line("user", "u-2", "I prefer detailed release notes.")],
    );
    s.answer(&candidate_answer(
        "I prefer detailed release notes.",
        "u-2",
        "preferences",
    ));
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 20);
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-2");
}

#[test]
fn an_instructed_rejection_keeps_the_claim_from_being_proposed_again() {
    let s = Setup::new();
    let quote = "I always choose smaller pull requests for easier review.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["total"], 1);
    // Daniel's instruction, typed in the aide's interactive session after
    // the pass opened the claim (2_design.md §12.2).
    let aide = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e61";
    let words = "reject p-1, it was a one-off";
    let at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let mut turn = line("user", "aide-1", words);
    turn["timestamp"] = json!(at);
    turn["entrypoint"] = json!("claude-desktop");
    s.write(aide, &[turn]);
    let rejected: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "proposal",
                "settle",
                "p-1",
                "--reject",
                "--author",
                "opus",
                "--session",
                aide,
                "--instruction",
                words,
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["closed_reason"], "instructed");
    assert_eq!(s.entry_count(), 0, "no entry is written");
    // He says it again; the next pass finds the claim and proposes nothing.
    s.append(ON, &[line("user", "u-2", quote)]);
    s.answer(&candidate_answer(quote, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(s.record(ON).unwrap()["scribed_through"], "u-2");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["proposals"][0]["closed_reason"], "instructed");
}

#[test]
fn candidate_ids_are_reserved_for_approval_and_agent_settlement_is_refused() {
    let s = Setup::new();
    let quote = "I always choose smaller pull requests for easier review.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let denied = s.fx.run(&[
        "proposal",
        "settle",
        "p-1",
        "--approve",
        "--author",
        "agent",
        "--json",
    ]);
    assert_eq!(denied.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&denied.stderr).unwrap()["code"],
        "user_only"
    );
    let direct: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--author",
                "agent",
                "Agent analysis with a different content.",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(direct["id"], "preferences-2");
    // A line he marks is cataloged at Stop as his entry; it too skips the
    // id the candidate reserved.
    let said = mark(
        &s,
        "u-2",
        "A separate decision Daniel explicitly marks. #scribe preferences",
        "preferences",
        "Daniel marks a separate decision.",
        "A separate decision Daniel explicitly marks.",
    );
    assert!(said.starts_with("scribe: added preferences-3 "), "{said}");
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-3.md").is_some());
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], context_of(quote));
    type_settlement(&s, "#scribe forget preferences-1");
    assert_eq!(proposals(&s)["proposals"][0]["candidate"]["quote"], "");
    type_settlement(&s, "#scribe approve p-1");
    let forgotten: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(forgotten["content"], "");
}

#[test]
fn interrupted_candidate_approval_is_replayed_once_from_the_durable_proposal() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    // Approved journal durable; entry file not yet written at the crash point.
    approve_claim_in_db(&s.fx.root(), 1);
    s.run(&["consistency", "scan", "--json"], b"");
    s.run(&["consistency", "scan", "--json"], b"");
    let es: Value = serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(es["total"], 1);
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], context_of(quote));
    assert_eq!(entry["author"]["kind"], "user");
}

#[test]
fn approving_a_candidate_covered_by_a_later_entry_writes_no_second_entry() {
    let s = Setup::new();
    let quote = "I prefer concise commit subjects on every change.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    assert_eq!(proposals(&s)["proposals"][0]["older"], "preferences-1");
    let added: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--project",
                "scribe",
                "--author",
                "opus",
                quote,
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(added["id"], "preferences-2");
    let out = type_settlement(&s, "#scribe approve p-1");
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reply["systemMessage"],
        "scribe: approved p-1 — already covered by preferences-2; no new entry"
    );
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-1.md").is_none());
    let listed: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(listed["total"], 1);
    let proposal = &proposals(&s)["proposals"][0];
    assert_eq!(proposal["state"], "rejected");
    assert!(proposal["newer"].is_null());
    let replay = type_settlement(&s, "#scribe approve p-1");
    let again: Value = serde_json::from_slice(&replay.stdout).unwrap();
    let message = again["systemMessage"].as_str().unwrap();
    assert!(message.contains("refused"), "{message}");
    assert!(message.contains("already rejected"), "{message}");
    assert!(!message.contains("preferences-2"), "{message}");
}

#[test]
fn a_longer_refinement_of_a_rejected_candidate_still_opens() {
    let s = Setup::new();
    let quote = "I prefer concise messages during lengthy builds";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    type_settlement(&s, "#scribe reject p-1");
    let refined = "I prefer concise messages during lengthy builds in every repository";
    s.append(ON, &[line("user", "u-2", refined)]);
    s.answer(&candidate_answer(refined, "u-2", "preferences"));
    s.run(&["session", "pass", "--session", ON], b"");
    let ps = proposals(&s);
    assert_eq!(ps["total"], 2, "{ps}");
    assert_eq!(ps["proposals"][0]["state"], "rejected");
    assert_eq!(ps["proposals"][1]["state"], "open");
    assert_eq!(ps["proposals"][1]["candidate"]["quote"], refined);
}

#[test]
fn consistency_scan_reports_an_approved_candidate_closed_as_covered() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let added: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "preferences",
                "--project",
                "scribe",
                "--author",
                "opus",
                quote,
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(added["id"], "preferences-2");
    approve_claim_in_db(&s.fx.root(), 1);
    let first = s.run(&["consistency", "scan", "--json"], b"");
    let report: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["covered"][0]["proposal"], "p-1");
    assert_eq!(report["covered"][0]["by"], "preferences-2");
    assert!(report["reapplied"].as_array().unwrap().is_empty());
    assert!(support::exported(&s.fx.root(), "entries/preferences/preferences-1.md").is_none());
    let settled = &proposals(&s)["proposals"][0];
    assert_eq!(settled["state"], "rejected");
    assert!(settled["newer"].is_null());
    let listed: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(listed["total"], 1);
    let audit = support::audit_text(&s.fx.root());
    let covered_lines = audit
        .lines()
        .filter(|line| {
            line.contains("\"command\":\"consistency scan\"")
                && line.contains("\"target\":\"p-1\"")
                && line.contains("\"status\":\"success\"")
        })
        .count();
    assert_eq!(covered_lines, 1, "{audit}");
    s.run(&["consistency", "scan", "--json"], b"");
    let again = support::audit_text(&s.fx.root());
    let covered_again = again
        .lines()
        .filter(|line| {
            line.contains("\"command\":\"consistency scan\"")
                && line.contains("\"target\":\"p-1\"")
                && line.contains("\"status\":\"success\"")
        })
        .count();
    assert_eq!(covered_again, 1, "{again}");
    assert_eq!(
        serde_json::from_slice::<Value>(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap()["total"],
        1
    );
}

#[test]
fn pass_closes_retired_tag_candidates_before_cap_and_suppresses_their_quotes() {
    let s = Setup::new();
    s.scribe("/scribe on");
    let cite = |msg: &str| {
        json!({
            "session": ON,
            "message": msg,
            "at": "2026-11-15T00:00:00Z",
            "prompt_sha256": "0".repeat(64),
            "line": 1,
            "cwd": "/nonexistent/work/scribe"
        })
    };
    let mut initial = Vec::new();
    let tags = [
        ("decisions", 7),
        ("task", 7),
        ("finding", 2),
        ("preferences", 2),
        ("standard", 2),
    ];
    let mut n = 0usize;
    let mut retired_counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for (tag, count) in tags {
        for i in 1..=count {
            n += 1;
            let quote = if n == 1 {
                String::from("I always prefer retired rule one for builds.")
            } else {
                format!("I always keep {tag} item {i} in the store.")
            };
            let is_active = matches!(tag, "preferences" | "standard");
            if !is_active {
                *retired_counts.entry(tag).or_default() += 1;
            }
            initial.push(json!({
                "id": format!("p-{n}"),
                "kind": "candidate",
                "older": format!("{tag}-{i}"),
                "newer": null,
                "candidate": {
                    "tag": tag,
                    "role": "user",
                    "quote": quote,
                    "evidence": null,
                    "cite": cite(&format!("seed-{n}")),
                    "project": "scribe",
                    "provider": "claude-code",
                    "host": "host",
                    "admitted": null
                },
                "reason": format!("Unmarked {tag} statement: {quote}"),
                "quotes": [],
                "state": "open",
                "opened_at": "2026-11-15T00:01:00Z",
                "settled_at": null,
                "settled_by": null,
                "author": {"kind": "scribe", "name": "session-pass"},
                "updated_by": null,
                "settled_author": null,
                "closed_reason": null
            }));
        }
    }
    assert_eq!(initial.len(), 20);
    assert_eq!(retired_counts.values().sum::<usize>(), 16);
    seed_claims(&s.fx.root(), &initial);

    let new_quote = "I prefer concise summaries in every session pass.";
    s.write(ON, &[line("user", "u-new", new_quote)]);
    s.answer(&candidate_answer(new_quote, "u-new", "preferences"));
    let audit_before = s.audit_lines();
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let listed = proposals(&s);
    assert_eq!(listed["total"], 21, "{listed}");
    let list = listed["proposals"].as_array().unwrap();
    for p in &list[..16] {
        assert_eq!(p["state"], "rejected", "{p}");
        assert_eq!(p["closed_reason"], "tag_retired", "{p}");
        assert_eq!(
            p["settled_author"],
            json!({"kind": "scribe", "name": "tag-retired"}),
            "{p}"
        );
        assert!(p["settled_by"].is_null(), "{p}");
        assert!(!p["settled_at"].is_null(), "{p}");
    }
    for p in &list[16..20] {
        assert_eq!(p["state"], "open", "{p}");
        assert!(p["closed_reason"].is_null(), "{p}");
        assert!(p["settled_author"].is_null(), "{p}");
    }
    assert_eq!(list[20]["id"], "p-21");
    assert_eq!(list[20]["older"], "preferences-3");
    assert_eq!(list[20]["state"], "open");
    assert_eq!(list[20]["candidate"]["quote"], new_quote);
    assert_eq!(s.entry_count(), 0, "pass writes no entries");

    let new_audit: Vec<Value> = support::audit_lines(&s.fx.root())
        .into_iter()
        .skip(audit_before)
        .collect();
    let closed_audit_targets: Vec<&str> = new_audit
        .iter()
        .filter(|e| {
            e["command"] == "session pass"
                && e["status"] == "success"
                && e["target"].as_str().is_some_and(|t| t.starts_with("p-"))
        })
        .map(|e| e["target"].as_str().unwrap())
        .collect();
    assert_eq!(closed_audit_targets.len(), 16, "{new_audit:?}");
    for i in 1..=16 {
        assert!(closed_audit_targets.contains(&format!("p-{i}").as_str()));
    }

    // A closed candidate's quote stays in the suppression set, and a second
    // pass writes no more closes.
    let dup_quote = "I always prefer retired rule one for builds.";
    s.append(ON, &[line("user", "u-dup", dup_quote)]);
    s.answer(&candidate_answer(dup_quote, "u-dup", "preferences"));
    let audit_before_second = s.audit_lines();
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(
        proposals(&s)["total"],
        21,
        "closed candidate quote is still suppressed"
    );
    let second_audit: Vec<Value> = support::audit_lines(&s.fx.root())
        .into_iter()
        .skip(audit_before_second)
        .collect();
    assert_eq!(second_audit.len(), 1, "only the session pass line");
    assert_eq!(second_audit[0]["target"], format!("session:{ON}"));

    // `proposal list --state rejected` shows `tag_retired` in table and JSON.
    let rejected_json: Value = serde_json::from_slice(
        &s.fx
            .ok(&["proposal", "list", "--state", "rejected", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(rejected_json["total"], 16);
    assert!(
        rejected_json["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["closed_reason"] == "tag_retired")
    );
    let rejected_table = text(
        &s.fx
            .ok(&[
                "proposal", "list", "--state", "rejected", "--format", "table",
            ])
            .stdout,
    );
    assert!(rejected_table.contains("CLOSED_REASON"), "{rejected_table}");
    assert_eq!(
        rejected_table.matches("tag_retired").count(),
        16,
        "{rejected_table}"
    );
}

#[test]
fn pass_with_custom_tag_config_keeps_open_preferences_candidates() {
    let s = Setup::new();
    s.scribe("/scribe on");
    let config = s.fx.root().join("config.toml");
    std::fs::write(&config, "[tags.remind]\naction = true\n").unwrap();
    make_private(&config);

    let open_pref = json!({
        "id": "p-1",
        "kind": "candidate",
        "older": "preferences-1",
        "newer": null,
        "candidate": {
            "tag": "preferences",
            "role": "user",
            "quote": "I always prefer clear commit messages.",
            "evidence": null,
            "cite": {
                "session": ON,
                "message": "seed-1",
                "at": "2026-11-15T00:00:00Z",
                "prompt_sha256": "0".repeat(64),
                "line": 1,
                "cwd": "/nonexistent/work/scribe"
            },
            "project": "scribe",
            "provider": "claude-code",
            "host": "host",
            "admitted": null
        },
        "reason": "Unmarked preferences statement: I always prefer clear commit messages.",
        "quotes": [],
        "state": "open",
        "opened_at": "2026-11-15T00:01:00Z",
        "settled_at": null,
        "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"},
        "updated_by": null,
        "settled_author": null,
        "closed_reason": null
    });
    seed_claims(&s.fx.root(), &[open_pref]);

    let new_quote = "I prefer concise summaries in every session pass.";
    s.write(ON, &[line("user", "u-new", new_quote)]);
    s.answer(&candidate_answer(new_quote, "u-new", "preferences"));
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let listed = proposals(&s);
    assert_eq!(listed["total"], 2, "{listed}");
    let list = listed["proposals"].as_array().unwrap();
    assert_eq!(list[0]["id"], "p-1");
    assert_eq!(list[0]["state"], "open");
    assert!(list[0]["closed_reason"].is_null());
    assert_eq!(list[1]["id"], "p-2");
    assert_eq!(list[1]["state"], "open");
    assert_eq!(list[1]["candidate"]["tag"], "preferences");
}

#[test]
fn forget_clears_quote_window_retirement_reason_and_admitted_matched_through_cli() {
    let s = Setup::new();
    // Configure a wildcard trigger so `admitted.matched` carries words from
    // Daniel's turn that must be erased on `#scribe forget`.
    s.config("[tags.preferences.capture]\ntriggers = [\"always * checked\"]\n");
    let candidate_quote = "I always keep secret-matched-word checked before merging.";
    s.write(
        ON,
        &[
            line("user", "u-1", candidate_quote),
            line("user", "u-2", "secret-window-context from earlier turn"),
            line(
                "user",
                "u-3",
                "secret-quoted-correction from Daniel's typed turn",
            ),
        ],
    );
    s.answer(&candidate_answer(candidate_quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    type_settlement(&s, "#scribe approve p-1");

    // 1) Candidate-backed entry with `admitted.matched`.
    let before_prop = proposals(&s)["proposals"][0].clone();
    assert_eq!(
        before_prop["candidate"]["admitted"]["trigger"],
        "always * checked"
    );
    assert_eq!(
        before_prop["candidate"]["admitted"]["matched"],
        "always keep secret-matched-word checked"
    );

    type_settlement(&s, "#scribe forget preferences-1");

    let forgot_pref: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(forgot_pref["status"], "forgotten");
    assert_eq!(forgot_pref["said"], "");
    assert_eq!(forgot_pref["content"], "");
    let after_prop = proposals(&s)["proposals"][0].clone();
    assert_eq!(after_prop["candidate"]["quote"], "");
    assert_eq!(after_prop["candidate"]["admitted"]["matched"], "");
    assert_eq!(
        after_prop["candidate"]["admitted"]["trigger"],
        "always * checked"
    );
    let prop_list_plain = text(&s.fx.ok(&["proposal", "list"]).stdout);
    let prop_list_table = text(&s.fx.ok(&["proposal", "list", "--format", "table"]).stdout);
    for rendered in [&prop_list_plain, &prop_list_table] {
        assert!(!rendered.contains("secret-matched-word"), "{rendered}");
    }

    // 2) Action entry with a transcript `window` from a turn marked
    //    `#scribe standard`, cataloged at Stop.
    mark(
        &s,
        "u-4",
        "secret-window-rule for CI #scribe standard",
        "standard",
        "Daniel's CI rule: secret-window-rule for CI.",
        "secret-window-rule for CI",
    );
    let before_win: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "standard-1", "--window", "--json"])
            .stdout,
    )
    .unwrap();
    assert!(
        before_win["window"].is_object(),
        "action capture keeps its window: {before_win}"
    );
    assert!(before_win["window_turns"].as_u64().unwrap_or(0) > 0);
    type_settlement(&s, "#scribe forget standard-1");
    let after_win: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "standard-1", "--window", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(after_win["status"], "forgotten");
    assert_eq!(after_win["said"], "");
    assert_eq!(after_win["content"], "");
    assert!(after_win["window"].is_null(), "{after_win}");
    assert!(after_win["window_turns"].is_null(), "{after_win}");

    // 3) Agent action entry with `quote` and `retirement_reason`.
    let session_rel = format!("session:{ON}");
    let added: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "add",
                "--tag",
                "standard",
                "--author",
                "opus",
                "--related",
                &session_rel,
                "--quote",
                "secret-quoted-correction from Daniel's typed turn",
                "secret-agent-rule for builds",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(added["id"], "standard-2");
    s.fx.ok(&[
        "entry",
        "retire",
        "standard-2",
        "--author",
        "opus",
        "--reason",
        "secret-retire-reason explaining why",
        "--json",
    ]);
    let before_retired: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "standard-2", "--json"]).stdout)
            .unwrap();
    assert_eq!(
        before_retired["quote"],
        "secret-quoted-correction from Daniel's typed turn"
    );
    assert_eq!(
        before_retired["retirement_reason"],
        "secret-retire-reason explaining why"
    );
    type_settlement(&s, "#scribe forget standard-2");
    let after_retired: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "standard-2", "--json"]).stdout)
            .unwrap();
    assert_eq!(after_retired["status"], "forgotten");
    assert_eq!(after_retired["said"], "");
    assert_eq!(after_retired["content"], "");
    assert!(after_retired["quote"].is_null(), "{after_retired}");
    assert!(
        after_retired["retirement_reason"].is_null(),
        "{after_retired}"
    );

    // Every entry reads back cleanly with zero warnings, renders in plain/table
    // without error, and none of the forgotten words remain on disk.
    let listed: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "list", "--json"]).stdout).unwrap();
    assert_eq!(listed["warnings"], json!([]));
    assert_eq!(listed["total"], 3);
    for id in ["preferences-1", "standard-1", "standard-2"] {
        let plain = text(&s.fx.ok(&["entry", "show", id]).stdout);
        let table = text(&s.fx.ok(&["entry", "show", id, "--format", "table"]).stdout);
        for rendered in [&plain, &table] {
            for secret in [
                "secret-matched-word",
                "secret-window-context",
                "secret-window-rule",
                "secret-quoted-correction",
                "secret-agent-rule",
                "secret-retire-reason",
            ] {
                assert!(!rendered.contains(secret), "{id}: {rendered}");
            }
        }
    }
    // The export, the old files' shapes: the move into `pre-sqlite-*` keeps
    // the user's originals by design, so the store's own content is checked.
    let root = s.fx.root();
    let disk = support::exported(&root, "proposals.json").unwrap()
        + &support::exported(&root, "entries/preferences/preferences-1.md").unwrap()
        + &support::exported(&root, "entries/standard/standard-1.md").unwrap()
        + &support::exported(&root, "entries/standard/standard-2.md").unwrap();
    for secret in [
        "secret-matched-word",
        "secret-window-context",
        "secret-window-rule",
        "secret-quoted-correction",
        "secret-agent-rule",
        "secret-retire-reason",
    ] {
        assert!(
            !disk.contains(secret),
            "forgotten text {secret:?} must leave disk"
        );
    }
}

#[test]
fn pass_and_catalog_audit_persisted_closures_when_marking_failure_aborts() {
    let seed_retired_candidate = |s: &Setup, session: &str| {
        let proposal = json!({
            "id": "p-1",
            "kind": "candidate",
            "older": "decisions-1",
            "newer": null,
            "candidate": {
                "tag": "decisions",
                "role": "user",
                "quote": "I always keep decisions item 1 documented.",
                "evidence": null,
                "cite": {
                    "session": session,
                    "message": "seed-1",
                    "at": "2026-11-15T00:00:00Z",
                    "prompt_sha256": "0".repeat(64),
                    "line": 1,
                    "cwd": "/nonexistent/work/scribe"
                },
                "project": "scribe",
                "provider": "claude-code",
                "host": "host",
                "admitted": null
            },
            "reason": "Unmarked decisions statement.",
            "quotes": [],
            "state": "open",
            "opened_at": "2026-11-15T00:01:00Z",
            "settled_at": null,
            "settled_by": null,
            "author": {"kind": "scribe", "name": "session-pass"},
            "updated_by": null,
            "settled_author": null,
            "closed_reason": null
        });
        seed_claims(&s.fx.root(), &[proposal]);
    };

    // (a) `session pass`: window 1 persists the retired-tag closure of `p-1`;
    // window 2 replaces `control.json` with a symlink and fails so
    // `job.failed(failure)?` returns `Err(store_symlink)` and bypasses
    // `Output::Pass`.
    {
        let s = Setup::new();
        s.config("[catalog]\nwindow_messages = 1\n");
        s.write(
            ON,
            &[
                line("user", "u-1", "First window message."),
                line("user", "u-2", "Second window message."),
            ],
        );
        s.scribe("/scribe on");
        seed_retired_candidate(&s, ON);
        let control_path = s.fx.root().join("control.json");
        let mut script = String::from(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\ncase $n in\n",
        );
        script.push_str(&format!("0) {} ;;\n", response(&answer())));
        script.push_str(&format!(
            "1) ln -sf /dev/null '{}' ; echo 'model boom' >&2 ; exit 3 ;;\n",
            control_path.display()
        ));
        script.push_str("*) exit 4 ;;\nesac\n");
        s.fx.fake_program("claude", &script);

        let audit_before = s.audit_lines();
        let out = s.run_any(&["session", "pass", "--session", ON, "--json"], b"");
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        let err: Value = serde_json::from_str(stderr.lines().last().unwrap()).expect(&stderr);
        assert_eq!(err["code"], "store_symlink", "{err}");

        // The export refuses the staged symlink; the symlink did its job.
        std::fs::remove_file(&control_path).unwrap();
        let stored = support::proposals_doc(&s.fx.root());
        assert_eq!(stored["proposals"][0]["state"], "rejected");
        assert_eq!(stored["proposals"][0]["closed_reason"], "tag_retired");

        let new_audit: Vec<Value> = support::audit_lines(&s.fx.root())
            .into_iter()
            .skip(audit_before)
            .collect();
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session pass"
                        && e["status"] == "success"
                        && e["target"] == "p-1"
                })
                .count(),
            1,
            "persisted closure p-1 must be audited on early ? abort: {new_audit:?}"
        );
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session pass"
                        && e["status"] == "failure"
                        && e["error_code"] == "store_symlink"
                })
                .count(),
            1,
            "{new_audit:?}"
        );
    }

    // (b) `session catalog`: same two-window failure where `job.failed` aborts.
    {
        let s = Setup::new();
        s.config("[catalog]\nwindow_messages = 1\n");
        s.write(
            NEVER,
            &[
                line("user", "u-1", "First catalog window."),
                line("user", "u-2", "Second catalog window."),
            ],
        );
        seed_retired_candidate(&s, NEVER);
        let control_path = s.fx.root().join("control.json");
        let mut script = String::from(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\ncase $n in\n",
        );
        script.push_str(&format!("0) {} ;;\n", response(&answer())));
        script.push_str(&format!(
            "1) ln -sf /dev/null '{}' ; echo 'model boom' >&2 ; exit 3 ;;\n",
            control_path.display()
        ));
        script.push_str("*) exit 4 ;;\nesac\n");
        s.fx.fake_program("claude", &script);

        let audit_before = s.audit_lines();
        let out = s.run_any(
            &["session", "catalog", NEVER, "--author", "opus", "--json"],
            b"",
        );
        let stderr = text(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        let err: Value = serde_json::from_str(stderr.lines().last().unwrap()).expect(&stderr);
        assert_eq!(err["code"], "store_symlink", "{err}");

        // The export refuses the staged symlink; the symlink did its job.
        std::fs::remove_file(&control_path).unwrap();
        let stored = support::proposals_doc(&s.fx.root());
        assert_eq!(stored["proposals"][0]["state"], "rejected");
        assert_eq!(stored["proposals"][0]["closed_reason"], "tag_retired");

        let new_audit: Vec<Value> = support::audit_lines(&s.fx.root())
            .into_iter()
            .skip(audit_before)
            .collect();
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session catalog"
                        && e["status"] == "success"
                        && e["target"] == "p-1"
                        && e["author"] == "opus"
                })
                .count(),
            1,
            "persisted closure p-1 must be audited on catalog early ? abort: {new_audit:?}"
        );
        assert_eq!(
            new_audit
                .iter()
                .filter(|e| {
                    e["command"] == "session catalog"
                        && e["status"] == "failure"
                        && e["error_code"] == "store_symlink"
                })
                .count(),
            1,
            "{new_audit:?}"
        );
    }
}

// --- statements (2_design.md §11.1) ------------------------------------------

#[test]
fn a_pass_claim_carries_a_context_and_subject_and_a_line_or_an_empty_subject_opens_nothing() {
    let s = Setup::new();
    let quote = "ok that makes sense. I prefer that session records keep the whole history in an append-only log.";
    let vague = "I prefer we keep it like this from now on.";
    let long = "I prefer squash merges on every agent branch we open.";
    let terse = "I prefer the short form of every release note.";
    s.write(
        ON,
        &[
            line("user", "u-1", quote),
            line("user", "u-2", vague),
            line("user", "u-3", long),
            line("user", "u-4", terse),
        ],
    );
    // The design's example (2_design.md §15.3): what was being discussed
    // and what the remark settles, standing alone.
    let context = "The user and the agent were choosing how scribe stores session records. The user rejected a single 2,500-character outcome because rewrites lose the session's start, and asked for an append-only log.";
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": quote, "message": "u-1", "tag": "preferences",
            "context": context, "subject": "session records"},
        // The window does not name what this turn is about.
        {"quote": vague, "message": "u-2", "tag": "preferences",
            "context": "The user and the agent were reviewing it. The user prefers to keep it like this.", "subject": " "},
        // A subject past 80 characters is not a short noun phrase.
        {"quote": long, "message": "u-3", "tag": "preferences",
            "context": "The user and the agent were merging. The user prefers squash merges on agent branches.",
            "subject": "x".repeat(81)},
        // A single line does not stand alone (2_design.md §15.3).
        {"quote": terse, "message": "u-4", "tag": "preferences",
            "context": "The user prefers short release notes.", "subject": "release notes"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    assert_eq!(candidate["quote"], quote);
    assert_eq!(candidate["context"], context);
    assert_eq!(candidate["statement"], Value::Null);
    assert_eq!(candidate["subject"], "session records");
    // The request asked for both, within their bounds.
    let prompt = s.prompts().join("\n");
    assert!(prompt.contains("context (a paragraph of"), "{prompt}");

    // `proposal list` and `proposal show` read the context first and
    // Daniel's words after it; `proposal show --json` carries it.
    let plain = text(&s.fx.ok(&["proposal", "list"]).stdout);
    assert!(
        plain.contains(&format!("\t{context} | The user's words: \"{quote}\"\t")),
        "{plain}"
    );
    let shown = text(&s.fx.ok(&["proposal", "show", "p-1"]).stdout);
    assert!(
        shown.contains(&format!("\t{context} | The user's words: \"{quote}\"\t")),
        "{shown}"
    );
    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["proposal", "show", "p-1", "--json"]).stdout).unwrap();
    assert_eq!(shown["candidate"]["context"], context);

    // Approval writes scribe's context as content and his words as quote;
    // the entry stays his.
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], context);
    assert_eq!(entry["quote"], quote);
    assert_eq!(
        entry["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(entry["author"]["kind"], "user");
    assert_eq!(entry["content_from"]["kind"], "message");
    assert_eq!(entry["content_from"]["role"], "user");
    assert_eq!(entry["content_from"]["message"], "u-1");
    assert_eq!(entry["cite"]["message"], "u-1");
    let plain = text(&s.fx.ok(&["entry", "show", "preferences-1"]).stdout);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("stated_by\tscribe:session-pass\t{quote}")),
        "{plain}"
    );

    // A turn he marks with `#scribe` is cataloged at Stop (§14): the
    // annotation states it and keeps his words as the quote; the entry
    // stays his.
    let words = "always run the gate before a push";
    let gate = "Daniel runs the gate before every push.";
    mark(
        &s,
        "u-5",
        &format!("{words} #scribe preferences"),
        "preferences",
        gate,
        words,
    );
    let typed: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-2", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(typed["content"], context_of(gate));
    assert_eq!(
        typed["stated_by"],
        json!({"kind": "scribe", "name": "annotation"})
    );
    assert_eq!(typed["quote"], words);
    assert_eq!(typed["author"]["kind"], "user");
}

#[test]
fn a_candidate_from_before_statements_approves_with_its_quote_as_content() {
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    // Rewrite it as a format-13 store holds it: no statement, no subject.
    support::sql(
        &s.fx.root(),
        "UPDATE claims SET c_context = NULL, c_statement = NULL, c_subject = NULL WHERE n = 1;",
    );
    assert_eq!(
        proposals(&s)["proposals"][0]["candidate"]["statement"],
        Value::Null
    );
    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], quote);
    assert_eq!(entry["quote"], Value::Null);
    assert_eq!(entry["stated_by"], Value::Null);
    assert_eq!(entry["author"]["kind"], "user");
}

#[test]
fn a_format_23_claim_reads_with_no_context_shows_its_statement_as_an_older_claim_and_approves_it() {
    // 2_design.md §15.3: a claim opened before format 24 is not rewritten.
    // It reads with `context: null`, `proposal show` puts its statement in
    // the context's place labelled `older claim`, and approval keeps the
    // statement as content; the next write persists format 24.
    let s = Setup::new();
    let quote = "I prefer short explanations beside each changed behavior.";
    let statement = "The user prefers a short explanation beside each changed behavior.";
    s.write(ON, &[line("user", "u-1", quote)]);
    s.answer(&candidate_answer(quote, "u-1", "preferences"));
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON], b"");
    let doc = support::proposals_doc(&s.fx.root());
    assert_eq!(doc["format"], 31);
    assert!(doc["proposals"][0]["candidate"]["context"].is_string());
    let conn = rusqlite::Connection::open(s.fx.root().join("scribe.db")).unwrap();
    conn.execute(
        "UPDATE claims SET c_context = NULL, c_statement = ?1 WHERE n = 1",
        [statement],
    )
    .unwrap();
    drop(conn);

    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["proposal", "show", "p-1", "--json"]).stdout).unwrap();
    assert_eq!(shown["candidate"]["context"], Value::Null);
    assert_eq!(shown["candidate"]["statement"], statement);
    let plain = text(&s.fx.ok(&["proposal", "show", "p-1"]).stdout);
    assert!(
        plain.contains(&format!(
            "\tolder claim: {statement} | The user's words: \"{quote}\"\t"
        )),
        "{plain}"
    );
    let table = text(
        &s.fx
            .ok(&["proposal", "show", "p-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains(&format!("older claim: {statement}")),
        "{table}"
    );

    type_settlement(&s, "#scribe approve p-1");
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], statement);
    assert_eq!(entry["quote"], quote);
    assert_eq!(
        entry["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    let doc = support::proposals_doc(&s.fx.root());
    assert_eq!(doc["format"], 31);
    assert_eq!(doc["proposals"][0]["candidate"]["context"], Value::Null);
    assert_eq!(doc["proposals"][0]["candidate"]["statement"], statement);
}

#[test]
fn a_context_needing_redaction_is_kept_redacted_and_a_quote_needing_it_is_omitted() {
    let s = Setup::new();
    let clean = "I prefer that the deploy job reads its token from the vault.";
    let leaky = "I prefer to paste sensitive-value-123 into the deploy job directly.";
    s.write(
        ON,
        &[line("user", "u-1", clean), line("user", "u-2", leaky)],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": clean, "message": "u-1", "tag": "preferences",
            "statement": "Daniel prefers that the deploy job reads sensitive-value-123 from the vault, never from the job file.",
            "subject": "the deploy job's sensitive-value-123"},
        {"quote": leaky, "message": "u-2", "tag": "preferences",
            "statement": "Daniel prefers pasting the deploy token into the job.",
            "subject": "the deploy job"}
    ]);
    s.answer(&body);
    s.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    s.scribe("/scribe on");
    let mut cmd = s.fx.cmd(&["session", "pass", "--session", ON, "--json"]);
    cmd.env("PATH", &s.path)
        .env("TEST_SECRET", "sensitive-value-123");
    let out = s.run_with(cmd, b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    assert_eq!(candidate["quote"], clean);
    assert_eq!(
        candidate["context"],
        context_of(
            "Daniel prefers that the deploy job reads [redacted:TEST_SECRET] from the vault, never from the job file."
        )
    );
    assert_eq!(
        candidate["subject"],
        "the deploy job's [redacted:TEST_SECRET]"
    );
    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap();
    assert!(!stored.contains("sensitive-value-123"), "{stored}");
}

#[test]
fn a_context_and_subject_at_their_caps_stay_when_redaction_lengthens_them() {
    // The caps hold the model's words; the mask `[redacted:TEST_SECRET]`
    // is longer than the secret it replaces, and the masked value is kept.
    const SECRET: &str = "sv-12345678";
    let s = Setup::new();
    let quote = "I prefer that the deploy job reads its token from the vault.";
    s.write(ON, &[line("user", "u-1", quote)]);
    let head = format!(
        "The user and the agent were setting up the deploy job. The user prefers that it reads {SECRET} from the vault"
    );
    let context = format!("{head}{}.", " and never from the job file".repeat(30));
    let context: String = context.chars().take(599).chain(['.']).collect();
    assert_eq!(context.chars().count(), 600);
    let subject = format!("the deploy job's {SECRET} {}", "x".repeat(80));
    let subject: String = subject.chars().take(80).collect();
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": quote, "message": "u-1", "tag": "preferences",
            "context": context, "subject": subject}
    ]);
    s.answer(&body);
    s.config("[redact]\nenv = [\"TEST_SECRET\"]\n");
    s.scribe("/scribe on");
    let mut cmd = s.fx.cmd(&["session", "pass", "--session", ON, "--json"]);
    cmd.env("PATH", &s.path).env("TEST_SECRET", SECRET);
    let out = s.run_with(cmd, b"");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let listed = proposals(&s);
    assert_eq!(listed["total"], 1, "{listed}");
    let candidate = &listed["proposals"][0]["candidate"];
    let masked_context = context.replace(SECRET, "[redacted:TEST_SECRET]");
    let masked_subject = subject.replace(SECRET, "[redacted:TEST_SECRET]");
    assert!(masked_context.chars().count() > 600);
    assert!(masked_subject.chars().count() > 80);
    assert_eq!(candidate["context"], masked_context.as_str());
    assert_eq!(candidate["subject"], masked_subject.as_str());
    let stored = support::exported(&s.fx.root(), "proposals.json").unwrap();
    assert!(!stored.contains(SECRET), "{stored}");

    // The longer, masked context still approves and reads back.
    let mut payload = s.payload("UserPromptSubmit");
    payload["prompt"] = json!("#scribe approve p-1");
    let mut cmd = s.fx.cmd(&["hook", "user-prompt-submit"]);
    cmd.env("PATH", &s.path).env("TEST_SECRET", SECRET);
    let out = s.run_with(cmd, payload.to_string().as_bytes());
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let entry: Value = serde_json::from_slice(
        &s.fx
            .ok(&["entry", "show", "preferences-1", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(entry["content"], masked_context.as_str());
    assert_eq!(entry["quote"], quote);
}

#[test]
fn a_claim_whose_context_an_entry_already_says_opens_nothing() {
    let s = Setup::new();
    let quote = "ok fine, I prefer that we keep the changelog out of task branches.";
    s.write(ON, &[line("user", "u-1", quote)]);
    let context = context_of("Daniel prefers that task branches never touch the changelog.");
    let mut body = answer();
    body["candidates"] = json!([{"quote": quote, "message": "u-1", "tag": "preferences",
        "context": context, "subject": "task branches"}]);
    s.answer(&body);
    let recorded = format!("Recorded earlier: {context}");
    s.fx.ok(&[
        "entry",
        "add",
        "--tag",
        "preferences",
        "--project",
        "scribe",
        "--author",
        "opus",
        &recorded,
    ]);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(proposals(&s)["total"], 0, "the context is already recorded");
}

// --- ideas the pass writes itself (2_design.md §11.6) -----------------------

/// An `idea` claim as the model returns it for a written tag.
fn idea_claim(quote: &str, message: &str, statement: &str) -> Value {
    json!({"quote": quote, "message": message, "tag": "idea",
           "statement": statement, "subject": "the routing table", "outcome": "open"})
}

fn entries_json(s: &Setup, args: &[&str]) -> Value {
    let mut all = vec!["entry", "list", "--json"];
    all.extend_from_slice(args);
    serde_json::from_slice(&s.fx.ok(&all).stdout).unwrap()
}

#[test]
fn the_pass_writes_daniels_idea_itself_as_an_entry_it_authors_with_no_proposal() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    let statement = "Daniel suggests caching the routing table per host so lookups stay local.";
    s.write(
        ON,
        &[
            line("user", "u-plain", "Please look at the routing table."),
            line("user", "u-idea", idea),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([idea_claim(idea, "u-idea", statement)]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    // The request names the turn under the written tag, and gives the
    // tag's definition and the written-tag rules.
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "- message u-idea (the user's turn): tag idea (written; trigger \"what if we\", matched \"What if we\")"
        ),
        "{prompt}"
    );
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    assert!(
        prompt.contains("- idea: qualifies: The user proposes something that could be built"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Already recorded under written tags: []"),
        "{prompt}"
    );

    // No proposal: one active entry, the statement as content, his span
    // as quote, authored and stated by the pass, outcome open.
    assert_eq!(proposals(&s)["total"], 0);
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "idea-1", "--json"]).stdout).unwrap();
    assert_eq!(shown["status"], "active");
    assert_eq!(shown["content"], context_of(statement));
    assert_eq!(shown["quote"], idea);
    assert_eq!(shown["said"], "");
    assert_eq!(
        shown["author"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(
        shown["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(shown["content_from"]["role"], "user");
    assert_eq!(shown["cite"]["message"], "u-idea");
    assert_eq!(
        shown["outcome"],
        json!({"state": "open", "reason": null, "task": null})
    );

    // Never listed as the user's own (`daniel` is the kind's name before
    // format 23, still accepted); listed under the pass's channel.
    for kind in ["user", "daniel"] {
        assert_eq!(
            entries_json(&s, &["--tag", "idea", "--author-kind", kind])["total"],
            0
        );
    }
    assert_eq!(
        entries_json(&s, &["--tag", "idea", "--author-kind", "scribe"])["total"],
        1
    );

    // `entry show` says the pass captured it, in plain and table form.
    let plain = text(&s.fx.ok(&["entry", "show", "idea-1"]).stdout);
    assert!(plain.lines().any(|l| l == "outcome\topen\t-"), "{plain}");
    assert!(
        plain
            .lines()
            .any(|l| l == format!("stated_by\tscribe:session-pass\t{idea}")),
        "{plain}"
    );
    let table = text(
        &s.fx
            .ok(&["entry", "show", "idea-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains("captured by the session pass (not typed by the user)"),
        "{table}"
    );

    // One audit line for the session and one for the idea written.
    let audit = audit_log(&s);
    assert!(
        audit
            .iter()
            .any(|l| l["command"] == "session pass" && l["target"] == "idea-1"),
        "{audit:?}"
    );
}

#[test]
fn an_ordinary_word_or_part_of_an_id_named_as_the_task_writes_the_idea_open() {
    // Each task the model names appears in the session's text, but neither
    // is a task id the session wrote: `task` is an ordinary word, and
    // `ORB-42` is only part of `ORB-4242`. Both ideas are written open.
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let word = "What if we made the stale-session sweep a task for the box?";
    let part = "What if we split the pass prompt? I filed ORB-4242 for that.";
    s.write(ON, &[line("user", "u-1", word), line("user", "u-2", part)]);
    let mut word_claim = idea_claim(word, "u-1", "Daniel suggests a box task for the sweep.");
    word_claim["outcome"] = json!("task");
    word_claim["task"] = json!("task");
    let mut part_claim = idea_claim(part, "u-2", "Daniel suggests splitting the pass prompt.");
    part_claim["outcome"] = json!("task");
    part_claim["task"] = json!("ORB-42");
    let mut body = answer();
    body["candidates"] = json!([word_claim, part_claim]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = entries_json(&s, &["--tag", "idea"]);
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{listed}");
    for (entry, quote) in entries.iter().zip([word, part]) {
        assert_eq!(entry["quote"], quote);
        assert_eq!(
            entry["outcome"],
            json!({"state": "open", "reason": null, "task": null})
        );
    }
}

#[test]
fn a_parked_idea_keeps_daniels_words_and_one_that_became_a_task_writes_nothing() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let sharding = "What if we sharded the audit log by month?";
    let dry_run = "Here's an idea: a dry-run flag for the session pass.";
    let popup = "What if we dropped the popup preview entirely?";
    let parked = "let's park the sharding idea until the log is big";
    s.write(
        ON,
        &[
            line("user", "u-1", sharding),
            line("user", "u-2", dry_run),
            line("assistant", "a-1", "Filed ORB-4242 for the dry-run flag."),
            line("user", "u-3", &format!("ok, {parked}.")),
            line("user", "u-4", popup),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        {"quote": sharding, "message": "u-1", "tag": "idea", "subject": "the audit log",
         "statement": "Daniel suggests sharding the audit log by month.",
         "outcome": "parked", "outcome_said": parked, "outcome_message": "u-3"},
        {"quote": dry_run, "message": "u-2", "tag": "idea", "subject": "session pass",
         "statement": "Daniel suggests a dry-run flag for the session pass.",
         "outcome": "task", "task": "ORB-4242"},
        // Parking words that are not his, verbatim: written open.
        {"quote": popup, "message": "u-4", "tag": "idea", "subject": "the tmux popup",
         "statement": "Daniel suggests dropping the tmux popup's preview.",
         "outcome": "parked", "outcome_said": "drop it for now", "outcome_message": "u-4"}
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let listed = entries_json(&s, &["--tag", "idea"]);
    let entries = listed["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "the task became a task: {listed}");
    assert_eq!(entries[0]["quote"], sharding);
    assert_eq!(
        entries[0]["outcome"],
        json!({"state": "parked", "reason": parked, "task": null})
    );
    assert_eq!(entries[1]["quote"], popup);
    assert_eq!(entries[1]["outcome"]["state"], "open");
    assert!(
        entries
            .iter()
            .all(|e| e["quote"] != dry_run && e["content"] != dry_run)
    );
    let plain = text(&s.fx.ok(&["entry", "show", "idea-1"]).stdout);
    assert!(
        plain
            .lines()
            .any(|l| l == format!("outcome\tparked\t{parked}")),
        "{plain}"
    );

    // Forgetting it blanks his parking words with the rest of his words.
    let out = type_settlement(&s, "#scribe forget idea-1");
    assert_eq!(out.status.code(), Some(0));
    let forgotten: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "idea-1", "--json"]).stdout).unwrap();
    assert_eq!(forgotten["status"], "forgotten");
    assert_eq!(
        forgotten["outcome"],
        json!({"state": "parked", "reason": null, "task": null})
    );
    assert_eq!(forgotten["quote"], Value::Null);
}

#[test]
fn an_assistant_turn_never_produces_an_idea_entry() {
    let s = Setup::new();
    let suggestion = "What if we cached the routing table per host instead?";
    s.write(
        ON,
        &[
            line("user", "u-1", "What if we looked at the routing next?"),
            line("assistant", "a-1", suggestion),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            suggestion,
            "a-1",
            "The assistant suggests a per-host cache."
        ),
        // Named under Daniel's firing turn, but the words are the assistant's.
        idea_claim(
            suggestion,
            "u-1",
            "The assistant suggests a per-host cache."
        )
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompt = &s.prompts()[0];
    assert!(
        !prompt.contains("message a-1 (the user's turn): tag"),
        "{prompt}"
    );
    assert_eq!(entries_json(&s, &["--tag", "idea"])["total"], 0);
    assert_eq!(proposals(&s)["total"], 0);
}

#[test]
fn a_fourth_idea_in_one_pass_is_not_written_and_proposals_keep_their_own_slots() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let ideas = [
        "What if we cached the routing table per host?",
        "What if we sharded the audit log by month?",
        "What if we added a dry-run flag to the pass?",
        "What if we dropped the popup preview entirely?",
    ];
    let preference = "I prefer short progress updates while an agent is working.";
    let mut lines: Vec<Value> = ideas
        .iter()
        .enumerate()
        .map(|(n, idea)| line("user", &format!("u-{n}"), idea))
        .collect();
    lines.push(line("user", "u-pref", preference));
    s.write(ON, &lines);
    let mut claims: Vec<Value> = ideas
        .iter()
        .enumerate()
        .map(|(n, idea)| {
            idea_claim(
                idea,
                &format!("u-{n}"),
                &format!(
                    "Daniel suggests: {}",
                    idea.trim_start_matches("What if we ")
                ),
            )
        })
        .collect();
    claims.push(json!({"quote": preference, "message": "u-pref", "tag": "preferences"}));
    let mut body = answer();
    body["candidates"] = json!(claims);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains(
            "at most 3 in this window under proposed tags and at most 3 under written tags"
        ),
        "{prompt}"
    );
    let listed = entries_json(&s, &["--tag", "idea"]);
    let quotes: Vec<&str> = listed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["quote"].as_str().unwrap())
        .collect();
    assert_eq!(quotes, ideas[..3].to_vec());
    // The preference still opens its candidate: its slots are its own.
    let open = proposals(&s);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["tag"], "preferences");
}

#[test]
fn ideas_an_earlier_window_drafted_but_did_not_write_leave_later_windows_their_slots() {
    // One message per window. In each of the first three, Daniel's idea
    // became a task in his own turn, so the drafted idea writes nothing;
    // the fourth window's idea must still be requested and written, since
    // only ideas the store wrote spend the pass's three write slots.
    let s = Setup::new();
    let filed = [
        "What if we sharded the audit log by month? I filed ORB-4241 for it.",
        "What if we added a dry-run flag to the pass? I filed ORB-4242 for it.",
        "What if we dropped the popup preview? I filed ORB-4243 for it.",
    ];
    let open = "What if we cached the routing table per host?";
    let mut lines: Vec<Value> = filed
        .iter()
        .enumerate()
        .map(|(n, idea)| line("user", &format!("u-{n}"), idea))
        .collect();
    lines.push(line("user", "u-open", open));
    s.write(ON, &lines);
    let mut answers: Vec<Option<Value>> = filed
        .iter()
        .enumerate()
        .map(|(n, idea)| {
            let mut claim = idea_claim(
                idea,
                &format!("u-{n}"),
                &format!("Daniel suggests idea {n}, now a task."),
            );
            claim["outcome"] = json!("task");
            claim["task"] = json!(format!("ORB-424{}", n + 1));
            let mut body = answer();
            body["candidates"] = json!([claim]);
            Some(body)
        })
        .collect();
    let mut last = answer();
    last["candidates"] = json!([idea_claim(
        open,
        "u-open",
        "Daniel suggests caching the routing table per host."
    )]);
    answers.push(Some(last));
    s.answers(&answers);
    s.scribe("/scribe on");
    s.config(&format!(
        "[catalog]\nwindow_messages = 1\n{}",
        support::IDEA_CATCHER
    ));
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let prompts = s.prompts();
    assert_eq!(prompts.len(), 4);
    assert!(
        prompts[3].contains("at most 3 under written tags"),
        "{}",
        prompts[3]
    );
    let listed = entries_json(&s, &["--tag", "idea"]);
    assert_eq!(listed["total"], 1, "{listed}");
    assert_eq!(listed["entries"][0]["quote"], open);
}

#[test]
fn an_idea_an_active_entry_already_covers_is_not_written_and_ideas_are_sent_as_exclusions() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    let statement = "Daniel suggests caching the routing table per host so lookups stay local.";
    s.write(ON, &[line("user", "u-1", idea)]);
    let mut body = answer();
    body["candidates"] = json!([idea_claim(idea, "u-1", statement)]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(entries_json(&s, &["--tag", "idea"])["total"], 1);

    // Later in the session he floats it again: once in the same words, once
    // paraphrased with the statement the model gives the same.
    let again =
        "What if we cached the routing table per host so lookups stay local? Still worth it.";
    let paraphrase = "What if we kept a per-host copy of the routing table?";
    s.append(
        ON,
        &[line("user", "u-2", again), line("user", "u-3", paraphrase)],
    );
    let mut body = answer();
    body["candidates"] = json!([
        idea_claim(
            idea,
            "u-2",
            "Daniel again suggests a per-host routing table cache."
        ),
        idea_claim(paraphrase, "u-3", statement)
    ]);
    s.answer(&body);
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert_eq!(
        entries_json(&s, &["--tag", "idea"])["total"],
        1,
        "the quote and the context are each covered by idea-1"
    );
    // The existing idea went to the model as an exclusion, context and
    // his words.
    let prompts = s.prompts();
    let second = prompts.last().unwrap();
    let excluded = format!(
        "Already recorded under written tags: [\"{} (the user's words: \\\"{idea}\\\")\"]",
        context_of(statement)
    );
    assert!(second.contains(&excluded), "{second}");
}

#[test]
fn the_session_start_stuck_line_does_not_count_pass_captured_ideas() {
    let s = Setup::new();
    s.config(support::IDEA_CATCHER);
    let idea = "What if we cached the routing table per host so lookups stay local?";
    s.write(ON, &[line("user", "u-1", idea)]);
    let mut body = answer();
    body["candidates"] = json!([idea_claim(
        idea,
        "u-1",
        "Daniel suggests caching the routing table per host."
    )]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    // Older than the stuck threshold, and nobody picked it up.
    support::sql(
        &s.fx.root(),
        "UPDATE entries SET created = '2026-01-01T00:00:00Z' WHERE id = 'idea-1';",
    );
    let mut payload = s.payload("SessionStart");
    payload["source"] = json!("startup");
    let out = s.hook("session-start", &payload);
    assert!(
        !text(&out.stdout).contains("not picked up"),
        "{}",
        text(&out.stdout)
    );
}

// --- memory files (2_design.md §15.4–§15.6, §15.11) -----------------------

const WEDDING: &str = "My sister's wedding is on 2026-12-12 and I'm still on the seating.";
const WEDDING_QUOTE: &str = "My sister's wedding is on 2026-12-12";
const VENUE: &str = "I still have to book the venue for the party sometime.";
const VENUE_QUOTE: &str = "I still have to book the venue";
const WEDDING_STEM: &str = "2026-12-12-sister-s-wedding";

fn memory_element(kind: &str, title: &str, quote: &str, message: &str) -> Value {
    json!({"kind": kind, "title": title, "text": format!("{title}, as the user said."),
           "quote": quote, "message": message, "date": null, "now_kind": null,
           "update": null, "completes": null})
}

/// An `ON` session with two memory-bearing turns and a model answer naming
/// both: a dated `now` item and an undated candidate.
fn memory_setup() -> Setup {
    let s = Setup::new();
    s.append(
        ON,
        &[line("user", "u-w", WEDDING), line("user", "u-v", VENUE)],
    );
    let mut body = answer();
    let mut wedding = memory_element("now", "Sister's wedding", WEDDING_QUOTE, "u-w");
    wedding["date"] = json!("2026-12-12");
    wedding["now_kind"] = json!("event");
    body["memory"] = json!([
        wedding,
        memory_element("now", "Book the party venue", VENUE_QUOTE, "u-v"),
    ]);
    body["signal"] = json!(2);
    s.answer(&body);
    s
}

fn memory_json(s: &Setup, args: &[&str]) -> Value {
    let mut all = vec!["memory"];
    all.extend_from_slice(args);
    all.push("--json");
    serde_json::from_slice(&s.run(&all, b"").stdout).unwrap()
}

/// Every file under the store's `memory/`, as text.
fn memory_text(s: &Setup) -> String {
    fn walk(dir: &std::path::Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = String::new();
    walk(&s.fx.root().join("memory"), &mut out);
    out
}

/// The store as bytes and modes, without SQLite's shared-memory index
/// (its read marks change on any read).
fn store_snapshot(s: &Setup) -> Vec<(PathBuf, Vec<u8>, u32)> {
    support::snapshot(&s.fx.root())
        .into_iter()
        .filter(|(path, _, _)| !path.ends_with("scribe.db-shm"))
        .collect()
}

fn pass_on(s: &Setup) -> Value {
    serde_json::from_slice(
        &s.run(&["session", "pass", "--session", ON, "--json"], b"")
            .stdout,
    )
    .unwrap()
}

#[test]
fn a_pass_writes_a_dated_now_item_as_its_file_and_keeps_an_undated_one_pending() {
    let s = memory_setup();
    s.scribe("/scribe on");
    let report = pass_on(&s);
    let pass = &report["passes"][0];
    assert_eq!(pass["result"], "written", "{report}");
    assert_eq!(pass["memory_written"], 1, "{report}");
    assert_eq!(pass["memory_pending"], 1, "{report}");
    assert_eq!(pass["memory_refused"], json!({}), "{report}");
    let prompt = &s.prompts()[0];
    assert!(
        prompt.contains("Return memory: at most 3 elements"),
        "{prompt}"
    );
    assert!(prompt.contains("Return signal: 0 to 3"), "{prompt}");

    let file = s.fx.root().join(format!("memory/now/{WEDDING_STEM}.md"));
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(
        written.starts_with("---\ndate: 2026-12-12\nkind: event\n"),
        "{written}"
    );
    assert!(
        written.contains(&format!("sources:\n- {ON}\n")),
        "{written}"
    );
    assert!(
        written.ends_with("---\n# Sister's wedding\n\nSister's wedding, as the user said.\n"),
        "{written}"
    );
    let shown = memory_json(&s, &["show", WEDDING_STEM]);
    assert_eq!(shown["kind"], "now", "{shown}");
    assert_eq!(shown["date"], "2026-12-12");
    assert_eq!(shown["sources"], json!([ON]));
    // The memory write stamps control.json at the memory format.
    let control: Value = support::control(&s.fx.root());
    assert_eq!(control["format"], 31);

    // The same window again writes nothing new.
    let before = std::fs::read(&file).unwrap();
    s.append(
        ON,
        &[line("user", "u-9", "One more thing about the crews.")],
    );
    let again = pass_on(&s);
    assert_eq!(again["passes"][0]["memory_written"], 0, "{again}");
    assert_eq!(std::fs::read(&file).unwrap(), before);
}

#[test]
fn a_pass_over_a_session_never_switched_on_writes_no_memory() {
    let s = memory_setup();
    let out = s.run_any(&["session", "pass", "--session", NEVER, "--json"], b"");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    s.run(&["session", "pass", "--json"], b"");
    assert!(s.prompts().is_empty(), "the model was never called");
    assert!(!s.fx.root().join("memory").exists());
}

#[test]
fn the_sensitive_gate_keeps_credentials_and_finance_words_out() {
    let s = Setup::new();
    let token = "deploy with ghp_abcdefghijklmnopqrstuvwxyz0123456789 by 2026-12-01";
    let salary = "my salary review is on 2026-12-03 with the manager";
    s.append(
        ON,
        &[line("user", "u-t", token), line("user", "u-f", salary)],
    );
    let mut body = answer();
    let mut first = memory_element("now", "Deploy with a token", token, "u-t");
    first["date"] = json!("2026-12-01");
    let mut second = memory_element("now", "Salary review", salary, "u-f");
    second["date"] = json!("2026-12-03");
    body["memory"] = json!([first, second]);
    s.answer(&body);
    s.scribe("/scribe on");
    let report = pass_on(&s);
    assert_eq!(report["passes"][0]["memory_written"], 0, "{report}");
    assert_eq!(
        report["passes"][0]["memory_refused"],
        json!({"memory_sensitive": 2}),
        "{report}"
    );
    let stored = memory_text(&s);
    for kept_out in ["ghp_", "alary"] {
        assert!(!stored.contains(kept_out), "{kept_out} reached memory");
    }
}

#[test]
fn a_milestone_is_written_only_when_the_user_calls_it_one() {
    let s = Setup::new();
    let big = "We shipped scribe 1.0 on 2026-11-02, a big day for me.";
    let plain = "We moved the office on 2026-11-01 without trouble.";
    s.append(ON, &[line("user", "u-b", big), line("user", "u-p", plain)]);
    let mut body = answer();
    let mut shipped = memory_element("milestone", "Shipped scribe 1.0", big, "u-b");
    shipped["date"] = json!("2026-11-02");
    let mut moved = memory_element("milestone", "Moved the office", plain, "u-p");
    moved["date"] = json!("2026-11-01");
    body["memory"] = json!([shipped, moved]);
    s.answer(&body);
    s.scribe("/scribe on");
    let report = pass_on(&s);
    assert_eq!(report["passes"][0]["memory_written"], 1, "{report}");
    assert_eq!(
        report["passes"][0]["memory_refused"],
        json!({"memory_not_stated": 1}),
        "{report}"
    );
    let block = memory_json(&s, &["show"]);
    assert_eq!(
        block["milestones"][0]["id"], "2026-11-02-shipped-scribe-1-0",
        "{block}"
    );
    assert_eq!(block["milestones"].as_array().unwrap().len(), 1);
}

#[test]
fn ack_marks_a_now_item_raised_once_per_session_and_forget_removes_it() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let ack = |args: &[&str]| -> Value {
        let mut all = vec!["ack", WEDDING_STEM, "--session", ON, "--author", "opus"];
        all.extend_from_slice(args);
        memory_json(&s, &all)
    };
    let acked = ack(&[]);
    assert_eq!(acked["raised"]["session"], ON, "{acked}");
    let file = s.fx.root().join(format!("memory/now/{WEDDING_STEM}.md"));
    let raised = std::fs::read(&file).unwrap();
    ack(&[]);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        raised,
        "the same session changes nothing"
    );

    let before: Value = serde_json::from_slice(
        &s.fx
            .ok(&["search", "wedding", "--kind", "message", "--json"])
            .stdout,
    )
    .unwrap();
    assert_eq!(before["total"], 1, "{before}");
    let unconfirmed = s.run_any(&["memory", "forget", WEDDING_STEM, "--author", "opus"], b"");
    assert_ne!(unconfirmed.status.code(), Some(0));
    assert!(file.exists());
    let profile = s.run_any(
        &[
            "memory",
            "forget",
            "profile",
            "--author",
            "opus",
            "--confirm",
            "--json",
        ],
        b"",
    );
    assert!(
        text(&profile.stderr).contains("usage_error"),
        "{}",
        text(&profile.stderr)
    );
    s.run(
        &[
            "memory",
            "forget",
            WEDDING_STEM,
            "--author",
            "opus",
            "--confirm",
            "--json",
        ],
        b"",
    );
    assert!(!file.exists());
    assert!(!memory_text(&s).contains("Sister's wedding"));
    // The quoted turn is gone from the messages too (§15.7), and the other
    // turn stays.
    let search = |terms: &str| -> Value {
        serde_json::from_slice(
            &s.fx
                .ok(&["search", terms, "--kind", "message", "--json"])
                .stdout,
        )
        .unwrap()
    };
    assert_eq!(search("wedding")["total"], 0, "{}", search("wedding"));
    assert_eq!(search("venue")["total"], 1, "{}", search("venue"));
    let db = std::fs::read(s.fx.root().join("scribe.db")).unwrap();
    let wal = std::fs::read(s.fx.root().join("scribe.db-wal")).unwrap_or_default();
    for bytes in [&db, &wal] {
        assert!(
            !String::from_utf8_lossy(bytes).contains("still on the seating"),
            "the forgotten turn's words are left in the database"
        );
    }
    let missing = s.run_any(&["memory", "show", WEDDING_STEM, "--json"], b"");
    assert!(
        text(&missing.stderr).contains("memory_not_found"),
        "{}",
        text(&missing.stderr)
    );
}

#[test]
fn the_session_start_page_carries_the_memory_block_in_interactive_sessions_only() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);

    let start = |entrypoint: &str| -> Value {
        let payload = json!({"session_id": NEVER, "transcript_path": s.transcript(NEVER),
            "cwd": s.cwd, "hook_event_name": "SessionStart", "source": "startup"});
        let mut cmd = s.fx.cmd(&["hook", "session-start"]);
        cmd.env("PATH", &s.path)
            .env("CLAUDE_CODE_ENTRYPOINT", entrypoint);
        let out = s.run_with(cmd, payload.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    };
    let interactive = start("cli");
    let page = interactive["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("{interactive}"));
    assert!(
        page.contains(&format!(
            "## Profile\n(none)\n## Now\n- {WEDDING_STEM} · due 2026-12-12 · Sister's wedding\n## Milestones\n(none)\n"
        )),
        "{page}"
    );
    assert!(page.contains("## Recent sessions"), "{page}");

    let headless = start("sdk-cli");
    assert_eq!(headless["hookSpecificOutput"], Value::Null, "{headless}");
}

#[test]
fn memory_show_and_brief_write_nothing() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let before = store_snapshot(&s);
    for args in [
        &["memory", "show"][..],
        &["memory", "show", "--json"],
        &["memory", "show", WEDDING_STEM],
        &["memory", "rules"],
        &["brief"],
        &["brief", "--json"],
    ] {
        s.run(args, b"");
    }
    assert_eq!(store_snapshot(&s), before);
}

#[test]
fn a_newer_store_is_refused_and_memory_settings_are_deprecated() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    let control = s.fx.root().join("control.json");
    let text_now = std::fs::read_to_string(&control).unwrap();
    let mut newer_stamp: Value = serde_json::from_str(&text_now).unwrap();
    assert_eq!(newer_stamp["format"], 31, "{text_now}");
    newer_stamp["format"] = json!(32);
    std::fs::write(&control, newer_stamp.to_string()).unwrap();
    let newer = s.run_any(&["memory", "refresh", "--json"], b"");
    assert_ne!(newer.status.code(), Some(0));
    assert!(
        text(&newer.stderr).contains("store_too_new"),
        "{}",
        text(&newer.stderr)
    );
    std::fs::write(&control, text_now).unwrap();

    // A command that loads the config warns and goes on.
    s.config("[memory]\ndecay_days = 60\n");
    let out = s.run(&["tag", "list"], b"");
    assert!(
        text(&out.stderr).contains("memory.decay_days is deprecated (config_deprecated)"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn memory_rules_prints_the_fixed_rules() {
    let s = Setup::new();
    let out = memory_json(&s, &["rules"]);
    let names: Vec<&str> = out["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["rule"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "max_per_pass",
            "max_pending",
            "now_keep_days",
            "milestone_lines",
            "profile_chars",
            "user_says"
        ]
    );
    assert!(
        out["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["origin"]["source"] == "shipped")
    );
}

/// A profile answer: the template with `projects` under Projects.
fn profile_answer(projects: &str) -> Value {
    let profile = format!(
        "## Role and work\n\n## Current focus\n\n## Projects\n- {projects}\n\n## Goals\n\n\
         ## Environment\n\n## How they work\n\n## Pinned\n\n"
    );
    json!({"profile": profile, "milestones": []})
}

#[test]
fn memory_refresh_writes_a_revision_and_has_nothing_new_on_a_second_run() {
    let s = memory_setup();
    s.scribe("/scribe on");
    pass_on(&s);
    s.answer(&profile_answer("scribe, a memory store."));
    let calls = s.prompts().len();
    let refreshed = memory_json(&s, &["refresh"]);
    assert_eq!(refreshed["result"], "written", "{refreshed}");
    assert_eq!(refreshed["revision"], 1);
    assert_eq!(refreshed["sessions"], 1);
    assert_eq!(s.prompts().len(), calls + 1);
    assert!(
        s.prompts()[calls].contains("profile"),
        "{}",
        s.prompts()[calls]
    );
    let profile = std::fs::read_to_string(s.fx.root().join("memory/profile.md")).unwrap();
    assert!(profile.contains("- scribe, a memory store."), "{profile}");
    let block = memory_json(&s, &["show"]);
    assert!(block["profile"]["as_of"].is_string(), "{block}");
    let revision = memory_json(&s, &["show", "--revision", "1"]);
    assert_eq!(revision["text"], profile);

    let again = memory_json(&s, &["refresh"]);
    assert_eq!(again["result"], "nothing_new", "{again}");
    assert_eq!(s.prompts().len(), calls + 1, "no model call");

    // A refused profile writes nothing.
    std::fs::write(
        s.fx.root().join("memory/profile.md"),
        profile.replacen("## Pinned\n", "## Pinned\n- Call me Daniel.\n", 1),
    )
    .unwrap();
    s.answer(&profile_answer("orbit."));
    let refused = s.run_any(&["memory", "refresh", "--json"], b"");
    assert_ne!(refused.status.code(), Some(0));
    assert!(
        text(&refused.stdout).contains("profile_pinned")
            || text(&refused.stderr).contains("profile_pinned")
    );
    assert!(
        std::fs::read_to_string(s.fx.root().join("memory/profile.md"))
            .unwrap()
            .contains("- Call me Daniel.")
    );
}

#[test]
fn a_format_24_memory_item_migrates_on_the_first_memory_write() {
    let s = Setup::new();
    let dir = s.fx.root().join("memory");
    s.fx.create_private_dir_all(&dir);
    let path = dir.join("m-1.md");
    std::fs::write(
        &path,
        format!(
            "---\nformat: 24\nkind: memory\nid: m-1\nsection: now\nstatus: active\n\
             statement: Sister's wedding on 2026-12-12.\nproject: null\n\
             event_date: 2026-12-12\nsensitive: false\nsensitive_kind: null\nacknowledged: null\n\
             retirement_reason: null\npromoted_from: null\npromotion: null\nmerged_into: null\n\
             superseded_by: null\nunrelated_to: []\npreference_proposal: null\n\
             author:\n  kind: scribe\n  name: session-pass\nupdated_by:\n  kind: scribe\n  name: session-pass\n\
             created: 2026-11-03T06:00:00Z\nupdated: 2026-11-03T06:00:00Z\noccurrences:\n\
             - n: 1\n  date: 2026-11-03T06:00:00Z\n  session: {ON}\n  message: u-w\n  host: host\n  project: scribe\n\
             \x20 quote: {WEDDING_QUOTE}\n  author:\n    kind: scribe\n    name: session-pass\n---\n"
        ),
    )
    .unwrap();
    make_private(&path);
    // A read leaves it and says so.
    let shown = s.run(&["memory", "show"], b"");
    assert!(path.exists());
    assert!(
        text(&shown.stderr).contains("1 format-24 memory item(s) not yet migrated"),
        "{}",
        text(&shown.stderr)
    );
    // The first write migrates it.
    s.answer(&profile_answer("scribe."));
    s.run(&["memory", "refresh", "--json"], b"");
    assert!(!path.exists());
    let block = memory_json(&s, &["show"]);
    assert_eq!(
        block["now"][0]["id"], "2026-12-12-sister-s-wedding-on-2026-12-12",
        "{block}"
    );
}

/// The ```toml block of specs/catchers.md that declares `[tags.<name>]`.
fn catcher_example(name: &str) -> String {
    let spec = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/design/scribe/specs/catchers.md"
    );
    let spec = std::fs::read_to_string(spec).unwrap();
    let header = format!("[tags.{name}]\n");
    spec.split("```toml\n")
        .skip(1)
        .filter_map(|block| block.split_once("```").map(|(toml, _)| toml))
        .find(|toml| toml.contains(&header))
        .unwrap_or_else(|| panic!("no example declares [tags.{name}]"))
        .to_owned()
}

fn written_claim(quote: &str, message: &str, tag: &str, statement: &str) -> Value {
    json!({"quote": quote, "message": message, "tag": tag,
           "statement": statement, "subject": tag, "outcome": "open"})
}

#[test]
fn config_only_catchers_from_the_spec_catch_from_their_speakers_with_no_code_change() {
    // The orbit collector reads the user, the todo catcher the assistant
    // and the issue example both, on an incident turn; each is only the
    // config block specs/catchers.md documents.
    let s = Setup::new();
    s.config(&format!(
        "{}\n{}\n{}",
        catcher_example("orbit"),
        catcher_example("todo"),
        catcher_example("issue")
    ));
    let orbit = "The orbit drain keeps stalling on big backlogs.";
    let todo = "You need to approve the GitHub app install yourself, since it needs your login.";
    let asked = "can you investigate why the drain keeps failing with HTTP 429";
    let found =
        "The root cause was parallel drains sharing one token; fixed by serialising pushes.";
    s.write(
        ON,
        &[
            line("user", "u-orbit", orbit),
            line("assistant", "a-todo", todo),
            line("user", "u-asked", asked),
            line("assistant", "a-found", found),
            line("user", "u-plain", "Thanks, that is all."),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([
        written_claim(
            orbit,
            "u-orbit",
            "orbit",
            "The user says the Orbit drain stalls on big backlogs."
        ),
        written_claim(
            todo,
            "a-todo",
            "todo",
            "The user must approve the GitHub app install."
        ),
        written_claim(
            asked,
            "u-asked",
            "issue",
            "The user asked why the drain fails with HTTP 429."
        ),
        written_claim(
            found,
            "a-found",
            "issue",
            "Parallel drains shared one token; serialising pushes fixed it."
        ),
    ]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let prompt = &s.prompts()[0];
    for listed in [
        "- message u-orbit (the user's turn): tag orbit (written; trigger \"orbit\", matched \"orbit\")",
        "- message a-todo (the assistant's turn): tag todo (written; trigger \"you need to\", matched \"You need to\")",
        "- message u-asked (the user's turn): tag issue (written; trigger \"can you investigate\", matched \"can you investigate\")",
        "- message a-found (the assistant's turn): tag issue (written; trigger \"root cause\", matched \"root cause\")",
    ] {
        assert!(prompt.contains(listed), "{listed}\n{prompt}");
    }
    assert!(!prompt.contains("message u-plain"), "{prompt}");
    // `todo` reads only the assistant: the user's turns never reach it.
    assert!(!prompt.contains("(the user's turn): tag todo"), "{prompt}");

    assert_eq!(proposals(&s)["total"], 0, "write catchers open no claims");
    for (tag, quotes) in [
        ("orbit", vec![(orbit, "user")]),
        ("todo", vec![(todo, "assistant")]),
        ("issue", vec![(asked, "user"), (found, "assistant")]),
    ] {
        let listed = entries_json(&s, &["--tag", tag]);
        assert_eq!(listed["total"], quotes.len(), "{tag}: {listed}");
        for (quote, role) in quotes {
            let entry = listed["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["quote"] == quote)
                .unwrap_or_else(|| panic!("{tag}: {quote}: {listed}"));
            assert_eq!(entry["content_from"]["role"], role, "{entry}");
            assert_eq!(entry["author"]["name"], "session-pass", "{entry}");
        }
    }
    // Readers never give the assistant's words to the user.
    let found: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "search",
                "approve GitHub app install",
                "--kind",
                "entry",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    let hit = found.to_string();
    assert!(hit.contains("the assistant's turn"), "{hit}");
    assert!(hit.contains("The assistant's words:"), "{hit}");
    assert!(!hit.contains("The user's words:"), "{hit}");
}

#[test]
fn an_assistant_quote_under_a_user_catcher_is_refused() {
    // `orbit` reads the user only: a claim quoting the assistant's turn is
    // not one of the listed turns, and the store writes nothing.
    let s = Setup::new();
    s.config(&catcher_example("orbit"));
    let said = "I restarted the orbit drain on the box.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Please restart it."),
            line("assistant", "a-1", said),
        ],
    );
    let mut body = answer();
    body["candidates"] = json!([written_claim(
        said,
        "a-1",
        "orbit",
        "The drain was restarted."
    )]);
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    assert!(
        s.prompts()[0].contains("Return candidates: [] (none are requested in this window)."),
        "{}",
        s.prompts()[0]
    );
    assert_eq!(entries_json(&s, &["--tag", "orbit"])["total"], 0);
}

#[test]
fn max_open_caps_one_tags_open_claims_and_leaves_the_others_theirs() {
    let s = Setup::new();
    s.config("[tags.preferences.capture]\nmax_open = 1\n");
    let quotes = [
        ("u-1", "I always want short commit subjects.", "preferences"),
        (
            "u-2",
            "I prefer tabs to spaces in Makefiles.",
            "preferences",
        ),
        (
            "u-3",
            "As a standard every repo keeps a CHANGELOG.",
            "standard",
        ),
    ];
    s.write(
        ON,
        &quotes
            .iter()
            .map(|(id, quote, _)| line("user", id, quote))
            .collect::<Vec<_>>(),
    );
    let mut body = answer();
    body["candidates"] = json!(
        quotes
            .iter()
            .map(|(id, quote, tag)| json!({"quote": quote, "message": id, "tag": tag}))
            .collect::<Vec<_>>()
    );
    s.answer(&body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");
    let open = proposals(&s);
    let tags: Vec<&str> = open["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["candidate"]["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["preferences", "standard"], "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["quote"], quotes[0].1);
}

#[test]
fn approved_assistant_claim_shows_statement_and_assistant_words_in_ask_and_search() {
    let s = Setup::new();
    s.config(
        "[tags.notes]\n\
         description = \"Noteworthy assistant observations.\"\n\n\
         [tags.notes.capture]\n\
         from = \"assistant\"\n\
         route = \"propose\"\n\
         triggers = [\"root cause\"]\n",
    );
    let quote = "The root cause is that parallel drains share a single auth token.";
    let context = "The user and the agent investigated repeated drain stalls. The agent reported that parallel drains share one auth token and collide on refresh.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Why does the drain stall?"),
            line("assistant", "a-1", quote),
        ],
    );
    let mut pass_body = answer();
    pass_body["candidates"] = json!([{
        "quote": quote,
        "message": "a-1",
        "tag": "notes",
        "context": context,
        "subject": "shared drain auth token"
    }]);
    s.answer(&pass_body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let open = proposals(&s);
    assert_eq!(open["total"], 1, "{open}");
    assert_eq!(open["proposals"][0]["candidate"]["role"], "assistant");
    let id = open["proposals"][0]["id"].as_str().unwrap().to_owned();
    type_settlement(&s, &format!("#scribe approve {id}"));

    let shown: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "notes-1", "--json"]).stdout).unwrap();
    assert_eq!(shown["author"]["kind"], "agent");
    assert_eq!(shown["author"]["name"], "assistant");
    assert_eq!(shown["content"], context);
    assert_eq!(shown["quote"], quote);
    assert_eq!(
        shown["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );

    let table = text(
        &s.fx
            .ok(&["entry", "show", "notes-1", "--format", "table"])
            .stdout,
    );
    assert!(
        table.contains("scribe's statement of the assistant's turn"),
        "{table}"
    );
    assert!(!table.contains("the user's turn"), "{table}");

    // `scribe search` labels it as scribe's statement of the assistant's turn
    // with the assistant's quote, never as an agent note or the user's words.
    let found: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "search",
                "parallel drains share",
                "--kind",
                "entry",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(found["total"], 1, "{found}");
    let snippet = found["hits"][0]["snippet"].as_str().unwrap();
    assert!(
        snippet.contains("scribe's statement of the assistant's turn:"),
        "{snippet}"
    );
    assert!(
        snippet.contains(&format!("The assistant's words: \"{quote}\"")),
        "{snippet}"
    );
    assert!(!snippet.contains("agent note"), "{snippet}");
    assert!(!snippet.contains("The user's words"), "{snippet}");
    assert!(!snippet.contains("the user's turn"), "{snippet}");

    // `question ask` sends the evidence labelled as scribe's statement of the
    // assistant's turn with `The assistant's words: "<quote>"`, and attributes
    // claims from the context and from the quote without ever calling them
    // an agent note or the user's words.
    s.answer(&json!({
        "status": "answered",
        "answer": "Parallel drains share a token.",
        "claims": [
            {
                "text": "from context",
                "quote": "parallel drains share one auth token",
                "cite": "notes-1"
            },
            {
                "text": "from assistant quote",
                "quote": "parallel drains share a single auth token",
                "cite": "notes-1"
            }
        ],
        "conflicts": []
    }));
    let ask_out = s.run(
        &[
            "question",
            "ask",
            "why do parallel drains stall?",
            "--tag",
            "notes",
            "--json",
        ],
        b"",
    );
    let asked: Value = serde_json::from_slice(&ask_out.stdout).unwrap();
    let ask_prompt = s.prompts().pop().unwrap();
    let evidence_item = ask_prompt
        .split("<evidence ")
        .find(|part| part.contains("cite=\"notes-1\""))
        .unwrap_or_else(|| panic!("{ask_prompt}"));
    assert!(
        evidence_item.contains("words=\"scribe's statement of the assistant's turn\""),
        "{evidence_item}"
    );
    assert!(
        evidence_item.contains(&format!("The assistant's words: \"{quote}\"")),
        "{evidence_item}"
    );
    assert!(!evidence_item.contains("agent note ("), "{evidence_item}");
    assert!(
        !evidence_item.contains("The user's words"),
        "{evidence_item}"
    );
    assert!(
        !evidence_item.contains("scribe's statement of the user's turn"),
        "{evidence_item}"
    );

    assert_eq!(asked["claims"].as_array().unwrap().len(), 2, "{asked}");
    assert_eq!(
        asked["claims"][0]["text"],
        "scribe's statement of the assistant's turn"
    );
    assert_eq!(
        asked["claims"][1]["text"],
        "assistant (recorded by the user)"
    );
    let ans = asked["answer"].as_str().unwrap();
    assert!(!ans.contains("agent note"), "{ans}");
    assert!(!ans.contains("The user:"), "{ans}");
}

#[test]
fn approved_assistant_claim_refuses_content_update_and_stays_readable() {
    let s = Setup::new();
    s.config(
        "[tags.notes]\n\
         description = \"Noteworthy assistant observations.\"\n\n\
         [tags.notes.capture]\n\
         from = \"assistant\"\n\
         route = \"propose\"\n\
         triggers = [\"root cause\"]\n",
    );
    let quote = "The root cause is that parallel drains share a single auth token.";
    let context = "The user and the agent investigated repeated drain stalls. The agent reported that parallel drains share one auth token and collide on refresh.";
    s.write(
        ON,
        &[
            line("user", "u-1", "Why does the drain stall?"),
            line("assistant", "a-1", quote),
        ],
    );
    let mut pass_body = answer();
    pass_body["candidates"] = json!([{
        "quote": quote,
        "message": "a-1",
        "tag": "notes",
        "context": context,
        "subject": "shared drain auth token"
    }]);
    s.answer(&pass_body);
    s.scribe("/scribe on");
    s.run(&["session", "pass", "--session", ON, "--json"], b"");

    let open = proposals(&s);
    let id = open["proposals"][0]["id"].as_str().unwrap().to_owned();
    type_settlement(&s, &format!("#scribe approve {id}"));

    let before: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "notes-1", "--json"]).stdout).unwrap();

    // Content edits (even alongside metadata flags) must be refused before
    // changing the approved assistant-context entry, keeping it readable.
    let out = s.fx.run(&[
        "entry",
        "update",
        "notes-1",
        "--author",
        "opus",
        "--content",
        "Rewritten by opus.",
        "--add-topic",
        "drains",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_agent_note", "{err}");

    let after_refusal: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "notes-1", "--json"]).stdout).unwrap();
    assert_eq!(after_refusal, before);

    // Metadata-only updates remain allowed and keep `stated_by` intact.
    let patched: Value = serde_json::from_slice(
        &s.fx
            .ok(&[
                "entry",
                "update",
                "notes-1",
                "--author",
                "opus",
                "--add-topic",
                "drains",
                "--add-related",
                "repo:scribe",
                "--json",
            ])
            .stdout,
    )
    .unwrap();
    assert_eq!(patched["content"], context);
    assert_eq!(
        patched["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(patched["topics"], json!(["drains"]));
    assert_eq!(patched["related"], json!(["repo:scribe"]));
    let reread: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "notes-1", "--json"]).stdout).unwrap();
    assert_eq!(reread["content"], context);
    assert_eq!(
        reread["stated_by"],
        json!({"kind": "scribe", "name": "session-pass"})
    );
    assert_eq!(reread["topics"], json!(["drains"]));
    assert_eq!(reread["related"], json!(["repo:scribe"]));
    assert_eq!(reread["admitted"], before["admitted"]);
}

#[test]
fn older_approved_assistant_finding_refuses_content_update() {
    let quote = "Measured: the reviewer mix alone predicts a 7-point first-pass gap.";
    let s = setup_stored_assistant_finding_candidate(quote);
    type_settlement(&s, "#scribe approve p-1");
    let before: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "finding-1", "--json"]).stdout).unwrap();

    let out = s.fx.run(&[
        "entry",
        "update",
        "finding-1",
        "--author",
        "opus",
        "--content",
        "Rewritten finding.",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "not_agent_note", "{err}");

    let after: Value =
        serde_json::from_slice(&s.fx.ok(&["entry", "show", "finding-1", "--json"]).stdout).unwrap();
    assert_eq!(after, before);
}

#[test]
fn unmatched_session_matrix_outcomes() {
    let setup = Setup::new();
    setup.scribe("/scribe on");
    let before = setup.control_bytes();
    let reason = setup.scribe(&format!("/scribe link {UNKNOWN}"));
    let expected = support::session_id_outcome("manual_link");
    assert_eq!(expected["exit"], 0); // scribe observed hook exit 0
    assert!(
        reason.contains(expected["value"].as_str().unwrap()),
        "{reason}"
    );
    assert_eq!(setup.audit_codes().last().unwrap(), &expected["code"]);
    assert_eq!(setup.control_bytes(), before);
    let out = setup.run_any(&["session", "pass", "--session", UNKNOWN, "--json"], b"");
    let expected = support::session_id_outcome("session_pass");
    assert_eq!(
        out.status.code(),
        expected["exit"].as_i64().map(|n| i32::try_from(n).unwrap())
    );
    let error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["code"], expected["code"]);
}
