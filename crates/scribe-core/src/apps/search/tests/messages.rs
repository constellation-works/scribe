//! The search hits made of stored messages.

use crate::entry::Speaker;
use crate::message::{MessageMatch, MessageSearch};
use crate::record::{DateRange, Project};
use crate::search::{self, SearchQuery};
use scribe_store::fixtures::{T0, session};
use std::num::NonZeroUsize;

fn query(terms: &str) -> SearchQuery {
    SearchQuery {
        terms: terms.to_owned(),
        kind: Some(crate::search::RecordKind::Message),
        tag: None,
        project: None,
        topics: Vec::new(),
        dates: DateRange::default(),
        include_hidden: false,
        limit: NonZeroUsize::new(20).unwrap(),
    }
}

fn found(session: &str, id: &str, score: f64) -> MessageMatch {
    MessageMatch {
        session: session.parse().unwrap(),
        message: id.to_owned(),
        role: Speaker::User,
        at: T0,
        text: String::from("keep the merge queue"),
        score,
        group_project: None,
    }
}

#[test]
fn message_hits_filter_through_their_session_and_hide_an_inactive_one() {
    let active = session("s-1", T0);
    let mut inactive = session("s-2", T0);
    inactive.active = false;
    let mut other = session("s-3", T0);
    other.project = Project::sanitized("orbit");
    let stored = MessageSearch {
        matches: vec![
            found("s-1", "m-1", 2.5),
            found("s-2", "m-2", 2.0),
            found("s-3", "m-3", 1.5),
            found("s-9", "m-9", 1.0),
        ],
        read_only_copy: false,
    };
    let sessions = [active, inactive, other];
    let all = search::messages(&query("merge"), &stored, &sessions).unwrap();
    let ids: Vec<_> = all
        .hits
        .iter()
        .map(|h| h.message.clone().unwrap())
        .collect();
    assert_eq!(ids, ["m-1", "m-3"], "no record for s-9; s-2 hidden");
    assert_eq!(all.hidden_matches, 1);
    assert_eq!(all.hits[0].cite, "session:s-1");
    assert_eq!(all.hits[0].kind.as_str(), "message");

    let mut scoped = query("merge");
    scoped.project = Some(Project::sanitized("orbit"));
    scoped.include_hidden = true;
    let hits = search::messages(&scoped, &stored, &sessions).unwrap().hits;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message.as_deref(), Some("m-3"));
    assert!(search::messages(&query("the"), &stored, &sessions).is_err());
}
