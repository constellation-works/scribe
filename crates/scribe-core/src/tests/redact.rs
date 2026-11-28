//! The redactor built from a `Config` (`Redactor::from_config`), whose
//! config left the store with `config` (ORB-27921 PR 5-13).

use crate::config::Config;
use crate::config::RedactorFromConfig;
use crate::{MIN_SECRET_CHARS, Redactor};
use scribe_store::fixtures::{T0, entry};
use scribe_store::testing::redact::{
    SECRET, entry_fields, entry_records, outcome, proposal, proposal_fields, session_fields,
    session_with_term,
};

#[test]
fn a_configured_secret_is_masked_in_every_covered_field() {
    let config = Config::parse("[redact]\nenv = [\"GITHUB_TOKEN\"]\n").unwrap();
    let redactor = Redactor::from_config(&config, |name| {
        (name == "GITHUB_TOKEN").then(|| SECRET.to_owned())
    });
    let leak = format!("the token is {SECRET}, keep it");
    let masked = "the token is [redacted:GITHUB_TOKEN], keep it";

    let mut e = entry("preferences-1", T0);
    e.retirement_reason = Some(String::new());
    e.quote = Some(String::new());
    e.evidence = Some(String::new());
    e.instruction = Some(crate::Instruction {
        session: "s-1".parse().unwrap(),
        message: String::from("m-1"),
        at: T0,
        quote: Some(String::new()),
    });
    e.outcome = Some(outcome());
    entry_records(&mut e);
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
    assert_eq!(
        unset.mask("ordinary stored text"),
        "ordinary stored text",
        "an unset redactor masks no configured value"
    );
    assert!(
        !unset.mask(SECRET).contains(SECRET),
        "a credential shape is masked even with no configured value"
    );
    let debug = format!("{redactor:?}");
    assert!(
        !debug.contains("abcdefgh"),
        "Debug never shows a value: {debug}"
    );
}
