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
        [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY"
        ]
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
    assert_eq!(
        names,
        ["finding", "idea", "remind", "rituals", "standard"],
        "a [tags] table replaces the defaults but keeps the shipped action tags"
    );
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
    assert_eq!(
        config.secret_names(),
        ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GITHUB_TOKEN"]
    );
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
fn projects_take_kebab_slugs_and_non_empty_paths() {
    let config = Config::parse(
        r#"
[projects.scribe]
paths = ["codebases/scribe", " codebases/scribe ", "crates/scribe"]

[projects.orbit]
paths = ["codebases/orbit"]
"#,
    )
    .unwrap();
    let scribe = config.projects.get(&"scribe".parse().unwrap()).unwrap();
    assert_eq!(
        scribe,
        &["codebases/scribe".to_owned(), "crates/scribe".to_owned()]
    );
    assert!(config.projects.contains_key(&"orbit".parse().unwrap()));
    assert!(Config::default().projects.is_empty());

    let err = Config::parse("[projects.Not_Kebab]\npaths = [\"x\"]\n").unwrap_err();
    assert!(err.message.contains("projects.Not_Kebab"), "{err}");
    let err = Config::parse("[projects.scribe]\npaths = []\n").unwrap_err();
    assert!(err.message.contains("at least one path"), "{err}");
    let err = Config::parse("[projects.scribe]\npaths = [\"  \"]\n").unwrap_err();
    assert!(err.message.contains("must not be empty"), "{err}");
    let err = Config::parse("[projects.scribe]\nroots = [\"x\"]\n").unwrap_err();
    assert!(err.message.contains("roots"), "{err}");
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

#[test]
fn provider_model_tables_preserve_explicit_ids_and_transports() {
    let config = Config::parse("[models]\nprovider='codex'\nask='custom-global'\n[codex.models]\nprovider='claude-code'\nsession='explicit-claude'\n").unwrap();
    assert_eq!(config.models.provider, crate::Provider::Codex);
    assert_eq!(config.models.ask, "custom-global");
    assert_eq!(config.models.session, "gpt-6-luna");
    let codex = config.for_provider(crate::Provider::Codex);
    assert_eq!(codex.models.provider, crate::Provider::ClaudeCode);
    assert_eq!(codex.models.session, "explicit-claude");
    assert_eq!(codex.models.ask, "claude-sonnet-5-5");
    assert!(Config::parse("[codex.models]\nprovider='unknown'\n").is_err());
    assert!(Config::parse("[codex.models]\nsession=''\n").is_err());
}

#[test]
fn action_tags_come_from_config_and_shipped_ones_can_be_retuned() {
    let config = Config::parse(
        "[tags.idea]\ndescription = \"ideas to try\"\naction = true\nask_first = true\n\
         [tags.standard]\nask_first = true\n\
         [tags.notes]\ndescription = \"plain notes\"\n",
    )
    .unwrap();
    let action = |n: &str| config.tags.action(&n.parse().unwrap());
    let idea = action("idea").unwrap();
    assert!(idea.ask_first && !idea.require_quote);
    let standard = action("standard").unwrap();
    assert!(
        standard.ask_first && standard.require_quote,
        "a config tag named like a shipped one starts from its settings"
    );
    assert_eq!(
        config
            .tags
            .resolve("standard")
            .unwrap()
            .description
            .as_deref(),
        Some("a rule change for the constellation standards, in Daniel's words")
    );
    assert!(action("remind").is_some() && action("idea").is_some());
    assert!(
        action("task").is_none() && action("friction").is_none(),
        "task and friction no longer ship"
    );
    let declared = Config::parse("[tags.task]\naction = true\n").unwrap();
    assert!(
        declared.tags.action(&"task".parse().unwrap()).is_some(),
        "a config can still declare task as an action tag"
    );
    assert!(action("notes").is_none());

    let err = Config::parse("[tags.notes]\nrequire_quote = true\n").unwrap_err();
    assert!(err.to_string().contains("add action = true"), "{err}");
}

#[test]
fn capture_rules_override_field_by_field_with_their_line() {
    use crate::tag::{CaptureFrom, CaptureRoute, RuleOrigin};
    let shipped = Config::parse("").unwrap();
    let shipped = &shipped.tags.resolve("preferences").unwrap().capture;
    assert_eq!(shipped.route(), Some(CaptureRoute::Propose));
    assert_eq!(shipped.from.as_ref().unwrap().value, CaptureFrom::Daniel);
    assert_eq!(
        shipped.criteria.as_ref().unwrap().origin,
        RuleOrigin::Shipped
    );

    let config = Config::parse(
        "[tags.preferences.capture]\n\ntriggers = [\"from now on\", \"from now on\"]\n",
    )
    .unwrap();
    let capture = &config.tags.resolve("preferences").unwrap().capture;
    let triggers = capture.triggers.as_ref().unwrap();
    assert_eq!(
        triggers.value,
        ["from now on"],
        "replaced whole, deduplicated"
    );
    assert_eq!(triggers.origin, RuleOrigin::Config { line: Some(3) });
    assert_eq!(capture.criteria, shipped.criteria);
    assert_eq!(capture.not, shipped.not);
    assert_eq!(capture.from, shipped.from);
}

#[test]
fn bad_capture_rules_are_refused_naming_the_field() {
    for (toml, needle) in [
        (
            "[tags.preferences.capture]\nfrom = \"assistant\"\n",
            "from = \"assistant\" is not accepted",
        ),
        ("[tags.preferences.capture]\nfrom = \"robot\"\n", "from"),
        ("[tags.preferences.capture]\nroute = \"file\"\n", "route"),
        (
            "[tags.preferences.capture]\ntriggers = [\"a * * b\"]\n",
            "'*'",
        ),
        (
            "[tags.notes.capture]\ncriteria = \"x\"\n",
            "scribe ships no capture rules",
        ),
    ] {
        let err = Config::parse(toml).unwrap_err();
        assert!(err.to_string().contains(needle), "{toml}: {err}");
    }
    let long = format!(
        "[tags.preferences.capture]\ncriteria = \"{}\"\n",
        "x".repeat(301)
    );
    assert!(Config::parse(&long).is_err());
    let many: Vec<String> = (0..33).map(|n| format!("\"w{n}\"")).collect();
    let many = format!(
        "[tags.preferences.capture]\ntriggers = [{}]\n",
        many.join(", ")
    );
    assert!(Config::parse(&many).is_err());
}
