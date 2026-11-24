use crate::control::{self, Switch};

#[test]
fn only_a_first_line_slash_scribe_is_a_command() {
    assert_eq!(control::parse("/scribe on"), Some(Switch::On));
    assert_eq!(control::parse("  /scribe OFF  \nmore"), Some(Switch::Off));
    assert_eq!(control::parse("/scribe status"), Some(Switch::Status));
    assert_eq!(control::parse("/scribe"), Some(Switch::Status));
    assert_eq!(control::parse("/scribe:scribe on"), Some(Switch::On));
    assert_eq!(
        control::parse("/scribe update now"),
        Some(Switch::Unknown(String::from("update now")))
    );
    for not in [
        "",
        "please /scribe on",
        "hello\n/scribe on",
        "/scribes on",
        "#scribe preferences: on",
    ] {
        assert_eq!(control::parse(not), None, "{not:?}");
    }
}

#[test]
fn link_commands_parse_with_a_target_and_an_optional_type() {
    use crate::control::{Switch, parse};
    use crate::session::{LinkType, SessionLink};
    let link = |kind, on| Switch::Link {
        link: SessionLink {
            kind,
            session: "s-1".parse().unwrap(),
        },
        on,
    };
    assert_eq!(
        parse("/scribe on --from s-1"),
        Some(link(LinkType::SpawnedFrom, true))
    );
    assert_eq!(
        parse("/scribe link s-1"),
        Some(link(LinkType::SpawnedFrom, false))
    );
    assert_eq!(
        parse("/scribe link s-1 --type continues"),
        Some(link(LinkType::Continues, false))
    );
    assert_eq!(
        parse("/scribe:scribe on --type continues --from s-1"),
        Some(link(LinkType::Continues, true))
    );
    for bad in [
        "/scribe link",
        "/scribe link a/b",
        "/scribe link s-1 s-2",
        "/scribe link s-1 --type related_to",
        "/scribe on --from",
        "/scribe on s-1",
    ] {
        assert!(matches!(parse(bad), Some(Switch::Unknown(_))), "{bad}");
    }
}

#[test]
fn group_add_names_a_slug_and_at_least_one_session() {
    let a = "11111111-1111-4111-8111-111111111111";
    let b = "22222222-2222-4222-8222-222222222222";
    assert_eq!(
        control::parse(&format!("/scribe GROUP Add water {a} {b}")),
        Some(Switch::GroupAdd {
            slug: "water".into(),
            sessions: vec![a.parse().unwrap(), b.parse().unwrap()],
        })
    );
    // Two words after `group` is still the slug form, even when it reads "add".
    assert_eq!(
        control::parse("/scribe group add"),
        Some(Switch::Group("add".into()))
    );
    // No session, or an id that is not one: the usage reply, never a write.
    for line in ["/scribe group add water", "/scribe group add water a/b"] {
        assert!(
            matches!(control::parse(line), Some(Switch::Unknown(_))),
            "{line}"
        );
    }
}
