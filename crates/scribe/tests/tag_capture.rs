//! Per-tag capture rules through the built binary
//! (docs/design/scribe/2_design.md §11.2): `scribe tag show` with each
//! field's origin, config overrides field by field, the refusals, and the
//! shipped tag set.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::{Value, json};
use support::{Fixture, make_private, snapshot, text};

fn config(fx: &Fixture, toml: &str) -> std::path::PathBuf {
    let root = fx.root();
    if !root.exists() {
        fx.create_private_dir_all(&root);
    }
    let file = root.join("config.toml");
    std::fs::write(&file, toml).unwrap();
    make_private(&file);
    file
}

fn show(fx: &Fixture, tag: &str) -> Value {
    serde_json::from_slice(&fx.ok(&["tag", "show", tag, "--json"]).stdout).unwrap()
}

const SHIPPED_TRIGGERS: [&str; 6] = [
    "always",
    "never",
    "should not",
    "from now on",
    "I prefer",
    "by default",
];

#[test]
fn with_no_capture_config_tag_show_prints_the_shipped_defaults() {
    let fx = Fixture::new();
    let shown = show(&fx, "preferences");
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    assert_eq!(
        shown["capture"],
        json!({
            "from": "user",
            "route": "propose",
            "triggers": SHIPPED_TRIGGERS,
            "criteria": "A standing rule from the user that applies beyond the work at hand.",
            "not": "A choice about one page or task (it goes on its Orbit task or the feature's 4_decisions.md); an acknowledgement.",
            "origin": {"from": shipped, "route": shipped, "triggers": shipped,
                       "criteria": shipped, "not": shipped}
        })
    );
    assert_eq!(shown["tag"], "preferences");
    assert_eq!(shown["action"], Value::Null);
    // An alias names the same tag.
    assert_eq!(show(&fx, "pref"), shown);

    // The pass writes `idea` entries itself; `standard` ships no rules.
    let idea = show(&fx, "idea");
    assert_eq!(idea["capture"]["route"], "write");
    assert_eq!(idea["capture"]["triggers"][4], "what if we");
    assert_eq!(idea["action"]["mode"], "free");
    let standard = show(&fx, "standard");
    let none = json!({"source": "none", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not"] {
        assert_eq!(standard["capture"][field], Value::Null, "{field}");
        assert_eq!(standard["capture"]["origin"][field], none, "{field}");
    }

    // Plain: one `<field>\t<value>\t<origin>` line each.
    let plain = text(&fx.ok(&["tag", "show", "preferences"]).stdout);
    assert!(
        plain.lines().any(|l| l == "route\tpropose\tshipped"),
        "{plain}"
    );
}

#[test]
fn overriding_one_field_changes_only_that_field_with_its_file_and_line() {
    let fx = Fixture::new();
    let before = show(&fx, "preferences");
    let file = config(
        &fx,
        "[tags.preferences]\naliases = [\"preference\", \"pref\"]\ndescription = \"how the user likes things done\"\nstale_after_days = 180\n\n[tags.preferences.capture]\ncriteria = \"A rule Daniel states for every project.\"\n",
    );
    let after = show(&fx, "preferences");
    assert_eq!(
        after["capture"]["criteria"],
        "A rule Daniel states for every project."
    );
    assert_eq!(
        after["capture"]["origin"]["criteria"],
        json!({"source": "config", "file": file.display().to_string(), "line": 7})
    );
    // Every other field, and its origin, is unchanged.
    let mut expected = before.clone();
    expected["capture"]["criteria"] = after["capture"]["criteria"].clone();
    expected["capture"]["origin"]["criteria"] = after["capture"]["origin"]["criteria"].clone();
    assert_eq!(after, expected);
    let plain = text(&fx.ok(&["tag", "show", "preferences"]).stdout);
    assert!(
        plain.lines().any(|l| l
            == format!(
                "criteria\tA rule Daniel states for every project.\tconfig {}:7",
                file.display()
            )),
        "{plain}"
    );

    // `triggers` replaces the whole list.
    config(
        &fx,
        "[tags.preferences.capture]\ntriggers = [\"from now on\"]\n",
    );
    let replaced = show(&fx, "preferences");
    assert_eq!(replaced["capture"]["triggers"], json!(["from now on"]));
    assert_eq!(replaced["capture"]["origin"]["triggers"]["line"], 2);
    assert_eq!(replaced["capture"]["origin"]["not"]["source"], "shipped");
    assert_eq!(
        replaced["aliases"],
        json!(["preference", "pref"]),
        "omitted aliases stay the shipped ones"
    );
    assert_eq!(replaced["description"], "how the user likes things done");
    assert_eq!(replaced["stale_after_days"], 180);
}

#[test]
fn tag_show_is_read_only_and_unknown_tags_are_refused() {
    let fx = Fixture::new();
    fx.seed();
    let before = snapshot(&fx.root());
    fx.ok(&["tag", "show", "preferences"]);
    fx.ok(&["tag", "show", "preferences", "--json"]);
    let out = fx.run(&["tag", "show", "nonesuch", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "tag_not_found", "{err}");
    assert_eq!(snapshot(&fx.root()), before, "no write, no lock, no audit");
}

#[test]
fn from_assistant_and_bad_capture_fields_are_refused_as_config_invalid() {
    for (toml, needle) in [
        (
            "[tags.preferences.capture]\nfrom = \"assistant\"\n",
            "from = \"assistant\" is not accepted",
        ),
        (
            "[tags.preferences.capture]\nroute = \"file\"\n",
            "is not one of: propose, write, agent",
        ),
        (
            "[tags.preferences.capture]\ntriggers = [\"* idea\"]\n",
            "'*' stands between two words",
        ),
        (
            "[tags.preferences.capture]\ntriggers = [\"no. stop\"]\n",
            "must be words separated by spaces",
        ),
        ("[tags.preferences.capture]\nstray = 1\n", "unknown field"),
        (
            "[tags.rituals.capture]\ntriggers = [\"every morning\"]\n",
            "needs from, route",
        ),
    ] {
        let fx = Fixture::new();
        config(&fx, toml);
        let out = fx.run(&["tag", "show", "preferences", "--json"]);
        assert_eq!(out.status.code(), Some(1), "{toml}");
        let err: Value = serde_json::from_slice(&out.stderr).unwrap();
        assert_eq!(err["code"], "config_invalid", "{toml}: {err}");
        assert!(
            err["error"].as_str().unwrap().contains(needle),
            "{toml}: {err}"
        );
    }
}

#[test]
fn a_new_tag_takes_capture_rules_from_config_and_no_issue_tag_ships() {
    let fx = Fixture::new();
    config(
        &fx,
        "[tags.rituals.capture]\nfrom = \"daniel\"\nroute = \"propose\"\ntriggers = [\"every morning\"]\n",
    );
    let rituals = show(&fx, "rituals");
    assert_eq!(rituals["capture"]["triggers"], json!(["every morning"]));
    assert_eq!(rituals["capture"]["criteria"], Value::Null);
    assert_eq!(rituals["capture"]["origin"]["criteria"]["source"], "none");

    let listed: Value = serde_json::from_slice(&fx.ok(&["tag", "list", "--json"]).stdout).unwrap();
    let routes: Vec<(String, Value)> = listed["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t["route"].clone()))
        .collect();
    assert!(routes.contains(&(String::from("rituals"), json!("propose"))));
    assert!(routes.contains(&(String::from("standard"), Value::Null)));
    assert!(
        !routes.iter().any(|(name, _)| name == "issue"),
        "no issue tag ships: {routes:?}"
    );
    let out = Fixture::new().run(&["tag", "show", "issue", "--json"]);
    assert_eq!(out.status.code(), Some(1));
}

fn listed(fx: &Fixture) -> Value {
    serde_json::from_slice(&fx.ok(&["tag", "list", "--json"]).stdout).unwrap()
}

fn reads(fx: &Fixture) {
    fx.ok(&["tag", "list"]);
    fx.ok(&["entry", "list", "--json"]);
    fx.ok(&["search", "preference", "--json"]);
}

#[test]
fn declaring_a_new_tag_keeps_all_shipped_tags_and_their_defaults() {
    let fx = Fixture::new();
    config(&fx, "[tags.remind]\naction = true\n");

    let listed = listed(&fx);
    let tags = listed["tags"].as_array().unwrap();
    let names: Vec<&str> = tags.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["idea", "preferences", "remind", "standard"]);

    let prefs = tags.iter().find(|t| t["name"] == "preferences").unwrap();
    assert_eq!(prefs["stale_after_days"], 180);
    assert_eq!(prefs["aliases"], json!(["preference", "pref"]));
    assert_eq!(prefs["description"], "how the user likes things done");
    assert_eq!(prefs["configured"], true);
    assert_eq!(prefs["source"], "config");
    assert_eq!(prefs["route"], "propose");
    assert_eq!(prefs["action"], Value::Null);

    let remind = tags.iter().find(|t| t["name"] == "remind").unwrap();
    assert_eq!(
        remind["action"],
        json!({"mode": "free", "require_quote": false, "retired": 0, "rejected": 0})
    );
    assert_eq!(remind["configured"], true);
    assert_eq!(remind["source"], "config");
    assert!(text(&fx.ok(&["tag", "list"]).stderr).is_empty());

    let shown = show(&fx, "preferences");
    assert_eq!(shown["stale_after_days"], 180);
    assert_eq!(shown["aliases"], json!(["preference", "pref"]));
    assert_eq!(shown["capture"]["triggers"], json!(SHIPPED_TRIGGERS));
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not"] {
        assert_eq!(shown["capture"]["origin"][field], shipped, "{field}");
    }
}

#[test]
fn partial_preferences_overrides_only_set_fields_and_reports_origins() {
    let fx = Fixture::new();
    let file = config(&fx, "[tags.preferences]\nstale_after_days = 90\n");

    let shown = show(&fx, "preferences");
    assert_eq!(shown["stale_after_days"], 90);
    assert_eq!(shown["aliases"], json!(["preference", "pref"]));
    assert_eq!(shown["description"], "how the user likes things done");
    assert_eq!(shown["capture"]["triggers"], json!(SHIPPED_TRIGGERS));
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not"] {
        assert_eq!(shown["capture"]["origin"][field], shipped, "{field}");
    }

    config(
        &fx,
        "[tags.preferences]\nstale_after_days = 90\n\n[tags.preferences.capture]\ncriteria = \"A standing rule across projects.\"\n",
    );
    let updated = show(&fx, "preferences");
    assert_eq!(updated["stale_after_days"], 90);
    assert_eq!(updated["aliases"], json!(["preference", "pref"]));
    assert_eq!(updated["description"], "how the user likes things done");
    assert_eq!(
        updated["capture"]["criteria"],
        "A standing rule across projects."
    );
    assert_eq!(
        updated["capture"]["origin"]["criteria"],
        json!({"source": "config", "file": file.display().to_string(), "line": 5})
    );
    for field in ["from", "route", "triggers", "not"] {
        assert_eq!(updated["capture"]["origin"][field], shipped, "{field}");
    }
}

#[test]
fn a_config_claim_of_a_shipped_word_stays_readable_and_warns() {
    let fx = Fixture::new();
    config(&fx, "[tags.rituals]\naliases = [\"pref\"]\n");
    reads(&fx);
    let list = fx.ok(&["tag", "list", "--json"]);
    let stderr = text(&list.stderr);
    assert!(
        stderr.contains("shipped tag preferences alias pref is claimed by config tag rituals"),
        "{stderr}"
    );
    let tags: Value = serde_json::from_slice(&list.stdout).unwrap();
    let prefs = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "preferences")
        .unwrap();
    assert_eq!(prefs["aliases"], json!(["preference"]));
    assert_eq!(show(&fx, "pref")["tag"], "rituals");
    assert_eq!(show(&fx, "preference")["tag"], "preferences");

    config(&fx, "[tags.rituals]\naliases = [\"standard\"]\n");
    reads(&fx);
    let list = fx.ok(&["tag", "list"]);
    let stderr = text(&list.stderr);
    assert!(
        stderr.contains("shipped tag standard is claimed by config tag rituals"),
        "{stderr}"
    );
    assert_eq!(show(&fx, "standard")["tag"], "rituals");
    assert_eq!(show(&fx, "idea")["tag"], "idea");

    // Two of his own entries still refuse the file.
    config(
        &fx,
        "[tags.rituals]\naliases = [\"pref\"]\n[tags.habits]\naliases = [\"pref\"]\n",
    );
    let refused = fx.run(&["tag", "list", "--json"]);
    assert_eq!(refused.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&refused.stderr).unwrap();
    assert_eq!(err["code"], "config_invalid", "{err}");
    assert!(err["error"].as_str().unwrap().contains("'pref'"), "{err}");
}

#[test]
fn the_mac_preferences_and_remind_config_reads_as_the_shipped_preferences_tag() {
    let fx = Fixture::new();
    config(
        &fx,
        "[tags.preferences]\naliases = [\"preference\", \"pref\"]\n\n[tags.remind]\naction = true\n",
    );
    reads(&fx);
    assert!(text(&fx.ok(&["tag", "list"]).stderr).is_empty());
    let tags = listed(&fx);
    let names: Vec<&str> = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["idea", "preferences", "remind", "standard"]);
    let prefs = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "preferences")
        .unwrap();
    assert_eq!(prefs["aliases"], json!(["preference", "pref"]));
    assert_eq!(prefs["stale_after_days"], 180);
    assert_eq!(prefs["description"], "how the user likes things done");
    assert_eq!(prefs["route"], "propose");
    let shown = show(&fx, "preferences");
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not"] {
        assert_eq!(shown["capture"]["origin"][field], shipped, "{field}");
    }
    assert_eq!(show(&fx, "pref")["tag"], "preferences");
    assert_eq!(show(&fx, "remind")["action"]["mode"], "free");
}
