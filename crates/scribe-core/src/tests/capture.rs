use super::T0;
use crate::capture::{self, Guess, Parsed, Referent, Refusal, Turn};
use crate::consistency::Settle;
use crate::entry::EntryId;
use crate::entry::{ContentFrom, Speaker, TagName};
use crate::models::{ModelError, Models, REFERENT_BUDGET, ReferentAnswer, ReferentRequest};
use crate::proposal::ProposalId;
use crate::record::Project;
use crate::tag::{TagDef, TagTable};
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
            Parsed::Settle(s) => Err((s.line, "settle")),
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
            tags: [
                "decisions",
                "finding",
                "idea",
                "preferences",
                "remind",
                "standard"
            ]
            .map(|t| TagName::try_from(String::from(t)).unwrap())
            .to_vec(),
        }
    );
    assert_eq!(
        refused.reason.to_string(),
        "no tag \"rituals\"; tags: decisions, finding, idea, preferences, remind, standard"
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
            global: false,
        })]
    );
    // More than the referent word is content, as form 2.
    assert_eq!(
        parse("#scribe add this rule to decisions"),
        [ok(1, "decisions", "this rule", &[])]
    );
}

#[test]
fn distill_is_recognised_but_not_supported_yet() {
    assert_eq!(
        parse("#scribe distill this discussion"),
        [Err((1, "unsupported_instruction"))]
    );
}

/// The settling action of each `#scribe` line of `prompt`.
fn settles(prompt: &str) -> Vec<Settle> {
    capture::scan(prompt, &tags())
        .into_iter()
        .filter_map(|p| match p {
            Parsed::Settle(s) => Some(s.action),
            Parsed::Instruction(_) | Parsed::Referent(_) | Parsed::Refused(_) => None,
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
            "#scribe approve p-4\n#scribe reject p-12.\n#scribe retire decisions-2\n#scribe forget rituals-7!"
        ),
        [
            Settle::Approve(proposal("p-4")),
            Settle::Reject(proposal("p-12")),
            Settle::Retire(entry("decisions-2")),
            Settle::Forget(entry("rituals-7")),
        ]
    );
}

#[test]
fn a_settling_verb_without_its_kind_of_id_is_refused() {
    for prompt in [
        "#scribe approve decisions-2",
        "#scribe reject",
        "#scribe forget decisions",
        "#scribe retire decisions-2 decisions-3",
        "#scribe approve p-4 and p-5",
    ] {
        assert_eq!(
            parse(prompt),
            [Err((1, "unparsed_instruction"))],
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
    assert_eq!(entry.project.as_str(), "scribe");
    assert_eq!(entry.author.kind, crate::AuthorKind::Daniel);
}

#[test]
fn the_global_opener_marks_each_capture_form_without_taking_its_content() {
    let form_one = capture::scan(
        "#scribe/global decisions: squash-merge agent PRs #ci",
        &tags(),
    );
    let Parsed::Instruction(one) = &form_one[0] else {
        panic!("expected an instruction: {form_one:?}");
    };
    assert!(one.global);
    assert_eq!(one.tag.as_str(), "decisions");
    assert_eq!(one.content, "squash-merge agent PRs");
    assert!(one.said.contains(&one.content));
    assert_eq!(topic_names(&one.topics), ["ci"]);

    let form_two = capture::scan(
        "#scribe/global add short commit subjects to my preferences",
        &tags(),
    );
    let Parsed::Instruction(two) = &form_two[0] else {
        panic!("expected an instruction: {form_two:?}");
    };
    assert!(two.global);
    assert_eq!(two.content, "short commit subjects");
    assert_eq!(two.tag.as_str(), "preferences");

    let form_three = capture::scan(
        "#scribe/global add this to decisions #merge-policy",
        &tags(),
    );
    let Parsed::Referent(three) = &form_three[0] else {
        panic!("expected a referent: {form_three:?}");
    };
    assert!(three.global);
    assert_eq!(three.tag.as_str(), "decisions");
    assert_eq!(topic_names(&three.topics), ["merge-policy"]);

    let turn = turn();
    let draft = capture::draft(one, &turn, &tags());
    assert_eq!(draft.project.as_str(), Project::GLOBAL);
    assert_eq!(
        draft.into_entry("decisions-1".parse().unwrap()).author.kind,
        crate::AuthorKind::Daniel
    );
    let messages = conversation();
    let window: Vec<&crate::transcript::Message> = messages.iter().collect();
    let (referent_draft, _) = capture::resolve(
        three,
        &window,
        span(
            "a-1",
            "squash-merge agent PRs and keep merge commits for releases.",
        ),
        &turn,
        &tags(),
    )
    .expect("a window");
    assert_eq!(referent_draft.project.as_str(), Project::GLOBAL);
}

#[test]
fn a_topic_hashtag_or_a_tag_named_global_is_not_the_marker() {
    let topical = capture::scan("#scribe decisions: run the gate #global #ci", &tags());
    let Parsed::Instruction(topical) = &topical[0] else {
        panic!("#global is a topic, not the marker: {topical:?}");
    };
    assert!(!topical.global);
    assert_eq!(topic_names(&topical.topics), ["global", "ci"]);
    assert_eq!(topical.content, "run the gate");
    let table = TagTable::new(vec![
        TagDef {
            name: "decisions".parse().unwrap(),
            aliases: vec!["decision".parse().unwrap()],
            description: None,
            stale_after_days: None,
            action: None,
            require_evidence: false,
        },
        TagDef {
            name: "global".parse().unwrap(),
            aliases: vec!["everywhere".parse().unwrap()],
            description: None,
            stale_after_days: None,
            action: None,
            require_evidence: false,
        },
    ])
    .unwrap();
    let named = capture::scan("#scribe global: keep the API stable", &table);
    let Parsed::Instruction(named) = &named[0] else {
        panic!("a tag named global is form 1: {named:?}");
    };
    assert!(!named.global);
    assert_eq!(named.tag.as_str(), "global");
    assert_eq!(named.content, "keep the API stable");
    let aliased = capture::scan("#scribe add keep it to everywhere", &table);
    let Parsed::Instruction(aliased) = &aliased[0] else {
        panic!("an alias named everywhere is form 2: {aliased:?}");
    };
    assert!(!aliased.global);
    assert_eq!(aliased.tag.as_str(), "global");
    let marked = capture::scan("#scribe/global everywhere: keep it #ci", &table);
    let Parsed::Instruction(marked) = &marked[0] else {
        panic!("the opener still marks a tag named global: {marked:?}");
    };
    assert!(marked.global);
    assert_eq!(marked.tag.as_str(), "global");
    assert_eq!(marked.content, "keep it");
    assert_eq!(topic_names(&marked.topics), ["ci"]);
}

fn topic_names(topics: &crate::Topics) -> Vec<&str> {
    topics.as_slice().iter().map(crate::Topic::as_str).collect()
}

#[test]
fn a_malformed_marker_or_a_marker_on_a_refused_line_is_refused_and_teaches_the_opener() {
    for prompt in [
        "#scribe/Global decisions: hello",
        "#scribe/all decisions: hello",
        "#scribe/",
    ] {
        let parsed = capture::scan(prompt, &tags());
        let Parsed::Refused(refused) = &parsed[0] else {
            panic!("expected a refusal: {parsed:?}");
        };
        assert_eq!(refused.reason, Refusal::MalformedMarker);
        assert!(
            refused.message().contains(capture::GLOBAL_OPENER),
            "{prompt}"
        );
    }
    let unknown = capture::scan("#scribe/global rituals: stand-up at nine", &tags());
    let Parsed::Refused(unknown) = &unknown[0] else {
        panic!("expected a refusal: {unknown:?}");
    };
    assert_eq!(unknown.reason.code(), "unknown_tag");
    assert!(unknown.message().contains(capture::GLOBAL_OPENER));
    let settle = capture::scan("#scribe/global approve p-4", &tags());
    let Parsed::Refused(settle) = &settle[0] else {
        panic!("a marker does not settle: {settle:?}");
    };
    assert_eq!(settle.reason, Refusal::MarkerNotOnEntry);
    assert!(settle.message().contains(capture::GLOBAL_OPENER));
    let bare = capture::scan("#scribe/global", &tags());
    let Parsed::Refused(bare) = &bare[0] else {
        panic!("expected a refusal: {bare:?}");
    };
    assert_eq!(bare.reason, Refusal::Unparsed);
    assert!(bare.message().contains(capture::GLOBAL_OPENER));
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
            "#scribe decisions: ship on fridays",
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
