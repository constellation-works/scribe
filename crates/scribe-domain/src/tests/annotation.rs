use crate::annotation::{self, MarkProblem, MarkRecord, MarkState};
use crate::tag::TagTable;
use time::macros::datetime;

// --- marker detection (§14.1) ---------------------------------------------

#[test]
fn a_marker_inside_a_sentence_is_found_with_its_line_and_hint() {
    let marks = annotation::marks("Context first.\nWe squash agent PRs #scribe preferences #merge");
    assert_eq!(marks.len(), 1);
    let mark = &marks[0];
    assert_eq!((mark.number, mark.line), (1, 2));
    assert_eq!(mark.said, "We squash agent PRs #scribe preferences #merge");
    assert_eq!(mark.word.as_deref(), Some("preferences"));
    assert_eq!(mark.topics.as_slice()[0].as_str(), "merge");
    assert!(!mark.global && mark.problem.is_none());
    let tags = TagTable::defaults();
    assert_eq!(
        mark.hint(&tags).map(ToString::to_string).as_deref(),
        Some("preferences")
    );
}

#[test]
fn a_marker_at_the_end_or_before_a_colon_or_bracketed_is_a_marker() {
    for text in [
        "keep updates short #scribe",
        "keep updates short #scribe: please",
        "keep updates short (#scribe)",
        "keep updates short #scribe.",
        "keep updates short (#scribe preferences), always",
        "#scribe/global keep updates short",
    ] {
        assert_eq!(annotation::marks(text).len(), 1, "{text}");
    }
    let global = annotation::marks("keep updates short #scribe/global");
    assert!(global[0].global, "{global:?}");
}

#[test]
fn a_global_opener_takes_sentence_punctuation() {
    for text in [
        "keep tabs on this #scribe/global.",
        "#scribe/global, idea: x",
        "(keep tabs on this #scribe/global)",
        "keep tabs on this #scribe/global!",
    ] {
        let marks = annotation::marks(text);
        assert_eq!(marks.len(), 1, "{text}");
        assert!(marks[0].global, "{text}: {marks:?}");
        assert_eq!(marks[0].problem, None, "{text}");
    }
    let bad = annotation::marks("keep tabs #scribe/globals.");
    assert_eq!(bad[0].problem, Some(MarkProblem::BadOpener), "{bad:?}");
}

#[test]
fn a_stray_quote_or_backtick_does_not_hide_a_marker_after_it() {
    for text in [
        "a 12\" pipe, remember #scribe idea",
        "a “stray opener, remember #scribe idea",
        "a stray ` tick, remember #scribe idea",
        "\"quoted\" then a 12\" pipe #scribe",
    ] {
        assert_eq!(annotation::marks(text).len(), 1, "{text}");
    }
    // A paired span still hides its mention, before or after the stray.
    assert!(annotation::marks("a 12\" pipe and `#scribe` in code").is_empty());
    assert!(annotation::marks("he wrote \"#scribe\" there, 12\" pipe").is_empty());
}

#[test]
fn a_mention_writes_nothing() {
    for text in [
        "the `#scribe` marker is neat",
        "he wrote \"#scribe\" there",
        "he wrote “#scribe” there",
        "```\nsome #scribe here\n```",
        "> quoted #scribe here",
        "<pasted_content>\npasted #scribe here\n</pasted_content>",
        "    indented #scribe code",
        "#scribe approve p-4",
        "#scribe forget preferences-2",
        "foo#scribe and #scribes",
    ] {
        assert!(annotation::marks(text).is_empty(), "{text}");
    }
}

#[test]
fn a_line_start_capture_form_is_a_marker_with_its_tag_as_the_hint() {
    // Forms 1-3 folded into the marker (2_design.md §14.6).
    let tags = crate::tests::tags_with_idea();
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

#[test]
fn a_settling_verb_a_bad_opener_and_a_fourth_marker_are_refused_before_any_call() {
    let settle = annotation::marks("ok then #scribe approve p-1");
    assert_eq!(settle[0].problem, Some(MarkProblem::Settling));
    assert_eq!(MarkProblem::Settling.code(), "unparsed_instruction");
    let opener = annotation::marks("ok then #scribe/team do it");
    assert_eq!(opener[0].problem, Some(MarkProblem::BadOpener));
    let four = annotation::marks("a #scribe b #scribe c #scribe d #scribe");
    assert_eq!(four.len(), 4);
    assert!(four[..3].iter().all(|m| m.problem.is_none()));
    assert_eq!(four[3].problem, Some(MarkProblem::TooMany));
    assert_eq!(MarkProblem::TooMany.code(), "too_many_marks");
}

// --- the window and the call (§14.2, §14.3) -------------------------------

// --- checks after the answer (§14.3) --------------------------------------

// --- marks.json records ---------------------------------------------------

#[test]
fn a_mark_record_is_consistent_only_with_the_fields_its_outcome_has() {
    let record = |outcome, entry: Option<&str>, code: Option<&str>| MarkRecord {
        session: "0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d10".parse().unwrap(),
        message: String::from("u-1"),
        prompt_sha256: "0".repeat(64),
        marker: 1,
        outcome,
        entry: entry.map(|e| e.parse().unwrap()),
        code: code.map(str::to_owned),
        attempts: 1,
        at: datetime!(2026-11-21 10:00 UTC),
    };
    assert!(record(MarkState::Entry, Some("preferences-1"), None).is_consistent());
    assert!(record(MarkState::None, None, None).is_consistent());
    assert!(record(MarkState::Refused, None, Some("model_failed")).is_consistent());
    assert!(record(MarkState::Pending, None, None).is_consistent());
    assert!(!record(MarkState::Entry, None, None).is_consistent());
    assert!(!record(MarkState::Refused, None, None).is_consistent());
    assert!(!record(MarkState::None, Some("preferences-1"), None).is_consistent());
    assert!(MarkState::None.is_final() && !MarkState::Pending.is_final());
}
