//! Issue records (2_design.md §11.4): ids, the reopen on a recurrence,
//! numbering that never reuses a number, forgetting, and the bounds.

use super::T0;
use crate::issue::{
    CAUSE_NOT_FOUND, FIX_NOT_FOUND, GoneReason, Issue, IssueFilter, IssueId, IssuePatch, IssueRef,
    IssueStatus, NewIssue, NewOccurrence, Occurrence, is_reserved_tag,
};
use crate::record::{Author, AuthorName, Label, Project, Related, Topics};
use std::num::NonZeroU32;
use time::Duration;

fn opus() -> Author {
    Author::agent(AuthorName::try_from(String::from("opus")).unwrap())
}

fn n(v: u32) -> NonZeroU32 {
    NonZeroU32::new(v).unwrap()
}

pub(crate) fn new_occurrence(request: &str) -> NewOccurrence {
    NewOccurrence {
        date: T0.date(),
        host: Label::sanitized("dk-server-1", "host"),
        project: Project::sanitized("orbit"),
        session: "s-1".parse().unwrap(),
        request_message: String::from("m-1"),
        request: request.to_owned(),
        summary: String::from("The drain stopped on HTTP 429."),
        error_lines: vec![String::from("HTTP 429 secondary rate limit")],
        cause: None,
        fix: Some(String::from("Serialize pushes")),
        links: vec![
            "task:ORB-1".parse().unwrap(),
            "session:s-1".parse().unwrap(),
        ],
    }
}

/// An open issue with one occurrence, every free-text field set.
pub(crate) fn sample() -> Issue {
    let mut issue = Issue {
        id: "issue-1".parse().unwrap(),
        title: String::from("GitHub API rate limit"),
        status: IssueStatus::Open,
        signature: vec![String::from("HTTP 429")],
        projects: Vec::new(),
        hosts: Vec::new(),
        topics: Topics::default(),
        related: Vec::new(),
        author: opus(),
        updated_by: None,
        created: T0,
        updated: T0,
        cause: String::from(CAUSE_NOT_FOUND),
        fix: String::from(FIX_NOT_FOUND),
        occurrences: Vec::new(),
        gone: Vec::new(),
    };
    let occurrence = new_occurrence("why does the drain fail")
        .check(opus())
        .unwrap();
    issue.occur(occurrence, opus(), T0);
    issue
}

fn occurrence(request: &str) -> Occurrence {
    new_occurrence(request).check(opus()).unwrap()
}

#[test]
fn ids_and_refs_parse_strictly_and_print_back() {
    let id: IssueId = "issue-12".parse().unwrap();
    assert_eq!(id.to_string(), "issue-12");
    assert_eq!(id.number(), 12);
    for bad in [
        "issue-0",
        "issue-",
        "issues-1",
        "Issue-1",
        "issue-01",
        "issue-1x",
        "preferences-1",
    ] {
        assert!(bad.parse::<IssueId>().is_err(), "{bad}");
    }
    let r: IssueRef = "issue-3#2".parse().unwrap();
    assert_eq!((r.issue.number(), r.occurrence), (3, Some(n(2))));
    assert_eq!(r.to_string(), "issue-3#2");
    assert_eq!("issue-3".parse::<IssueRef>().unwrap().occurrence, None);
    for bad in ["issue-3#0", "issue-3#", "issue-3#x"] {
        assert!(bad.parse::<IssueRef>().is_err(), "{bad}");
    }
    let ids = ["issue-1".parse().unwrap(), "issue-4".parse().unwrap()];
    assert_eq!(IssueId::next_in(&ids).to_string(), "issue-5");
}

#[test]
fn an_occurrence_on_a_resolved_issue_reopens_it_and_is_marked_recurred() {
    let mut issue = sample();
    let second = issue.occur(occurrence("again"), opus(), T0 + Duration::hours(1));
    assert_eq!(second, n(2));
    assert_eq!(issue.status, IssueStatus::Open);
    assert!(!issue.occurrence(second).unwrap().recurred);
    for status in [IssueStatus::Mitigated, IssueStatus::Open] {
        issue.status = status;
        let k = issue.occur(occurrence("again"), opus(), T0);
        assert!(!issue.occurrence(k).unwrap().recurred, "{status}");
    }
    issue.status = IssueStatus::Resolved;
    let third = issue.occur(occurrence("back"), opus(), T0 + Duration::hours(2));
    assert_eq!(issue.status, IssueStatus::Open);
    assert!(issue.occurrence(third).unwrap().recurred);
    assert_eq!(issue.occurrences[0].n, third, "newest first");
    assert_eq!(issue.updated, T0 + Duration::hours(2));
}

#[test]
fn a_number_moved_or_forgotten_is_never_given_again() {
    let mut issue = sample();
    issue.occur(occurrence("two"), opus(), T0);
    let gone = issue
        .take(
            n(2),
            crate::issue::Gone {
                n: n(2),
                reason: GoneReason::Moved,
                to: Some(String::from("issue-2#1")),
            },
        )
        .unwrap();
    assert_eq!(gone.request, "two");
    assert_eq!(issue.next_occurrence(), n(3));
    let err = issue.no_occurrence(n(2)).to_string();
    assert!(err.contains("moved to issue-2#1"), "{err}");
    assert!(issue.take(n(2), issue.gone[0].clone()).is_err());
}

#[test]
fn forgetting_blanks_every_word_and_keeps_the_numbers() {
    let mut issue = sample();
    issue.topics = Topics::try_from(vec!["github".parse().unwrap()]).unwrap();
    assert!(issue.forget(Author::daniel(), T0));
    assert_eq!(issue.status, IssueStatus::Forgotten);
    assert!(issue.title.is_empty() && issue.cause.is_empty() && issue.fix.is_empty());
    assert!(issue.signature.is_empty() && issue.occurrences.is_empty());
    assert!(issue.topics.as_slice().is_empty());
    assert!(issue.projects.is_empty() && issue.related.is_empty());
    assert_eq!(issue.gone.len(), 1);
    assert_eq!(issue.gone[0].reason, GoneReason::Forgotten);
    assert_eq!(issue.next_occurrence(), n(2));
    assert!(!issue.forget(Author::daniel(), T0), "forgotten once");
}

#[test]
fn an_occurrence_is_bounded_flattened_and_linked_without_its_own_session() {
    let o = occurrence("why  does\nthe drain fail");
    assert_eq!(o.request, "why does the drain fail");
    assert_eq!(o.cause, CAUSE_NOT_FOUND);
    assert_eq!(o.fix, "Serialize pushes");
    assert_eq!(o.links, vec!["task:ORB-1".parse::<Related>().unwrap()]);

    let refused = |edit: &dyn Fn(&mut NewOccurrence)| {
        let mut new = new_occurrence("why does the drain fail");
        edit(&mut new);
        new.check(opus()).unwrap_err().code()
    };
    assert_eq!(refused(&|o| o.summary = "x".repeat(1201)), "invalid_text");
    assert_eq!(
        refused(&|o| o.summary = String::from("a\n### 2. heading")),
        "invalid_text"
    );
    assert_eq!(refused(&|o| o.summary = String::from("  ")), "invalid_text");
    assert_eq!(refused(&|o| o.request = "x".repeat(1201)), "invalid_text");
    assert_eq!(
        refused(&|o| o.error_lines = vec![String::from("a\nb")]),
        "invalid_text"
    );
    assert_eq!(
        refused(&|o| o.error_lines = vec!["x".repeat(301)]),
        "invalid_text"
    );
    assert_eq!(
        refused(&|o| o.error_lines = vec![String::from("e"); 11]),
        "invalid_text"
    );
    assert_eq!(
        refused(&|o| o.cause = Some("x".repeat(601))),
        "invalid_text"
    );
}

#[test]
fn a_new_issue_and_a_patch_are_bounded() {
    let new = |title: &str, signature: Vec<String>| NewIssue {
        title: title.to_owned(),
        signature,
        topics: Topics::default(),
        cause: None,
        fix: None,
    };
    let (title, signature, cause, fix) = new("Rate limit", vec![String::from("429")])
        .check()
        .unwrap();
    assert_eq!(
        (title.as_str(), cause.as_str(), fix.as_str()),
        ("Rate limit", CAUSE_NOT_FOUND, FIX_NOT_FOUND)
    );
    assert_eq!(signature, ["429"]);
    assert!(
        new(&"x".repeat(121), vec![String::from("429")])
            .check()
            .is_err()
    );
    assert!(
        new("t", Vec::new()).check().is_err(),
        "a signature is required"
    );
    assert!(new("t", vec![String::from("s"); 6]).check().is_err());
    assert!(new("t", vec!["s".repeat(201)]).check().is_err());

    assert!(IssuePatch::default().check().is_err(), "an empty patch");
    let forgotten = IssuePatch {
        status: Some(IssueStatus::Forgotten),
        ..IssuePatch::default()
    };
    assert!(forgotten.check().is_err(), "only #scribe forget forgets");
    let mut issue = sample();
    let patch = IssuePatch {
        status: Some(IssueStatus::Resolved),
        ..IssuePatch::default()
    };
    assert!(patch.apply(&mut issue).unwrap());
    assert!(
        !patch.apply(&mut issue).unwrap(),
        "unchanged the second time"
    );
}

#[test]
fn the_list_hides_forgotten_issues_unless_asked_and_filters_by_place() {
    let mut forgotten = sample();
    forgotten.id = "issue-2".parse().unwrap();
    forgotten.forget(Author::daniel(), T0 + Duration::hours(1));
    let issues = vec![sample(), forgotten];
    let all = IssueFilter::default().select(issues.clone());
    assert_eq!(all.total, 1);
    let only = IssueFilter {
        status: Some(IssueStatus::Forgotten),
        ..IssueFilter::default()
    };
    assert_eq!(
        only.select(issues.clone()).items[0].id.to_string(),
        "issue-2"
    );
    let elsewhere = IssueFilter {
        project: Some(Project::sanitized("scribe")),
        ..IssueFilter::default()
    };
    assert_eq!(elsewhere.select(issues).total, 0);
}

#[test]
fn only_the_word_issue_is_reserved() {
    assert!(is_reserved_tag("issue") && is_reserved_tag("ISSUE"));
    assert!(!is_reserved_tag("issues") && !is_reserved_tag("myissue"));
}
