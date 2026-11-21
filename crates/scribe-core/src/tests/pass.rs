use super::{T0, session};
use crate::entry::Speaker;
use crate::models::{
    NewSegmentAnswer, PassMessage, SegmentAnswer, SessionAnswer, SessionRequest, TermAnswer,
};
use crate::pass::{self, Invalid, PassMeta, WINDOW_CHARS};
use crate::session::{SegmentOrigin, SegmentState, SessionStatus};
use crate::transcript::Message;
use time::Duration;

fn message(uuid: &str, role: Speaker, text: &str) -> Message {
    Message {
        uuid: uuid.to_owned(),
        role,
        at: T0,
        blocks: vec![text.to_owned()],
    }
}

const DEFINED: &str =
    "by a resident orchestrator I mean one agent that stays up and owns the backlog";
const ASSISTANT: &str = "A graduated crew is one that has passed three reviews without a repair.";

/// A session: Daniel defines a term, the assistant defines another, a pasted
/// log, and the synthetic turns Claude Code writes.
fn transcript() -> Vec<Message> {
    vec![
        message("u-1", Speaker::User, &format!("Let's plan. {DEFINED}.")),
        message("a-1", Speaker::Assistant, ASSISTANT),
        message(
            "u-2",
            Speaker::User,
            "<task-notification>agent finished</task-notification>",
        ),
        message("u-3", Speaker::User, "/scribe on"),
        message("u-4", Speaker::User, "<command-name>/clear</command-name>"),
        message(
            "u-5",
            Speaker::User,
            "Look at this:\n<pasted_content id=\"1\">\nerror: the pasted words are not mine at all\n</pasted_content>\nwhy?",
        ),
        message("a-2", Speaker::Assistant, "Because the lock was held."),
    ]
}

#[test]
fn the_conversation_drops_synthetic_turns_and_sets_pasted_blocks_apart() {
    let conversation = pass::conversation(&transcript());
    let ids: Vec<&str> = conversation.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["u-1", "a-1", "u-5", "a-2"]);
    let pasted = &conversation[2];
    assert_eq!(pasted.text, "Look at this:\n\nwhy?");
    assert_eq!(
        pasted.pasted,
        ["error: the pasted words are not mine at all"]
    );
    assert_eq!(pass::user_turns_after(&conversation, None), 2);
    assert_eq!(pass::user_turns_after(&conversation, Some("a-1")), 1);
    assert_eq!(pass::user_turns_after(&conversation, Some("a-2")), 0);
    assert_eq!(
        pass::user_turns_after(&conversation, Some("gone")),
        2,
        "a scribed_through no longer in the transcript counts everything"
    );
}

fn plain(uuid: &str, chars: usize) -> PassMessage {
    PassMessage {
        uuid: uuid.to_owned(),
        role: Speaker::User,
        text: "x".repeat(chars),
        pasted: Vec::new(),
    }
}

#[test]
fn the_window_starts_after_scribed_through_and_keeps_the_oldest_first() {
    let conversation = vec![plain("m-1", 10), plain("m-2", 60), plain("m-3", 60)];
    let all = pass::window(&conversation, None, 1_000);
    assert_eq!(all.messages.len(), 3);
    assert_eq!(all.dropped, 0);

    let after = pass::window(&conversation, Some("m-1"), 1_000);
    let ids: Vec<&str> = after.messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["m-2", "m-3"]);
    assert!(
        pass::window(&conversation, Some("m-3"), 1_000)
            .messages
            .is_empty()
    );

    let tight = pass::window(&conversation, None, 100);
    let ids: Vec<&str> = tight.messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["m-1", "m-2"]);
    assert_eq!(tight.dropped, 0);

    let huge = vec![plain("m-1", WINDOW_CHARS + 10)];
    let cut = pass::window(&huge, None, WINDOW_CHARS);
    assert!(cut.messages[0].text.ends_with(crate::models::CUT_MARK));
    assert_eq!(cut.messages[0].text.chars().count(), WINDOW_CHARS);

    // Even tiny bounds and cuts between own/pasted text stay within the
    // Unicode-character budget, without attributing pasted words to Daniel.
    let pasted = vec![PassMessage {
        text: "é".repeat(50),
        pasted: vec!["pasted".repeat(30)],
        ..plain("pasted", 0)
    }];
    for max in [1, 32, 50, 80] {
        let bounded = pass::window(&pasted, None, max);
        let message = &bounded.messages[0];
        assert_eq!(message.chars(), max);
        assert!(!message.text.contains("pasted"));
        let tail = message.pasted.last().unwrap_or(&message.text);
        let marker: String = crate::models::CUT_MARK.chars().take(max).collect();
        assert!(tail.ends_with(&marker));
    }
}

fn answer() -> SessionAnswer {
    SessionAnswer {
        candidates: Vec::new(),
        title: String::from("Planning the orchestrator"),
        summary: String::from("Daniel planned the resident orchestrator."),
        goals: String::from("Define the orchestrator."),
        open: None,
        new: vec![NewSegmentAnswer {
            starts_at: String::from("u-1"),
            heading: String::from("Planning the orchestrator"),
            text: String::from("A definition."),
            topics: Vec::new(),
            related: Vec::new(),
        }],
        next: vec![String::from("write the design"), String::from(" ")],
        topics: vec![String::from("orchestration"), String::from("orchestration")],
        related: vec![String::from("task:ORB-1"), String::from("not a reference")],
        terms: Vec::new(),
    }
}

fn check(answer: SessionAnswer, conversation: &[PassMessage]) -> Result<pass::Distilled, Invalid> {
    let previous = answer.open.as_ref().map(|_| session("s-1", T0));
    let request = pass::request(
        "fake",
        previous.as_ref(),
        Vec::new(),
        pass::window(conversation, None, WINDOW_CHARS),
        Vec::new(),
        Vec::new(),
        false,
    );
    pass::check(answer, conversation, &request)
}

fn term(term: &str, said: &str, message: &str) -> TermAnswer {
    TermAnswer {
        term: term.to_owned(),
        said: said.to_owned(),
        message: message.to_owned(),
    }
}

#[test]
fn a_term_is_kept_only_when_said_is_verbatim_from_daniels_own_turn() {
    let conversation = pass::conversation(&transcript());
    let mut a = answer();
    a.terms = vec![
        // Daniel's words in the named turn.
        term("resident orchestrator", DEFINED, "u-1"),
        // The assistant's words: never Daniel's.
        term(
            "graduated crew",
            "A graduated crew is one that has passed three reviews",
            "a-1",
        ),
        // A pasted block: not his words either.
        term("pasted", "the pasted words are not mine", "u-5"),
        // A paraphrase.
        term("resident", "a resident orchestrator is an agent", "u-1"),
        // The same term again, differently cased.
        term("Resident  Orchestrator", DEFINED, "u-1"),
    ];
    let distilled = check(a, &conversation).unwrap();
    assert_eq!(distilled.terms.len(), 1);
    let kept = &distilled.terms[0];
    assert_eq!(kept.term, "resident orchestrator");
    assert_eq!(kept.said, DEFINED);
    assert_eq!(kept.message, "u-1", "the turn that holds the words");
    assert_eq!(distilled.dropped_terms, 3);

    assert_eq!(distilled.next, "- write the design");
    assert_eq!(distilled.next_open, 1);
    assert_eq!(distilled.topics.as_slice().len(), 1);
    let related: Vec<String> = distilled.related.iter().map(ToString::to_string).collect();
    assert_eq!(related, ["task:ORB-1"]);
}

#[test]
fn an_answer_over_budget_or_out_of_shape_is_refused_whole() {
    let conversation = pass::conversation(&transcript());
    let mut over = answer();
    over.summary = "s".repeat(1201);
    assert_eq!(
        check(over, &conversation),
        Err(Invalid::OverBudget {
            field: "Summary",
            chars: 1201,
            max: 1200
        })
    );
    let mut next = answer();
    next.next = vec!["n".repeat(999)];
    assert!(matches!(
        check(next, &conversation),
        Err(Invalid::OverBudget { field: "next", .. })
    ));
    let mut title = answer();
    title.title = String::from("two\nlines");
    assert_eq!(check(title, &conversation), Err(Invalid::Title));
    let mut heading = answer();
    heading.summary = String::from("done\n## Next\nmore");
    assert_eq!(
        check(heading, &conversation),
        Err(Invalid::Heading("Summary"))
    );
    let mut topics = answer();
    topics.topics = vec![String::from("Continuous Integration")];
    assert!(matches!(
        check(topics, &conversation),
        Err(Invalid::Topics(_))
    ));
    let mut many = answer();
    many.topics = (0..6).map(|n| format!("t{n}")).collect();
    assert!(matches!(
        check(many, &conversation),
        Err(Invalid::Topics(_))
    ));
    let mut empty = answer();
    empty.summary = String::from("  ");
    assert_eq!(check(empty, &conversation), Err(Invalid::EmptySummary));
}

fn meta(now: time::OffsetDateTime, through: &str, final_pass: bool) -> PassMeta {
    PassMeta {
        id: "s-1".parse().unwrap(),
        project: "scribe".parse().unwrap(),
        provider: String::from("claude-code").try_into().unwrap(),
        host: String::from("host").try_into().unwrap(),
        cwd: String::from("/work/scribe"),
        started: T0,
        on_since: T0 + Duration::minutes(1),
        through: Some(through.to_owned()),
        now,
        final_pass,
        first_message: Some(String::from("m-1")),
        from: T0,
        to: now,
        window: vec![(String::from("m-1"), T0), (through.to_owned(), now)],
        segment_windows: pass::SEGMENT_WINDOWS,
        origin: SegmentOrigin::Pass,
    }
}

#[test]
fn a_record_advances_scribed_through_and_keeps_where_it_lives() {
    let conversation = pass::conversation(&transcript());
    let mut a = answer();
    a.terms = vec![term("resident orchestrator", DEFINED, "u-1")];
    let now = T0 + Duration::minutes(30);
    let first = pass::record(
        None,
        check(a, &conversation).unwrap(),
        meta(now, "a-1", false),
    );
    assert_eq!(first.passes, 1);
    assert_eq!(first.scribed_through.as_deref(), Some("a-1"));
    assert_eq!(first.created, now);
    assert_eq!(first.started, T0);
    assert_eq!(first.status, SessionStatus::Open);
    assert_eq!(first.terms.len(), 1);

    let mut previous = session("s-1", T0 - Duration::days(40));
    previous.passes = 4;
    previous.terms = first.terms.clone();
    let later = now + Duration::minutes(20);
    let mut b = answer();
    b.terms = vec![term("RESIDENT orchestrator", DEFINED, "u-1")];
    let second = pass::record(
        Some(&previous),
        check(b, &conversation).unwrap(),
        meta(later, "a-2", true),
    );
    assert_eq!(second.passes, 5);
    assert_eq!(second.scribed_through.as_deref(), Some("a-2"));
    assert_eq!(second.started, previous.started, "the file stays put");
    assert_eq!(second.created, previous.created);
    assert_eq!(second.cwd, previous.cwd);
    assert_eq!(second.updated, later);
    assert_eq!(second.status, SessionStatus::Final);
    assert_eq!(second.terms, previous.terms, "a known term is not repeated");

    let finalized = pass::finalized(&previous, later);
    assert_eq!(finalized.status, SessionStatus::Final);
    assert_eq!(finalized.passes, previous.passes);
}

#[test]
fn passes_grow_the_open_segment_and_the_last_one_seals_it() {
    // An open segment is rewritten; an upgraded sealed log gains segment n + 1.
    let conversation = pass::conversation(&transcript());
    let now = T0 + Duration::minutes(30);
    let first = pass::record(
        None,
        check(answer(), &conversation).unwrap(),
        meta(now, "a-1", false),
    );
    assert_eq!(first.log.len(), 1);
    let one = &first.log[0];
    assert_eq!((one.n.get(), one.state), (1, SegmentState::Open));
    assert_eq!(one.origin, SegmentOrigin::Pass);
    assert_eq!(one.heading, "Planning the orchestrator");
    assert_eq!(one.text, "A definition.");
    assert_eq!(one.first_message.as_deref(), Some("m-1"));
    assert_eq!(one.last_message.as_deref(), Some("a-1"));
    assert_eq!((one.from, one.to), (T0, now));
    assert!(crate::session::check_log(&first).is_ok());

    let later = now + Duration::minutes(20);
    let mut b = answer();
    b.open = Some(SegmentAnswer {
        heading: b.new[0].heading.clone(),
        text: String::from("A definition, and a plan."),
        topics: Vec::new(),
        related: Vec::new(),
    });
    b.new.clear();
    let second = pass::record(
        Some(&first),
        check(b, &conversation).unwrap(),
        meta(later, "a-2", false),
    );
    assert_eq!(second.log.len(), 1, "the open segment grows");
    assert_eq!(second.log[0].windows.get(), 2);
    assert_eq!(second.log[0].text, "A definition, and a plan.");
    assert_eq!(second.log[0].last_message.as_deref(), Some("a-2"));
    assert_eq!(second.log[0].from, T0, "where it began is kept");

    let ended = pass::finalized(&second, later);
    assert_eq!(ended.log[0].state, SegmentState::Sealed);
    assert!(crate::session::check_log(&ended).is_ok());

    let mut upgraded = session("s-1", T0);
    upgraded.log[0].state = SegmentState::Sealed;
    upgraded.log[0].origin = SegmentOrigin::Upgrade;
    let next = pass::record(
        Some(&upgraded),
        check(answer(), &conversation).unwrap(),
        meta(later, "a-2", true),
    );
    assert_eq!(next.log.len(), 2);
    assert_eq!(
        next.log[0], upgraded.log[0],
        "a sealed segment is not touched"
    );
    assert_eq!(
        (next.log[1].n.get(), next.log[1].state),
        (2, SegmentState::Sealed)
    );
    assert!(crate::session::check_log(&next).is_ok());
}

#[test]
fn the_request_carries_the_previous_record_the_window_and_what_to_reuse() {
    let conversation = pass::conversation(&transcript());
    let previous = session("s-1", T0);
    let window = pass::window(&conversation, Some("a-1"), WINDOW_CHARS);
    let request = pass::request(
        "claude-haiku-5-5",
        Some(&previous),
        Vec::new(),
        window,
        vec![String::from("ci")],
        vec![String::from("graduate to level 4")],
        true,
    );
    assert_eq!(request.budget, crate::models::SESSION_BUDGET);
    let prompt = request.prompt();
    for part in [
        "--- user · u-5 ---",
        "--- Pasted data (not Daniel's words) ---\nerror: the pasted words are not mine at all",
        "--- assistant · a-2 ---",
        "Existing topics: ci",
        "Existing terms: graduate to level 4",
        "## Summary\nsummary",
        "this is its final record",
    ] {
        assert!(prompt.contains(part), "{part:?} missing from:\n{prompt}");
    }
    assert!(!prompt.contains("--- user · u-1 ---"), "already folded in");
}

/// The request for `window` after a record whose open segment is near the
/// segment budget and ends at `last`.
fn overflow_request(last: &str, window: &[&str]) -> SessionRequest {
    let mut previous = session("s-1", T0);
    previous.log[0].text = "o".repeat(800);
    previous.log[0].last_message = Some(last.to_owned());
    pass::request(
        "fake",
        Some(&previous),
        Vec::new(),
        pass::Window {
            messages: window.iter().map(|id| plain(id, 10)).collect(),
            dropped: 0,
        },
        Vec::new(),
        Vec::new(),
        false,
    )
}

fn segment_at(starts_at: &str, text: String) -> NewSegmentAnswer {
    NewSegmentAnswer {
        starts_at: starts_at.to_owned(),
        heading: String::from("Next part"),
        text,
        topics: Vec::new(),
        related: Vec::new(),
    }
}

/// An answer whose new segments cannot continue the 800-character open one.
fn overflow_answer(new: Vec<NewSegmentAnswer>) -> SessionAnswer {
    SessionAnswer {
        candidates: Vec::new(),
        open: Some(SegmentAnswer {
            heading: String::from("part 1"),
            text: "o".repeat(800),
            topics: Vec::new(),
            related: Vec::new(),
        }),
        new,
        ..answer()
    }
}

fn conversation_of(ids: &[&str]) -> Vec<PassMessage> {
    ids.iter().map(|id| plain(id, 10)).collect()
}

#[test]
fn repair_starts_an_overflowing_segment_at_a_window_message_the_check_accepts() {
    // The window's first message is not a conversation turn, so a start there
    // cannot be ordered against the open segment's last message.
    let conversation = conversation_of(&["u-1", "a-1", "a-2"]);
    let request = overflow_request("a-1", &["ghost", "a-2"]);
    let answer = overflow_answer(vec![segment_at("a-1", "n".repeat(300))]);
    assert!(pass::check(answer.clone(), &conversation, &request).is_err());

    let repaired = pass::repair(answer, &conversation, &request).unwrap();

    assert_eq!(repaired.new.len(), 1);
    assert_eq!(
        repaired.new[0].0, 1,
        "a-2, the first turn after the open one"
    );
    assert_eq!(repaired.new[0].1.text, "n".repeat(300));
    assert!(!repaired.seal_open);
}

#[test]
fn repair_skips_window_messages_at_or_before_the_open_segments_last_message() {
    let conversation = conversation_of(&["u-1", "a-1", "u-2", "a-2"]);
    let request = overflow_request("u-2", &["a-1", "u-2", "a-2"]);
    let answer = overflow_answer(vec![segment_at("a-1", "n".repeat(300))]);

    let repaired = pass::repair(answer, &conversation, &request).unwrap();

    assert_eq!(repaired.new.len(), 1);
    assert_eq!(repaired.new[0].0, 2, "not the window's first message");
}

#[test]
fn repair_seals_the_open_segment_when_no_window_message_can_start_the_content() {
    let conversation = conversation_of(&["u-1", "a-1", "u-2"]);
    let request = overflow_request("u-2", &["a-1", "u-2"]);
    let answer = overflow_answer(vec![segment_at("a-1", "n".repeat(300))]);

    let repaired = pass::repair(answer, &conversation, &request).unwrap();

    assert!(repaired.new.is_empty());
    assert!(
        repaired.seal_open,
        "the open segment ends at the window's end"
    );
    let open = repaired.open.unwrap();
    assert!(open.text.chars().count() <= pass::SEGMENT_CHARS);
    assert!(open.text.starts_with(&"o".repeat(800)));
    assert!(repaired.repairs.iter().any(|r| r.contains("cut at")));
}

#[test]
fn repair_never_places_a_relocated_segment_after_a_start_the_model_kept() {
    // The only start before the model's own second start is not after the
    // open segment, so the first segment's content ends the open one instead.
    let conversation = conversation_of(&["u-1", "a-1", "u-2", "a-2"]);
    let request = overflow_request("u-2", &["a-1", "u-2", "a-2"]);
    let answer = overflow_answer(vec![
        segment_at("a-1", "n".repeat(300)),
        segment_at("a-2", String::from("Kept as the model wrote it.")),
    ]);

    let repaired = pass::repair(answer, &conversation, &request).unwrap();

    assert_eq!(repaired.new.len(), 1);
    assert_eq!(repaired.new[0].0, 2);
    assert_eq!(repaired.new[0].1.text, "Kept as the model wrote it.");
    assert!(repaired.open.unwrap().text.chars().count() <= pass::SEGMENT_CHARS);
}

fn said(uuid: &str, role: Speaker, text: &str) -> PassMessage {
    PassMessage {
        uuid: uuid.to_owned(),
        role,
        text: text.to_owned(),
        pasted: Vec::new(),
    }
}

#[test]
fn only_daniels_own_lines_that_fire_a_trigger_are_named() {
    let tags = crate::tag::TagTable::defaults();
    let messages = vec![
        said("m-1", Speaker::User, "Let's look at the build."),
        said("m-2", Speaker::Assistant, "I always run the tests first."),
        said(
            "m-3",
            Speaker::User,
            "> you should never do that\n```\nalways\n```\n    never\n#scribe always",
        ),
        said("m-4", Speaker::User, "From now on, use tabs."),
        said("m-5", Speaker::User, "What if we cache it?"),
    ];
    let (turns, rules) = pass::capture_request(&tags, &messages);
    let named: Vec<(&str, &str, &str, &str)> = turns
        .iter()
        .map(|t| {
            (
                t.message.as_str(),
                t.tag.as_str(),
                t.trigger.as_str(),
                t.matched.as_str(),
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            ("m-4", "preferences", "from now on", "From now on"),
            ("m-5", "idea", "what if we", "What if we"),
        ]
    );
    assert_eq!(rules.len(), 2);
    assert!(
        rules
            .iter()
            .all(|r| r.criteria.is_some() && r.not.is_some())
    );

    let (none, no_rules) = pass::capture_request(&tags, &messages[..3]);
    assert!(none.is_empty() && no_rules.is_empty());
}

#[test]
fn admission_recomputes_the_trigger_for_the_claimed_tag() {
    let tags = crate::tag::TagTable::defaults();
    let preferences: crate::entry::TagName = "preferences".parse().unwrap();
    let idea: crate::entry::TagName = "idea".parse().unwrap();
    let admitted = pass::admission(&tags, &preferences, "I prefer short names.").unwrap();
    assert_eq!(admitted.trigger, "I prefer");
    assert_eq!(admitted.matched, "I prefer");
    assert_eq!(
        admitted.criteria,
        tags.get(&preferences)
            .unwrap()
            .capture
            .criteria
            .as_ref()
            .map(|c| c.value.clone())
    );
    assert!(pass::admission(&tags, &idea, "I prefer short names.").is_none());
    assert!(pass::admission(&tags, &"standard".parse().unwrap(), "always").is_none());
}

#[test]
fn covers_statement_is_directional_on_token_boundaries_and_polarity() {
    let covers = pass::covers_statement;
    assert!(covers("use rg", "use rg"), "an exact duplicate is covered");
    assert!(
        covers(
            "I prefer concise messages during lengthy builds.",
            "I prefer concise messages during lengthy builds!"
        ),
        "case and punctuation collapse to the same tokens"
    );
    assert!(
        covers("don\u{2019}t force push", "don't force push"),
        "a curly apostrophe folds into the same token"
    );
    assert!(
        !covers("use rg", "use rg, not grep, in scripts"),
        "a longer refinement is written"
    );
    assert!(
        !covers(
            "squash-merge agent PRs",
            "squash-merge agent PRs except on release branches"
        ),
        "an added exception is written"
    );
    assert!(
        !covers("test", "latest"),
        "a token does not match inside another word"
    );
    assert!(!covers("latest", "test"));
    assert!(
        !covers("use grep, not rg", "use rg, not grep"),
        "reordered rules are not the same statement"
    );
    assert!(!covers("use rg, not grep", "use grep, not rg"));
    assert!(
        !covers("use grep in scripts", "never use grep in scripts"),
        "a negation is not covered"
    );
    assert!(!covers("never use grep in scripts", "use grep in scripts"));
    assert!(!covers(
        "always use rg in scripts",
        "never use rg in scripts"
    ));
    assert!(!covers(
        "never use rg in scripts",
        "always use rg in scripts"
    ));
    assert!(
        !covers("we shouldn't use cargo test in CI", "use cargo test in CI"),
        "shouldn't is a negation"
    );
    assert!(
        !covers("we shouldnt use cargo test in CI", "use cargo test in CI"),
        "shouldnt is the apostrophe-less spelling"
    );
    assert!(
        !covers(
            "we can't run integration tests in CI",
            "run integration tests in CI"
        ),
        "can't is a negation"
    );
    assert!(
        !covers(
            "we cant run integration tests in CI",
            "run integration tests in CI"
        ),
        "cant is the apostrophe-less spelling"
    );
    assert!(
        !covers("we cannot force push to main", "force push to main"),
        "cannot is a negation"
    );
    assert!(
        !covers(
            "we won't cache build artifacts in CI",
            "cache build artifacts in CI"
        ),
        "won't is a negation"
    );
    assert!(
        !covers(
            "we wont cache build artifacts in CI",
            "cache build artifacts in CI"
        ),
        "wont is the apostrophe-less spelling"
    );
    assert!(
        !covers(
            "the job doesn't upload symbols in CI",
            "upload symbols in CI"
        ),
        "doesn't is a negation"
    );
    assert!(
        !covers(
            "the job doesnt upload symbols in CI",
            "upload symbols in CI"
        ),
        "doesnt is the apostrophe-less spelling"
    );
    assert!(
        !covers(
            "the job didn't upload symbols from CI",
            "upload symbols from CI"
        ),
        "didn't is a negation"
    );
    assert!(
        !covers(
            "the job didnt upload symbols from CI",
            "upload symbols from CI"
        ),
        "didnt is the apostrophe-less spelling"
    );
    assert!(
        !covers(
            "the agent isn't allowed to force push",
            "allowed to force push"
        ),
        "isn't is a negation"
    );
    assert!(
        !covers(
            "the agent isnt allowed to force push",
            "allowed to force push"
        ),
        "isnt is the apostrophe-less spelling"
    );
    assert!(
        !covers(
            "review the diff without running the full suite",
            "running the full suite"
        ),
        "without is an exception"
    );
    assert!(
        covers("use rg in scripts today please", "use rg in scripts"),
        "an existing statement covers a contiguous window that adds nothing"
    );
    assert!(
        !covers("use rg in scripts today", "use rg in today"),
        "containment is a contiguous window, not a gapped subsequence"
    );
    assert!(
        !covers("use rg", "use vim"),
        "below four tokens only an equal sequence matches"
    );
    assert!(!covers("", "use rg"));
    assert!(!covers("use rg", ""));
}

#[test]
fn covering_entry_keeps_the_oldest_active_entry_of_the_same_tag_and_project() {
    use crate::entry::EntryStatus;
    let statement = "use rg in scripts today";
    let mut oldest = super::entry("preferences-1", T0);
    oldest.content = statement.to_owned();
    let mut later = super::entry("preferences-2", T0 + Duration::hours(1));
    later.content = format!("{statement} today");
    let mut retired = super::entry("preferences-3", T0 - Duration::hours(1));
    retired.content = statement.to_owned();
    retired.status = EntryStatus::Retired;
    let mut superseded = super::entry("preferences-4", T0 - Duration::hours(2));
    superseded.content = statement.to_owned();
    superseded.status = EntryStatus::Superseded;
    let mut forgotten = super::entry("preferences-5", T0 - Duration::hours(3));
    forgotten.status = EntryStatus::Forgotten;
    forgotten.content.clear();
    let mut other_tag = super::entry("idea-1", T0 - Duration::hours(4));
    other_tag.content = statement.to_owned();
    let mut other_project = super::entry("preferences-6", T0 - Duration::hours(5));
    other_project.content = statement.to_owned();
    other_project.project = "other".parse().unwrap();
    let tag: crate::entry::TagName = "preferences".parse().unwrap();
    let project: crate::record::Project = "scribe".parse().unwrap();
    let active = [
        later,
        retired.clone(),
        superseded.clone(),
        forgotten.clone(),
        other_tag.clone(),
        other_project.clone(),
        oldest,
    ];
    let found = pass::covering_entry(&active, statement, &tag, &project);
    assert_eq!(found.unwrap().id.to_string(), "preferences-1");
    let inactive = [retired, superseded, forgotten, other_tag, other_project];
    let blocked = pass::covering_entry(&inactive, statement, &tag, &project);
    assert!(
        blocked.is_none(),
        "retired, superseded, forgotten, other tag and other project do not cover"
    );
}

#[test]
fn covers_turn_keeps_a_retired_entry_for_the_turn_it_was_active() {
    use crate::entry::EntryStatus;
    let statement = "use cargo test in CI";
    let mut retired_after = super::entry("preferences-1", T0);
    retired_after.content = statement.to_owned();
    retired_after.status = EntryStatus::Retired;
    retired_after.inactive_since = Some(T0 + Duration::hours(2));
    retired_after.updated = T0 + Duration::hours(2);
    let mut retired_before = super::entry("preferences-2", T0 - Duration::hours(3));
    retired_before.content = statement.to_owned();
    retired_before.status = EntryStatus::Retired;
    retired_before.inactive_since = Some(T0 - Duration::hours(1));
    retired_before.updated = T0 - Duration::hours(1);
    let mut superseded_after = super::entry("preferences-3", T0 - Duration::hours(1));
    superseded_after.content = statement.to_owned();
    superseded_after.status = EntryStatus::Superseded;
    superseded_after.inactive_since = Some(T0 + Duration::minutes(1));
    superseded_after.updated = T0 + Duration::minutes(1);
    let mut forgotten = super::entry("preferences-4", T0 - Duration::hours(1));
    forgotten.content = statement.to_owned();
    forgotten.status = EntryStatus::Forgotten;
    forgotten.inactive_since = Some(T0 + Duration::hours(1));
    forgotten.updated = T0 + Duration::hours(1);
    forgotten.content.clear();
    let tag: crate::entry::TagName = "preferences".parse().unwrap();
    let project: crate::record::Project = "scribe".parse().unwrap();
    let covers =
        |entry: &crate::entry::Entry| pass::covers_turn(entry, statement, &tag, &project, T0);
    let mut same_second = super::entry("preferences-5", T0);
    same_second.content = statement.to_owned();
    same_second.status = EntryStatus::Retired;
    same_second.inactive_since = Some(T0);
    same_second.updated = T0;
    assert!(
        covers(&retired_after),
        "retired after the turn still covers it"
    );
    assert!(
        covers(&same_second),
        "a retirement in the turn's second still covers it"
    );
    assert!(
        covers(&superseded_after),
        "superseded after the turn still covers it"
    );
    assert!(
        !covers(&retired_before),
        "retired before the turn does not cover it"
    );
    assert!(!covers(&forgotten), "a forgotten entry has no content left");
    assert!(
        pass::covering_entry(&[retired_after], statement, &tag, &project).is_none(),
        "a new line after retirement is not covered by the live check"
    );
}

#[test]
fn covers_turn_compares_a_sub_second_turn_with_its_retirement_in_whole_seconds() {
    use crate::entry::EntryStatus;
    let statement = "use cargo test in CI";
    let tag: crate::entry::TagName = "preferences".parse().unwrap();
    let project: crate::record::Project = "scribe".parse().unwrap();
    // Typed at .250 and retired at .800 of the same second; the store
    // keeps the retirement as whole seconds.
    let mut entry = super::entry("preferences-1", T0 - Duration::hours(1));
    entry.content = statement.to_owned();
    entry.status = EntryStatus::Retired;
    entry.inactive_since = Some(T0);
    entry.updated = T0;
    let typed = T0 + Duration::milliseconds(250);
    assert!(
        pass::covers_turn(&entry, statement, &tag, &project, typed),
        "a turn inside the retirement's second is covered"
    );
    assert!(
        !pass::covers_turn(
            &entry,
            statement,
            &tag,
            &project,
            T0 + Duration::seconds(1) + Duration::milliseconds(250)
        ),
        "a turn in the next second is not"
    );
    // An entry recovered with a sub-second creation covers a later turn of
    // its own second.
    let mut recovered = super::entry("preferences-2", T0 + Duration::milliseconds(900));
    recovered.content = statement.to_owned();
    assert!(pass::covers_turn(
        &recovered, statement, &tag, &project, typed
    ));
}

#[test]
fn covers_turn_ends_at_the_time_an_entry_left_active_not_its_last_update() {
    use crate::entry::EntryStatus;
    let statement = "use cargo test in CI";
    let tag: crate::entry::TagName = "preferences".parse().unwrap();
    let project: crate::record::Project = "scribe".parse().unwrap();
    let mut entry = super::entry("preferences-1", T0 - Duration::hours(3));
    entry.content = statement.to_owned();
    entry.status = EntryStatus::Retired;
    entry.inactive_since = Some(T0 - Duration::hours(1));
    // A topic edit or a filled cite after retirement.
    entry.updated = T0 + Duration::hours(1);
    assert!(
        !pass::covers_turn(&entry, statement, &tag, &project, T0),
        "a turn after the retirement is not covered by a later metadata write"
    );
    assert!(pass::covers_turn(
        &entry,
        statement,
        &tag,
        &project,
        T0 - Duration::hours(2)
    ));
}

#[test]
fn a_claim_needs_a_subject_and_a_statement_within_their_bounds() {
    use crate::models::{EntryCandidateAnswer, FiringTurn};
    let turns: Vec<Message> = (1..=4)
        .map(|n| {
            message(
                &format!("u-{n}"),
                Speaker::User,
                &format!("I prefer option number {n} for every build we run."),
            )
        })
        .collect();
    let conversation = pass::conversation(&turns);
    let mut request = pass::request(
        "fake",
        None,
        Vec::new(),
        pass::window(&conversation, None, WINDOW_CHARS),
        Vec::new(),
        Vec::new(),
        false,
    );
    request.candidate_limit = 3;
    request.candidate_turns = (1..=4)
        .map(|n| FiringTurn {
            message: format!("u-{n}"),
            tag: String::from("preferences"),
            trigger: String::from("I prefer"),
            matched: String::from("I prefer"),
        })
        .collect();
    let claim = |n: usize, statement: String, subject: String| EntryCandidateAnswer {
        quote: format!("I prefer option number {n} for every build we run."),
        message: format!("u-{n}"),
        tag: String::from("preferences"),
        statement,
        subject,
    };
    let mut a = answer();
    a.candidates = vec![
        // At the bounds, padded: kept, trimmed.
        claim(
            1,
            format!(" {} ", "s".repeat(300)),
            format!(" {} ", "t".repeat(80)),
        ),
        // A statement past 300 characters.
        claim(2, "s".repeat(301), String::from("the build")),
        // No subject: the window does not name what the turn is about.
        claim(
            3,
            String::from("Daniel prefers option 3."),
            String::from("  "),
        ),
        // A subject past 80 characters.
        claim(4, String::from("Daniel prefers option 4."), "t".repeat(81)),
    ];
    let distilled = pass::check(a, &conversation, &request).unwrap();
    assert_eq!(distilled.candidates.len(), 1, "{:?}", distilled.candidates);
    assert_eq!(distilled.candidates[0].message, "u-1");
    assert_eq!(distilled.candidates[0].statement, "s".repeat(300));
    assert_eq!(distilled.candidates[0].subject, "t".repeat(80));
}
