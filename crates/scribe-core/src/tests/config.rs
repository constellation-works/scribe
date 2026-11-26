use crate::config::Config;

#[test]
fn an_empty_config_is_the_defaults() {
    let config = Config::parse("").unwrap();
    assert_eq!(config, Config::default());
    assert_eq!(config.models.ask, "claude-sonnet-5-5");
    assert!(config.warnings.is_empty());
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
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "rituals",
            "standard"
        ],
        "a [tags] table merges with the shipped tags"
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
fn a_referent_model_still_loads_with_a_warning_that_it_is_unused() {
    let config =
        Config::parse("[models]\nreferent = \"x\"\n\n[codex.models]\nreferent = \"y\"\n").unwrap();
    assert_eq!(config.warnings.len(), 2, "{:?}", config.warnings);
    assert!(config.warnings[0].contains("models.referent is unused"));
    assert!(config.warnings[1].contains("codex.models.referent is unused"));
    assert!(
        config
            .warnings
            .iter()
            .all(|w| w.contains("models.annotation"))
    );
}

#[test]
fn removal_is_an_instruction_key_that_no_model_table_takes() {
    // 4_decisions.md "The consistency scan and Stop call no model": the
    // accepted keys differ per table. `removal` names an instruction text.
    let kept = r#"
[models]
annotation = "x"
consistency = "y"

[codex.models]
annotation = "x"
consistency = "y"

[instructions]
annotation = "Keep it."
consistency = "Keep it."
removal = "Keep it."
"#;
    assert!(Config::parse(kept).is_ok());
    for table in ["models", "codex.models"] {
        let err = Config::parse(&format!("[{table}]\nremoval = \"x\"\n")).unwrap_err();
        assert!(err.message.contains("removal"), "{table}: {err}");
    }
    let err = Config::parse("[instructions]\nremoval = \" \"\n").unwrap_err();
    assert!(err.message.contains("removal"), "{err}");
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
    let err = Config::parse("[memory]\ndecay_days = 60\n").unwrap_err();
    assert!(err.message.contains("memory"), "{err}");
    assert_eq!(err.line, Some(1));
    let err = Config::parse("[instructions]\nmemory_pairs = \"x\"\n").unwrap_err();
    assert!(err.message.contains("memory_pairs"), "{err}");
    assert_eq!(err.line, Some(2));
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
        Some(
            "A rule for how code or docs are written across {user}'s projects, in {user}'s words."
        )
    );
    assert!(action("standard").is_some() && action("idea").is_some());
    assert!(
        action("task").is_none() && action("friction").is_none() && action("remind").is_none(),
        "task, friction and remind no longer ship"
    );
    assert!(
        config.tags.resolve("finding").is_none(),
        "finding no longer ships"
    );
    let finding = Config::parse("[tags.finding]\nrequire_evidence = true\n").unwrap();
    assert!(
        finding.tags.require_evidence(&"finding".parse().unwrap()),
        "a config can still declare [tags.finding] require_evidence = true"
    );
    let plain_finding = Config::parse("[tags.finding]\n").unwrap();
    assert!(
        !plain_finding
            .tags
            .require_evidence(&"finding".parse().unwrap()),
        "without require_evidence = true, finding is a plain memory tag"
    );
    let declared = Config::parse("[tags.task]\naction = true\n").unwrap();
    assert!(
        declared.tags.action(&"task".parse().unwrap()).is_some(),
        "a config can still declare task as an action tag"
    );
    assert!(action("notes").is_none());

    let err = Config::parse("[tags.notes]\nask_first = true\n").unwrap_err();
    assert!(err.to_string().contains("add action = true"), "{err}");

    let quoted_note = Config::parse("[tags.notes]\nrequire_quote = true\n").unwrap();
    let notes = quoted_note.tags.resolve("notes").unwrap();
    assert!(notes.action.is_none());
    assert!(notes.requires_quote());

    let handoff = Config::parse("[tags.handoff]\nstale_after_days = 14\n").unwrap();
    let handoff = handoff.tags.resolve("handoff").unwrap();
    assert!(handoff.action.is_none());
    assert!(handoff.requires_quote());
    assert_eq!(handoff.stale_after_days, std::num::NonZeroU32::new(14));
}

#[test]
fn capture_rules_override_field_by_field_with_their_line() {
    use crate::tag::{CaptureFrom, CaptureRoute, RuleOrigin};
    let shipped = Config::parse("").unwrap();
    let shipped = &shipped.tags.resolve("preferences").unwrap().capture;
    assert_eq!(shipped.route(), Some(CaptureRoute::Propose));
    assert_eq!(shipped.from.as_ref().unwrap().value, CaptureFrom::User);
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

    let custom = Config::parse(
        "[tags.a.capture]\nfrom = \"user\"\nroute = \"propose\"\ntriggers = [\"one\"]\n\
         [tags.b.capture]\nfrom = \"assistant\"\nroute = \"write\"\ntriggers = [\"two\"]\n\
         [tags.c.capture]\nfrom = \"both\"\nroute = \"write\"\ntriggers = [\"three\"]\n",
    )
    .unwrap();
    assert!(custom.tag_warnings.is_empty(), "{:?}", custom.tag_warnings);
    assert_eq!(
        custom.tags.resolve("a").unwrap().capture.from(),
        Some(CaptureFrom::User)
    );
    assert_eq!(
        custom.tags.resolve("a").unwrap().capture.route(),
        Some(CaptureRoute::Propose)
    );
    assert_eq!(
        custom.tags.resolve("b").unwrap().capture.from(),
        Some(CaptureFrom::Assistant)
    );
    assert_eq!(
        custom.tags.resolve("b").unwrap().capture.route(),
        Some(CaptureRoute::Write)
    );
    assert_eq!(
        custom.tags.resolve("c").unwrap().capture.from(),
        Some(CaptureFrom::Both)
    );
    assert_eq!(
        custom.tags.resolve("c").unwrap().capture.route(),
        Some(CaptureRoute::Write)
    );
}

#[test]
fn a_partial_preferences_table_keeps_shipped_aliases_description_and_stale_days() {
    let shipped = Config::parse("")
        .unwrap()
        .tags
        .resolve("preferences")
        .unwrap()
        .clone();
    let only_capture =
        Config::parse("[tags.preferences.capture]\ntriggers = [\"from now on\"]\n").unwrap();
    let prefs = only_capture.tags.resolve("preferences").unwrap();
    assert_eq!(prefs.aliases, shipped.aliases);
    assert_eq!(
        prefs.description.as_deref(),
        Some("A standing rule {user} wants agents to follow.")
    );
    assert_eq!(prefs.stale_after_days, std::num::NonZeroU32::new(180));
    assert_eq!(
        prefs.capture.triggers.as_ref().unwrap().value,
        ["from now on"]
    );

    let cleared =
        Config::parse("[tags.preferences]\naliases = []\nstale_after_days = 7\n").unwrap();
    let cleared = cleared.tags.resolve("preferences").unwrap();
    assert!(
        cleared.aliases.is_empty(),
        "an explicit empty list is not an omit"
    );
    assert_eq!(cleared.stale_after_days, std::num::NonZeroU32::new(7));
    assert_eq!(cleared.description, shipped.description);

    let rituals = Config::parse("[tags.rituals]\n").unwrap();
    assert_eq!(
        rituals.tags.resolve("preferences"),
        Some(&shipped),
        "a config that never mentions preferences keeps the shipped preferences tag"
    );
    assert!(rituals.tag_warnings.is_empty());
    let rituals = rituals.tags.resolve("rituals").unwrap();
    assert!(rituals.aliases.is_empty());
    assert_eq!(rituals.stale_after_days, None);
    assert_eq!(rituals.description, None);

    let stale_only = Config::parse("[tags.preferences]\nstale_after_days = 90\n").unwrap();
    let stale_prefs = stale_only.tags.resolve("preferences").unwrap();
    assert_eq!(stale_prefs.stale_after_days, std::num::NonZeroU32::new(90));
    assert_eq!(stale_prefs.aliases, shipped.aliases);
    assert_eq!(stale_prefs.description, shipped.description);
    assert_eq!(stale_prefs.capture, shipped.capture);
    assert!(stale_only.tag_warnings.is_empty());

    let remind = Config::parse("[tags.remind]\naction = true\n").unwrap();
    let names: Vec<&str> = remind.tags.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "remind",
            "standard"
        ]
    );
    assert_eq!(remind.tags.resolve("preferences"), Some(&shipped));
    assert!(remind.tag_warnings.is_empty());
    assert!(
        remind
            .tags
            .action(&"remind".parse().unwrap())
            .is_some_and(|action| !action.require_quote && !action.ask_first)
    );
}

#[test]
fn a_shipped_word_yields_to_config_and_two_config_entries_do_not() {
    use crate::tag::{ActionTag, CaptureRules, TagTable};

    let alias_of_shipped = Config::parse("[tags.rituals]\naliases = [\"pref\"]\n").unwrap();
    assert_eq!(
        alias_of_shipped.tags.resolve("pref").unwrap().name.as_str(),
        "rituals"
    );
    let prefs = alias_of_shipped.tags.resolve("preferences").unwrap();
    assert_eq!(
        prefs.aliases.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        ["preference"],
        "preferences loses only the alias the config claimed"
    );
    assert_eq!(prefs.stale_after_days, std::num::NonZeroU32::new(180));
    assert_eq!(
        prefs.description.as_deref(),
        Some("A standing rule {user} wants agents to follow.")
    );
    assert_eq!(prefs.capture, CaptureRules::shipped("preferences"));
    assert_eq!(
        alias_of_shipped.tag_warnings,
        ["shipped tag preferences alias pref is claimed by config tag rituals"]
    );
    assert!(alias_of_shipped.tags.resolve("standard").is_some());
    assert!(alias_of_shipped.tags.resolve("idea").is_none());

    let name_of_shipped = Config::parse("[tags.rituals]\naliases = [\"standard\"]\n").unwrap();
    assert_eq!(
        name_of_shipped
            .tags
            .resolve("standard")
            .unwrap()
            .name
            .as_str(),
        "rituals"
    );
    assert!(
        name_of_shipped
            .tags
            .get(&"standard".parse().unwrap())
            .is_none()
    );
    assert!(name_of_shipped.tags.resolve("preferences").is_some());
    assert!(name_of_shipped.tags.resolve("idea").is_none());
    assert_eq!(
        name_of_shipped.tag_warnings,
        ["shipped tag standard is claimed by config tag rituals"]
    );

    // An omitted alias list does not claim the shipped aliases, so another
    // entry may take one and the declared shipped tag keeps its other fields.
    let inherited = Config::parse(
        "[tags.preferences]\nstale_after_days = 90\n[tags.rituals]\naliases = [\"pref\"]\n",
    )
    .unwrap();
    let prefs = inherited.tags.resolve("preferences").unwrap();
    assert_eq!(prefs.stale_after_days, std::num::NonZeroU32::new(90));
    assert_eq!(
        prefs.aliases.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
        ["preference"]
    );
    assert_eq!(
        inherited.tags.resolve("pref").unwrap().name.as_str(),
        "rituals"
    );

    let err = Config::parse(
        "[tags.rituals]\naliases = [\"pref\"]\n[tags.habits]\naliases = [\"pref\"]\n",
    )
    .unwrap_err();
    assert!(err.message.contains("'pref'"), "{err}");
    assert!(err.message.contains("rituals"), "{err}");
    assert!(err.message.contains("habits"), "{err}");

    let err = Config::parse(
        "[tags.preferences]\naliases = [\"preference\", \"pref\"]\n[tags.rituals]\naliases = [\"pref\"]\n",
    )
    .unwrap_err();
    assert!(err.message.contains("'pref'"), "{err}");

    // Daniel's Mac file: the preferences block repeats the shipped aliases,
    // and remind is an extra action tag. The effective table matches a
    // preferences tag that still carries every shipped field.
    let mac = Config::parse(
        "[tags.preferences]\naliases = [\"preference\", \"pref\"]\n\n[tags.remind]\naction = true\n",
    )
    .unwrap();
    assert!(mac.tag_warnings.is_empty(), "{:?}", mac.tag_warnings);
    let names: Vec<&str> = mac.tags.tags().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "decision",
            "handoff",
            "open-item",
            "preferences",
            "remind",
            "standard"
        ]
    );
    let shipped = TagTable::shipped_preferences().unwrap();
    let prefs = mac.tags.resolve("preferences").unwrap();
    assert_eq!(prefs.aliases, shipped.aliases);
    assert_eq!(prefs.description, shipped.description);
    assert_eq!(prefs.stale_after_days, shipped.stale_after_days);
    assert_eq!(prefs.action, None);
    assert!(!prefs.require_evidence);
    assert_eq!(prefs.capture, shipped.capture);
    assert!(mac.tags.resolve("idea").is_none());
    assert!(TagTable::shipped_action("idea").is_none());
    assert_eq!(
        mac.tags.resolve("standard").unwrap(),
        &TagTable::shipped_action("standard").unwrap()
    );
    let remind = mac.tags.resolve("remind").unwrap();
    assert_eq!(
        remind.action,
        Some(ActionTag {
            require_quote: false,
            ask_first: false,
        })
    );
    assert!(remind.aliases.is_empty());
    assert_eq!(remind.description, None);
    assert_eq!(remind.stale_after_days, None);
    assert_eq!(remind.capture, CaptureRules::default());
}

#[test]
fn bad_capture_rules_are_refused_naming_the_field() {
    for (toml, line, needle) in [
        (
            "[tags.preferences.capture]\nfrom = \"robot\"\n",
            Some(2),
            "from",
        ),
        (
            "[tags.preferences.capture]\nfrom = \"daniel\"\n",
            Some(2),
            "is not one of: user, assistant, both",
        ),
        (
            "[tags.preferences.capture]\nroute = \"file\"\n",
            Some(2),
            "route",
        ),
        (
            "[tags.preferences.capture]\nroute = \"agent\"\n",
            Some(2),
            "is not one of: propose, write",
        ),
        (
            "[tags.preferences.capture]\ntriggers = [\"a * * b\"]\n",
            Some(2),
            "'*'",
        ),
        (
            "[tags.notes.capture]\ncriteria = \"x\"\n",
            None,
            "scribe ships no capture rules",
        ),
        (
            "[tags.issue.capture]\ntriggers = [\"root cause\"]\n",
            None,
            "needs from, route",
        ),
    ] {
        let err = Config::parse(toml).unwrap_err();
        assert_eq!(err.line, line, "{toml}: {err}");
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

#[test]
fn unresolved_redact_env_names_only_the_unset_redact_names() {
    use crate::config::unresolved_redact_env;
    let config = Config::parse("[redact]\nenv = [\"GITHUB_TOKEN\", \"EXTRA_TOKEN\"]\n").unwrap();
    assert_eq!(
        unresolved_redact_env(&config, |name| {
            (name == "GITHUB_TOKEN").then(|| String::from("a long enough value"))
        }),
        ["EXTRA_TOKEN"]
    );
    assert_eq!(
        unresolved_redact_env(&config, |_| Some(String::new())),
        ["GITHUB_TOKEN", "EXTRA_TOKEN"],
        "an empty value counts as unset"
    );
    assert!(
        unresolved_redact_env(&Config::default(), |_| None).is_empty(),
        "the default credentials are alternatives, not [redact] env names"
    );
}
