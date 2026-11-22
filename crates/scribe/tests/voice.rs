//! `[user] name` and `[instructions]` through the built binary: the `ask`
//! prompt a fake `claude` receives names the configured user and opens with
//! the configured instruction text, and the output contract after it is the
//! shipped one (docs/design/scribe/specs/store.md#config).
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::json;
use std::io::Write;
use support::{Fixture, text};

/// The shipped `ask` instruction text, as it reads with no name set.
const SHIPPED_ASK: &str = "You answer a question about what the user decided, prefers or did, using only the numbered evidence below.";

/// Run `question ask` on the fixture store with `config` appended to its
/// `config.toml`, a fake `claude` answering `no_record`; the prompt it read.
fn ask_prompt(config: &str) -> String {
    let fx = Fixture::new();
    fx.seed();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(fx.root().join("config.toml"))
        .unwrap();
    file.write_all(config.as_bytes()).unwrap();
    let reply = fx.elsewhere("claude-reply.json");
    let answer = json!({"status":"no_record", "answer":"", "claims":[], "conflicts":[]});
    std::fs::write(
        &reply,
        json!({"type":"result", "is_error":false, "result":"", "structured_output":answer})
            .to_string(),
    )
    .unwrap();
    let prompt = fx.elsewhere("prompt.txt");
    let script = format!(
        "#!/bin/sh\ncat > '{}'\ncat '{}'\n",
        prompt.display(),
        reply.display()
    );
    let path = fx.fake_program("claude", &script);
    let out = fx
        .cmd(&["question", "ask", "which commit subjects are preferred?"])
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    std::fs::read_to_string(prompt).unwrap()
}

#[test]
fn an_unnamed_user_is_the_user_and_a_named_one_is_named() {
    let unnamed = ask_prompt("");
    assert!(unnamed.starts_with(SHIPPED_ASK), "{unnamed}");
    assert!(!unnamed.contains("Daniel"), "{unnamed}");
    assert!(unnamed.contains("words=\"The user\""), "{unnamed}");

    let named = ask_prompt("\n[user]\nname = \"Sam\"\n");
    assert!(
        named.starts_with("You answer a question about what Sam decided"),
        "{named}"
    );
    assert!(named.contains("words=\"Sam\""), "{named}");
    assert!(!named.contains("the user"), "{named}");
}

#[test]
fn an_ask_instruction_override_reaches_the_prompt_and_the_contract_follows_unchanged() {
    let shipped = ask_prompt("");
    let custom = ask_prompt(
        "\n[instructions]\nask = \"Answer what {user} prefers about commits, from the evidence below.\"\n",
    );
    let override_text = "Answer what the user prefers about commits, from the evidence below.";
    assert!(!custom.contains(SHIPPED_ASK), "{custom}");
    assert_eq!(
        custom.strip_prefix(override_text),
        shipped.strip_prefix(SHIPPED_ASK),
        "the contract after the instruction text must be the shipped one"
    );
}

/// A configured name reaches prompts only: `tag show` and `tag list` print
/// the shipped description and criteria as "the user" (2_design.md §15.8).
#[test]
fn a_configured_name_stays_out_of_tag_output() {
    let fx = Fixture::new();
    fx.seed();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(fx.root().join("config.toml"))
        .unwrap();
    file.write_all(b"\n[user]\nname = \"Sam\"\n").unwrap();
    let shown: serde_json::Value =
        serde_json::from_slice(&fx.ok(&["tag", "show", "preferences", "--json"]).stdout).unwrap();
    assert_eq!(shown["description"], "how the user likes things done");
    assert_eq!(
        shown["capture"]["criteria"],
        "A standing rule from the user that applies beyond the work at hand."
    );
    let listed = fx.ok(&["tag", "list", "--json"]).stdout;
    let listed = text(&listed);
    assert!(
        !listed.contains("Sam") && !listed.contains("{user}"),
        "{listed}"
    );
}
