use super::super::redact::{entry_fields, proposal_fields, session_fields, tag_fields};
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
        older: "preferences-1".parse().unwrap(),
        newer: Some("preferences-2".parse().unwrap()),
        reason: String::new(),
        quotes: vec![
            Quote {
                entry: "preferences-1".parse().unwrap(),
                text: String::new(),
            },
            Quote {
                entry: "preferences-2".parse().unwrap(),
                text: String::new(),
            },
        ],
        state: ProposalState::Open,
        opened_at: T0,
        settled_at: None,
        settled_by: None,
        author: crate::record::Author::consistency(),
        updated_by: None,
        settled_author: None,
        closed_reason: None,
        instruction: Some(crate::Instruction {
            session: "s-1".parse().unwrap(),
            message: String::from("m-1"),
            at: T0,
            quote: Some(String::new()),
        }),
        candidate: Some(crate::EntryCandidate {
            tag: "preferences".parse().unwrap(),
            role: crate::Speaker::User,
            quote: String::new(),
            statement: Some(String::new()),
            subject: Some(String::new()),
            evidence: Some(String::new()),
            cite: entry("preferences-1", T0).cite.unwrap(),
            project: crate::Project::sanitized("scribe"),
            provider: crate::Label::sanitized("claude-code", "claude-code"),
            host: crate::Label::sanitized("host", "host"),
            admitted: Some(crate::Admitted {
                trigger: String::from("I prefer"),
                matched: String::new(),
                criteria: Some(String::new()),
            }),
        }),
    }
}

/// A session record with one term, so the term's fields are listed.
fn session_with_term() -> SessionRecord {
    let mut s = session("s-1", T0);
    s.active_change = Some(crate::activity::ActiveChange {
        at: T0,
        by: crate::record::Author::user(),
        route: crate::activity::ActiveRoute::Manual,
        reason: String::new(),
        cite: None,
    });
    s.terms.push(Term {
        term: String::new(),
        said: String::new(),
        message: String::from("m-1"),
    });
    s
}

/// An outcome with both free-text fields set, so both are listed. No
/// stored entry holds both (each state allows one); the redactor's field
/// list does not depend on the state.
fn outcome() -> crate::Outcome {
    crate::Outcome {
        state: crate::OutcomeState::Parked,
        reason: Some(String::new()),
        task: Some(String::new()),
    }
}

#[test]
fn the_redacted_field_inventory_is_exactly_the_fields_masked() {
    // A new free-text field must join both REDACTED_FIELDS and its record's
    // field list, or this fails.
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
    e.removal_instruction = e.instruction.clone();
    e.window = Some(crate::Window {
        turns: vec![crate::WindowTurn {
            role: crate::Speaker::User,
            message: None,
            at: T0,
            text: String::new(),
            cut: false,
        }],
        truncated: false,
        omitted: 0,
    });
    let mut s = session_with_term();
    let mut p = proposal();
    let mut names: Vec<&str> = entry_fields(&mut e).into_iter().map(|(n, _)| n).collect();
    names.extend(session_fields(&mut s).into_iter().map(|(n, _)| n));
    names.extend(proposal_fields(&mut p).into_iter().map(|(n, _)| n));
    let mut tag = store_tag();
    names.extend(tag_fields(&mut tag).into_iter().map(|(n, _)| n));
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
    let mut e = entry("preferences-1", T0);
    e.retirement_reason = Some(String::new());
    e.cite.as_mut().unwrap().cwd = String::from("/work/abc");
    redactor.entry(&mut e);
    assert_eq!(e.cite.as_ref().unwrap().cwd, "/work/abc");
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

fn assert_shape_masked(text: &str, secret: &str, marker: &str) {
    let masked = Redactor::default().mask(text);
    assert!(!masked.contains(secret), "secret survived: {masked}");
    assert!(masked.contains(marker), "no {marker} in: {masked}");
}

#[test]
fn a_bearer_token_in_an_authorization_header_is_masked() {
    let token = "abcdefghijklmnopqrstuvwxyz0123456789";
    assert_shape_masked(
        &format!("Authorization: Bearer {token}"),
        token,
        "[redacted:bearer]",
    );
    assert_eq!(
        Redactor::default().mask(&format!("Authorization: Bearer {token}")),
        "Authorization: Bearer [redacted:bearer]",
        "the header name is kept, the token is not"
    );
    assert_shape_masked(
        &format!("authorization: bearer {token}"),
        token,
        "[redacted:bearer]",
    );
}

#[test]
fn a_ghp_token_is_masked() {
    let token = format!("ghp_{}", "0123456789abcdefghij0123456789abcdef");
    assert_shape_masked(
        &format!("pushed with {token} yesterday"),
        &token,
        "[redacted:github-token]",
    );
}

#[test]
fn a_github_pat_token_is_masked() {
    let token = format!("github_pat_{}", "11ABCDEFG0a1b2c3d4e5f6g7h8i9j0k1l2");
    assert_shape_masked(
        &format!("token: {token}"),
        &token,
        "[redacted:github-token]",
    );
}

#[test]
fn an_sk_ant_key_is_masked() {
    let token = format!("sk-ant-{}", "api03-0123456789abcdefghij0123456789");
    assert_shape_masked(&format!("key {token}"), &token, "[redacted:api-key]");
}

#[test]
fn a_plain_sk_key_is_masked() {
    let token = format!("sk-{}", "proj-0123456789abcdefghij0123456789");
    assert_shape_masked(&format!("key {token}"), &token, "[redacted:api-key]");
}

#[test]
fn an_akia_access_key_id_is_masked() {
    assert_shape_masked(
        "aws key AKIAIOSFODNN7EXAMPLE here",
        "AKIAIOSFODNN7EXAMPLE",
        "[redacted:aws-access-key]",
    );
}

#[test]
fn slack_tokens_are_masked() {
    for prefix in ["xoxb-", "xoxp-"] {
        let token = format!("{prefix}{}", "123456789012-abcdefghij0123");
        assert_shape_masked(&format!("slack {token}"), &token, "[redacted:slack-token]");
    }
}

#[test]
fn a_pem_private_key_block_is_masked_whole() {
    let body = "MIIEvwIBADANBgkqhkiG9w0BAQEFAASC";
    let text = format!(
        "before\n-----BEGIN RSA PRIVATE KEY-----\n{body}\n-----END RSA PRIVATE KEY-----\nafter"
    );
    let masked = Redactor::default().mask(&text);
    assert!(!masked.contains(body), "{masked}");
    assert!(!masked.contains("PRIVATE KEY-----"), "{masked}");
    assert_eq!(masked, "before\n[redacted:private-key]\nafter");
}

#[test]
fn a_credential_shape_is_masked_in_a_stored_entry_field() {
    let token = format!("ghp_{}", "0123456789abcdefghij0123456789abcdef");
    let mut e = entry("preferences-1", T0);
    e.retirement_reason = Some(String::new());
    e.content = format!("echoed {token}");
    Redactor::default().entry(&mut e);
    assert!(!e.content.contains(&token), "{}", e.content);
    assert!(e.content.contains("[redacted:github-token]"));
}

#[test]
fn ordinary_words_and_short_lookalikes_survive_shape_masking() {
    let redactor = Redactor::default();
    for text in [
        "the task-pilot routine and session-pass keep working",
        "a bearer of good news about tokens and private keys",
        "discussing sk-ant- and ghp_ prefixes masks nothing by itself",
        "short lookalikes: sk-short, ghp_short, AKIA123, xoxb-short",
        "an identifier like task-pilot-1234567890 is not a credential",
    ] {
        assert_eq!(redactor.mask(text), text, "{text}");
    }
}

#[test]
fn unresolved_redact_env_names_only_the_unset_redact_names() {
    use super::super::redact::unresolved_redact_env;
    let config = Config::parse("[redact]\nenv = [\"GITHUB_TOKEN\", \"EXTRA_TOKEN\"]\n").unwrap();
    assert_eq!(
        unresolved_redact_env(&config, |name| {
            (name == "GITHUB_TOKEN").then(|| String::from("a long enough value"))
        }),
        ["EXTRA_TOKEN"]
    );
    assert_eq!(
        unresolved_redact_env(&config, |_| Some(String::new())),
        ["GITHUB_TOKEN", "EXTRA_TOKEN"],
        "an empty value counts as unset"
    );
    assert!(
        unresolved_redact_env(&Config::default(), |_| None).is_empty(),
        "the default credentials are alternatives, not [redact] env names"
    );
}

#[test]
fn a_masked_segment_heading_stays_within_its_limit() {
    let redactor = Redactor::new([(String::from("TOKEN_WITH_A_LONG_NAME"), SECRET.to_owned())]);
    let mut s = session("s-1", T0);
    s.log[0].heading = format!("{} {SECRET}", "h".repeat(60));
    redactor.session(&mut s);
    let heading = &s.log[0].heading;
    assert!(!heading.contains(SECRET), "{heading}");
    assert_eq!(
        heading.chars().count(),
        crate::session::SEGMENT_HEADING_CHARS
    );
    assert!(crate::session::check_log(&s).is_ok());
}

fn store_tag() -> crate::StoreTag {
    crate::StoreTag {
        name: "notes".parse().unwrap(),
        aliases: Vec::new(),
        description: String::new(),
        stale_after_days: None,
        author: crate::Author::agent("opus".parse().unwrap()),
        updated_by: None,
        created: T0,
        updated: T0,
    }
}

#[test]
fn tag_description_is_redacted_at_the_same_boundary() {
    let redactor = Redactor::new([(String::from("TOKEN"), SECRET.to_owned())]);
    let mut tag = store_tag();
    tag.description = SECRET.to_owned();
    redactor.tag(&mut tag);
    assert_eq!(tag.description, "[redacted:TOKEN]");
}
