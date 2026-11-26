//! Prompts name "the user" or `[user] name`, and `[instructions]` replaces
//! only a use's instruction text (crate::voice).

use crate::entry::Speaker;
use crate::models::{
    Budgets, FiringTurn, PROFILE_INSTRUCTIONS, PassMessage, ProfileRequest,
    SESSION_HEADER_INSTRUCTIONS, SESSION_INSTRUCTIONS, SessionHeader, SessionHeaderRequest,
    TagCriteria,
};
use crate::pass;
use crate::tag::TagTable;
use crate::voice::{ModelUse, UserName, Voice};
use std::time::Duration;

/// The shipped instruction text of `model_use`.
fn shipped(model_use: ModelUse) -> &'static str {
    match model_use {
        ModelUse::Session => SESSION_INSTRUCTIONS,
        ModelUse::SessionHeader => SESSION_HEADER_INSTRUCTIONS,
        ModelUse::Profile => PROFILE_INSTRUCTIONS,
    }
}

/// The prompt `model_use` sends, voiced with `voice`. The session request
/// asks for claims and memory, so every optional part of it is rendered.
fn prompt(model_use: ModelUse, voice: Voice, _shipped_tags: bool) -> String {
    let budget = Duration::from_secs(5);
    match model_use {
        ModelUse::Session => {
            let mut request = pass::request(pass::RequestInput {
                model: "m",
                previous: None,
                linked: Vec::new(),
                window: pass::Window {
                    messages: vec![PassMessage {
                        uuid: String::from("u-1"),
                        role: Speaker::User,
                        text: String::from("I prefer short replies"),
                        pasted: vec![String::from("a pasted log")],
                    }],
                    dropped: 0,
                },
                topics: Vec::new(),
                terms: Vec::new(),
                final_pass: false,
                voice,
            });
            request.candidate_limit = 1;
            request.write_limit = 1;
            request.candidate_turns = vec![
                FiringTurn {
                    message: String::from("u-1"),
                    tag: String::from("preferences"),
                    trigger: String::from("I prefer"),
                    matched: String::from("I prefer"),
                },
                FiringTurn {
                    message: String::from("u-1"),
                    tag: String::from("idea"),
                    trigger: String::from("what if we"),
                    matched: String::from("what if we"),
                },
            ];
            request.candidate_rules = vec![
                TagCriteria {
                    tag: String::from("preferences"),
                    criteria: None,
                    not: None,
                    writes: false,
                },
                TagCriteria {
                    tag: String::from("idea"),
                    criteria: None,
                    not: None,
                    writes: true,
                },
            ];
            request.memory = true;
            request.memory_limit = 3;
            request.prompt()
        }
        ModelUse::SessionHeader => SessionHeaderRequest {
            model: String::from("m"),
            budget,
            header: SessionHeader {
                title: String::from("title"),
                summary: String::from("summary"),
                goals: String::from("goals"),
                next: vec![String::from("next")],
            },
            budgets: Budgets {
                title: 80,
                summary: 1200,
                goals: 400,
                next: 1000,
            },
            correction: String::from("summary too long"),
            voice,
        }
        .prompt(),
        ModelUse::Profile => ProfileRequest {
            model: String::from("m"),
            budget,
            profile: crate::memory::profile::template(),
            corrections: None,
            hints: Vec::new(),
            sessions: Vec::new(),
            now: Vec::new(),
            passed: Vec::new(),
            milestones: Vec::new(),
            preferences: Vec::new(),
            voice,
        }
        .prompt(),
    }
}

fn named(name: &str) -> Voice {
    Voice {
        user: UserName::named(name).unwrap(),
        instructions: None,
    }
}

#[test]
fn no_shipped_prompt_names_a_person_and_each_says_the_user() {
    for model_use in ModelUse::ALL {
        let prompt = prompt(model_use, Voice::default(), true);
        assert!(!prompt.contains("Daniel"), "{model_use:?}: {prompt}");
        // The header retry rewrites prose and names no one.
        if model_use != ModelUse::SessionHeader {
            assert!(
                prompt.to_lowercase().contains("the user"),
                "{model_use:?}: {prompt}"
            );
        }
    }
}

#[test]
fn a_configured_name_replaces_the_user_in_every_prompt() {
    for model_use in ModelUse::ALL {
        let prompt = prompt(model_use, named("Sam"), false);
        assert!(!prompt.contains("Daniel"), "{model_use:?}: {prompt}");
        assert!(
            !prompt.to_lowercase().contains("the user"),
            "{model_use:?}: {prompt}"
        );
        if model_use != ModelUse::SessionHeader {
            assert!(prompt.contains("Sam"), "{model_use:?}: {prompt}");
        }
    }
}

#[test]
fn an_instruction_override_reaches_the_request_and_the_contract_stays() {
    for model_use in ModelUse::ALL {
        let custom = "Catch only what {user} says about release dates. {User} decides.";
        for user in [UserName::default(), UserName::named("Sam").unwrap()] {
            let default = prompt(
                model_use,
                Voice {
                    user: user.clone(),
                    instructions: None,
                },
                false,
            );
            let overridden = prompt(
                model_use,
                Voice {
                    user: user.clone(),
                    instructions: Some(custom.to_owned()),
                },
                false,
            );
            let shipped = user.fill(shipped(model_use));
            let filled = user.fill(custom);
            let contract = default
                .strip_prefix(&shipped)
                .unwrap_or_else(|| panic!("{model_use:?}: {default}"));
            assert_eq!(
                overridden.strip_prefix(&filled),
                Some(contract),
                "{model_use:?}"
            );
            assert!(!overridden.contains(&shipped), "{model_use:?}");
        }
    }
    let sam = UserName::named("Sam").unwrap();
    assert_eq!(
        sam.fill("Catch what {user} says. {User} decides."),
        "Catch what Sam says. Sam decides."
    );
    assert_eq!(
        UserName::default().fill("Catch what {user} says. {User} decides."),
        "Catch what the user says. The user decides."
    );
}

/// The session prompt with the shipped `preferences` and `standard`
/// criteria and exclusions as its candidate rules, voiced with `voice`.
fn session_with_shipped_rules(voice: Voice) -> String {
    let tags = TagTable::defaults();
    let rules = ["preferences", "standard"].map(|tag| {
        let def = tags
            .get(&crate::entry::TagName::try_from(tag.to_owned()).unwrap())
            .unwrap();
        TagCriteria {
            tag: tag.to_owned(),
            criteria: def.capture.criteria.as_ref().map(|r| r.value.clone()),
            not: def.capture.not.as_ref().map(|r| r.value.clone()),
            writes: tag == "idea",
        }
    });
    let mut request = pass::request(pass::RequestInput {
        model: "m",
        previous: None,
        linked: Vec::new(),
        window: pass::Window {
            messages: Vec::new(),
            dropped: 0,
        },
        topics: Vec::new(),
        terms: Vec::new(),
        final_pass: false,
        voice,
    });
    request.candidate_limit = 1;
    request.write_limit = 1;
    request.candidate_turns = vec![FiringTurn {
        message: String::from("u-1"),
        tag: String::from("idea"),
        trigger: String::from("what if we"),
        matched: String::from("what if we"),
    }];
    request.candidate_rules = rules.to_vec();
    request.prompt()
}

#[test]
fn shipped_tag_descriptions_and_criteria_name_the_configured_user_in_prompts() {
    for (voice, noun) in [(Voice::default(), "the user"), (named("Sam"), "Sam")] {
        let prompt = session_with_shipped_rules(voice.clone());
        assert!(
            !prompt.contains("{user}") && !prompt.contains("{User}"),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!("A standing rule from {noun} that applies")),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!("written across {noun}'s projects")),
            "{prompt}"
        );
    }
}

#[test]
fn an_instruction_override_cannot_drop_the_session_or_annotation_behaviour_rules() {
    let keep = "Keep what the record so far says unless the new messages change it.";
    let (model_use, rule) = (ModelUse::Session, keep);
    let overridden = prompt(
        model_use,
        Voice {
            user: UserName::default(),
            instructions: Some(String::from("Record only release dates.")),
        },
        false,
    );
    assert!(overridden.starts_with("Record only release dates."));
    assert!(!overridden.contains(shipped(model_use)), "{overridden}");
    assert!(overridden.contains(rule), "{model_use:?}: {overridden}");
    assert!(!shipped(model_use).contains(rule), "{model_use:?}");
}
