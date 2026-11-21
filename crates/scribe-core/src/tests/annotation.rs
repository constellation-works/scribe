use crate::annotation::{
    self, CheckRefusal, Checked, MarkProblem, MarkRecord, MarkState, TURN_CHARS, WINDOW_CHARS,
};
use crate::models::MarkAnswer;
use crate::tag::TagTable;
use crate::transcript::Message;
use crate::{Speaker, capture};
use std::time::Duration;
use time::macros::datetime;

fn message(uuid: &str, role: Speaker, text: &str) -> Message {
    Message {
        uuid: uuid.to_owned(),
        role,
        at: datetime!(2026-11-21 10:00 UTC),
        blocks: vec![text.to_owned()],
    }
}

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
        // The last is a line-start instruction (form 1-4), not a marker.
        let expected = usize::from(!text.starts_with("#scribe"));
        assert_eq!(annotation::marks(text).len(), expected, "{text}");
    }
    let global = annotation::marks("keep updates short #scribe/global");
    assert!(global[0].global, "{global:?}");
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
        "#scribe preferences: a form-2 line",
        "foo#scribe and #scribes",
    ] {
        assert!(annotation::marks(text).is_empty(), "{text}");
    }
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

#[test]
fn the_window_holds_eight_before_the_turn_and_its_reply_within_forty_thousand_characters() {
    let mut messages: Vec<Message> = (0..12)
        .map(|i| {
            let role = if i % 2 == 0 {
                Speaker::User
            } else {
                Speaker::Assistant
            };
            message(&format!("m-{i}"), role, &"x".repeat(6_000))
        })
        .collect();
    messages.push(message("marked", Speaker::User, &"y".repeat(9_000)));
    messages.push(message("reply", Speaker::Assistant, &"z".repeat(6_000)));
    messages.push(message("next", Speaker::User, "later"));
    let window = annotation::window(&messages, "marked");
    let total: usize = window.iter().map(|m| m.text.chars().count()).sum();
    assert!(total <= WINDOW_CHARS, "{total}");
    let marked = window.iter().find(|m| m.marked).unwrap();
    assert_eq!(marked.uuid, "marked");
    assert!(marked.text.chars().count() <= TURN_CHARS + 10);
    assert!(window.iter().any(|m| m.uuid == "reply"));
    assert!(!window.iter().any(|m| m.uuid == "next"));
    assert!(
        window
            .iter()
            .filter(|m| !m.marked && m.uuid != "reply")
            .count()
            <= 8
    );
    // Oldest dropped first: what is kept before the turn is the latest.
    assert!(window.iter().any(|m| m.uuid == "m-11"));
    assert!(!window.iter().any(|m| m.uuid == "m-0"));
    assert!(annotation::window(&messages, "absent").is_empty());
}

#[test]
fn a_call_is_bounded_at_thirty_seconds() {
    assert_eq!(annotation::budget(), Duration::from_secs(30));
    let marks = annotation::marks("short updates #scribe preferences");
    let request = annotation::request(&[&marks[0]], Vec::new(), &TagTable::defaults(), "m");
    assert_eq!(request.budget, Duration::from_secs(30));
    assert_eq!(request.marks[0].hint.as_deref(), Some("preferences"));
}

// --- checks after the answer (§14.3) --------------------------------------

const TYPED: &str = "I always want short status updates #scribe preferences";

fn entry_answer(quote: &str, tag: &str) -> MarkAnswer {
    MarkAnswer {
        marker: 1,
        kind: String::from("entry"),
        tag: tag.to_owned(),
        statement: String::from("Daniel wants short status updates."),
        subject: String::from("status updates"),
        quote: quote.to_owned(),
        message: String::from("u-1"),
        reason: String::new(),
    }
}

#[test]
fn a_verbatim_quote_of_his_own_words_checks_into_an_entry() {
    let tags = TagTable::defaults();
    let turn = message("u-1", Speaker::User, TYPED);
    let mark = &annotation::marks(TYPED)[0];
    let checked = annotation::check(
        mark,
        &entry_answer("I always want short status updates", "preferences"),
        &[&turn],
        &tags,
    )
    .unwrap();
    let Checked::Entry(entry) = checked else {
        panic!("{checked:?}")
    };
    assert_eq!(entry.quote, "I always want short status updates");
    assert_eq!(entry.message, "u-1");
    assert_eq!(entry.tag.as_str(), "preferences");
}

#[test]
fn an_answer_that_fails_a_check_is_refused_with_its_code() {
    let tags = TagTable::defaults();
    let turn = message("u-1", Speaker::User, TYPED);
    let mark = &annotation::marks(TYPED)[0];
    let cases = [
        (
            entry_answer("I never want long updates", "preferences"),
            CheckRefusal::NotVerbatim,
        ),
        (
            entry_answer("short", "preferences"),
            CheckRefusal::OutOfBounds,
        ),
        (
            entry_answer("I always want short status updates", "nope"),
            CheckRefusal::WrongTag,
        ),
        (
            MarkAnswer {
                kind: String::from("maybe"),
                ..MarkAnswer::default()
            },
            CheckRefusal::OutOfBounds,
        ),
    ];
    for (answer, refusal) in cases {
        assert_eq!(
            annotation::check(mark, &answer, &[&turn], &tags),
            Err(refusal.clone()),
            "{answer:?}"
        );
    }
    // A quote from a turn outside the typed ones is not his.
    let mut other = entry_answer("I always want short status updates", "preferences");
    other.message = String::from("a-1");
    assert_eq!(
        annotation::check(mark, &other, &[&turn], &tags),
        Err(CheckRefusal::NotVerbatim)
    );
    assert_eq!(CheckRefusal::NotVerbatim.code(), "quote_not_verbatim");
}

#[test]
fn a_none_answer_keeps_its_reason() {
    let tags = TagTable::defaults();
    let turn = message("u-1", Speaker::User, TYPED);
    let mark = &annotation::marks(TYPED)[0];
    let none = MarkAnswer {
        marker: 1,
        kind: String::from("none"),
        reason: String::from("only a passing remark"),
        ..MarkAnswer::default()
    };
    assert_eq!(
        annotation::check(mark, &none, &[&turn], &tags),
        Ok(Checked::None(String::from("only a passing remark")))
    );
}

#[test]
fn a_draft_is_daniels_with_scribes_statement_and_his_quote() {
    let tags = TagTable::defaults();
    let turn_message = message("u-1", Speaker::User, TYPED);
    let mark = &annotation::marks(TYPED)[0];
    let Ok(Checked::Entry(annotated)) = annotation::check(
        mark,
        &entry_answer("I always want short status updates", "preferences"),
        &[&turn_message],
        &tags,
    ) else {
        panic!("checks")
    };
    let turn = capture::Turn {
        session: "0b8ef4a6-0c6a-4d77-9c41-6a4f7b2c1d10".parse().unwrap(),
        at: turn_message.at,
        prompt_sha256: capture::prompt_sha256(TYPED),
        cwd: String::from("/work/scribe"),
        project: crate::Project::sanitized("scribe"),
        provider: crate::Label::sanitized("claude-code", "claude-code"),
        host: crate::Label::sanitized("mac", "mac"),
    };
    let draft = annotation::draft(mark, annotated, &turn, "u-1", &tags);
    assert_eq!(draft.content, "Daniel wants short status updates.");
    assert_eq!(
        draft.quote.as_deref(),
        Some("I always want short status updates")
    );
    assert_eq!(draft.stated_by, Some(crate::Author::annotation()));
    assert_eq!(draft.said, TYPED);
    assert_eq!(draft.cite.message.as_deref(), Some("u-1"));
    assert_eq!(draft.cite.line, 1);
    let entry = draft.into_entry("preferences-1".parse().unwrap());
    assert_eq!(entry.author, crate::Author::daniel());
    assert!(entry.approved_by.is_none());
}

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
