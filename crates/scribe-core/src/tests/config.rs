use crate::config::Config;

#[test]
fn an_empty_config_is_the_defaults() {
    let config = Config::parse("").unwrap();
    assert_eq!(config, Config::default());
    assert_eq!(config.models.ask, "claude-sonnet-5-5");
    assert_eq!(config.models.referent, "claude-haiku-5-5");
    assert_eq!(config.catalog.every_minutes.get(), 20);
    assert_eq!(
        config.secret_names(),
        ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]
    );
}

#[test]
fn a_full_config_is_read_whole() {
    let config = Config::parse(
        r#"
[tags.rituals]
aliases = ["Ritual"]
description = "things done every time"
stale_after_days = 30

[models]
ask = "claude-opus-5-5"
credentials = ["ANTHROPIC_API_KEY"]

[catalog]
every_minutes = 5
every_turns = 3

[redact]
env = ["GITHUB_TOKEN", "ANTHROPIC_API_KEY"]
"#,
    )
    .unwrap();
    let names: Vec<&str> = config.tags.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["rituals"], "a [tags] table replaces the defaults");
    assert_eq!(
        config.tags.resolve("ritual").unwrap().name.as_str(),
        "rituals"
    );
    assert_eq!(config.models.ask, "claude-opus-5-5");
    assert_eq!(
        config.models.session, "claude-haiku-5-5",
        "an unset key keeps its default"
    );
    assert_eq!(config.catalog.every_turns.get(), 3);
    assert_eq!(config.secret_names(), ["ANTHROPIC_API_KEY", "GITHUB_TOKEN"]);
}

#[test]
fn an_unknown_key_or_bad_value_is_refused_naming_it() {
    // STD-02 R28: refused at load, never defaulted.
    let err = Config::parse("[models]\nask = \"x\"\nanswer = \"y\"\n").unwrap_err();
    assert!(err.message.contains("answer"), "{err}");
    assert_eq!(err.line, Some(3));
    let err = Config::parse("[catalog]\nevery_minutes = 0\n").unwrap_err();
    assert_eq!(err.line, Some(2), "{err}");
    let err = Config::parse("[tags.Bad]\n").unwrap_err();
    assert!(err.message.contains("tags.Bad"), "{err}");
    let err = Config::parse("[redact]\nenv = [\"not a name\"]\n").unwrap_err();
    assert!(err.message.contains("not a name"), "{err}");
    let err = Config::parse("[tags.a]\naliases = [\"b\"]\n[tags.b]\n").unwrap_err();
    assert!(err.message.contains("'b'"), "{err}");
    assert!(!err.to_string().contains('\n'), "one line: {err}");
}

#[test]
fn segment_windows_is_positive_and_defaults_to_three() {
    assert_eq!(Config::default().segment_windows.get(), 3);
    assert_eq!(
        Config::parse("[catalog]\nsegment_windows = 2\n")
            .unwrap()
            .segment_windows
            .get(),
        2
    );
    for raw in ["0", "-1", "1.5", "\"three\""] {
        assert!(Config::parse(&format!("[catalog]\nsegment_windows = {raw}\n")).is_err());
    }
}

#[test]
fn sessions_inactive_after_days_defaults_to_120_and_must_be_a_positive_integer() {
    assert_eq!(
        Config::parse("")
            .unwrap()
            .sessions
            .inactive_after_days
            .get(),
        120
    );
    let config = Config::parse("[sessions]\ninactive_after_days = 30\n").unwrap();
    assert_eq!(config.sessions.inactive_after_days.get(), 30);

    for bad in ["0", "-5", "1.5", "\"ninety\""] {
        let err =
            Config::parse(&format!("[sessions]\ninactive_after_days = {bad}\n")).expect_err(bad);
        assert_eq!(err.line, Some(2), "{bad}: {err}");
    }
    let err = Config::parse("[sessions]\nactive_days = 3\n").unwrap_err();
    assert!(err.message.contains("active_days"), "{err}");
}
