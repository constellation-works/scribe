#[test]
fn a_context_stands_alone_from_two_sentences_up_to_600_characters() {
    assert!(crate::context_stands_alone(
        "The user and the agent chose a store. The user asked for a log."
    ));
    assert!(crate::context_stands_alone(
        "They weighed it (twice). The user said \"keep the log.\""
    ));
    for line in [
        "",
        "The user prefers a log.",
        "The user prefers a log, e.g. one per session",
        "v1.2 is out and the user prefers it",
    ] {
        assert!(!crate::context_stands_alone(line), "{line}");
    }
    let long = format!("{}. {}.", "a".repeat(299), "b".repeat(299));
    assert_eq!(long.chars().count(), 601);
    assert!(!crate::context_stands_alone(&long));
}
