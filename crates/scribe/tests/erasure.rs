//! Exercise the shipped forget routes and search the persisted bytes.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/erasure.rs"]
mod subject;
mod support;

use crate::support::command::Input::Cli;
use rusqlite::Connection;
use scribe_core::RebuildVerbs;
use scribe_core::testing::ViewTestReads;
use scribe_core::{Redactor, Store};
use serde_json::json;
use subject::*;
use support::command::Exit::Success;
use support::{Fixture, seed_records};
use time::format_description::well_known::Rfc3339;

fn entry_routes() {
    for via_hook in [false, true] {
        let fx = Fixture::new();
        let entry = EntrySeed::new(1);
        seed_entries(&fx, std::slice::from_ref(&entry));
        let prior = fx.elsewhere("caller-export");
        export(&fx, &prior, "markdown");
        assert!(!copies(&prior, &entry.edited).is_empty());
        let words = format!("forget {} for good", entry.id);
        transcript(&fx, std::slice::from_ref(&words));
        let args = entry_forget_args(&entry.id, &words);
        if via_hook {
            let prompt = format!("#scribe forget {}", entry.id);
            assert!(submit(&fx, &prompt).contains("forgot"));
        } else {
            fx.check(Cli(&args), Success);
        }
        assert_eq!(pending(&fx), 0);
        sweep(&fx, &entry.markers());
        assert!(archived_has(&fx, KEPT), "the unrelated archived line stays");
        assert!(!copies(&prior, &entry.edited).is_empty());
    }
}

/// Two forgets at once each erase their own entry from the archive.
fn concurrent_entries() {
    let fx = Fixture::new();
    let entries = [EntrySeed::new(1), EntrySeed::new(2)];
    seed_entries(&fx, &entries);
    let words = entries
        .each_ref()
        .map(|entry| format!("forget {} for good", entry.id));
    transcript(&fx, &words);
    forget_concurrently(&fx, &entries, &words);
    assert_eq!(pending(&fx), 0);
    sweep(
        &fx,
        &entries
            .iter()
            .flat_map(EntrySeed::markers)
            .collect::<Vec<_>>(),
    );
    assert!(archived_has(&fx, KEPT));
}

fn session_route() {
    const RECORD: &str = "erase-session-record-95bde";
    const CHECKPOINT: &str = "erase-rebuild-checkpoint-40c70";
    const TURN: &str = "erase-indexed-session-turn-825d2";
    const CLAIM: &str = "erase-session-claim-a9dbd";
    let fx = Fixture::new();
    let rel = format!("sessions/2026/11/{SOURCE}.md");
    let source = Fixture::seeded();
    let legacy = support::record_text(&source.root(), SOURCE)
        .unwrap()
        .replace(
            "Discussed how agent PRs are merged and recorded the decision.",
            RECORD,
        );
    assert!(legacy.contains(RECORD));
    seed_records(&fx.root(), &[&legacy]);
    let archive = fx.root().join(ARCHIVE);
    put(&fx, &archive.join(&rel), &legacy);
    put(
        &fx,
        &archive.join("audit.jsonl"),
        &(legacy_audit(&format!("session:{SOURCE}"), RECORD)
            + &legacy_audit("preferences-99", KEPT)),
    );
    let proposals = json!({"format": 11, "proposals": [{
        "id": "p-1", "kind": "candidate", "older": "preferences-1", "newer": null,
        "reason": "The user stated a preference", "quotes": [], "state": "open",
        "opened_at": AT.format(&Rfc3339).unwrap(), "settled_at": null, "settled_by": null,
        "author": {"kind": "scribe", "name": "session-pass"}, "updated_by": null,
        "settled_author": null, "closed_reason": null,
        "candidate": {
            "tag": "preferences", "role": "user", "quote": CLAIM, "project": "scribe",
            "provider": "claude-code", "host": "host", "admitted": null, "evidence": null,
            "cite": {"session": SOURCE, "message": "candidate-turn", "at": AT.format(&Rfc3339).unwrap(),
                "prompt_sha256": "0".repeat(64), "line": 1, "cwd": "/work/scribe"},
        },
    }]});
    put(&fx, &archive.join("proposals.json"), &proposals.to_string());
    put(
        &fx,
        &archive.join(format!("sessions/2026/11/.{SOURCE}.rebuild.md")),
        &legacy.replace(RECORD, CHECKPOINT),
    );
    let imported = json_output(&fx.check(Cli(&["store", "migrate", "--json"]), Success));
    assert_eq!(imported["archives"], 1, "{imported}");
    let store = Store::open(fx.root());
    write_turn(&store, TURN, "private-turn");
    assert!(!archive.exists(), "the write imported the archive");
    for marker in [RECORD, CHECKPOINT, CLAIM] {
        assert!(archived_has(&fx, marker), "{marker} archived");
    }
    let mut checkpoint = store.session(&SOURCE.parse().unwrap()).unwrap().record;
    CHECKPOINT.clone_into(&mut checkpoint.summary);
    store
        .checkpoint_rebuild(checkpoint, None, &Redactor::default())
        .unwrap();
    for marker in [TURN, CHECKPOINT] {
        assert!(contains(&fx.root().join("scribe.db-wal"), marker));
    }
    let prompt = "#scribe forget s-1";
    let reply = submit(&fx, prompt);
    assert!(reply.contains("forgot s-1"), "{reply}");
    sweep(&fx, &[RECORD, CHECKPOINT, TURN, CLAIM]);
    assert!(archived_has(&fx, KEPT), "the unrelated audit line stays");
    // The session's own audit line keeps its audit fields.
    let kept: i64 = db(&fx)
        .query_row(
            "SELECT count(*) FROM archive_records WHERE kind = 'audit' AND instr(record, ?1) > 0",
            [format!("session:{SOURCE}")],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1);
}

fn memory_route(with_linked_turn: bool) {
    const STEM: &str = "2026-12-01-release-ships";
    const BODY: &str = "erase-memory-body-94d87";
    const HISTORY: &str = "erase-memory-history-91a7c";
    const TURN: &str = "erase-indexed-memory-turn-ba701";
    let fx = Fixture::new();
    let file = fx.root().join(format!("memory/now/{STEM}.md"));
    put(
        &fx,
        &file,
        &format!(
            "---\ndate: 2026-12-01\nkind: deadline\nsources: [{SOURCE}]\nupdated: 2026-11-02T20:00:00Z\n---\n# Release ships\n\n{BODY}\n",
        ),
    );
    fx.check(
        Cli(&[
            "memory",
            "ack",
            STEM,
            "--session",
            SOURCE,
            "--author",
            "opus",
        ]),
        Success,
    );
    assert!(contains(&fx.root().join("scribe.db-wal"), BODY));
    // A live item can have past text in history and a linked source turn.
    let writer = Connection::open(fx.root().join("scribe.db")).unwrap();
    writer.execute(
        "INSERT INTO memory_history (item, kind, text, sources, removed_at) VALUES (?1, 'now', ?2, ?3, ?4)",
        (STEM, HISTORY, json!([SOURCE]).to_string(), "2026-11-02T20:00:00Z"),
    ).unwrap();
    if with_linked_turn {
        write_turn(&Store::open(fx.root()), TURN, "memory-turn");
        writer
            .execute(
                "INSERT INTO memory_messages (item, session, message) VALUES (?1, ?2, 'memory-turn')",
                (STEM, SOURCE),
            )
            .unwrap();
    }
    drop(writer);
    assert!(contains(&file, BODY));
    assert!(contains(&fx.root().join("scribe.db"), BODY));
    let mut markers = vec![BODY, HISTORY];
    if with_linked_turn {
        markers.push(TURN);
    }
    for marker in &markers {
        assert!(!copies(&fx.root(), marker).is_empty());
    }
    fx.check(
        Cli(&["memory", "forget", STEM, "--author", "opus", "--confirm"]),
        Success,
    );
    sweep(&fx, &markers);
}

fn imported_issue_route() {
    const CAUSE: &str = "erase-imported-issue-cause-2fa40";
    const FIX: &str = "erase-imported-issue-fix-514df";
    const CORRECTED: &str =
        "Corrected incident notes\nCause: Different cause.\nFix: Different fix.";
    let fx = Fixture::new();
    fx.config("[tags.issue]\ndescription = \"Stored incidents\"\n");
    // Two live issue entries; issue-2 quotes the cause the archived issue-1
    // file holds, so the file is its source whatever its name says.
    let issue = |n: usize, content: &str, quote: &str| {
        agent_entry(1, content)
            .replace("preferences-1", &format!("issue-{n}"))
            .replace("tag: preferences", "tag: issue")
            .replace("quote: null", &format!("quote: \"{quote}\""))
    };
    let kept = issue(1, "kept live issue", "kept live quote");
    let forgotten = issue(2, &format!("Cause: {CAUSE}. Fix: {FIX}"), CAUSE);
    seed_records(&fx.root(), &[&kept, &forgotten]);

    let legacy = include_str!("fixtures/issue-store/issue-1.md");
    let source = legacy
        .replacen(
            "## Cause\ncause not found",
            &format!("## Cause\n{CAUSE}"),
            1,
        )
        .replacen("## Fix\nfix not found", &format!("## Fix\n{FIX}"), 1);
    let neighbor = legacy
        .replace("id: issue-1", "id: issue-2")
        .replace("2026-11-22T04:09:43Z", "2026-11-22T04:10:00Z")
        .replace("GitHub API secondary rate limit during drains", KEPT);
    let archive = fx.root().join("pre-issue-2026-11-22");
    put(&fx, &archive.join("issue-1.md"), &source);
    put(&fx, &archive.join("issue-2.md"), &neighbor);
    // The next write imports the archive like any other.
    fx.check(
        &[
            "update",
            "issue-2",
            "--author",
            "opus",
            "--content",
            CORRECTED,
        ],
        Success,
    );
    assert!(!archive.exists(), "the write imported the archive");
    assert!(archived_has(&fx, CAUSE));
    assert!(archived_has(&fx, KEPT));

    let prompt = "#scribe forget issue-2";
    assert!(submit(&fx, prompt).contains("forgot"));
    assert_eq!(pending(&fx), 0);
    sweep(&fx, &[CAUSE, FIX]);
    assert!(
        archived_has(&fx, KEPT),
        "the unrelated archived issue stays"
    );
    let unrelated = json_output(&fx.check(&["show", "issue-1", "--json"], Success));
    assert_eq!(unrelated["content"], "kept live issue");
}

#[test]
fn public_forgets_leave_no_copy_of_erased_text_in_the_store() {
    entry_routes();
    concurrent_entries();
    session_route();
    memory_route(false);
    memory_route(true);
    imported_issue_route();
}
