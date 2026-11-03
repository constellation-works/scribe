use super::T0;
use crate::capture::{self, Parsed, Refusal, Turn};
use crate::entry::{ContentFrom, TagName};
use crate::tag::TagTable;

/// `(line, tag, content, topics)` of a recorded instruction, or `(line, code)`
/// of a refusal.
type Outcome = Result<(u32, String, String, Vec<String>), (u32, &'static str)>;

fn tags() -> TagTable {
    TagTable::defaults()
}

/// Every `#scribe` line of `prompt`, parsed.
fn parse(prompt: &str) -> Vec<Outcome> {
    capture::scan(prompt, &tags())
        .into_iter()
        .map(|p| match p {
            Parsed::Instruction(i) => {
                assert!(
                    i.said.contains(&i.content),
                    "content is a substring of said"
                );
                Ok((
                    i.line,
                    i.tag.to_string(),
                    i.content,
                    i.topics
                        .as_slice()
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                ))
            }
            Parsed::Refused(r) => Err((r.line, r.reason.code())),
        })
        .collect()
}

fn ok(line: u32, tag: &str, content: &str, topics: &[&str]) -> Outcome {
    Ok((
        line,
        tag.to_owned(),
        content.to_owned(),
        topics.iter().map(|t| (*t).to_owned()).collect(),
    ))
}

#[test]
fn form_one_takes_the_tag_before_the_colon_and_the_content_after() {
    assert_eq!(
        parse("#scribe decisions: squash-merge agent PRs; keep merge commits for releases"),
        [ok(
            1,
            "decisions",
            "squash-merge agent PRs; keep merge commits for releases",
            &[]
        )]
    );
    // Aliases resolve case-insensitively to the tag's name.
    assert_eq!(
        parse("#scribe Pref: short commit subjects"),
        [ok(1, "preferences", "short commit subjects", &[])]
    );
}

#[test]
fn form_two_takes_the_tag_from_the_last_word_group() {
    assert_eq!(
        parse("#scribe add squash-merge to agent PRs to my decisions"),
        [ok(1, "decisions", "squash-merge to agent PRs", &[])]
    );
    assert_eq!(
        parse("#scribe record tabs over spaces under the preference"),
        [ok(1, "preferences", "tabs over spaces", &[])]
    );
    assert_eq!(
        parse("#scribe ship on fridays in decision."),
        [ok(1, "decisions", "ship on fridays", &[])],
        "the verb is optional and a trailing full stop is not part of the tag"
    );
}

#[test]
fn trailing_hashtags_are_topics_and_not_content() {
    assert_eq!(
        parse("#scribe decisions: run the gate #ci #Release"),
        [ok(1, "decisions", "run the gate", &["ci", "release"])]
    );
    assert_eq!(
        parse("#scribe add run the gate to decisions #ci #ci"),
        [ok(1, "decisions", "run the gate", &["ci"])]
    );
    assert_eq!(
        parse("#scribe decisions: x y #a #b #c #d #e #f"),
        [Err((1, "unparsed_instruction"))]
    );
}

#[test]
fn each_line_is_one_instruction_numbered_within_the_prompt() {
    let prompt =
        "first a question\n#scribe decisions: one\n  #scribe add two to preferences\r\nthe end";
    assert_eq!(
        parse(prompt),
        [
            ok(2, "decisions", "one", &[]),
            ok(3, "preferences", "two", &[])
        ]
    );
}

#[test]
fn lines_in_fences_blockquotes_and_pasted_content_are_not_instructions() {
    let prompt = "\
```
#scribe decisions: fenced
```
~~~~
#scribe decisions: tilde fenced
```
#scribe decisions: still fenced, the closer is too short
~~~~
> #scribe decisions: quoted
<pasted_content id=\"1\">
#scribe decisions: pasted
</pasted_content>
<pasted_content>#scribe decisions: one-line paste</pasted_content>
#scribe decisions: real";
    assert_eq!(parse(prompt), [ok(14, "decisions", "real", &[])]);
}

#[test]
fn bare_hashtags_and_lookalikes_are_not_instructions() {
    for prompt in [
        "#decision squash-merge",
        "#decisions: squash-merge",
        "#preferences tabs",
        "#scribes decisions: no",
        "text then #scribe decisions: mid-line",
    ] {
        assert!(capture::lines(prompt).is_empty(), "{prompt}");
    }
}

#[test]
fn an_unknown_tag_is_refused_with_every_configured_tag() {
    let parsed = capture::scan("#scribe add it to rituals", &tags());
    let Parsed::Refused(refused) = &parsed[0] else {
        panic!("expected a refusal: {parsed:?}");
    };
    assert_eq!(refused.said, "#scribe add it to rituals");
    assert_eq!(
        refused.reason,
        Refusal::UnknownTag {
            word: String::from("rituals"),
            tags: vec![
                TagName::try_from(String::from("decisions")).unwrap(),
                TagName::try_from(String::from("preferences")).unwrap()
            ],
        }
    );
    assert_eq!(
        refused.reason.to_string(),
        "no tag \"rituals\"; tags: decisions, preferences"
    );
    assert_eq!(
        parse("#scribe rituals: stand-up at nine"),
        [Err((1, "unknown_tag"))]
    );
}

#[test]
fn lines_matching_no_form_are_refused() {
    for prompt in [
        "#scribe",
        "#scribe remember for decisions that tabs win",
        "#scribe decisions:",
        "#scribe add to decisions",
        "#scribe #ci",
    ] {
        assert_eq!(
            parse(prompt),
            [Err((1, "unparsed_instruction"))],
            "{prompt}"
        );
    }
}

#[test]
fn referents_and_settling_verbs_are_recognised_but_not_supported_yet() {
    for prompt in [
        "#scribe add this to decisions",
        "#scribe record it in my preferences",
        "#scribe that to decisions",
        "#scribe approve p-4",
        "#scribe forget decisions-2",
        "#scribe distill this discussion",
    ] {
        assert_eq!(
            parse(prompt),
            [Err((1, "unsupported_instruction"))],
            "{prompt}"
        );
    }
}

#[test]
fn the_prompt_hash_is_lowercase_sha256_hex() {
    assert_eq!(
        capture::prompt_sha256("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_draft_carries_the_turn_and_the_tags_review_date() {
    let parsed = capture::scan("#scribe pref: tabs #style", &tags());
    let Parsed::Instruction(instruction) = &parsed[0] else {
        panic!("expected an instruction");
    };
    let turn = Turn {
        session: "s-1".parse().unwrap(),
        at: T0,
        prompt_sha256: capture::prompt_sha256("#scribe pref: tabs #style"),
        cwd: String::from("/work"),
        project: "scribe".parse().unwrap(),
        provider: String::from("claude-code").try_into().unwrap(),
        host: String::from("host").try_into().unwrap(),
    };
    let draft = capture::draft(instruction, &turn, &tags());
    assert_eq!(draft.content, "tabs");
    assert_eq!(draft.said, "#scribe pref: tabs #style");
    assert_eq!(draft.content_from, ContentFrom::Instruction);
    assert_eq!(draft.cite.message, None);
    assert_eq!(draft.cite.line, 1);
    assert_eq!(draft.cite.prompt_sha256, turn.prompt_sha256);
    // preferences go stale after 180 days by default.
    assert_eq!(
        draft.review_after,
        Some(time::macros::date!(2027 - 04 - 30))
    );
    let entry = draft.into_entry("preferences-3".parse().unwrap());
    assert_eq!(entry.updated, entry.created);
    assert_eq!(entry.status, crate::entry::EntryStatus::Active);
}
