use super::{T0, session};
use crate::entry::Speaker;
use crate::models::{PassMessage, SessionAnswer, TermAnswer};
use crate::pass::{self, Invalid, PassMeta, WINDOW_CHARS};
use crate::session::SessionStatus;
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
fn the_window_starts_after_scribed_through_and_drops_the_oldest_first() {
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
    assert_eq!(ids, ["m-3"]);
    assert_eq!(tight.dropped, 2);

    let huge = vec![plain("m-1", WINDOW_CHARS + 10)];
    let cut = pass::window(&huge, None, WINDOW_CHARS);
    assert!(cut.messages[0].text.ends_with(crate::models::CUT_MARK));
    assert_eq!(
        cut.messages[0].text.chars().count(),
        WINDOW_CHARS + crate::models::CUT_MARK.chars().count()
    );
}

fn answer() -> SessionAnswer {
    SessionAnswer {
        title: String::from("Planning the orchestrator"),
        summary: String::from("Daniel planned the resident orchestrator."),
        goals: String::from("Define the orchestrator."),
        outcome: String::from("A definition."),
        next: vec![String::from("write the design"), String::from(" ")],
        topics: vec![String::from("orchestration"), String::from("orchestration")],
        related: vec![String::from("task:ORB-1"), String::from("not a reference")],
        terms: Vec::new(),
    }
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
        // Daniel's words, though the model named the wrong message.
        term("resident orchestrator", DEFINED, "a-1"),
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
    let distilled = pass::check(a, &conversation).unwrap();
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
    over.summary = "s".repeat(1501);
    assert_eq!(
        pass::check(over, &conversation),
        Err(Invalid::OverBudget {
            field: "Summary",
            chars: 1501,
            max: 1500
        })
    );
    let mut next = answer();
    next.next = vec!["n".repeat(999)];
    assert!(matches!(
        pass::check(next, &conversation),
        Err(Invalid::OverBudget { field: "next", .. })
    ));
    let mut title = answer();
    title.title = String::from("two\nlines");
    assert_eq!(pass::check(title, &conversation), Err(Invalid::Title));
    let mut heading = answer();
    heading.outcome = String::from("done\n## Next\nmore");
    assert_eq!(
        pass::check(heading, &conversation),
        Err(Invalid::Heading("Outcome"))
    );
    let mut topics = answer();
    topics.topics = vec![String::from("Continuous Integration")];
    assert!(matches!(
        pass::check(topics, &conversation),
        Err(Invalid::Topics(_))
    ));
    let mut many = answer();
    many.topics = (0..6).map(|n| format!("t{n}")).collect();
    assert!(matches!(
        pass::check(many, &conversation),
        Err(Invalid::Topics(_))
    ));
    let mut empty = answer();
    empty.summary = String::from("  ");
    assert_eq!(
        pass::check(empty, &conversation),
        Err(Invalid::EmptySummary)
    );
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
        pass::check(a, &conversation).unwrap(),
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
        pass::check(b, &conversation).unwrap(),
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
fn the_request_carries_the_previous_record_the_window_and_what_to_reuse() {
    let conversation = pass::conversation(&transcript());
    let previous = session("s-1", T0);
    let window = pass::window(&conversation, Some("a-1"), WINDOW_CHARS);
    let request = pass::request(
        "claude-haiku-5-5",
        Some(&previous),
        window,
        vec![String::from("ci")],
        vec![String::from("graduate to level 4")],
        true,
    );
    assert_eq!(request.budget, crate::models::SESSION_BUDGET);
    let prompt = request.prompt();
    for part in [
        "<message id=\"u-5\" role=\"user\">",
        "<pasted>\nerror: the pasted words are not mine at all\n</pasted>",
        "<message id=\"a-2\" role=\"assistant\">",
        "Existing topics: ci",
        "Existing terms: graduate to level 4",
        "## Summary\nsummary",
        "this is its final record",
    ] {
        assert!(prompt.contains(part), "{part:?} missing from:\n{prompt}");
    }
    assert!(!prompt.contains("<message id=\"u-1\""), "already folded in");
}
