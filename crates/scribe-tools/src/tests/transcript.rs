use crate::entry::Speaker;
use crate::transcript;
use serde_json::json;

const TRANSCRIPT: &str = r#"{"type":"session-owner","sessionId":"s-1"}
{"type":"user","uuid":"u-1","timestamp":"2026-11-01T10:00:00.250Z","message":{"role":"user","content":"hello\n#scribe preferences: one"}}
{"type":"assistant","uuid":"a-1","timestamp":"2026-11-01T10:00:05Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"Noted."},{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}
{"type":"user","uuid":"u-2","timestamp":"2026-11-01T10:00:06Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"output"}]}}
{"type":"user","uuid":"u-3","timestamp":"2026-11-01T10:00:07Z","isMeta":true,"message":{"role":"user","content":"<command-name>/clear</command-name>"}}
{"type":"assistant","uuid":"a-2","timestamp":"2026-11-01T10:00:08Z","isSidechain":true,"message":{"role":"assistant","content":"a subagent"}}
not json at all
{"type":"user","uuid":"u-4","timestamp":"2026-11-01T10:01:00Z","message":{"role":"user","content":[{"type":"text","text":"part one"},{"type":"image","source":{}},{"type":"text","text":"part two"}]}}
{"type":"attachment","uuid":"x","timestamp":"2026-11-01T10:01:00Z"}
"#;

#[test]
fn only_conversation_text_is_kept() {
    let messages = transcript::parse(TRANSCRIPT.as_bytes());
    let summary: Vec<(&str, Speaker, String)> = messages
        .iter()
        .map(|m| (m.uuid.as_str(), m.role, m.text()))
        .collect();
    assert_eq!(
        summary,
        [
            (
                "u-1",
                Speaker::User,
                String::from("hello\n#scribe preferences: one")
            ),
            ("a-1", Speaker::Assistant, String::from("Noted.")),
            ("u-4", Speaker::User, String::from("part one\n\npart two")),
        ]
    );
    assert_eq!(messages[2].blocks, ["part one", "part two"]);
    assert_eq!(
        messages[0].at,
        time::macros::datetime!(2026-11-01 10:00:00.25 UTC)
    );
}

#[test]
fn daniel_turns_drop_synthetic_lines_and_slash_scribe_and_a_survey_cuts_at_300() {
    let long = "w".repeat(400);
    let raw = format!(
        "{}\n{}\n{}\n{}\n",
        user("u-1", "2026-10-15T10:00:00Z", &long),
        user(
            "u-2",
            "2026-10-15T11:00:00Z",
            "<command-name>/clear</command-name>"
        ),
        user("u-3", "2026-10-15T12:00:00Z", "/scribe on"),
        user(
            "u-4",
            "2026-10-15T13:00:00Z",
            "keep <pasted_content> pasted note </pasted_content> this"
        ),
    );
    let messages = transcript::parse(raw.as_bytes());
    let turns = transcript::user_turns(&messages);
    assert_eq!(turns.len(), 2, "synthetic and /scribe are not Daniel's");
    assert_eq!(turns[1].own, "keep  this");
    assert_eq!(turns[1].pasted, ["pasted note"]);
    let survey = transcript::survey(raw.as_bytes());
    assert_eq!(survey.user_turns, 2);
    assert_eq!(
        survey.first_prompt.chars().count(),
        transcript::PROMPT_CHARS
    );
    assert!(!survey.first_prompt.contains('…'));
    assert_eq!(survey.first_turns.len(), 2);
    let now = time::macros::datetime!(2026-10-16 13:00:00 UTC);
    let ended = survey.ended.unwrap();
    assert!(transcript::is_day_old(ended, now));
    assert!(!transcript::is_day_old(
        ended,
        now - time::Duration::seconds(1)
    ));
}

#[test]
fn mention_counts_keep_zeros_and_a_tie_takes_the_smaller_slug() {
    use crate::record::Project;
    use std::collections::BTreeMap;
    let orbit: Project = "orbit".parse().unwrap();
    let scribe: Project = "scribe".parse().unwrap();
    let site: Project = "constellation-site".parse().unwrap();
    let mut projects = BTreeMap::new();
    projects.insert(
        orbit.clone(),
        vec![
            String::from("codebases/orbit"),
            String::from("codebases/orbit"),
        ],
    );
    projects.insert(scribe.clone(), vec![String::from("codebases/scribe")]);
    projects.insert(site.clone(), vec![String::from("constellation-site")]);
    let bytes = b"see codebases/orbit and codebases/orbit/src and codebases/scribe once. Codebases/orbit does not count";
    let counts = transcript::mention_counts(bytes, &projects);
    assert_eq!(
        counts[&orbit], 2,
        "duplicate paths count once; the second hit is a new span"
    );
    assert_eq!(counts[&scribe], 1);
    assert_eq!(
        counts[&site], 0,
        "a configured project with no hit is still present"
    );
    let cwd: Project = "scribe".parse().unwrap();
    assert_eq!(transcript::attribute(&counts, &cwd).as_str(), "orbit");
    let mut tied = counts.clone();
    tied.insert(orbit.clone(), 1);
    assert_eq!(
        transcript::attribute(&tied, &cwd).as_str(),
        "orbit",
        "an equal count takes the smaller slug, not the cwd"
    );
    tied.insert(orbit, 0);
    tied.insert(scribe.clone(), 0);
    tied.insert(site, 0);
    assert_eq!(transcript::attribute(&tied, &cwd).as_str(), "scribe");
}

fn user(uuid: &str, at: &str, text: &str) -> String {
    format!(
        r#"{{"type":"user","uuid":"{uuid}","timestamp":"{at}","message":{{"role":"user","content":{text}}}}}"#,
        text = serde_json::to_string(text).unwrap()
    )
}

#[test]
fn survey_with_cwd_matches_survey_and_cwd_from_one_pass() {
    let raw = format!(
        "{}\n{}\n",
        r#"{"type":"user","uuid":"u-1","timestamp":"2026-11-01T10:00:00Z","cwd":"/work/first","message":{"role":"user","content":"hello"}}"#,
        r#"{"type":"user","uuid":"u-2","timestamp":"2026-11-01T11:00:00Z","cwd":"/work/second","message":{"role":"user","content":"again"}}"#,
    );
    let (survey, cwd) = transcript::survey_with_cwd(raw.as_bytes());
    assert_eq!(survey, transcript::survey(raw.as_bytes()));
    assert_eq!(cwd.as_deref(), Some("/work/first"));
    assert_eq!(cwd, transcript::cwd(raw.as_bytes()));
    assert_eq!(survey.user_turns, 2);
}

#[test]
fn an_empty_or_unreadable_transcript_has_no_messages() {
    assert!(transcript::parse(b"").is_empty());
    assert!(transcript::parse(&[0xff, 0xfe, b'\n']).is_empty());
}

#[test]
fn a_replayed_line_keeps_its_first_position_and_counts_once() {
    let raw = [
        user("u-1", "2026-11-01T10:00:00Z", "first"),
        user("u-2", "2026-11-01T10:01:00Z", "second"),
        // Claude Code wrote both again on resume, then the session went on.
        user("u-1", "2026-11-01T10:00:00Z", "first"),
        user("u-2", "2026-11-01T10:01:00Z", "second"),
        user("u-3", "2026-11-02T09:00:00Z", "after the resume"),
    ]
    .join("\n");
    let messages = transcript::parse(raw.as_bytes());
    let ids: Vec<&str> = messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["u-1", "u-2", "u-3"]);
    assert_eq!(transcript::survey(raw.as_bytes()).user_turns, 3);
}

#[test]
fn a_slash_command_he_typed_is_his_in_its_args_and_its_tags_are_not() {
    let formation = "<command-message>formation</command-message>\n<command-name>/formation</command-name>\n<command-args>duo scribe (opus:high, sonnet:high)</command-args>";
    let raw = [
        user("u-1", "2026-11-23T10:00:00Z", formation),
        // The expanded skill body Claude Code writes after the command is meta.
        json!({"type": "user", "uuid": "u-2", "timestamp": "2026-11-23T10:00:01Z", "isMeta": true,
            "message": {"role": "user", "content": "Base directory for this skill: /skills/formation"}})
        .to_string(),
        user("u-3", "2026-11-23T10:01:00Z", "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"),
        user("u-4", "2026-11-23T10:02:00Z", "<command-name>/compact</command-name>\n<command-message>compact</command-message>\n<command-args>keep the plan</command-args>"),
        user("u-5", "2026-11-23T10:03:00Z", "<command-name>/scribe</command-name>\n<command-message>scribe</command-message>\n<command-args>group add s-2</command-args>"),
        user("u-6", "2026-11-23T10:04:00Z", "<command-name>/formation</command-name>\n<command-message>formation</command-message>\n<command-args>  </command-args>"),
        user("u-7", "2026-11-23T10:05:00Z", "<local-command-stdout>done</local-command-stdout>"),
    ]
    .join("\n");
    let messages = transcript::parse(raw.as_bytes());
    let ids: Vec<&str> = messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["u-1", "u-3", "u-4", "u-5", "u-6", "u-7"]);
    let own: Vec<String> = transcript::user_turns(&messages)
        .into_iter()
        .map(|turn| turn.own)
        .collect();
    assert_eq!(own, ["duo scribe (opus:high, sonnet:high)"]);
}

#[test]
fn a_settings_command_args_are_a_value_not_his_words() {
    // `/model` and the other settings commands write a value into a setting.
    // The desktop app writes the same `/model` line when another session
    // changes the model, so the line is not his turn whatever its origin.
    let settings = [
        ("/config", "config"),
        ("/effort", "effort"),
        ("/fast", "fast"),
        ("/model", "model"),
        ("/output-style", "output-style"),
        ("/permissions", "permissions"),
    ];
    for (name, message) in settings {
        let text = format!(
            "<command-name>{name}</command-name>\n<command-message>{message}</command-message>\n<command-args>claude-haiku-5-5</command-args>"
        );
        assert_eq!(transcript::slash_command_args(&text), None, "{name}");
    }
    let raw = [
        user(
            "u-model",
            "2026-11-23T21:50:41Z",
            "<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>claude-haiku-5-5</command-args>",
        ),
        // A command outside the list keeps its args as his words.
        user(
            "u-formation",
            "2026-11-23T21:51:00Z",
            "<command-name>/formation</command-name>\n<command-message>formation</command-message>\n<command-args>duo scribe</command-args>",
        ),
    ]
    .join("\n");
    let messages = transcript::parse(raw.as_bytes());
    let own: Vec<String> = transcript::user_turns(&messages)
        .into_iter()
        .map(|turn| turn.own)
        .collect();
    assert_eq!(own, ["duo scribe"]);
}

/// A `queued_command` attachment line (a message typed while the agent was
/// busy), with `extra` fields on the line itself.
fn queued_line(uuid: &str, attachment: serde_json::Value, extra: serde_json::Value) -> String {
    let mut line = json!({"type": "attachment", "uuid": uuid,
        "timestamp": "2026-11-23T11:00:00Z", "attachment": attachment});
    if let (Some(line), Some(extra)) = (line.as_object_mut(), extra.as_object()) {
        line.extend(extra.clone());
    }
    line.to_string()
}

/// The attachment of a message Daniel typed in the desktop app.
fn typed_queued(prompt: &str) -> serde_json::Value {
    json!({"type": "queued_command", "prompt": prompt,
        "origin": {"kind": "human"}, "humanTurn": true})
}

#[test]
fn a_queued_message_he_typed_is_a_user_turn_and_other_queued_lines_are_not() {
    let mut no_turn = typed_queued("no turn");
    no_turn["humanTurn"] = json!(false);
    let mut peer = typed_queued("peer");
    peer["origin"] = json!({"kind": "peer"});
    let mut no_origin = typed_queued("no origin");
    no_origin.as_object_mut().unwrap().remove("origin");
    let mut meta = typed_queued("attachment meta");
    meta["isMeta"] = json!(true);
    let mut hook = typed_queued("hook");
    hook["type"] = json!("hook_success");
    let raw = [
        queued_line(
            "q-typed",
            typed_queued("can you create the scribe group"),
            json!({"entrypoint": "claude-desktop"}),
        ),
        queued_line("q-no-turn", no_turn, json!({})),
        queued_line("q-peer", peer, json!({})),
        queued_line("q-no-origin", no_origin, json!({})),
        queued_line("q-meta", meta, json!({})),
        queued_line(
            "q-line-meta",
            typed_queued("line meta"),
            json!({"isMeta": true}),
        ),
        queued_line(
            "q-sidechain",
            typed_queued("sidechain"),
            json!({"isSidechain": true}),
        ),
        queued_line("q-hook", hook, json!({})),
    ]
    .join("\n");
    let messages = transcript::parse(raw.as_bytes());
    let ids: Vec<&str> = messages.iter().map(|m| m.uuid.as_str()).collect();
    assert_eq!(ids, ["q-typed"]);
    assert_eq!(messages[0].role, Speaker::User);
    assert_eq!(messages[0].text(), "can you create the scribe group");
    let own: Vec<String> = transcript::user_turns(&messages)
        .into_iter()
        .map(|turn| turn.own)
        .collect();
    assert_eq!(own, ["can you create the scribe group"]);
    assert_eq!(transcript::survey(raw.as_bytes()).user_turns, 1);
}
