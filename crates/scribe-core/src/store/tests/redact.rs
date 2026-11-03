use super::super::redact::{entry_fields, proposal_fields, session_fields};
use crate::config::Config;
use crate::proposal::{Proposal, ProposalKind, ProposalState, Quote};
use crate::session::{SessionRecord, Term};
use crate::store::{MIN_SECRET_CHARS, REDACTED_FIELDS, Redactor};
use crate::tests::{T0, entry, session};

const SECRET: &str = "sk-live-0123456789abcdef";

fn proposal() -> Proposal {
    Proposal {
        id: "p-1".parse().unwrap(),
        kind: ProposalKind::Supersedes,
        older: "decisions-1".parse().unwrap(),
        newer: Some("decisions-2".parse().unwrap()),
        reason: String::new(),
        quotes: vec![
            Quote {
                entry: "decisions-1".parse().unwrap(),
                text: String::new(),
            },
            Quote {
                entry: "decisions-2".parse().unwrap(),
                text: String::new(),
            },
        ],
        state: ProposalState::Open,
        opened_at: T0,
        settled_at: None,
        settled_by: None,
    }
}

/// A session record with one term, so the term's fields are listed.
fn session_with_term() -> SessionRecord {
    let mut s = session("s-1", T0);
    s.terms.push(Term {
        term: String::new(),
        said: String::new(),
        message: String::from("m-1"),
    });
    s
}

#[test]
fn the_redacted_field_inventory_is_exactly_the_fields_masked() {
    // A new free-text field must join both REDACTED_FIELDS and its record's
    // field list, or this fails.
    let mut e = entry("decisions-1", T0);
    let mut s = session_with_term();
    let mut p = proposal();
    let mut names: Vec<&str> = entry_fields(&mut e).into_iter().map(|(n, _)| n).collect();
    names.extend(session_fields(&mut s).into_iter().map(|(n, _)| n));
    names.extend(proposal_fields(&mut p).into_iter().map(|(n, _)| n));
    names.dedup();
    assert_eq!(names, REDACTED_FIELDS);
}

#[test]
fn a_configured_secret_is_masked_in_every_covered_field() {
    let config = Config::parse("[redact]\nenv = [\"GITHUB_TOKEN\"]\n").unwrap();
    let redactor = Redactor::from_config(&config, |name| {
        (name == "GITHUB_TOKEN").then(|| SECRET.to_owned())
    });
    let leak = format!("the token is {SECRET}, keep it");
    let masked = "the token is [redacted:GITHUB_TOKEN], keep it";

    let mut e = entry("decisions-1", T0);
    for (_, text) in entry_fields(&mut e) {
        *text = leak.clone();
    }
    redactor.entry(&mut e);
    for (name, text) in entry_fields(&mut e) {
        assert_eq!(text, masked, "{name}");
    }

    let mut s = session_with_term();
    for (_, text) in session_fields(&mut s) {
        *text = leak.clone();
    }
    redactor.session(&mut s);
    for (name, text) in session_fields(&mut s) {
        assert_eq!(text, masked, "{name}");
    }

    let mut p = proposal();
    for (_, text) in proposal_fields(&mut p) {
        *text = leak.clone();
    }
    redactor.proposal(&mut p);
    for (name, text) in proposal_fields(&mut p) {
        assert_eq!(text, masked, "{name}");
    }
}

#[test]
fn identifiers_and_paths_are_not_masked() {
    let redactor = Redactor::new([(String::from("TOKEN"), String::from("/work/abc"))]);
    let mut e = entry("decisions-1", T0);
    e.cite.cwd = String::from("/work/abc");
    redactor.entry(&mut e);
    assert_eq!(e.cite.cwd, "/work/abc");
}

#[test]
fn short_unset_and_overlapping_values_are_handled() {
    let short = "x".repeat(MIN_SECRET_CHARS - 1);
    let redactor = Redactor::new([
        (String::from("SHORT"), short.clone()),
        (String::from("INNER"), String::from("abcdefgh")),
        (String::from("OUTER"), String::from("abcdefgh-ijkl")),
    ]);
    assert_eq!(redactor.mask(&short), short, "too short to mask");
    assert_eq!(
        redactor.mask("abcdefgh-ijkl"),
        "[redacted:OUTER]",
        "longest first"
    );
    assert_eq!(redactor.mask("abcdefgh"), "[redacted:INNER]");
    let unset = Redactor::from_config(&Config::default(), |_| None);
    assert_eq!(unset.mask(SECRET), SECRET);
    let debug = format!("{redactor:?}");
    assert!(
        !debug.contains("abcdefgh"),
        "Debug never shows a value: {debug}"
    );
}
