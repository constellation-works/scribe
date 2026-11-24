//! The session answer's JSON schema (`crate::models::SESSION_SCHEMA`).

#[test]
fn the_session_schema_bounds_claims_to_six_of_680_characters_and_4080_per_answer() {
    // 2_design.md §15.3: at most 6 candidates, each a context of at most
    // 600 characters and a subject of at most 80, so the two add at most
    // 4,080 value characters to one answer.
    let schema: serde_json::Value = serde_json::from_str(crate::models::SESSION_SCHEMA).unwrap();
    let candidates = &schema["properties"]["candidates"];
    let fields = &candidates["items"]["properties"];
    let max = |v: &serde_json::Value| usize::try_from(v.as_u64().unwrap()).unwrap();
    let count = max(&candidates["maxItems"]);
    let context = max(&fields["context"]["maxLength"]);
    let subject = max(&fields["subject"]["maxLength"]);
    assert_eq!(
        (count, context, subject),
        (6, crate::CONTEXT_CHARS, crate::SUBJECT_CHARS)
    );
    assert_eq!(count * (context + subject), 4_080);
    assert!(fields.get("statement").is_none(), "{fields}");
    let required = candidates["items"]["required"].as_array().unwrap();
    assert!(
        required.contains(&serde_json::json!("context")),
        "{required:?}"
    );
}
