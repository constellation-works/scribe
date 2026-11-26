//! Per-tag capture rules through the built binary
//! (docs/design/scribe/2_design.md §11.2): `scribe tag show` with each
//! field's origin, config overrides field by field, the refusals, and the
//! shipped tag set.
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "integration tests stage files, databases, environment and child processes directly"
)]

#[path = "support/tag_capture.rs"]
mod subject;
mod support;

use crate::support::command::Exit::Code;
use crate::support::command::{Json, Text};
use serde_json::{Value, json};
use subject::*;
use support::command::Exit::{Capture, Success};
use support::{Fixture, snapshot, text};

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
            "not": "A choice about one page or task; an acknowledgement; a one-off command.",
            "max_open": 10,
            "origin": {"from": shipped, "route": shipped, "triggers": shipped,
                       "criteria": shipped, "not": shipped, "max_open": shipped}
        })
    );
    assert_eq!(shown["tag"], "preferences");
    assert_eq!(shown["action"], Value::Null);
    // An alias names the same tag.
    assert_eq!(show(&fx, "pref"), shown);

    // `standard` is the other shipped catcher; `idea` is only a config
    // example (specs/catchers.md).
    let standard = show(&fx, "standard");
    assert_eq!(standard["capture"]["route"], "propose");
    assert_eq!(standard["capture"]["from"], "user");
    assert_eq!(standard["capture"]["triggers"][0], "as a standard");
    assert_eq!(standard["capture"]["max_open"], 10);
    assert_eq!(standard["action"]["require_quote"], true);
    assert_eq!(
        fx.check(&["show", "idea", "--json"], Capture).status.code(),
        Some(1)
    );
    // A tag that catches nothing has no cap.
    config(&fx, "[tags.rituals]\naction = true\n");
    let rituals = show(&fx, "rituals");
    let none = json!({"source": "none", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not", "max_open"] {
        assert_eq!(rituals["capture"][field], Value::Null, "{field}");
        assert_eq!(rituals["capture"]["origin"][field], none, "{field}");
    }
    config(&fx, "");

    // Plain: one `<field>\t<value>\t<origin>` line each.
    let plain = fx.check(&["show", "preferences"], Text);
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
        "[tags.preferences]\naliases = [\"preference\", \"pref\"]\ndescription = \"A standing rule {user} wants agents to follow.\"\nstale_after_days = 180\n\n[tags.preferences.capture]\ncriteria = \"A rule Daniel states for every project.\"\n",
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
    let plain = fx.check(&["show", "preferences"], Text);
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
    assert_eq!(
        replaced["description"],
        "A standing rule the user wants agents to follow."
    );
    assert_eq!(replaced["stale_after_days"], 180);
}

#[test]
fn tag_show_is_read_only_and_unknown_tags_are_refused() {
    let fx = Fixture::seeded();
    // The first open of scribe.db (an unknown tag's refusal opens it) leaves
    // an empty `scribe.db-wal`.
    fx.check(&["show", "nonesuch", "--json"], Capture);
    let before = snapshot(&fx.root());
    fx.check(&["show", "preferences"], Success);
    fx.check(&["show", "preferences", "--json"], Success);
    let out = fx.check(&["show", "nonesuch", "--json"], Code(1));
    let err: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(err["code"], "tag_not_found", "{err}");
    assert_eq!(snapshot(&fx.root()), before, "no write, no lock, no audit");
}

#[test]
fn bad_capture_fields_are_refused_as_config_invalid() {
    for (toml, needle) in [
        (
            "[tags.preferences.capture]\nfrom = \"everyone\"\n",
            "line 2: tags.preferences.capture.from: 'everyone' is not one of: user, assistant, both",
        ),
        (
            "[tags.preferences.capture]\nfrom = \"daniel\"\n",
            "line 2: tags.preferences.capture.from: 'daniel' is not one of: user, assistant, both",
        ),
        (
            "[tags.preferences.capture]\nroute = \"file\"\n",
            "line 2: tags.preferences.capture.route: 'file' is not one of: propose, write",
        ),
        (
            "[tags.preferences.capture]\nroute = \"agent\"\n",
            "line 2: tags.preferences.capture.route: 'agent' is not one of: propose, write",
        ),
        (
            "[tags.preferences.capture]\nmax_open = 0\n",
            "max_open: from 1 to 20",
        ),
        (
            "[tags.preferences.capture]\nmax_open = 21\n",
            "max_open: from 1 to 20",
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
        let out = fx.check(&["show", "preferences", "--json"], Capture);
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
        "[tags.rituals.capture]\nfrom = \"user\"\nroute = \"propose\"\ntriggers = [\"every morning\"]\n",
    );
    let rituals = show(&fx, "rituals");
    assert_eq!(rituals["capture"]["from"], "user");
    assert_eq!(rituals["capture"]["route"], "propose");
    assert_eq!(rituals["capture"]["triggers"], json!(["every morning"]));
    assert_eq!(rituals["capture"]["criteria"], Value::Null);
    assert_eq!(rituals["capture"]["origin"]["criteria"]["source"], "none");

    let listed: Value = fx.check(&["list", "--json"], Json);
    let routes: Vec<(String, Value)> = listed["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_owned(), t["route"].clone()))
        .collect();
    assert!(routes.contains(&(String::from("rituals"), json!("propose"))));
    assert!(routes.contains(&(String::from("standard"), json!("propose"))));
    assert!(
        !routes.iter().any(|(name, _)| name == "issue"),
        "no issue tag ships: {routes:?}"
    );
    let out = Fixture::new().check(&["show", "issue", "--json"], Capture);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn declaring_a_new_tag_keeps_all_shipped_tags_and_their_defaults() {
    let fx = Fixture::new();
    config(&fx, "[tags.remind]\naction = true\n");

    let listed = listed(&fx);
    let tags = listed["tags"].as_array().unwrap();
    let names: Vec<&str> = tags.iter().map(|t| t["name"].as_str().unwrap()).collect();
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

    let prefs = tags.iter().find(|t| t["name"] == "preferences").unwrap();
    assert_eq!(prefs["stale_after_days"], 180);
    assert_eq!(prefs["aliases"], json!(["preference", "pref"]));
    assert_eq!(
        prefs["description"],
        "A standing rule the user wants agents to follow."
    );
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
    assert!(text(&fx.check(&["list"], Success).stderr).is_empty());

    let shown = show(&fx, "preferences");
    assert_eq!(shown["stale_after_days"], 180);
    assert_eq!(shown["aliases"], json!(["preference", "pref"]));
    assert_eq!(shown["capture"]["triggers"], json!(SHIPPED_TRIGGERS));
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not", "max_open"] {
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
    assert_eq!(
        shown["description"],
        "A standing rule the user wants agents to follow."
    );
    assert_eq!(shown["capture"]["triggers"], json!(SHIPPED_TRIGGERS));
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not", "max_open"] {
        assert_eq!(shown["capture"]["origin"][field], shipped, "{field}");
    }

    config(
        &fx,
        "[tags.preferences]\nstale_after_days = 90\n\n[tags.preferences.capture]\ncriteria = \"A standing rule across projects.\"\n",
    );
    let updated = show(&fx, "preferences");
    assert_eq!(updated["stale_after_days"], 90);
    assert_eq!(updated["aliases"], json!(["preference", "pref"]));
    assert_eq!(
        updated["description"],
        "A standing rule the user wants agents to follow."
    );
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
    let list = fx.check(&["list", "--json"], Success);
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
    let list = fx.check(&["list"], Success);
    let stderr = text(&list.stderr);
    assert!(
        stderr.contains("shipped tag standard is claimed by config tag rituals"),
        "{stderr}"
    );
    assert_eq!(show(&fx, "standard")["tag"], "rituals");
    assert_eq!(show(&fx, "preference")["tag"], "preferences");

    // Two of his own entries still refuse the file.
    config(
        &fx,
        "[tags.rituals]\naliases = [\"pref\"]\n[tags.habits]\naliases = [\"pref\"]\n",
    );
    let refused = fx.check(&["list", "--json"], Code(1));
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
    assert!(text(&fx.check(&["list"], Success).stderr).is_empty());
    let tags = listed(&fx);
    let names: Vec<&str> = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
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
    let prefs = tags["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "preferences")
        .unwrap();
    assert_eq!(prefs["aliases"], json!(["preference", "pref"]));
    assert_eq!(prefs["stale_after_days"], 180);
    assert_eq!(
        prefs["description"],
        "A standing rule the user wants agents to follow."
    );
    assert_eq!(prefs["route"], "propose");
    let shown = show(&fx, "preferences");
    let shipped = json!({"source": "shipped", "file": null, "line": null});
    for field in ["from", "route", "triggers", "criteria", "not", "max_open"] {
        assert_eq!(shown["capture"]["origin"][field], shipped, "{field}");
    }
    assert_eq!(show(&fx, "pref")["tag"], "preferences");
    assert_eq!(show(&fx, "remind")["action"]["mode"], "free");
}
