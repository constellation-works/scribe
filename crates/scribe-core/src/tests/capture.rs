use super::T0;
use crate::capture::{self, Guess, Parsed, Referent, Refusal, Turn};
use crate::entry::{ContentFrom, Speaker, TagName};
use crate::models::{ModelError, Models, REFERENT_BUDGET, ReferentAnswer, ReferentRequest};
use crate::tag::TagTable;
use crate::transcript::Message;
use std::cell::RefCell;
use std::time::Duration;

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
            Parsed::Referent(r) => Err((r.line, "referent")),
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
fn referent_lines_are_form_three_with_their_tag_and_topics() {
    for prompt in [
        "#scribe add this to decisions",
        "#scribe record it in my preferences",
        "#scribe that to decisions",
        "#scribe add these under the decision.",
    ] {
        assert_eq!(parse(prompt), [Err((1, "referent"))], "{prompt}");
    }
    let parsed = capture::scan("#scribe add THIS to decisions #ci", &tags());
    assert_eq!(
        parsed,
        [Parsed::Referent(Referent {
            line: 1,
            said: String::from("#scribe add THIS to decisions #ci"),
            tag: "decisions".parse().unwrap(),
            topics: vec!["ci".parse().unwrap()].try_into().unwrap(),
        })]
    );
    // More than the referent word is content, as form 2.
    assert_eq!(
        parse("#scribe add this rule to decisions"),
        [ok(1, "decisions", "this rule", &[])]
    );
}

#[test]
fn settling_verbs_and_distill_are_recognised_but_not_supported_yet() {
    for prompt in [
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

// --- form 3: referents --------------------------------------------------

fn message(uuid: &str, role: Speaker, text: &str) -> Message {
    Message {
        uuid: uuid.to_owned(),
        role,
        at: T0,
        blocks: vec![text.to_owned()],
    }
}

const PROMPT: &str = "#scribe add this to decisions";

fn turn() -> Turn {
    Turn {
        session: "s-1".parse().unwrap(),
        at: T0,
        prompt_sha256: capture::prompt_sha256(PROMPT),
        cwd: String::from("/work"),
        project: "scribe".parse().unwrap(),
        provider: String::from("claude-code").try_into().unwrap(),
        host: String::from("host").try_into().unwrap(),
    }
}

fn conversation() -> Vec<Message> {
    vec![
        message("u-1", Speaker::User, "Should we squash agent PRs?"),
        message(
            "a-1",
            Speaker::Assistant,
            "Options: squash or merge.\n\nI recommend we squash-merge agent PRs and keep merge commits for releases.",
        ),
        message(
            "u-2",
            Speaker::User,
            "Yes, let us squash-merge agent PRs from now on.",
        ),
        message(
            "a-2",
            Speaker::Assistant,
            "Understood. I will squash-merge agent PRs.",
        ),
    ]
}

/// A `Models` that answers every referent with what it was given and
/// remembers each request.
struct FakeModels {
    answer: Result<ReferentAnswer, ModelError>,
    asked: RefCell<Vec<ReferentRequest>>,
}

impl FakeModels {
    fn answering(answer: Result<ReferentAnswer, ModelError>) -> Self {
        Self {
            answer,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl Models for FakeModels {
    fn referent(&self, request: &ReferentRequest) -> Result<ReferentAnswer, ModelError> {
        self.asked.borrow_mut().push(request.clone());
        self.answer.clone()
    }
    fn session(
        &self,
        _request: &crate::models::SessionRequest,
    ) -> Result<crate::models::SessionAnswer, ModelError> {
        Err(ModelError::Unavailable(String::from("referent tests only")))
    }
}

fn span(message: &str, span: &str) -> Result<ReferentAnswer, ModelError> {
    Ok(ReferentAnswer {
        message: message.to_owned(),
        span: span.to_owned(),
    })
}

/// Parse [`PROMPT`], ask `models`, and resolve: the draft and its guess.
fn capture_referent(
    models: &FakeModels,
    messages: &[Message],
) -> (crate::entry::NewEntry, Option<Guess>) {
    let parsed = capture::scan(PROMPT, &tags());
    let Parsed::Referent(referent) = &parsed[0] else {
        panic!("expected a referent: {parsed:?}");
    };
    let window = capture::window(messages, &turn().prompt_sha256);
    let request = capture::referent_request(referent, &window, "claude-haiku-5-5");
    let answer = models.referent(&request);
    capture::resolve(referent, &window, answer, &turn(), &tags()).expect("a non-empty window")
}

#[test]
fn a_verbatim_span_of_a_recent_message_is_recorded_citing_its_role() {
    let models = FakeModels::answering(span(
        "u-2",
        "  let us squash-merge agent PRs from now on.\n",
    ));
    let (draft, guess) = capture_referent(&models, &conversation());
    assert_eq!(guess, None);
    assert_eq!(draft.content, "let us squash-merge agent PRs from now on.");
    assert_eq!(
        draft.content_from,
        ContentFrom::Message {
            role: Speaker::User,
            message: String::from("u-2"),
            at: T0,
        }
    );
    assert_eq!(draft.said, PROMPT);
    assert_eq!(draft.tag.as_str(), "decisions");
    assert_eq!(draft.cite.line, 1);
    assert_eq!(
        draft.cite.message, None,
        "the prompt's uuid comes from Stop"
    );

    let asked = models.asked.borrow();
    assert_eq!(asked.len(), 1, "one model call per line");
    assert_eq!(asked[0].model, "claude-haiku-5-5");
    assert_eq!(asked[0].budget, REFERENT_BUDGET);
    assert_eq!(asked[0].instruction, PROMPT);
    let uuids: Vec<&str> = asked[0]
        .candidates
        .iter()
        .map(|c| c.uuid.as_str())
        .collect();
    assert_eq!(uuids, ["u-1", "a-1", "u-2", "a-2"]);
    let prompt = asked[0].prompt();
    assert!(
        prompt.contains("<message id=\"a-1\" role=\"assistant\">"),
        "{prompt}"
    );
    assert!(prompt.contains(PROMPT));
}

#[test]
fn a_span_of_an_assistant_message_cites_the_assistant() {
    let models = FakeModels::answering(span(
        "a-1",
        "squash-merge agent PRs and keep merge commits for releases",
    ));
    let (draft, guess) = capture_referent(&models, &conversation());
    assert_eq!(guess, None);
    assert!(matches!(
        draft.content_from,
        ContentFrom::Message { role: Speaker::Assistant, ref message, .. } if message == "a-1"
    ));
}

/// The fallback: the previous assistant message's first 600 characters,
/// cited as that message.
fn assert_guessed(draft: &crate::entry::NewEntry, guess: Option<Guess>, expected: &Guess) {
    assert_eq!(guess.as_ref(), Some(expected));
    assert_eq!(draft.content, "Understood. I will squash-merge agent PRs.");
    assert_eq!(
        draft.content_from,
        ContentFrom::Message {
            role: Speaker::Assistant,
            message: String::from("a-2"),
            at: T0,
        }
    );
}

#[test]
fn a_span_failing_the_verbatim_check_falls_back_to_the_previous_assistant_message() {
    for answer in [
        span("u-2", "let us SQUASH-merge agent PRs"),
        span("u-2", "squash agent PRs, my decision"),
        span("u-2", "squash"),
    ] {
        let models = FakeModels::answering(answer);
        let (draft, guess) = capture_referent(&models, &conversation());
        assert_guessed(&draft, guess, &Guess::NotVerbatim);
    }
}

#[test]
fn a_message_outside_the_window_falls_back() {
    let models = FakeModels::answering(span("u-0", "let us squash-merge agent PRs"));
    let (draft, guess) = capture_referent(&models, &conversation());
    assert_guessed(&draft, guess, &Guess::UnknownMessage);
}

#[test]
fn a_timeout_or_a_model_error_falls_back() {
    for err in [
        ModelError::Timeout {
            budget: Duration::from_secs(20),
        },
        ModelError::Failed(String::from("exit status 1")),
        ModelError::Unavailable(String::from("no claude on PATH")),
        ModelError::Malformed(String::from("not JSON")),
    ] {
        let models = FakeModels::answering(Err(err.clone()));
        let (draft, guess) = capture_referent(&models, &conversation());
        assert_guessed(&draft, guess, &Guess::Model(err.clone()));
        assert!(Guess::Model(err).to_string().starts_with("the model"));
    }
}

#[test]
fn the_fallback_is_cut_to_600_characters_and_uses_the_last_message_without_an_assistant() {
    let long = "x".repeat(700);
    let messages = vec![message("a-1", Speaker::Assistant, &long)];
    let models = FakeModels::answering(Err(ModelError::Failed(String::from("boom"))));
    let (draft, _) = capture_referent(&models, &messages);
    assert_eq!(draft.content.chars().count(), capture::FALLBACK_CHARS);

    let messages = vec![message("u-1", Speaker::User, "only Daniel spoke so far")];
    let (draft, _) = capture_referent(&models, &messages);
    assert_eq!(draft.content, "only Daniel spoke so far");
    assert!(matches!(
        draft.content_from,
        ContentFrom::Message {
            role: Speaker::User,
            ..
        }
    ));
}

#[test]
fn the_window_is_the_last_twelve_messages_with_text_before_the_prompt() {
    let mut messages: Vec<Message> = (1..=15)
        .map(|n| {
            message(
                &format!("m-{n}"),
                Speaker::Assistant,
                &format!("message {n}"),
            )
        })
        .collect();
    messages.insert(3, message("blank", Speaker::User, "  \n"));
    let window = capture::window(&messages, &turn().prompt_sha256);
    let uuids: Vec<&str> = window.iter().map(|m| m.uuid.as_str()).collect();
    let expected: Vec<String> = (4..=15).map(|n| format!("m-{n}")).collect();
    assert_eq!(uuids, expected);

    // A retried hook finds its prompt already written: it and what follows
    // are not candidates.
    messages.push(message("prompt", Speaker::User, PROMPT));
    messages.push(message(
        "after",
        Speaker::Assistant,
        "a reply to the prompt",
    ));
    let window = capture::window(&messages, &turn().prompt_sha256);
    assert_eq!(window.last().map(|m| m.uuid.as_str()), Some("m-15"));
    assert!(capture::window(&[], "x").is_empty());
}

#[test]
fn an_empty_window_resolves_to_nothing_and_has_its_own_refusal() {
    let parsed = capture::scan(PROMPT, &tags());
    let Parsed::Referent(referent) = &parsed[0] else {
        panic!("expected a referent");
    };
    assert!(
        capture::resolve(referent, &[], span("a", "whatever text"), &turn(), &tags()).is_none()
    );
    assert_eq!(Refusal::NoReferent.code(), "no_referent");
}

#[test]
fn a_long_message_is_cut_in_the_prompt_and_the_cut_is_marked() {
    let long = "y".repeat(crate::models::PROMPT_MESSAGE_CHARS + 50);
    let messages = vec![message("a-1", Speaker::Assistant, &long)];
    let window = capture::window(&messages, "none");
    let parsed = capture::scan(PROMPT, &tags());
    let Parsed::Referent(referent) = &parsed[0] else {
        panic!("expected a referent");
    };
    let request = capture::referent_request(referent, &window, "m");
    let prompt = request.prompt();
    assert!(prompt.contains(crate::models::CUT_MARK));
    assert!(!prompt.contains(&long));
    assert_eq!(request.candidates[0].text, long, "the check sees it whole");
}
