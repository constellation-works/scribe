//! The trigger matcher (2_design.md §11.2): case, word boundaries, `*`,
//! sentence ends and phrase checks. It takes and returns plain values; no
//! I/O is reachable from it.

use crate::text::trigger::{self, Fired, MATCHED_CHARS, TriggerError};

fn find(phrase: &str, text: &str) -> Option<String> {
    trigger::find(phrase, text)
}

#[test]
fn matching_ignores_case_and_returns_the_words_verbatim() {
    assert_eq!(
        find("I prefer", "Well, i PREFER tabs").as_deref(),
        Some("i PREFER")
    );
    assert_eq!(
        find("from now on", "From Now On, use it").as_deref(),
        Some("From Now On")
    );
    assert_eq!(find("always", "Nothing here"), None);
}

#[test]
fn matching_is_on_word_boundaries() {
    assert_eq!(find("never", "nevertheless it works"), None);
    assert_eq!(find("never", "it's whenever"), None);
    assert_eq!(find("always", "(always)").as_deref(), Some("always"));
    assert_eq!(
        find("by default", "by-default it is").as_deref(),
        Some("by-default")
    );
    // An apostrophe stays inside a word, curly or straight.
    assert_eq!(
        find("here's an idea", "Here’s an idea:").as_deref(),
        Some("Here’s an idea")
    );
    assert_eq!(find("here", "here's"), None);
}

#[test]
fn a_wildcard_spans_zero_to_four_words_and_no_more() {
    let phrase = "have * an idea";
    assert_eq!(
        find(phrase, "I have an idea").as_deref(),
        Some("have an idea")
    );
    assert_eq!(
        find(phrase, "I have got an idea").as_deref(),
        Some("have got an idea")
    );
    assert_eq!(
        find(phrase, "I have one two three four an idea").as_deref(),
        Some("have one two three four an idea")
    );
    assert_eq!(find(phrase, "I have one two three four five an idea"), None);
}

#[test]
fn a_match_never_crosses_a_sentence_end() {
    for text in [
        "I have. An idea",
        "I have! an idea",
        "I have? an idea",
        "I have\nan idea",
        "I have\r\nan idea",
        "I have\0an idea",
    ] {
        assert_eq!(find("have * an idea", text), None, "{text:?}");
        assert_eq!(find("have an idea", text), None, "{text:?}");
    }
    assert_eq!(find("from now on", "From now. On the other hand"), None);
    // A later sentence can still match.
    assert_eq!(
        find("never", "Fine. Never again.").as_deref(),
        Some("Never")
    );
}

#[test]
fn bad_phrases_are_refused() {
    assert!(trigger::check("what if we").is_ok());
    assert!(trigger::check("have * an idea").is_ok());
    assert!(matches!(trigger::check("   "), Err(TriggerError::Empty(_))));
    assert!(matches!(
        trigger::check("* idea"),
        Err(TriggerError::Wildcard(_))
    ));
    assert!(matches!(
        trigger::check("idea *"),
        Err(TriggerError::Wildcard(_))
    ));
    assert!(matches!(
        trigger::check("a * * b"),
        Err(TriggerError::Wildcard(_))
    ));
    assert!(matches!(
        trigger::check("a *b"),
        Err(TriggerError::Shape(_))
    ));
    assert!(matches!(
        trigger::check("stop. now"),
        Err(TriggerError::Shape(_))
    ));
    assert!(matches!(
        trigger::check(&"a".repeat(121)),
        Err(TriggerError::TooLong(_))
    ));
    assert_eq!(
        find("* idea", "an idea"),
        None,
        "an invalid phrase never fires"
    );
}

#[test]
fn shape_names_every_refused_character_and_punctuation_separates_words() {
    for phrase in [
        "stop. now",
        "stop! now",
        "stop? now",
        "a\nb",
        "a\rb",
        "a\0b",
    ] {
        let err = trigger::check(phrase).unwrap_err();
        let text = err.to_string();
        assert!(matches!(err, TriggerError::Shape(_)), "{phrase:?}: {err:?}");
        for needle in ["'.'", "'!'", "'?'", "line break", "NUL"] {
            assert!(text.contains(needle), "{phrase:?}: {text}");
        }
    }
    assert_eq!(find("always", "always_on").as_deref(), Some("always"));
    assert_eq!(
        find("from now on", "from now, on we go").as_deref(),
        Some("from now, on")
    );
    // Punctuation inside the configured trigger phrase itself is also a word separator.
    for phrase in ["by-default", "always_on", "from now, on", "(always)"] {
        assert!(trigger::check(phrase).is_ok(), "{phrase:?}");
    }
    assert_eq!(
        find("by-default", "use this by default").as_deref(),
        Some("by default")
    );
    assert_eq!(
        find("always_on", "keep it always on").as_deref(),
        Some("always on")
    );
    assert_eq!(
        find("from now, on", "from now on we go").as_deref(),
        Some("from now on")
    );
    assert_eq!(find("(always)", "always check").as_deref(), Some("always"));
}

#[test]
fn the_first_trigger_in_list_order_fires_and_matched_is_capped() {
    let triggers = vec![String::from("never"), String::from("always")];
    assert_eq!(
        trigger::first_fired(&triggers, "always and never"),
        Some(Fired {
            trigger: String::from("never"),
            matched: String::from("never"),
        })
    );
    assert_eq!(trigger::first_fired(&triggers, "sometimes"), None);
    let long = format!("start {} end", "word ".repeat(40));
    let matched = find("start * end", &long);
    assert_eq!(matched, None, "forty words exceed one wildcard");
    let wide = format!("start {} end", "w".repeat(200));
    let matched = find("start * end", &wide).unwrap();
    assert_eq!(matched.chars().count(), MATCHED_CHARS);
    assert!(wide.starts_with(&matched));
}
