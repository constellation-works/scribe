use crate::entry::Speaker;
use crate::transcript;

const TRANSCRIPT: &str = r#"{"type":"session-owner","sessionId":"s-1"}
{"type":"user","uuid":"u-1","timestamp":"2026-11-01T10:00:00.250Z","message":{"role":"user","content":"hello\n#scribe decisions: one"}}
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
                String::from("hello\n#scribe decisions: one")
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
fn an_empty_or_unreadable_transcript_has_no_messages() {
    assert!(transcript::parse(b"").is_empty());
    assert!(transcript::parse(&[0xff, 0xfe, b'\n']).is_empty());
}
