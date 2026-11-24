use crate::entry::Speaker;
use crate::models::PassMessage;
use crate::pass;

fn said(uuid: &str, role: Speaker, text: &str) -> PassMessage {
    PassMessage {
        uuid: uuid.to_owned(),
        role,
        text: text.to_owned(),
        pasted: Vec::new(),
    }
}

#[test]
fn only_daniels_own_lines_that_fire_a_trigger_are_named() {
    let tags = crate::tests::tags_with_idea();
    let messages = vec![
        said("m-1", Speaker::User, "Let's look at the build."),
        said("m-2", Speaker::Assistant, "I always run the tests first."),
        said(
            "m-3",
            Speaker::User,
            "> you should never do that\n```\nalways\n```\n    never\n#scribe always",
        ),
        said("m-4", Speaker::User, "From now on, use tabs."),
        said("m-5", Speaker::User, "What if we cache it?"),
    ];
    let (turns, rules) = pass::capture_request(&tags, &messages);
    let named: Vec<(&str, &str, &str, &str)> = turns
        .iter()
        .map(|t| {
            (
                t.message.as_str(),
                t.tag.as_str(),
                t.trigger.as_str(),
                t.matched.as_str(),
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            ("m-4", "preferences", "from now on", "From now on"),
            ("m-5", "idea", "what if we", "What if we"),
        ]
    );
    assert_eq!(rules.len(), 2);
    assert!(
        rules
            .iter()
            .all(|r| r.criteria.is_some() && r.not.is_some())
    );

    let (none, no_rules) = pass::capture_request(&tags, &messages[..3]);
    assert!(none.is_empty() && no_rules.is_empty());
}

#[test]
fn admission_recomputes_the_trigger_for_the_claimed_tag_and_speaker() {
    use crate::entry::Speaker;
    let tags = crate::tag::TagTable::defaults();
    let preferences: crate::entry::TagName = "preferences".parse().unwrap();
    let idea: crate::entry::TagName = "idea".parse().unwrap();
    let admitted =
        pass::admission(&tags, &preferences, Speaker::User, "I prefer short names.").unwrap();
    assert_eq!(admitted.trigger, "I prefer");
    assert_eq!(admitted.matched, "I prefer");
    assert_eq!(
        admitted.criteria,
        tags.get(&preferences)
            .unwrap()
            .capture
            .criteria
            .as_ref()
            // Stored and shown, so `{user}` reads "the user".
            .map(|c| crate::voice::UserName::default().fill(&c.value))
    );
    // `preferences` reads the user's turns only (`from = "user"`).
    assert!(
        pass::admission(
            &tags,
            &preferences,
            Speaker::Assistant,
            "I prefer short names."
        )
        .is_none()
    );
    assert!(pass::admission(&tags, &idea, Speaker::User, "I prefer short names.").is_none());
    assert!(
        pass::admission(&tags, &"standard".parse().unwrap(), Speaker::User, "always").is_none()
    );
}
