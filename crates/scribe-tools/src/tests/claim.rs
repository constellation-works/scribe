use super::T0;
use crate::text::claim as pass;
use time::Duration;

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
fn a_stated_entry_covers_a_line_matching_its_quote() {
    let quote = "always run the whole gate before a push";
    let tag: crate::entry::TagName = "preferences".parse().unwrap();
    let project: crate::record::Project = "scribe".parse().unwrap();
    let mut stated = super::entry("preferences-1", T0 - Duration::hours(3));
    stated.content =
        "Daniel requires the complete check suite to pass ahead of every upload.".to_owned();
    stated.quote = Some(quote.to_owned());
    stated.stated_by = Some(crate::record::Author::session_pass());
    let found = pass::covering_entry(std::slice::from_ref(&stated), quote, &tag, &project);
    assert_eq!(
        found.map(|e| e.id.to_string()).as_deref(),
        Some("preferences-1")
    );
    // A `quote` without `stated_by` (an agent's action entry) is not what
    // the entry says, so it covers nothing.
    let mut unstated = super::entry("preferences-2", T0 - Duration::hours(3));
    unstated.content = "something else entirely, in other words".to_owned();
    unstated.quote = Some(quote.to_owned());
    assert!(pass::covering_entry(&[unstated.clone()], quote, &tag, &project).is_none());
}
