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
const D: &str = "00000000-0000-4000-8000-000000000004";
const E: &str = "00000000-0000-4000-8000-000000000005";

fn write_private(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn claude_turn_line(uuid: &str, text: &str) -> String {
    format!(
        "{}\n",
        json!({
            "type": "user",
            "uuid": uuid,
            "timestamp": "2090-11-20T01:00:00Z",
            "cwd": "/work",
            "message": {"role": "user", "content": text}
        })
    )
}

fn codex_rollout_body(id: &str, text: &str) -> String {
    [
        json!({"type": "session_meta", "payload": {"id": id, "source": "vscode", "thread_source": "user", "cwd": "/work"}}),
        json!({"timestamp": "2090-11-20T01:00:00Z", "type": "response_item", "payload": {
            "type": "message",
            "role": "user",
            "id": "u1",
            "content": [{"type": "input_text", "text": text}],
            "internal_chat_message_metadata_passthrough": {
                "turn_id": "t1",
                "content_item_kinds": ["user.text"],
                "content_item_metadata": [{"provenance": {"type": "user"}}]
            }
        }}),
    ]
    .iter()
    .map(|row| format!("{row}\n"))
    .collect()
}

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

#[test]
fn cross_provider_collision_stays_refused_when_discovered_refused_entry_becomes_regular() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let projects = home.join(".claude/projects");
    let regular_dir = projects.join("regular");
    let refused_dir = projects.join("refused");
    let codex_home = home.join(".codex");
    let codex_sessions = codex_home.join("sessions/2090/11/20");
    let codex_archived = codex_home.join("archived_sessions");

    // B collides across Claude Code and Codex and also has a third refused
    // entry that transitions into a regular transcript if opened.
    write_private(
        &regular_dir.join(format!("{B}.jsonl")),
        &claude_turn_line("b-claude", "collidingclaudeorchid"),
    );
    write_private(
        &codex_sessions.join(format!("rollout-2090-11-20T01-00-00-{B}.jsonl")),
        &codex_rollout_body(B, "collidingcodexorchid"),
    );
    let refused_b_path = refused_dir.join(format!("{B}.jsonl"));
    std::fs::create_dir_all(&refused_b_path).unwrap();

    // D has a regular Claude transcript and a refused Codex entry that
    // transitions into a regular Codex rollout before open.
    write_private(
        &regular_dir.join(format!("{D}.jsonl")),
        &claude_turn_line("d-claude", "oneproviderclaudeorchid"),
    );
    let refused_d_path = codex_archived.join(format!("rollout-2090-11-19T01-00-00-{D}.jsonl"));
    std::fs::create_dir_all(&refused_d_path).unwrap();

    // C (Claude) and E (Codex) are unrelated and nonambiguous.
    write_private(
        &regular_dir.join(format!("{C}.jsonl")),
        &claude_turn_line("c-claude", "nonambiguousclaudeorchid"),
    );
    write_private(
        &codex_sessions.join(format!("rollout-2090-11-20T01-00-00-{E}.jsonl")),
        &codex_rollout_body(E, "nonambiguouscodexorchid"),
    );

    let root = dir.path().join("root");
    std::fs::create_dir(&root).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ctx = Context::at(root, Some(projects)).with_codex_home(Some(codex_home));
    let payload: PromptPayload = serde_json::from_value(json!({
        "session_id": A,
        "transcript_path": regular_dir.join(format!("{A}.jsonl")),
        "cwd": "/work",
        "prompt": "/scribe group add planning"
    }))
    .unwrap();
    let session = A.parse::<SessionId>().unwrap();
    let id_b = B.parse::<SessionId>().unwrap();
    let id_c = C.parse::<SessionId>().unwrap();
    let id_d = D.parse::<SessionId>().unwrap();
    let id_e = E.parse::<SessionId>().unwrap();

    // Case 1: B had two regular transcripts across providers plus a refused
    // entry that becomes a regular transcript if read_past reaches it.
    let named_bc = [id_b.clone(), id_c.clone()];
    let output = run_with_transcript_reader(
        "planning",
        &named_bc,
        &payload,
        &session,
        &ctx,
        |provider_ctx, id, path| {
            if path == refused_b_path {
                std::fs::remove_dir(path).unwrap();
                write_private(path, &claude_turn_line("b-refused", "transitionedorchid"));
            }
            group_turns::past(provider_ctx, id, path)
        },
    )
    .unwrap();
    let Output::Hook(reply) = output else {
        panic!("group add should return a hook reply");
    };
    let reason = reply.block.unwrap();
    assert!(reason.contains("group add refused"), "{reason}");
    assert!(
        reason.contains("exists in multiple transcript providers"),
        "{reason}"
    );
    assert!(reason.contains("nothing changed"), "{reason}");
    assert!(ctx.store().groups().unwrap().is_empty());

    // Case 2: D had one regular Claude transcript and one refused Codex entry
    // that becomes a readable Codex rollout before the checked open.
    let named_dc = [id_d.clone(), id_c.clone()];
    let output = run_with_transcript_reader(
        "planning",
        &named_dc,
        &payload,
        &session,
        &ctx,
        |provider_ctx, id, path| {
            if path == refused_d_path {
                assert_eq!(id.as_str(), D);
                std::fs::remove_dir(path).unwrap();
                write_private(path, &codex_rollout_body(D, "codextransitionedorchid"));
            }
            group_turns::past(provider_ctx, id, path)
        },
    )
    .unwrap();
    let Output::Hook(reply) = output else {
        panic!("group add should return a hook reply");
    };
    let reason = reply.block.unwrap();
    assert!(reason.contains("group add refused"), "{reason}");
    assert!(
        reason.contains("exists in multiple transcript providers"),
        "{reason}"
    );
    assert!(reason.contains("nothing changed"), "{reason}");
    assert!(ctx.store().groups().unwrap().is_empty());

    for unpersisted in [
        "collidingclaudeorchid",
        "collidingcodexorchid",
        "transitionedorchid",
        "oneproviderclaudeorchid",
        "codextransitionedorchid",
        "nonambiguousclaudeorchid",
    ] {
        assert!(
            ctx.store()
                .search_messages(&[String::from(unpersisted)])
                .unwrap()
                .matches
                .is_empty(),
            "{unpersisted} should not be stored"
        );
    }

    // Unrelated nonambiguous sessions in both providers still join while B
    // (still colliding in Claude and Codex) and D remain on disk.
    let output = group_add::run(
        "planning",
        &[id_c.clone(), id_e.clone()],
        &payload,
        &session,
        &ctx,
    )
    .unwrap();
    let Output::Hook(reply) = output else {
        panic!("group add should return a hook reply");
    };
    let reason = reply.block.unwrap();
    assert!(reason.contains("group planning (g-1) opened"), "{reason}");
    assert!(
        reason.contains(&format!(
            "{C}: added; transcript on disk, 1 typed turn stored"
        )),
        "{reason}"
    );
    assert!(
        reason.contains(&format!(
            "{E}: added; transcript on disk, 1 typed turn stored"
        )),
        "{reason}"
    );
    let groups = ctx.store().groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members(), [id_c, id_e].into());
    for persisted in ["nonambiguousclaudeorchid", "nonambiguouscodexorchid"] {
        assert_eq!(
            ctx.store()
                .search_messages(&[String::from(persisted)])
                .unwrap()
                .matches
                .len(),
            1,
            "{persisted} should be stored"
        );
    }
}
