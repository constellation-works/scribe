//! Prompts name "the user" or `[user] name`, and `[instructions]` replaces
//! only a use's instruction text (crate::voice).

use crate::config::Config;
use crate::voice::{ModelUse, UserName};

#[test]
fn config_reads_the_user_name_and_per_use_instructions() {
    let config = Config::parse(
        "[user]\nname = \" Sam \"\n\n[instructions]\nask = \"Answer for {user}.\"\nsession = \"\"\"\nKeep the record.\nBriefly.\n\"\"\"\n",
    )
    .unwrap();
    assert_eq!(config.user.name(), Some("Sam"));
    assert_eq!(
        config.voice(ModelUse::Session).instructions.as_deref(),
        Some("Keep the record.\nBriefly.")
    );
    let defaults = Config::parse("").unwrap();
    assert_eq!(defaults.user, UserName::default());
}

#[test]
fn config_refuses_a_bad_name_or_instruction() {
    let long_name = format!("[user]\nname = \"{}\"\n", "a".repeat(65));
    let long_text = format!("[instructions]\nask = \"{}\"\n", "a".repeat(4_001));
    for (text, needle) in [
        ("[user]\nname = \"  \"\n", "user.name must not be empty"),
        ("[user]\nname = \"{user}\"\n", "braces"),
        ("[user]\nname = \"Sam\\u0007\"\n", "control"),
        (long_name.as_str(), "at most 64"),
        ("[user]\nemail = \"x\"\n", "unknown field"),
        (
            "[instructions]\nask = \" \"\n",
            "instructions.ask must not be empty",
        ),
        ("[instructions]\nreferent = \"x\"\n", "unknown field"),
        ("[instructions]\nask = \"a\\u0007b\"\n", "control"),
        (long_text.as_str(), "at most 4000"),
    ] {
        let err = Config::parse(text).unwrap_err();
        assert!(err.message.contains(needle), "{text}: {}", err.message);
    }
}
