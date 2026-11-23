//! Deterministic checks for typed group additions.

use super::super::group_add::{self, run_with_transcript_reader};
use crate::app::Context;
use crate::commands::hook::{group_turns, payload::PromptPayload};
use crate::output::Output;
use scribe_core::SessionId;
use serde_json::json;

const A: &str = "00000000-0000-4000-8000-000000000001";
const B: &str = "00000000-0000-4000-8000-000000000002";
const C: &str = "00000000-0000-4000-8000-000000000003";

#[test]
fn removing_a_discovered_refused_entry_refuses_the_whole_addition() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let projects = home.join(".claude/projects");
    let regular_dir = projects.join("regular");
    let refused_dir = projects.join("refused");
    std::fs::create_dir_all(&regular_dir).unwrap();
    std::fs::create_dir_all(refused_dir.join(format!("{B}.jsonl"))).unwrap();
    let regular = regular_dir.join(format!("{B}.jsonl"));
    std::fs::write(
        &regular,
        format!(
            "{}\n",
            json!({
                "type": "user",
                "uuid": "mixed-turn",
                "timestamp": "2090-11-20T01:00:00Z",
                "cwd": "/work",
                "message": {"role": "user", "content": "unpersistedorchid typed"}
            })
        ),
    )
    .unwrap();

    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ctx = Context::at(root, Some(projects));
    let payload: PromptPayload = serde_json::from_value(json!({
        "session_id": A,
        "transcript_path": home.join(".claude/projects/regular").join(format!("{A}.jsonl")),
        "cwd": "/work",
        "prompt": "/scribe group add planning"
    }))
    .unwrap();
    let named = [B.parse::<SessionId>().unwrap(), C.parse().unwrap()];
    let session = A.parse::<SessionId>().unwrap();
    let refused_path = refused_dir.join(format!("{B}.jsonl"));

    let output = run_with_transcript_reader(
        "planning",
        &named,
        &payload,
        &session,
        &ctx,
        |provider_ctx, id, path| {
            assert_eq!(id.as_str(), B);
            assert_eq!(path, refused_path);
            std::fs::remove_dir(path).unwrap();
            group_turns::past(provider_ctx, id, path)
        },
    )
    .unwrap();
    let Output::Hook(reply) = output else {
        panic!("group add should return a hook reply");
    };
    let reason = reply.block.unwrap();
    assert!(reason.contains("group add refused"), "{reason}");
    assert!(reason.contains("nothing changed"), "{reason}");

    // The refused named session's surviving regular transcript was not
    // backfilled, and neither named session joined.
    assert!(ctx.store().groups().unwrap().is_empty());
    assert!(
        ctx.store()
            .search_messages(&[String::from("unpersistedorchid")])
            .unwrap()
            .matches
            .is_empty()
    );

    // A session that was genuinely absent at discovery remains membership only.
    let output = group_add::run("planning", &[named[1].clone()], &payload, &session, &ctx).unwrap();
    let Output::Hook(reply) = output else {
        panic!("group add should return a hook reply");
    };
    assert!(
        reply
            .block
            .as_deref()
            .is_some_and(|reason| reason.contains("membership only, no transcript on disk")),
        "{:?}",
        reply.block
    );
    let groups = ctx.store().groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members(), [named[1].clone()].into());
}
