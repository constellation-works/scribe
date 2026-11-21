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
