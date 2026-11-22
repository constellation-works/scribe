use super::T0;
use crate::capture::{self, Parsed, Refusal};
use crate::consistency::Settle;
use crate::entry::{EntryId, Speaker};
use crate::proposal::ProposalId;
use crate::transcript::Message;

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
fn the_prompt_hash_is_lowercase_sha256_hex() {
    assert_eq!(
        capture::prompt_sha256("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn turn_message(uuid: &str, role: Speaker, minutes: i64, text: &str) -> Message {
    Message {
        uuid: uuid.to_owned(),
        role,
        at: T0 + time::Duration::minutes(minutes),
        blocks: vec![text.to_owned()],
    }
}

#[test]
fn an_action_window_holds_the_turns_since_the_last_scribe_line_then_the_prompt() {
    let prompt = "#scribe standard: name the unit in every size";
    let messages = [
        turn_message("u-0", Speaker::User, 0, "much earlier"),
        turn_message(
            "u-1",
            Speaker::User,
            1,
            "#scribe preferences: ship on fridays",
        ),
        turn_message("a-1", Speaker::Assistant, 2, "Recorded."),
        turn_message("u-2", Speaker::User, 3, "the size field has no unit"),
        turn_message("a-2", Speaker::Assistant, 4, "  "),
        turn_message("a-3", Speaker::Assistant, 5, "Bytes, then."),
    ];
    let at = T0 + time::Duration::minutes(6);
    let window = capture::action_window(&messages, &capture::prompt_sha256(prompt), prompt, at);
    let view: Vec<(Option<&str>, Speaker, &str)> = window
        .turns
        .iter()
        .map(|t| (t.message.as_deref(), t.role, t.text.as_str()))
        .collect();
    assert_eq!(
        view,
        [
            (Some("a-1"), Speaker::Assistant, "Recorded."),
            (Some("u-2"), Speaker::User, "the size field has no unit"),
            (Some("a-3"), Speaker::Assistant, "Bytes, then."),
            (None, Speaker::User, prompt),
        ],
        "after the last #scribe line, empty turns dropped, the prompt last"
    );
    assert_eq!(window.turns[3].at, at);
    assert!(!window.truncated);
    assert_eq!(window.omitted, 0);

    // A retried hook finds the prompt in the transcript: it and what
    // follows are not counted twice.
    let mut retried = messages.to_vec();
    retried.push(turn_message("u-3", Speaker::User, 6, prompt));
    retried.push(turn_message("a-4", Speaker::Assistant, 7, "later reply"));
    let again = capture::action_window(&retried, &capture::prompt_sha256(prompt), prompt, at);
    assert_eq!(again, window);
}

#[test]
fn an_action_window_over_the_cap_drops_its_oldest_turns_and_says_so() {
    let prompt = "#scribe remind: tidy the sizes";
    let messages: Vec<Message> = (0..60)
        .map(|n| {
            turn_message(
                &format!("m-{n}"),
                Speaker::Assistant,
                n,
                &format!("turn {n}"),
            )
        })
        .collect();
    let window = capture::action_window(&messages, &capture::prompt_sha256(prompt), prompt, T0);
    assert_eq!(window.turns.len(), capture::WINDOW_TURNS);
    assert!(window.truncated);
    assert_eq!(window.omitted, 61 - 40);
    assert_eq!(
        window.turns[0].message.as_deref(),
        Some("m-21"),
        "newest kept"
    );
    assert_eq!(window.turns.last().unwrap().text, prompt);

    // The character cap: each turn is cut to its own cap first, then the
    // oldest go until the total fits.
    let long = "x".repeat(capture::WINDOW_TURN_CHARS + 10);
    let messages: Vec<Message> = (0..5)
        .map(|n| turn_message(&format!("m-{n}"), Speaker::User, n, &long))
        .collect();
    let window = capture::action_window(&messages, &capture::prompt_sha256(prompt), prompt, T0);
    let chars: usize = window.turns.iter().map(|t| t.text.chars().count()).sum();
    assert!(chars <= capture::WINDOW_CHARS, "{chars}");
    assert_eq!(window.turns.len(), 3, "two cut turns and the prompt fit");
    assert!(window.turns[0].cut && !window.turns[2].cut);
    assert_eq!(window.omitted, 3);
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
