use crate::markers::Settle;
use crate::markers::{self as capture, Parsed, Refusal};
use crate::{markers as annotation, markers::MarkProblem};
use scribe_domain::entry::EntryId;
use scribe_domain::proposal::ProposalId;
use scribe_domain::tag::TagTable;

/// `(line, code)` of each settling line of `prompt`: `settle`,
/// or the refusal's code.
fn parse(prompt: &str) -> Vec<(u32, &'static str)> {
    capture::scan(prompt)
        .into_iter()
        .map(|p| match p {
            Parsed::Settle(s) => (s.line, "settle"),
            Parsed::Refused(r) => (r.line, r.reason.code()),
        })
        .collect()
}

#[test]
fn a_capture_line_is_a_marker_not_a_command() {
    // Forms 1-3 folded into the marker (2_design.md §14.6): the prompt
    // hook parses none of them, and `annotation::marks` has each.
    for prompt in [
        "#scribe idea: a dry-run flag for prune",
        "#scribe preferences: squash-merge agent PRs #ci",
        "#scribe add short commit subjects to my preferences",
        "#scribe add this to preferences",
        "#scribe/global preferences: squash-merge agent PRs",
        "#scribe rituals: stand-up at nine",
        "#scribe",
    ] {
        assert!(parse(prompt).is_empty(), "{prompt}");
        assert!(!capture::settles(prompt), "{prompt}");
        assert_eq!(capture::lines(prompt).len(), 1, "{prompt}");
    }
    assert!(capture::settles("#scribe approve p-4"));
    assert!(capture::settles("#scribe forget preferences"));
}

#[test]
fn each_settling_line_is_numbered_within_the_prompt() {
    let prompt = "first a question\n#scribe idea: one\n  #scribe approve p-2\r\nthe end";
    assert_eq!(parse(prompt), [(3, "settle")]);
}

#[test]
fn lines_in_fences_blockquotes_and_pasted_content_are_not_instructions() {
    let prompt = "\
```
#scribe approve p-1
```
~~~~
#scribe approve p-2
```
#scribe approve p-3
~~~~
> #scribe approve p-4
<pasted_content id=\"1\">
#scribe approve p-5
</pasted_content>
<pasted_content>#scribe approve p-6</pasted_content>
#scribe approve p-7";
    assert_eq!(parse(prompt), [(14, "settle")]);
}

#[test]
fn bare_hashtags_and_lookalikes_are_not_instructions() {
    for prompt in [
        "#preference squash-merge",
        "#preferences: squash-merge",
        "#preferences tabs",
        "#scribes preferences: no",
        "text then #scribe approve p-4",
    ] {
        assert!(capture::lines(prompt).is_empty(), "{prompt}");
    }
}

/// The settling action of each `#scribe` line of `prompt`.
fn settles(prompt: &str) -> Vec<Settle> {
    capture::scan(prompt)
        .into_iter()
        .filter_map(|p| match p {
            Parsed::Settle(s) => Some(s.action),
            Parsed::Refused(_) => None,
        })
        .collect()
}

fn proposal(id: &str) -> ProposalId {
    ProposalId::try_from(id.to_owned()).unwrap()
}

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_owned()).unwrap()
}

#[test]
fn form_four_takes_a_settling_verb_and_one_id() {
    assert_eq!(
        settles(
            "#scribe approve p-4\n#scribe reject p-12.\n#scribe retire preferences-2\n#scribe forget rituals-7! #tidy"
        ),
        [
            Settle::Approve(proposal("p-4")),
            Settle::Reject(proposal("p-12")),
            Settle::Retire(entry("preferences-2")),
            Settle::Forget(entry("rituals-7")),
        ]
    );
}

#[test]
fn a_settling_verb_without_its_kind_of_id_is_refused() {
    for prompt in [
        "#scribe approve preferences-2",
        "#scribe reject",
        "#scribe forget preferences",
        "#scribe retire preferences-2 preferences-3",
        "#scribe approve p-4 and p-5",
    ] {
        assert_eq!(parse(prompt), [(1, "unparsed_instruction")], "{prompt}");
    }
    let refused = capture::scan("#scribe approve preferences-2");
    let [Parsed::Refused(refused)] = refused.as_slice() else {
        panic!("{refused:?}");
    };
    assert!(
        refused.message().contains("approve takes"),
        "{}",
        refused.message()
    );
}

#[test]
fn the_global_opener_on_a_settling_line_is_refused_and_teaches_the_opener() {
    let settle = capture::scan("#scribe/global approve p-4");
    let [Parsed::Refused(settle)] = settle.as_slice() else {
        panic!("a marker does not settle: {settle:?}");
    };
    assert_eq!(settle.reason, Refusal::MarkerNotOnEntry);
    assert!(settle.message().contains(capture::GLOBAL_OPENER));
}

#[test]
fn forget_issue_n_names_the_entry_an_issue_became_and_an_occurrence_is_refused() {
    // Issue records are `issue` entries since 2_design.md §15.9 phase 2,
    // so `issue-3` is an entry id and `#<n>` an occurrence no longer.
    assert_eq!(
        settles("#scribe forget issue-3"),
        [Settle::Forget(
            EntryId::try_from(String::from("issue-3")).unwrap()
        )]
    );
    assert_eq!(
        parse("#scribe forget issue-3#2\n#scribe retire issue-3#2"),
        [(1, "unparsed_instruction"), (2, "unparsed_instruction")]
    );
}

#[test]
fn forget_takes_a_printed_session_prefix_before_an_entry_id() {
    let session = |id: &str| Settle::ForgetSession(id.parse().unwrap());
    // The first group of a UUID, as a table prints it, with a trailing stop.
    assert_eq!(settles("#scribe forget 7c2b9d14."), [session("7c2b9d14")]);
    assert_eq!(settles("#scribe forget 7c2b"), [session("7c2b")]);
    // A longer prefix that ends in digits would read as an entry id.
    for prefix in [
        "7c2b9d14-1234",
        "7c2b9d14-8e5f",
        "7c2b9d14-8e5f-4a3b-1234",
        "7c2b9d14-8e5f-4a3b-b1c6-2026",
    ] {
        assert_eq!(
            settles(&format!("#scribe forget {prefix}")),
            [session(prefix)]
        );
    }
    // Entry ids keep their meaning, and retire never takes a session.
    assert_eq!(
        settles("#scribe forget preferences-12"),
        [Settle::Forget(
            EntryId::try_from(String::from("preferences-12")).unwrap()
        )]
    );
    assert_eq!(
        settles("#scribe forget beef-12"),
        [Settle::Forget(
            EntryId::try_from(String::from("beef-12")).unwrap()
        )]
    );
    // Too short, not hex, or not shaped like a UUID cut short.
    assert_eq!(
        parse(
            "#scribe forget 7c2\n#scribe forget zzzzzzzz\n#scribe forget 7c2b9d14-\n#scribe forget 7c2b9d14-8e5f1\n#scribe retire 7c2b9d14"
        ),
        [
            (1, "unparsed_instruction"),
            (2, "unparsed_instruction"),
            (3, "unparsed_instruction"),
            (4, "unparsed_instruction"),
            (5, "unparsed_instruction"),
        ]
    );
}

#[test]
fn forget_takes_a_session_as_s_n_or_its_uuid_before_an_entry_id() {
    let session = |id: &str| Settle::ForgetSession(id.parse().unwrap());
    assert_eq!(settles("#scribe forget s-12"), [session("s-12")]);
    // A UUID whose last group is all digits would read as an entry id.
    let uuid = "7c2b9d14-8e5f-4a3b-b1c6-202611220930";
    assert_eq!(settles(&format!("#scribe forget {uuid}.")), [session(uuid)]);
    assert_eq!(
        settles("#scribe forget preferences-4"),
        [Settle::Forget(
            EntryId::try_from(String::from("preferences-4")).unwrap()
        )]
    );
    // Retire takes entries only, and `s-012` does not round-trip.
    assert_eq!(
        parse("#scribe retire 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02\n#scribe forget s-012"),
        [(1, "unparsed_instruction"), (2, "unparsed_instruction")]
    );
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
