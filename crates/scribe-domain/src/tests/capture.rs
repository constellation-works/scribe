use crate::capture::{self, Parsed, Refusal};
use crate::consistency::Settle;
use crate::entry::EntryId;
use crate::proposal::ProposalId;

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
