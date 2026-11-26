use crate::support;
use crate::support::Fixture;
use crate::support::command::Exit::Capture;
use crate::support::command::Json;
use crate::support::command::{Expectation, Input};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Output;

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &[], expect)
    }
}

pub(super) const HELP: &[(&str, &[&str])] = &[
    ("root", &["--help"]),
    ("init", &["init", "--help"]),
    ("entry", &["entry", "--help"]),
    ("entry-add", &["entry", "add", "--help"]),
    ("entry-update", &["entry", "update", "--help"]),
    ("entry-retire", &["entry", "retire", "--help"]),
    ("entry-forget", &["entry", "forget", "--help"]),
    ("entry-list", &["entry", "list", "--help"]),
    ("entry-show", &["entry", "show", "--help"]),
    ("entry-pickup", &["entry", "pickup", "--help"]),
    ("entry-answer", &["entry", "answer", "--help"]),
    ("entry-close", &["entry", "close", "--help"]),
    ("session", &["session", "--help"]),
    ("session-list", &["session", "list", "--help"]),
    ("session-show", &["session", "show", "--help"]),
    ("session-pass", &["session", "pass", "--help"]),
    (
        "session-pass-prepare",
        &["session", "pass-prepare", "--help"],
    ),
    ("session-pass-apply", &["session", "pass-apply", "--help"]),
    ("session-rebuild", &["session", "rebuild", "--help"]),
    ("session-deactivate", &["session", "deactivate", "--help"]),
    ("session-activate", &["session", "activate", "--help"]),
    ("session-catalog", &["session", "catalog", "--help"]),
    ("group", &["group", "--help"]),
    ("group-open", &["group", "open", "--help"]),
    ("group-join", &["group", "join", "--help"]),
    ("group-backfill", &["group", "backfill", "--help"]),
    ("group-leave", &["group", "leave", "--help"]),
    ("group-close", &["group", "close", "--help"]),
    ("group-evidence", &["group", "evidence", "--help"]),
    ("group-show", &["group", "show", "--help"]),
    ("group-list", &["group", "list", "--help"]),
    ("proposal", &["proposal", "--help"]),
    ("proposal-add", &["proposal", "add", "--help"]),
    ("proposal-update", &["proposal", "update", "--help"]),
    ("proposal-settle", &["proposal", "settle", "--help"]),
    ("proposal-list", &["proposal", "list", "--help"]),
    ("proposal-show", &["proposal", "show", "--help"]),
    ("memory", &["memory", "--help"]),
    ("memory-show", &["memory", "show", "--help"]),
    ("memory-rules", &["memory", "rules", "--help"]),
    ("memory-ack", &["memory", "ack", "--help"]),
    ("memory-forget", &["memory", "forget", "--help"]),
    ("memory-restore", &["memory", "restore", "--help"]),
    ("memory-refresh", &["memory", "refresh", "--help"]),
    ("memory-prepare", &["memory", "prepare", "--help"]),
    ("memory-apply", &["memory", "apply", "--help"]),
    ("tag", &["tag", "--help"]),
    ("tag-add", &["tag", "add", "--help"]),
    ("tag-update", &["tag", "update", "--help"]),
    ("tag-list", &["tag", "list", "--help"]),
    ("tag-show", &["tag", "show", "--help"]),
    ("brief", &["brief", "--help"]),
    ("search", &["search", "--help"]),
    ("transcript", &["transcript", "--help"]),
    ("transcript-list", &["transcript", "list", "--help"]),
    ("transcript-show", &["transcript", "show", "--help"]),
    ("consistency", &["consistency", "--help"]),
    ("consistency-scan", &["consistency", "scan", "--help"]),
    ("mark", &["mark", "--help"]),
    ("mark-list", &["mark", "list", "--help"]),
    ("store", &["store", "--help"]),
    ("store-migrate", &["store", "migrate", "--help"]),
    ("export", &["export", "--help"]),
    ("hook", &["hook", "--help"]),
    ("hook-session-start", &["hook", "session-start", "--help"]),
    (
        "hook-user-prompt-submit",
        &["hook", "user-prompt-submit", "--help"],
    ),
    ("hook-stop", &["hook", "stop", "--help"]),
    ("hook-session-end", &["hook", "session-end", "--help"]),
    ("plugin", &["plugin", "--help"]),
    ("plugin-call", &["plugin", "call", "--help"]),
];

/// Each command whose output is pinned, by golden name; every one is
/// captured plain, as a table and as JSON.
pub(super) const OUTPUT: &[(&str, &[&str])] = &[
    ("entry-list", &["entry", "list"]),
    ("entry-show", &["entry", "show", "decisions-2"]),
    ("session-list", &["session", "list"]),
    ("session-list-tree", &["session", "list", "--tree"]),
    // An inactive session is listed only on request, with an ACTIVE column.
    (
        "session-list-inactive",
        &["session", "list", "--include-inactive"],
    ),
    (
        "session-show",
        &["session", "show", "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"],
    ),
    // The log's texts, and one segment by its citation.
    (
        "session-show-log",
        &[
            "session",
            "show",
            "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
            "--log",
        ],
    ),
    (
        "session-show-segment",
        &[
            "session",
            "show",
            "session:7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02#2",
        ],
    ),
    // An inactive record, with the change that hid it.
    (
        "session-show-inactive",
        &["session", "show", "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05"],
    ),
    // A legacy (format 1) record whose child is derived from the other's link.
    (
        "session-show-parent",
        &["session", "show", "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01"],
    ),
    ("proposal-list", &["proposal", "list"]),
    // The last two sessions in the project and the open consistency claim.
    (
        "brief",
        &[
            "brief",
            "--project",
            "constellation-works/scribe",
            "--sessions",
            "2",
        ],
    ),
    // Ranked with no model call: entries, session headers and segments.
    ("search", &["search", "merge agent PRs"]),
    ("tag-list", &["tag", "list"]),
    // Capture rules, each field with its origin: all shipped here.
    ("tag-show", &["tag", "show", "preferences"]),
];

pub(super) const MODES: &[(&str, &[&str])] = &[
    ("plain.txt", &[]),
    ("table.txt", &["--format", "table"]),
    ("json", &["--json"]),
];

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("goldens")
}

fn updating() -> bool {
    std::env::var_os("UPDATE_GOLDENS").is_some_and(|v| v == "1")
}

pub(super) fn report(failures: &[String]) {
    assert!(
        failures.is_empty(),
        "{}\n\nIf the change is intended: make goldens UPDATE=1, then review the diff.",
        failures.join("\n\n")
    );
}

/// The subcommand names on a help page, less clap's built-in `help`.
///
/// A noun's page lists them under `Commands:`. The root page lists them under
/// group headings and has no `Commands:` line.
pub(super) fn commands_in(help: &str) -> Vec<String> {
    command_rows_in(help)
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| name != "help")
        .collect()
}

/// `(name, summary)` for every subcommand row on a help page.
pub(super) fn command_rows_in(help: &str) -> Vec<(String, String)> {
    if help.contains("\nCommands:\n") || help.starts_with("Commands:\n") {
        return help
            .split("Commands:")
            .nth(1)
            .unwrap_or_default()
            .lines()
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .filter_map(command_row)
            .collect();
    }
    // Grouped root help has no `Commands:` line. Rows sit under headings such
    // as `Setup:`, and stop at clap's own flag headings. A leaf page's
    // `<ID>` argument, and the examples, are not commands.
    let mut rows = Vec::new();
    let mut in_group = false;
    for line in help.lines() {
        if !line.starts_with(' ') && line.ends_with(':') {
            if matches!(
                line,
                "Options:" | "Global options:" | "Arguments:" | "Usage:"
            ) {
                break;
            }
            in_group = true;
            continue;
        }
        if in_group {
            rows.extend(command_row(line));
        }
    }
    rows
}

/// One command row: two spaces, a name, then its summary. A wrapped
/// continuation is indented further and is not a new command.
fn command_row(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("  ")?;
    if rest.is_empty() || rest.starts_with(' ') {
        return None;
    }
    Some(match rest.split_once(char::is_whitespace) {
        Some((name, summary)) => (name.to_owned(), summary.trim().to_owned()),
        None => (rest.to_owned(), String::new()),
    })
}

/// Run the built binary with optional `PATH` and a JSON payload on stdin.
pub(super) fn run_with_input(
    fx: &Fixture,
    args: &[&str],
    input: &[u8],
    path: Option<&std::ffi::OsStr>,
) -> Output {
    let mut command = fx.cmd(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    fx.check(&mut command, Capture.stdin(input))
}

/// Daniel's turn the stated-claim goldens quote, verbatim.
const STATED_QUOTE: &str = "ok that makes sense. then let's keep the whole history in an append-only log, not one rewritten outcome.";

/// Scribe's statement of it, the approved entry's content.
const STATED: &str = "The user prefers that session records keep the whole session's history in an append-only log rather than one rewritten outcome.";

/// Which of the three stated-claim stores [`stated_records`] builds.
#[derive(Clone, Copy, PartialEq)]
enum Stated {
    /// `p-5` approved by Daniel, `p-6` open.
    Plain,
    /// [`Stated::Plain`] plus an open claim `p-7` carrying a context paragraph.
    Context,
    /// [`Stated::Plain`], with both claims settled by an agent on Daniel's
    /// checked instruction.
    Instructed,
}

/// The Markdown of the approved entry `preferences-2` and the JSON of the
/// proposals `variant` holds, ready for `support::seed_records`.
fn stated_records(variant: Stated) -> (String, String) {
    let sha = "0".repeat(64);
    let instructed = variant == Stated::Instructed;
    let instruction = format!(
        "{{session: {INSTRUCTED_SESSION}, message: user-instruct, at: 2026-11-15T00:04:30Z, quote: \"{INSTRUCTED_WORDS}\"}}"
    );
    let (said, approved_by, entry_instruction, author) = if instructed {
        (
            format!(
                "scribe proposal settle p-5 --approve --author opus --session {INSTRUCTED_SESSION} --instruction '{INSTRUCTED_WORDS}'"
            ),
            "{kind: agent, name: opus}".to_owned(),
            instruction,
            r#"{"kind":"agent", "name":"opus"}"#,
        )
    } else {
        (
            "#scribe approve p-5".to_owned(),
            "null".to_owned(),
            "null".to_owned(),
            r#"{"kind":"user", "name":"user"}"#,
        )
    };
    let entry = format!(
        "---
format: 31
kind: entry
id: preferences-2
tag: preferences
status: active
project: constellation-works/scribe
topics: []
related: []
provider: claude-code
host: dk-server-1
author: {{kind: user, name: user}}
updated_by: null
created: 2026-11-15T00:05:00Z
updated: 2026-11-15T00:05:00Z
inactive_since: null
said: \"{said}\"
content_from: {{kind: message, role: user, message: user-history, at: 2026-11-15T00:00:00Z}}
cite: {{session: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02, message: user-history, at: 2026-11-15T00:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /home/user/workspace/scribe}}
retirement_reason: null
superseded_by: null
review_after: null
quote: \"{STATED_QUOTE}\"
window: null
evidence: null
as_of: null
approved_by: {approved_by}
instruction: {entry_instruction}
removal_instruction: null
stated_by: {{kind: scribe, name: session-pass}}
outcome: null
---
{STATED}
"
    );
    let cite = |message: &str| {
        json!({"session":"7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02", "message":message,
            "at":"2026-11-15T00:00:00Z", "prompt_sha256":sha, "line":1,
            "cwd":"/home/user/workspace/scribe"})
    };
    let admitted = json!({"trigger":"let's keep", "matched":"let's keep",
        "criteria":"A standing rule from the user that applies beyond the work at hand."});
    let author: Value = serde_json::from_str(author).unwrap();
    let mut approved = json!({
        "id":"p-5", "kind":"candidate", "older":"preferences-2", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":STATED_QUOTE,
            "statement":STATED, "subject":"session records", "context":null,
            "evidence":null, "cite":cite("user-history"),
            "project":"constellation-works/scribe", "provider":"claude-code",
            "host":"dk-server-1", "admitted":admitted
        },
        "reason":format!("Unmarked preferences statement: {STATED_QUOTE}"), "quotes":[],
        "state":"approved", "opened_at":"2026-11-15T00:01:00Z",
        "settled_at":"2026-11-15T00:05:00Z", "settled_by":cite("user-approve"),
        "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":{"kind":"user", "name":"user"}, "updated_by":null,
        "closed_reason":null, "instruction":null
    });
    let open_quote = "yeah, for the popup just show the newest five, nothing older.";
    let mut open = json!({
        "id":"p-6", "kind":"candidate", "older":"preferences-3", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":open_quote,
            "statement":"The user prefers that the scribe tmux popup lists only the five newest records.",
            "subject":"the scribe tmux popup", "context":null,
            "evidence":null, "cite":cite("user-popup"),
            "project":"constellation-works/scribe", "provider":"claude-code",
            "host":"dk-server-1", "admitted":admitted
        },
        "reason":format!("Unmarked preferences statement: {open_quote}"), "quotes":[],
        "state":"open", "opened_at":"2026-11-15T00:06:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null, "instruction":null
    });
    let mut proposals = Vec::new();
    if variant == Stated::Context {
        let mut claim = open.clone();
        claim["id"] = json!("p-7");
        claim["older"] = json!("preferences-4");
        claim["opened_at"] = json!("2026-11-15T00:08:00Z");
        claim["reason"] = json!(format!("Unmarked preferences statement: {CONTEXT_QUOTE}"));
        claim["candidate"]["quote"] = json!(CONTEXT_QUOTE);
        claim["candidate"]["context"] = json!(CONTEXT);
        claim["candidate"]["statement"] = Value::Null;
        claim["candidate"]["subject"] = json!("session records");
        claim["candidate"]["cite"]["message"] = json!("user-segments");
        proposals.push(claim);
    }
    if instructed {
        let inst = json!({"session":INSTRUCTED_SESSION, "message":"user-instruct",
            "at":"2026-11-15T00:04:30Z", "quote":INSTRUCTED_WORDS});
        let settled_by = json!({"session":INSTRUCTED_SESSION, "message":"user-instruct",
            "at":"2026-11-15T00:04:30Z", "prompt_sha256":sha, "line":1,
            "cwd":"/home/user/workspace/scribe"});
        for p in [&mut approved, &mut open] {
            p["settled_author"] = author.clone();
            p["settled_by"] = settled_by.clone();
            p["instruction"] = inst.clone();
        }
        open["instruction"]["message"] = json!("user-reject");
        open["instruction"]["at"] = json!("2026-11-15T00:06:30Z");
        open["instruction"]["quote"] = json!(REJECTED_WORDS);
        open["settled_by"]["message"] = json!("user-reject");
        open["settled_by"]["at"] = json!("2026-11-15T00:06:30Z");
        open["state"] = json!("rejected");
        open["settled_at"] = json!("2026-11-15T00:07:00Z");
        open["closed_reason"] = json!("instructed");
    }
    proposals.insert(0, open);
    proposals.insert(0, approved);
    let body = json!({"format":31, "proposals":proposals}).to_string();
    (entry, body)
}

/// A fixture store seeded with [`stated_records`] of `variant`.
fn stated_store(variant: Stated) -> Fixture {
    let fx = Fixture::seeded();
    let (entry, proposals) = stated_records(variant);
    // Seeding adds claims: drop the fixture's two so only the stated ones remain.
    support::sql(&fx.root(), "DELETE FROM claims;");
    support::seed_records(&fx.root(), &[&entry, &proposals]);
    fx
}

/// A store holding one approved pass claim with scribe's statement
/// (`preferences-2`, from `p-5`) and one open one (`p-6`)
/// (2_design.md §11.1).
pub(super) fn stated_fixture() -> Fixture {
    stated_store(Stated::Plain)
}

/// The context paragraph of [`context_fixture`]'s open claim `p-7`
/// (2_design.md §15.3): what was being discussed and what the turn settles.
const CONTEXT: &str = "The user and the agent were choosing how scribe stores session records. The user rejected a single 2,500-character outcome because rewrites lose the session's start, and asked for an append-only log.";

/// The user's words `p-7` cites.
const CONTEXT_QUOTE: &str =
    "no, keep every window as its own segment so the start of the session is never rewritten.";

/// [`stated_fixture`] with its claims `p-5` and `p-6` kept as older claims
/// (a statement, no context), and an open claim `p-7` with a context
/// paragraph.
pub(super) fn context_fixture() -> Fixture {
    stated_store(Stated::Context)
}

/// Daniel's instruction that approved `p-5` in the instructed goldens.
pub(super) const INSTRUCTED_WORDS: &str = "approve p-5, that is how session records should work";

/// His instruction that rejected `p-6`.
pub(super) const REJECTED_WORDS: &str = "reject p-6, the popup can list more";

/// The interactive session he typed it in.
pub(super) const INSTRUCTED_SESSION: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c03";

/// [`stated_fixture`], with `p-5` approved by the agent `opus` on Daniel's
/// checked instruction rather than by him typing it, and `p-6` rejected on
/// the same instruction (2_design.md §12.3).
pub(super) fn instructed_fixture() -> Fixture {
    stated_store(Stated::Instructed)
}

/// Daniel's turn the captured-idea goldens quote, verbatim.
const CAPTURED_QUOTE: &str = "what if we cached the routing table per host? not now though, let's park it until the drains are quiet.";

/// Scribe's statement of it, the captured entry's content.
const CAPTURED: &str =
    "The user suggests caching the routing table per host so lookups stay local.";

/// His words parking it.
const PARKED: &str = "let's park it until the drains are quiet";

/// One idea entry as a complete Markdown record, with its `outcome` line.
#[allow(clippy::too_many_arguments)]
fn idea_entry(
    id: &str,
    author: &str,
    said: &str,
    content_from: &str,
    quote: &str,
    stated_by: &str,
    outcome: &str,
    body: &str,
) -> String {
    let sha = "0".repeat(64);
    format!(
        "---
format: 31
kind: entry
id: {id}
tag: idea
status: active
project: constellation-works/scribe
topics: []
related: []
provider: claude-code
host: dk-server-1
author: {author}
updated_by: null
created: 2026-11-15T00:05:00Z
updated: 2026-11-15T00:05:00Z
inactive_since: null
said: \"{said}\"
content_from: {content_from}
cite: {{session: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02, message: user-idea, at: 2026-11-15T00:00:00Z, prompt_sha256: \"{sha}\", line: 1, cwd: /home/user/workspace/scribe}}
retirement_reason: null
superseded_by: null
review_after: null
quote: {quote}
window: null
evidence: null
as_of: null
approved_by: null
instruction: null
removal_instruction: null
stated_by: {stated_by}
outcome: {outcome}
---
{body}
"
    )
}

/// A store holding an idea the session pass captured itself (`idea-1`,
/// parked in Daniel's words) and one he typed with `#scribe idea:`
/// (`idea-2`) (2_design.md §11.6).
pub(super) fn captured_fixture() -> Fixture {
    let fx = Fixture::seeded();
    // `idea` is a config example now (specs/catchers.md): declare it.
    let config = fx.root().join("config.toml");
    let seeded = std::fs::read_to_string(&config).unwrap_or_default();
    fx.config(&format!("{seeded}\n{}", support::IDEA_CATCHER));
    let pass = "{kind: scribe, name: session-pass}";
    let captured = idea_entry(
        "idea-1",
        pass,
        "",
        "{kind: message, role: user, message: user-idea, at: 2026-11-15T00:00:00Z}",
        &format!("\"{CAPTURED_QUOTE}\""),
        pass,
        &format!("{{state: parked, reason: \"{PARKED}\", task: null}}"),
        CAPTURED,
    );
    let typed = idea_entry(
        "idea-2",
        "{kind: user, name: user}",
        "#scribe idea: a dry-run flag for the session pass",
        "{kind: instruction}",
        "null",
        "null",
        "null",
        "a dry-run flag for the session pass",
    );
    support::seed_records(&fx.root(), &[&captured, &typed]);
    fx
}

/// `out` with an action entry's age, which moves with the clock, masked as
/// `<age>`: the JSON `age_seconds`, the table's `age` field and the plain
/// row's last column.
pub(super) fn without_age(out: &str) -> String {
    let lines: Vec<String> = out
        .lines()
        .map(|line| {
            if let Some((head, _)) = line.split_once("\"age_seconds\": ") {
                format!("{head}\"age_seconds\": \"<age>\",")
            } else if let Some(rest) = line.strip_prefix("age ") {
                format!(
                    "age {}<age>",
                    " ".repeat(rest.len() - rest.trim_start().len())
                )
            } else if line.split('\t').count() == 8 {
                let mut cols: Vec<&str> = line.split('\t').collect();
                cols[7] = "<age>";
                cols.join("\t")
            } else {
                line.to_owned()
            }
        })
        .collect();
    let mut joined = lines.join("\n");
    if out.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

/// Every session of the fixture store as `session show --json` reads it,
/// in `session list` order.
pub(super) fn every_session_shown(fx: &Fixture) -> Value {
    let list: Value = fx.check(&["session", "list", "--include-inactive", "--json"], Json);
    let shown: Vec<Value> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let id = s["id"].as_str().unwrap();
            fx.check(&["session", "show", id, "--json"], Json)
        })
        .collect();
    json!({"list": list, "shown": shown})
}

/// Compare `actual` to the golden at `rel`, or rewrite it when updating.
/// Returns a failure description instead of panicking so one run reports
/// every stale golden at once.
pub(super) fn compare_golden(rel: &str, actual: &str) -> Option<String> {
    let path = golden_dir().join(rel);
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return None;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    (expected != actual).then(|| {
        format!(
            "golden {rel} differs\n--- expected ({})\n{expected}\n--- actual\n{actual}",
            path.display()
        )
    })
}
