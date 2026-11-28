use crate::annotation;

#[test]
fn a_line_start_capture_form_is_a_marker_with_its_tag_as_the_hint() {
    // Forms 1-3 folded into the marker (2_design.md §14.6).
    let tags = crate::tests::catchers::tags_with_idea();
    let hint = |text: &str| {
        let marks = annotation::marks(text);
        assert_eq!(marks.len(), 1, "{text}: {marks:?}");
        assert!(marks[0].problem.is_none(), "{text}: {marks:?}");
        marks[0].hint(&tags).map(ToString::to_string)
    };
    assert_eq!(
        hint("#scribe idea: a dry-run flag for prune").as_deref(),
        Some("idea")
    );
    assert_eq!(
        hint("#scribe add this to preferences").as_deref(),
        Some("preferences")
    );
    assert_eq!(
        hint("#scribe remember short subjects under my preferences. #git").as_deref(),
        Some("preferences")
    );
    assert_eq!(
        hint("#scribe pref: tabs #style").as_deref(),
        Some("preferences"),
        "an alias resolves"
    );
    assert_eq!(hint("#scribe keep updates short"), None);
    assert_eq!(
        hint("#scribe issue: the drain hits 429s"),
        None,
        "the issue tag is never a hint"
    );
    assert_eq!(hint("#scribe add the 429s to issue"), None);
    let global = annotation::marks("#scribe/global preferences: squash-merge agent PRs #ci");
    assert!(global[0].global, "{global:?}");
    assert_eq!(global[0].topics.as_slice()[0].as_str(), "ci");
    assert_eq!(
        global[0].hint(&tags).map(ToString::to_string).as_deref(),
        Some("preferences")
    );
    // A tail is read only on a line the marker opens.
    let inline = annotation::marks("ok, #scribe add this to preferences");
    assert_eq!(inline[0].tail, None);
}
