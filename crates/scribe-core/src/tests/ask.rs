//! `ask` over fixture records, a fake `Models` that counts its calls and a
//! fake task source (2_design.md §6).

use super::{T0, entry, session};
use crate::ask::{
    self, Answer, Corpus, Found, Lookup, NoTasks, Question, RecordKind, Scope, Status, Task,
    TaskError, TaskQuery, Tasks,
};
use crate::entry::{ContentFrom, Entry, EntryStatus, Speaker};
use crate::models::{
    AskAnswer, AskRequest, ClaimAnswer, ConflictAnswer, ModelError, Models, SessionAnswer,
    SessionRequest,
};
use crate::session::{SessionRecord, Term};
use crate::tag::TagTable;
use std::cell::RefCell;
use time::Duration;

/// Answers every question with `reply` and keeps each request.
struct FakeModels {
    reply: AskAnswer,
    calls: RefCell<Vec<AskRequest>>,
}

impl FakeModels {
    fn answering(reply: AskAnswer) -> Self {
        Self {
            reply,
            calls: RefCell::new(Vec::new()),
        }
    }

    fn silent() -> Self {
        Self::answering(AskAnswer::default())
    }

    fn calls(&self) -> usize {
        self.calls.borrow().len()
    }

    fn cites(&self) -> Vec<String> {
        self.calls.borrow()[0]
            .evidence
            .iter()
            .map(|e| e.cite.clone())
            .collect()
    }
}

impl Models for FakeModels {
    fn session(&self, _: &SessionRequest) -> Result<SessionAnswer, ModelError> {
        Err(ModelError::Unavailable(String::from("not in this test")))
    }

    fn ask(&self, request: &AskRequest) -> Result<AskAnswer, ModelError> {
        self.calls.borrow_mut().push(request.clone());
        Ok(self.reply.clone())
    }
}

/// Returns `tasks` (or fails) and keeps each query.
struct FakeTasks {
    tasks: Result<Vec<Task>, TaskError>,
    queries: RefCell<Vec<TaskQuery>>,
}

impl FakeTasks {
    fn new(tasks: Vec<Task>) -> Self {
        Self {
            tasks: Ok(tasks),
            queries: RefCell::new(Vec::new()),
        }
    }

    fn failing() -> Self {
        Self {
            tasks: Err(TaskError(String::from("down"))),
            queries: RefCell::new(Vec::new()),
        }
    }
}

impl Tasks for FakeTasks {
    fn search(&self, query: &TaskQuery) -> Result<Vec<Task>, TaskError> {
        self.queries.borrow_mut().push(query.clone());
        self.tasks.clone()
    }
}

fn decision(id: &str, days: i64, content: &str) -> Entry {
    let mut e = entry(id, T0 + Duration::days(days));
    e.content = content.to_owned();
    e.said = format!("#scribe preferences: {content}");
    e
}

fn record(id: &str, days: i64, title: &str) -> SessionRecord {
    let mut r = session(id, T0 + Duration::days(days));
    r.title = title.to_owned();
    r
}

fn task(id: &str, days: i64, title: &str, summary: &str) -> Task {
    Task {
        id: id.to_owned(),
        title: title.to_owned(),
        status: String::from("done"),
        tags: vec![String::from("orchestration"), String::from("on-call")],
        updated: Some(T0 + Duration::days(days)),
        description: String::from("the brief"),
        execution_summary: summary.to_owned(),
    }
}

fn question(text: &str) -> Question {
    Question {
        text: text.to_owned(),
        scope: Scope::default(),
        include_hidden: false,
    }
}

fn run<T: Tasks>(
    q: &Question,
    entries: &[Entry],
    sessions: &[SessionRecord],
    models: &FakeModels,
    tasks: &T,
) -> Answer {
    let tags = TagTable::defaults();
    let corpus = Corpus {
        entries,
        sessions,
        messages: &[],
        tags: &tags,
        user: &crate::voice::UserName::default(),
    };
    ask::answer(
        q,
        corpus,
        models,
        tasks,
        "claude-sonnet-5-5",
        &crate::voice::Voice::default(),
        T0 + Duration::days(30),
    )
    .unwrap()
}

fn claim(text: &str, quote: &str, cite: &str) -> ClaimAnswer {
    ClaimAnswer {
        text: text.to_owned(),
        quote: quote.to_owned(),
        cite: cite.to_owned(),
    }
}

fn answered(claims: Vec<ClaimAnswer>) -> AskAnswer {
    AskAnswer {
        status: String::from("answered"),
        answer: String::from("Squash-merge them."),
        claims,
        conflicts: Vec::new(),
    }
}

const SQUASH: &str = "squash-merge agent PRs; keep merge commits for releases";

#[test]
fn nothing_matching_is_no_record_without_a_model_call() {
    let models = FakeModels::silent();
    let entries = [decision("preferences-1", 0, SQUASH)];
    let a = run(&question("zebra quantum"), &entries, &[], &models, &NoTasks);
    assert_eq!(a.status, Status::NoRecord);
    assert_eq!(a.answer, ask::NO_RECORD);
    assert_eq!(a.evidence_considered, 0);
    assert_eq!(models.calls(), 0);
}

#[test]
fn a_question_of_only_stopwords_is_no_record_without_a_model_call() {
    let models = FakeModels::silent();
    let entries = [decision("preferences-1", 0, SQUASH)];
    let a = run(&question("what is it?"), &entries, &[], &models, &NoTasks);
    assert_eq!(a.status, Status::NoRecord);
    assert_eq!(models.calls(), 0);
}

#[test]
fn claims_whose_quote_or_cite_fails_are_dropped_and_counted() {
    let models = FakeModels::answering(answered(vec![
        claim("squash", "squash-merge agent PRs", "preferences-1"),
        claim("invented", "always rebase agent PRs", "preferences-1"),
        claim("uncited", "squash-merge agent PRs", "preferences-9"),
    ]));
    let entries = [decision("preferences-1", 0, SQUASH)];
    let a = run(
        &question("how do we merge agent PRs?"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(models.calls(), 1);
    assert_eq!(a.status, Status::Answered);
    assert_eq!(a.dropped_quotes, 2);
    assert_eq!(a.claims.len(), 1);
    assert_eq!(a.claims[0].cite, "preferences-1");
    assert_eq!(a.evidence_considered, 1);
}

#[test]
fn a_quote_matches_across_whitespace_and_unicode_forms() {
    let models = FakeModels::answering(answered(vec![claim(
        "squash",
        "squash-merge   agent\nPRs;",
        "preferences-1",
    )]));
    let entries = [decision("preferences-1", 0, SQUASH)];
    let a = run(
        &question("merge agent PRs"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(a.dropped_quotes, 0);
    assert_eq!(a.status, Status::Answered);
}

#[test]
fn every_claim_dropped_is_no_record() {
    let models = FakeModels::answering(answered(vec![claim(
        "invented",
        "always rebase agent PRs",
        "preferences-1",
    )]));
    let entries = [decision("preferences-1", 0, SQUASH)];
    let a = run(
        &question("merge agent PRs"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(a.status, Status::NoRecord);
    assert_eq!(a.dropped_quotes, 1);
    assert_eq!(a.evidence_considered, 1);
    assert!(a.claims.is_empty());
}

#[test]
fn two_disagreeing_entries_are_a_conflict_with_both_cites() {
    let models = FakeModels::answering(AskAnswer {
        status: String::from("conflict"),
        answer: String::from("The records disagree."),
        claims: vec![
            claim("squash", "squash-merge agent PRs", "preferences-1"),
            claim(
                "merge commit",
                "merge agent PRs with a merge commit",
                "preferences-2",
            ),
        ],
        conflicts: vec![
            ConflictAnswer {
                cites: vec![String::from("preferences-1"), String::from("preferences-2")],
                note: String::from("squash or merge commit"),
            },
            ConflictAnswer {
                cites: vec![String::from("preferences-1"), String::from("preferences-7")],
                note: String::from("one cite is not evidence"),
            },
        ],
    });
    let entries = [
        decision("preferences-1", 0, SQUASH),
        decision("preferences-2", 1, "merge agent PRs with a merge commit"),
    ];
    let a = run(
        &question("how do we merge agent PRs?"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(a.status, Status::Conflict);
    assert_eq!(a.conflicts.len(), 1);
    assert_eq!(
        a.conflicts[0].cites,
        vec![String::from("preferences-1"), String::from("preferences-2")]
    );
    assert_eq!(a.claims.len(), 2);
}

#[test]
fn hidden_entries_are_left_out_and_counted_unless_include_hidden() {
    let mut old = decision("preferences-1", 0, "merge agent PRs with a merge commit");
    old.status = EntryStatus::Superseded;
    let mut forgotten = decision("preferences-3", 0, "merge agent PRs by hand");
    forgotten.status = EntryStatus::Forgotten;
    let entries = [old, decision("preferences-2", 1, SQUASH), forgotten];

    let models = FakeModels::answering(answered(vec![claim(
        "squash",
        "squash-merge agent PRs",
        "preferences-2",
    )]));
    let a = run(
        &question("merge agent PRs"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(models.cites(), vec![String::from("preferences-2")]);
    assert_eq!(a.hidden_matches, 1);
    assert!(a.answer.ends_with(ask::HIDDEN_NOTE), "{}", a.answer);

    let models = FakeModels::answering(answered(vec![claim(
        "squash",
        "squash-merge agent PRs",
        "preferences-2",
    )]));
    let mut q = question("merge agent PRs");
    q.include_hidden = true;
    let a = run(&q, &entries, &[], &models, &NoTasks);
    let cites = models.cites();
    assert!(cites.contains(&String::from("preferences-1")), "{cites:?}");
    assert!(
        !cites.contains(&String::from("preferences-3")),
        "forgotten: {cites:?}"
    );
    assert_eq!(a.hidden_matches, 0);
    let status = &models.calls.borrow()[0].evidence[1].status;
    assert_eq!(status, "superseded");
}

#[test]
fn evidence_is_newest_first_and_labels_whose_words_they_are() {
    let mut said = decision(
        "preferences-2",
        2,
        "agent PRs are squash-merged by the crew",
    );
    said.content_from = ContentFrom::Message {
        role: Speaker::Assistant,
        message: String::from("m-1"),
        at: T0,
    };
    let entries = [decision("preferences-1", 0, SQUASH), said];
    let mut r = record("s-1", 1, "merge talk");
    r.summary = String::from("We discussed how agent PRs merge.");
    let models = FakeModels::silent();
    run(
        &question("merge agent PRs"),
        &entries,
        &[r],
        &models,
        &NoTasks,
    );
    let calls = models.calls.borrow();
    let evidence = &calls[0].evidence;
    let cites: Vec<&str> = evidence.iter().map(|e| e.cite.as_str()).collect();
    assert_eq!(cites, vec!["preferences-2", "session:s-1", "preferences-1"]);
    assert_eq!(evidence[0].whose, "assistant (recorded by the user)");
    assert_eq!(evidence[1].whose, "session summary");
    assert_eq!(evidence[2].whose, "The user");
    assert!(
        calls[0]
            .prompt()
            .contains("<question>\nmerge agent PRs\n</question>")
    );
}

#[test]
fn tasks_are_evidence_and_a_failing_task_source_costs_only_them() {
    let entries = [decision("preferences-1", 0, SQUASH)];
    let tasks = FakeTasks::new(vec![task("T-1", 3, "merge agent PRs faster", "")]);
    let models = FakeModels::silent();
    run(&question("merge agent PRs"), &entries, &[], &models, &tasks);
    assert!(models.cites().contains(&String::from("task:T-1")));
    assert_eq!(models.calls.borrow()[0].evidence[0].whose, "Orbit task");

    let models = FakeModels::answering(answered(vec![claim(
        "squash",
        "squash-merge agent PRs",
        "preferences-1",
    )]));
    let a = run(
        &question("merge agent PRs"),
        &entries,
        &[],
        &models,
        &FakeTasks::failing(),
    );
    assert_eq!(a.status, Status::Answered);
    assert_eq!(models.cites(), vec![String::from("preferences-1")]);
}

#[test]
fn a_tag_scope_keeps_only_that_tags_entries() {
    let mut pref = entry("rituals-1", T0);
    pref.content = String::from("agent PRs get short titles");
    let entries = [decision("preferences-1", 0, SQUASH), pref];
    let mut r = record("s-1", 1, "merge talk");
    r.summary = String::from("agent PRs");
    let tasks = FakeTasks::new(vec![task("T-1", 3, "agent PRs", "")]);
    let models = FakeModels::silent();
    let mut q = question("agent PRs");
    q.scope.tags = vec!["preferences".parse().unwrap()];
    run(&q, &entries, &[r], &models, &tasks);
    assert_eq!(models.cites(), vec![String::from("preferences-1")]);
    assert!(tasks.queries.borrow().is_empty(), "tasks were searched");
}

#[test]
fn a_kind_scope_keeps_only_that_kind() {
    let entries = [decision("preferences-1", 0, SQUASH)];
    let mut r = record("s-1", 1, "merge talk");
    r.summary = String::from("agent PRs");
    let models = FakeModels::silent();
    let mut q = question("agent PRs");
    q.scope.kind = Some(RecordKind::Session);
    run(&q, &entries, &[r], &models, &NoTasks);
    assert_eq!(models.cites(), vec![String::from("session:s-1")]);
}

#[test]
fn the_last_session_is_the_newest_record_returned_whole_without_a_model_call() {
    let sessions = [
        record("s-1", 5, "older"),
        record("s-2", 9, "newest"),
        record("s-3", 7, "middle"),
    ];
    let models = FakeModels::silent();
    let a = run(
        &question("can you fetch me last session summary?"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(models.calls(), 0);
    assert_eq!(a.status, Status::Record);
    assert_eq!(a.lookup, Some(Lookup::LastSession));
    assert_eq!(
        a.records,
        vec![Found::Session {
            record: Box::new(sessions[1].clone()),
            terms: Vec::new(),
        }]
    );
}

#[test]
fn the_last_session_for_a_project_is_the_newest_in_that_project() {
    let mut orbit = record("s-2", 9, "orbit work");
    orbit.project = "constellation-works/orbit".parse().unwrap();
    let sessions = [record("s-1", 5, "scribe work"), orbit];
    let models = FakeModels::silent();
    let a = run(
        &question("what was the latest session for scribe?"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::LastSession));
    assert_eq!(a.records[0].cite(), "session:s-1");
}

#[test]
fn a_session_named_by_id_or_title_is_returned() {
    let sessions = [
        record("s-1", 5, "the release checklist"),
        record("s-2", 9, "other"),
    ];
    let models = FakeModels::silent();
    let a = run(
        &question("show session:s-1 please"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::NamedSession));
    assert_eq!(a.records[0].cite(), "session:s-1");
    let a = run(
        &question("what came out of The Release Checklist?"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::NamedSession));
    assert_eq!(a.records[0].cite(), "session:s-1");
    assert_eq!(models.calls(), 0);
}

#[test]
fn open_next_steps_are_the_newest_records_with_open_items() {
    let mut a1 = record("s-1", 5, "a");
    a1.next_open = 2;
    let mut a2 = record("s-2", 9, "b");
    a2.next_open = 1;
    let done = record("s-3", 10, "c");
    let models = FakeModels::silent();
    let a = run(
        &question("what is pending?"),
        &[],
        &[a1, a2, done],
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::NextSteps));
    let cites: Vec<String> = a.records.iter().map(Found::cite).collect();
    assert_eq!(cites, vec!["session:s-2", "session:s-1"]);
    assert_eq!(models.calls(), 0);
}

#[test]
fn a_tags_entries_this_week_are_returned_newest_first() {
    // now is T0 + 30 days, a Tuesday; the week began on the Monday.
    let entries = [
        decision("preferences-1", 0, "old"),
        decision("preferences-2", 29, "monday's"),
        decision("preferences-3", 30, "today's"),
    ];
    let models = FakeModels::silent();
    let a = run(
        &question("preferences this week"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::TagPeriod));
    let cites: Vec<String> = a.records.iter().map(Found::cite).collect();
    assert_eq!(cites, vec!["preferences-3", "preferences-2"]);
    assert_eq!(models.calls(), 0);
}

#[test]
fn a_term_returns_the_sessions_that_define_it_with_their_spans() {
    let mut defining = record("s-1", 5, "levels");
    defining.terms = vec![
        Term {
            term: String::from("Graduate to  level 4"),
            said: String::from("graduate to level 4 means the crew merges alone"),
            message: String::from("m-1"),
        },
        Term {
            term: String::from("resident orchestrator"),
            said: String::from("a resident orchestrator stays on"),
            message: String::from("m-2"),
        },
    ];
    let other = record("s-2", 9, "other");
    let entries = [decision(
        "preferences-1",
        0,
        "we graduate to level 4 when CI is green for a month",
    )];
    let models = FakeModels::silent();
    let a = run(
        &question("graduate to level 4"),
        &entries,
        &[defining.clone(), other],
        &models,
        &NoTasks,
    );
    assert_eq!(models.calls(), 0);
    assert_eq!(a.status, Status::Record);
    assert_eq!(a.lookup, Some(Lookup::Term));
    assert_eq!(
        a.records[0],
        Found::Session {
            record: Box::new(defining.clone()),
            terms: vec![defining.terms[0].clone()],
        }
    );
    assert_eq!(a.records[1].cite(), "preferences-1");
    let a = run(
        &question("What did I mean by resident orchestrator?"),
        &[],
        &[defining],
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::Term));
}

#[test]
fn the_last_on_call_session_is_the_newest_orchestration_task_for_that_workspace() {
    let tasks = FakeTasks::new(vec![
        task("T-1", 1, "[on-call]: orbit - opus", "the first summary"),
        task("T-2", 5, "[on-call]: orbit - opus", "the newest summary"),
        task("T-3", 9, "[on-call]: orbit - opus", ""),
        task("T-4", 12, "[on-call]: scribe - opus", "scribe's"),
        task("T-5", 13, "orbitals [on-call]", "a near miss"),
    ]);
    let models = FakeModels::silent();
    let a = run(
        &question("last on-call session for orbit"),
        &[],
        &[],
        &models,
        &tasks,
    );
    assert_eq!(models.calls(), 0);
    assert_eq!(a.lookup, Some(Lookup::OrchestratorSession));
    let Found::Task(found) = &a.records[0] else {
        panic!("not a task: {:?}", a.records);
    };
    assert_eq!(found.id, "T-2");
    assert_eq!(found.execution_summary, "the newest summary");
    let queries = tasks.queries.borrow();
    assert_eq!(queries[0].tags, vec![String::from("orchestration")]);
    assert_eq!(queries[0].text, "orbit");
}

#[test]
fn a_lookup_that_finds_nothing_falls_through_to_retrieval() {
    let entries = [decision(
        "preferences-1",
        0,
        "the last session of a sprint is a demo",
    )];
    let models = FakeModels::silent();
    let a = run(
        &question("what happens in the last session of a sprint?"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, None);
    assert_eq!(models.calls(), 1);
    let models = FakeModels::silent();
    let a = run(
        &question("last on-call session for orbit"),
        &[],
        &[],
        &models,
        &FakeTasks::new(Vec::new()),
    );
    assert_eq!(a.lookup, None);
    assert_eq!(a.status, Status::NoRecord);
}

#[test]
fn a_question_is_one_to_two_thousand_characters() {
    assert!(ask::check_question("ok").is_ok());
    assert!(ask::check_question("  \n").is_err());
    assert!(ask::check_question(&"x".repeat(2_000)).is_ok());
    assert!(ask::check_question(&"x".repeat(2_001)).is_err());
}

#[test]
fn tokens_drop_stopwords_and_short_words_and_strip_suffixes() {
    assert_eq!(
        ask::tokens("What did we decide about merging the PRs?"),
        vec!["decid", "merg", "prs"]
    );
    assert_eq!(
        ask::tokens("release releases released releasing class classes"),
        vec!["releas", "releas", "releas", "releas", "class", "class"]
    );
}

#[test]
fn a_log_segment_is_evidence_of_its_own_cited_by_number() {
    let mut r = record("s-1", 1, "release planning");
    r.log[0].heading = String::from("Merge policy");
    r.log[0].text = String::from("Daniel chose to squash-merge agent PRs.");
    let models = FakeModels::answering(answered(vec![
        claim("squash", "squash-merge agent PRs", "session:s-1#1"),
        claim("squash again", "squash-merge agent PRs", "session:s-1"),
    ]));
    let a = run(&question("merge agent PRs"), &[], &[r], &models, &NoTasks);
    assert_eq!(models.cites(), vec![String::from("session:s-1#1")]);
    assert_eq!(a.status, Status::Answered);
    let kept: Vec<&str> = a.claims.iter().map(|c| c.cite.as_str()).collect();
    assert_eq!(
        kept,
        ["session:s-1#1"],
        "the header does not hold the quote"
    );
    assert_eq!(a.dropped_quotes, 1);
}

#[test]
fn a_segment_cite_cannot_quote_the_session_title() {
    let mut r = record("s-1", 1, "release planning");
    r.log[0].heading = String::from("Merge policy");
    r.log[0].text = String::from("Daniel chose to squash-merge agent PRs.");
    let models = FakeModels::answering(answered(vec![claim(
        "the session was release planning",
        "release planning",
        "session:s-1#1",
    )]));

    let a = run(
        &question("release squash-merge"),
        &[],
        &[r],
        &models,
        &NoTasks,
    );

    assert_eq!(a.status, Status::NoRecord);
    assert!(a.claims.is_empty());
    assert_eq!(a.dropped_quotes, 1);
}

#[test]
fn an_inactive_session_is_hidden_from_lookups_and_evidence_unless_include_hidden() {
    let mut old = record("s-2", 9, "merge agent PRs by hand");
    old.active = false;
    let current = record("s-1", 5, "merge agent PRs with squash");
    let sessions = [current, old];

    let models = FakeModels::silent();
    let a = run(
        &question("can you fetch me last session summary?"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(a.lookup, Some(Lookup::LastSession));
    assert_eq!(
        a.records[0].cite(),
        "session:s-1",
        "the inactive one is newer"
    );

    let models = FakeModels::silent();
    let a = run(
        &question("merge agent PRs"),
        &[],
        &sessions,
        &models,
        &NoTasks,
    );
    assert_eq!(models.cites(), vec![String::from("session:s-1")]);
    assert_eq!(a.hidden_matches, 1);

    let models = FakeModels::silent();
    let mut q = question("merge agent PRs");
    q.include_hidden = true;
    let a = run(&q, &[], &sessions, &models, &NoTasks);
    assert!(models.cites().contains(&String::from("session:s-2")));
    assert_eq!(a.hidden_matches, 0);
    let mut last = question("can you fetch me last session summary?");
    last.include_hidden = true;
    let a = run(&last, &[], &sessions, &FakeModels::silent(), &NoTasks);
    assert_eq!(a.records[0].cite(), "session:s-2");
}

#[test]
fn agent_evidence_cannot_be_presented_as_daniels_words_even_by_the_model() {
    let mut agent = decision("preferences-1", 0, SQUASH);
    agent.author = crate::Author::agent("daniel".parse().unwrap());
    agent.cite = None;
    agent.said.clear();
    agent.content_from = ContentFrom::Agent;
    let mut reply = answered(vec![claim(
        "Daniel said to squash",
        SQUASH,
        "preferences-1",
    )]);
    reply.answer = String::from("Daniel says to squash.");
    let models = FakeModels::answering(reply);
    let result = run(
        &question("how should we merge PRs?"),
        &[agent],
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(
        models.calls.borrow()[0].evidence[0].whose,
        "agent note (daniel)"
    );
    assert_eq!(result.claims[0].text, "agent note (daniel)");
    assert!(result.answer.starts_with("agent note (daniel):"));
    assert!(!result.answer.contains("Daniel says"));
}

#[test]
fn a_finding_is_presented_as_that_agents_claim_as_of_its_date_with_evidence() {
    let text = "the reviewer mix alone predicts a 7-point first-pass gap";
    let mut finding = decision("finding-1", 0, text);
    finding.author = crate::Author::agent("opus".parse().unwrap());
    finding.cite = None;
    finding.said.clear();
    finding.content_from = ContentFrom::Agent;
    finding.evidence = Some(String::from("exp-5342a4d6"));
    finding.as_of = Some(time::macros::date!(2026 - 11 - 14));
    let mut reply = answered(vec![claim("Daniel measured the gap", text, "finding-1")]);
    reply.answer = String::from("Daniel measured a 7-point gap.");
    let models = FakeModels::answering(reply);
    let result = run(
        &question("what does the reviewer mix predict for the first-pass gap?"),
        &[finding],
        &[],
        &models,
        &NoTasks,
    );
    let label = "agent claim (opus) as of 2026-11-14; evidence: exp-5342a4d6";
    assert_eq!(models.calls.borrow()[0].evidence[0].whose, label);
    assert_eq!(result.claims[0].text, label);
    assert!(result.answer.starts_with(label), "{}", result.answer);
    assert!(!result.answer.contains("Daniel measured"));
}

#[test]
fn daniels_evidence_wins_top_k_over_a_newer_identical_agent_note() {
    let mut entries: Vec<Entry> = (1..=ask::TOP_K)
        .map(|n| {
            let mut e = decision(&format!("preferences-{n}"), 0, SQUASH);
            e.said.clear();
            e
        })
        .collect();
    let mut agent = decision("preferences-21", 1, SQUASH);
    agent.author = crate::Author::agent("opus".parse().unwrap());
    agent.cite = None;
    agent.content_from = ContentFrom::Agent;
    agent.said.clear();
    entries.push(agent);
    let models = FakeModels::silent();
    run(
        &question("squash merge PRs"),
        &entries,
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(models.cites().len(), ask::TOP_K);
    assert!(
        !models.cites().iter().any(|id| id == "preferences-21"),
        "the agent note must lose the last retrieval slot despite its recency"
    );
}

#[test]
fn mixed_author_conflicts_keep_both_labels_and_a_nonempty_safe_answer() {
    let daniel = decision("preferences-1", 0, "squash merge all changes");
    let mut agent = decision("preferences-2", 1, "rebase merge all changes");
    agent.author = crate::Author::agent("opus".parse().unwrap());
    agent.cite = None;
    agent.said.clear();
    agent.content_from = ContentFrom::Agent;
    let models = FakeModels::answering(AskAnswer {
        status: String::from("conflict"),
        answer: String::from("Daniel says both."),
        claims: Vec::new(),
        conflicts: vec![ConflictAnswer {
            cites: vec!["preferences-1".to_owned(), "preferences-2".to_owned()],
            note: String::from("Daniel contradicts himself."),
        }],
    });
    let result = run(
        &question("how should we merge changes?"),
        &[daniel, agent],
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(result.status, Status::Conflict);
    assert!(
        result.conflicts[0]
            .note
            .contains("The user [preferences-1]")
    );
    assert!(
        result.conflicts[0]
            .note
            .contains("agent note (opus) [preferences-2]")
    );
    assert!(
        !result.answer.is_empty(),
        "a conflict without claims still needs a safe answer"
    );
    assert!(!result.answer.contains("Daniel says"));
}

/// An approved pass claim: scribe's statement as content, Daniel's
/// verbatim words in `quote` (2_design.md §11.1).
fn stated(id: &str, statement: &str, quote: &str) -> Entry {
    let mut e = decision(id, 0, statement);
    e.said = String::from("#scribe approve p-1");
    e.content_from = ContentFrom::Message {
        role: Speaker::User,
        message: String::from("u-1"),
        at: T0,
    };
    e.quote = Some(quote.to_owned());
    e.stated_by = Some(crate::Author::session_pass());
    e
}

const STATEMENT: &str =
    "Daniel prefers that session records keep the whole history in an append-only log.";
const HIS_WORDS: &str = "ok that makes sense, keep the whole history in an append-only log then.";

#[test]
fn a_stated_entry_is_scribes_statement_followed_by_daniels_labelled_words() {
    let models = FakeModels::answering(answered(Vec::new()));
    run(
        &question("how do session records keep their history?"),
        &[stated("preferences-1", STATEMENT, HIS_WORDS)],
        &[],
        &models,
        &NoTasks,
    );
    let item = &models.calls.borrow()[0].evidence[0];
    assert_eq!(item.whose, "scribe's statement of the user's turn");
    assert_eq!(
        item.text,
        format!("{STATEMENT}\nThe user's words: \"{HIS_WORDS}\"")
    );
    assert_eq!(item.quote.as_deref(), Some(HIS_WORDS));
}

#[test]
fn a_claim_is_daniels_only_when_its_quote_is_in_his_words_never_in_the_statement() {
    let mut reply = answered(vec![
        claim(
            "Daniel said to keep the history",
            "Daniel prefers that session records keep the whole history",
            "preferences-1",
        ),
        claim(
            "Daniel said it",
            "keep the whole history in an append-only log then",
            "preferences-1",
        ),
    ]);
    reply.answer = String::from("Daniel said session records keep the whole history.");
    let models = FakeModels::answering(reply);
    let result = run(
        &question("how do session records keep their history?"),
        &[stated("preferences-1", STATEMENT, HIS_WORDS)],
        &[],
        &models,
        &NoTasks,
    );
    assert_eq!(result.claims.len(), 2, "both quotes are in the item");
    // The span only in scribe's statement is never attributed to Daniel.
    assert_eq!(
        result.claims[0].text,
        "scribe's statement of the user's turn"
    );
    // The span in his quoted words is his.
    assert_eq!(result.claims[1].text, "The user");
    // The answer is built from those labels, not the model's prose.
    assert!(!result.answer.contains("Daniel said"), "{}", result.answer);
    assert_eq!(
        result.answer,
        "scribe's statement of the user's turn: \"Daniel prefers that session records keep the whole history\" [preferences-1]\n\
         The user: \"keep the whole history in an append-only log then\" [preferences-1]"
    );
}

#[test]
fn an_approved_assistant_context_entry_is_scribes_statement_with_the_assistants_words() {
    let context = "The user and the agent checked the drain stall. The agent found parallel drains share one auth token.";
    let assistant_words = "The root cause is that parallel drains share a single auth token.";
    let mut e = decision("notes-1", 0, context);
    e.author = crate::Author::assistant();
    e.said = String::from("#scribe approve p-1");
    e.content_from = ContentFrom::Message {
        role: Speaker::Assistant,
        message: String::from("a-1"),
        at: T0,
    };
    e.quote = Some(assistant_words.to_owned());
    e.stated_by = Some(crate::Author::session_pass());

    // Pre-format-24 approved assistant entry without context/stated_by stays an agent note.
    let mut legacy = decision("notes-2", 0, "parallel drains fail under concurrency");
    legacy.author = crate::Author::assistant();
    legacy.said = String::from("#scribe approve p-2");
    legacy.content_from = ContentFrom::Message {
        role: Speaker::Assistant,
        message: String::from("a-2"),
        at: T0,
    };

    let mut reply = answered(vec![
        claim(
            "Daniel found the token issue",
            "parallel drains share one auth token",
            "notes-1",
        ),
        claim(
            "Daniel said the root cause",
            "parallel drains share a single auth token",
            "notes-1",
        ),
    ]);
    reply.answer = String::from("Daniel found the root cause.");
    let models = FakeModels::answering(reply);
    let result = run(
        &question("why do parallel drains stall?"),
        &[e, legacy],
        &[],
        &models,
        &NoTasks,
    );

    let evidence = &models.calls.borrow()[0].evidence;
    let item = evidence.iter().find(|i| i.cite == "notes-1").unwrap();
    assert_eq!(item.whose, "scribe's statement of the assistant's turn");
    assert_eq!(
        item.text,
        format!("{context}\nThe assistant's words: \"{assistant_words}\"")
    );
    assert_eq!(item.quote.as_deref(), Some(assistant_words));
    let legacy_item = evidence.iter().find(|i| i.cite == "notes-2").unwrap();
    assert_eq!(legacy_item.whose, "agent note (assistant)");

    assert_eq!(result.claims.len(), 2);
    assert_eq!(
        result.claims[0].text,
        "scribe's statement of the assistant's turn"
    );
    assert_eq!(result.claims[1].text, "assistant (recorded by the user)");
    assert!(!result.answer.contains("Daniel found"), "{}", result.answer);
    assert!(!result.answer.contains("The user:"), "{}", result.answer);
    assert!(!result.answer.contains("agent note"), "{}", result.answer);
    assert_eq!(
        result.answer,
        "scribe's statement of the assistant's turn: \"parallel drains share one auth token\" [notes-1]\n\
         assistant (recorded by the user): \"parallel drains share a single auth token\" [notes-1]"
    );
}

#[test]
fn a_spoken_instruction_entry_labels_the_agents_statement_and_cites_users_words_in_ask() {
    let statement = "The user prefers concise progress updates during long tasks.";
    let his_words = "keep your progress updates concise while working on long tasks";
    let mut spoken = decision("preferences-1", 0, statement);
    spoken.author = crate::Author::agent("opus".parse().unwrap());
    spoken.said.clear();
    spoken.content_from = ContentFrom::Agent;
    spoken.cite = None;
    spoken.quote = Some(his_words.to_owned());
    spoken.stated_by = Some(crate::Author::agent("opus".parse().unwrap()));

    // A standard/handoff agent note with a quote but no stated_by stays an agent note.
    let mut note = decision("standard-1", 0, "Name the unit in every size.");
    note.author = crate::Author::agent("opus".parse().unwrap());
    note.said.clear();
    note.content_from = ContentFrom::Agent;
    note.cite = None;
    note.quote = Some(String::from("every size needs its unit"));

    let mut reply = answered(vec![
        claim(
            "The user wants concise updates",
            "concise progress updates during long tasks",
            "preferences-1",
        ),
        claim(
            "The user said it",
            "keep your progress updates concise",
            "preferences-1",
        ),
        claim(
            "The user set a standard",
            "Name the unit in every size.",
            "standard-1",
        ),
    ]);
    reply.answer = String::from("The user wants concise updates.");
    let models = FakeModels::answering(reply);
    let result = run(
        &question("how should progress updates look?"),
        &[spoken, note],
        &[],
        &models,
        &NoTasks,
    );

    let evidence = &models.calls.borrow()[0].evidence;
    let item = evidence.iter().find(|i| i.cite == "preferences-1").unwrap();
    assert_eq!(item.whose, "agent (opus)'s statement of the user's turn");
    assert_eq!(
        item.text,
        format!("{statement}\nThe user's words: \"{his_words}\"")
    );
    assert_eq!(item.quote.as_deref(), Some(his_words));

    assert_eq!(
        result.claims.len(),
        2,
        "standard is an action tag so skipped unless named"
    );
    assert_eq!(
        result.claims[0].text,
        "agent (opus)'s statement of the user's turn"
    );
    assert_eq!(result.claims[1].text, "The user");
    assert_eq!(
        result.answer,
        "agent (opus)'s statement of the user's turn: \"concise progress updates during long tasks\" [preferences-1]\n\
         The user: \"keep your progress updates concise\" [preferences-1]"
    );

    // Asking with `--tag standard` still treats `standard-1` as an ordinary agent note.
    let mut q_std = question("how should sizes be formatted?");
    q_std.scope.tags.push("standard".parse().unwrap());
    let models_std = FakeModels::answering(answered(vec![claim(
        "Unit rule",
        "Name the unit in every size.",
        "standard-1",
    )]));
    let std_entry = decision("standard-1", 0, "Name the unit in every size.");
    let mut std_agent = std_entry;
    std_agent.author = crate::Author::agent("opus".parse().unwrap());
    std_agent.said.clear();
    std_agent.content_from = ContentFrom::Agent;
    std_agent.cite = None;
    std_agent.quote = Some(String::from("every size needs its unit"));
    let res_std = run(&q_std, &[std_agent], &[], &models_std, &NoTasks);
    assert_eq!(res_std.claims[0].text, "agent note (opus)");
}

#[test]
fn naming_a_project_keeps_its_group_only_messages_next_to_its_entries() {
    let entries = [decision("preferences-1", 0, SQUASH)];
    let said = |session: &str, project: &str| crate::message::MessageMatch {
        session: session.parse().unwrap(),
        message: String::from("m-1"),
        role: Speaker::User,
        at: T0 + Duration::days(1),
        text: String::from("squash-merge agent PRs here"),
        score: 1.0,
        group_project: Some(project.parse().unwrap()),
    };
    let messages = [said("g-1", "scribe"), said("g-2", "orbit")];
    let tags = TagTable::defaults();
    let corpus = Corpus {
        entries: &entries,
        sessions: &[],
        messages: &messages,
        tags: &tags,
        user: &crate::voice::UserName::default(),
    };
    let models = FakeModels::silent();
    ask::answer(
        &question("how does scribe squash-merge agent PRs"),
        corpus,
        &models,
        &NoTasks,
        "claude-sonnet-5-5",
        &crate::voice::Voice::default(),
        T0 + Duration::days(30),
    )
    .unwrap();
    let mut cites = models.cites();
    cites.sort();
    assert_eq!(cites, ["preferences-1", "session:g-1#message:m-1"]);
}
