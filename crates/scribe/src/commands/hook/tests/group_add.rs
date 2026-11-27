//! Deterministic checks for typed group additions.

use crate::commands::hook::group_add::{self, run_with_transcript_reader};
use crate::commands::hook::group_turns;
use crate::output::Output;
use scribe_core::GroupVerbs;
use scribe_core::SessionId;
use scribe_core::apps::hook::PromptPayload;
use scribe_core::composition::Context;
use scribe_core::testing::StoreTestReads;
use serde_json::json;

const A: &str = "00000000-0000-4000-8000-000000000001";
const B: &str = "00000000-0000-4000-8000-000000000002";
const C: &str = "00000000-0000-4000-8000-000000000003";
const D: &str = "00000000-0000-4000-8000-000000000004";
const E: &str = "00000000-0000-4000-8000-000000000005";

/// A callback the read seam runs with the path about to be read.
type ReadCallback = Box<dyn FnMut(&std::path::Path)>;

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

/// Two project directories under one Claude projects root each hold a refused
/// entry named for B. Discovery must keep both, and the checked open must reach
/// each: a readable first entry cannot stand in for the other. The variants
/// make the first entry the checked open reaches readable, and then every
/// entry. Neither depends on which directory the enumeration yields first.
#[test]
fn two_refused_entries_in_one_root_refuse_the_whole_line_when_one_becomes_readable() {
    for every_path_transitions in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let projects = home.join(".claude/projects");
        let refused_a = projects.join("refused-a").join(format!("{B}.jsonl"));
        let refused_b = projects.join("refused-b").join(format!("{B}.jsonl"));
        std::fs::create_dir_all(&refused_a).unwrap();
        std::fs::create_dir_all(&refused_b).unwrap();

        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let ctx =
            Context::at(root, Some(projects.clone())).with_codex_home(Some(home.join(".codex")));
        let payload: PromptPayload = serde_json::from_value(json!({
            "session_id": A,
            "transcript_path": projects.join("regular").join(format!("{A}.jsonl")),
            "cwd": "/work",
            "prompt": "/scribe group add planning"
        }))
        .unwrap();
        let session = A.parse::<SessionId>().unwrap();
        let named = [B.parse::<SessionId>().unwrap()];
        let opened = std::cell::RefCell::new(Vec::new());

        let output = run_with_transcript_reader(
            "planning",
            &named,
            &payload,
            &session,
            &ctx,
            |provider_ctx, id, path| {
                assert_eq!(id.as_str(), B);
                let first = opened.borrow().is_empty();
                opened.borrow_mut().push(path.to_path_buf());
                if first || every_path_transitions {
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
        assert!(reason.contains("nothing changed"), "{reason}");
        assert!(ctx.store().groups().unwrap().is_empty(), "{reason}");
        assert!(
            ctx.store()
                .search_messages(&[String::from("transitionedorchid")])
                .unwrap()
                .matches
                .is_empty(),
            "{reason}"
        );

        // Both refused entries were reached by the checked open, in either order.
        let mut reached = opened.into_inner();
        reached.sort();
        assert_eq!(
            reached,
            [refused_a, refused_b],
            "every_path_transitions={every_path_transitions}"
        );
    }
}

// The seams are thread-local so concurrent tests never share hook input or a
// discovery/read interleaving. The real command dispatcher still owns audit.
thread_local! {
    static HOOK_INPUT: std::cell::RefCell<Option<Vec<u8>>> = const { std::cell::RefCell::new(None) };
    static BEFORE_READ: std::cell::RefCell<Option<ReadCallback>> = const { std::cell::RefCell::new(None) };
}

pub(in crate::commands::hook) fn take_hook_input() -> Option<Vec<u8>> {
    HOOK_INPUT.with(|input| input.borrow_mut().take())
}

pub(in crate::commands::hook) fn before_past_read(path: &std::path::Path) {
    BEFORE_READ.with(|callback| {
        if let Some(callback) = callback.borrow_mut().as_mut() {
            callback(path);
        }
    });
}

fn submit(ctx: &Context, input: serde_json::Value) -> Output {
    use crate::commands::hook::HookArgs;
    use crate::commands::{Command, HookCommand};
    HOOK_INPUT.with(|slot| *slot.borrow_mut() = Some(input.to_string().into_bytes()));
    crate::commands::run(
        Command::Hook(HookCommand::UserPromptSubmit(HookArgs {})),
        ctx,
    )
    .unwrap()
}

/// ORB-27582. 2_design.md §4.4 "Adding past sessions": each applicable
/// combination is an isolated transaction through the actual hook dispatcher.
/// Missing transcripts have no append, and Codex filenames encode UUIDs, so
/// short exact IDs use a stored Claude ID instead of a Codex filename.
#[cfg(unix)]
#[test]
fn transcript_enrollment_state_matrix() {
    use scribe_core::control::{Switch, switch};
    use scribe_core::{Provider, Redactor};
    use std::io::Write;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum State {
        Regular,
        Absent,
        Refused,
        Symlink,
        Replaced,
        Vanished,
        Mixed,
        MixedVanished,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Source {
        Claude,
        Codex,
        Archived,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum IdForm {
        Full,
        Prefix,
        Ambiguous,
        ShortExact,
        StoredOnly,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Timing {
        Before,
        During,
        After,
    }
    const TARGET: &str = "12345678-1234-4123-8123-123456789abc";
    const SENTINEL: &str = "ffffffff-0000-4000-8000-000000000001";
    const COLLISION: &str = "12345678-ffff-4fff-8fff-ffffffffffff";
    for state in [
        State::Regular,
        State::Absent,
        State::Refused,
        State::Symlink,
        State::Replaced,
        State::Vanished,
        State::Mixed,
        State::MixedVanished,
    ] {
        for source in [Source::Claude, Source::Codex, Source::Archived] {
            for form in [
                IdForm::Full,
                IdForm::Prefix,
                IdForm::Ambiguous,
                IdForm::ShortExact,
                IdForm::StoredOnly,
            ] {
                for timing in [Timing::Before, Timing::During, Timing::After] {
                    // A stored-only id has no file; a short on-disk Codex id
                    // cannot be discovered because rollout filenames use UUIDs.
                    if form == IdForm::StoredOnly && state != State::Absent
                        || form == IdForm::ShortExact && source != Source::Claude
                        || timing != Timing::Before
                            && (state != State::Regular
                                || matches!(form, IdForm::Ambiguous | IdForm::StoredOnly))
                    {
                        continue;
                    }
                    let row = format!("{state:?}/{source:?}/{form:?}/{timing:?}");
                    let dir = tempfile::tempdir().unwrap();
                    let projects = dir.path().join("home/.claude/projects");
                    let codex_home = dir.path().join("home/.codex");
                    let target = if form == IdForm::ShortExact {
                        "abc"
                    } else {
                        TARGET
                    };
                    let path = match source {
                        Source::Claude => projects.join("regular").join(format!("{target}.jsonl")),
                        Source::Codex => codex_home
                            .join("sessions/2020/01/01")
                            .join(format!("rollout-2020-01-01T00-00-00-{target}.jsonl")),
                        Source::Archived => codex_home
                            .join("archived_sessions")
                            .join(format!("rollout-2020-01-01T00-00-00-{target}.jsonl")),
                    };
                    let refused = match source {
                        Source::Claude => projects.join("refused").join(format!("{target}.jsonl")),
                        Source::Codex | Source::Archived => codex_home
                            .join("archived_sessions")
                            .join(format!("rollout-2020-01-02T00-00-00-{target}.jsonl")),
                    };
                    let base = match source {
                        Source::Claude => claude_turn_line("b1", "matrixbaseorchid")
                            .replace("2090-11-20", "2020-01-01"),
                        Source::Codex | Source::Archived => {
                            codex_rollout_body(target, "matrixbaseorchid")
                                .replace("2090-11-20", "2020-01-01")
                        }
                    };
                    let late = match source {
                        Source::Claude => claude_turn_line("b2", "matrixlateorchid")
                            .replace("2090-11-20", "2020-01-01"),
                        Source::Codex | Source::Archived => {
                            codex_rollout_body(target, "matrixlateorchid")
                                .lines()
                                .skip(1)
                                .collect::<Vec<_>>()
                                .join("\n")
                                .replace("\"u1\"", "\"u2\"")
                                .replace("\"t1\"", "\"t2\"")
                                .replace("2090-11-20", "2020-01-01")
                                + "\n"
                        }
                    };
                    if matches!(
                        state,
                        State::Regular | State::Mixed | State::MixedVanished | State::Replaced
                    ) {
                        write_private(&path, &base);
                        if source == Source::Claude {
                            let foreign = format!(
                                "{}\n{}\n",
                                json!({"type":"user", "uuid":"foreign", "sessionId": C, "timestamp":"2020-01-01T00:00:00Z", "message":{"role":"user", "content":"foreignorchid"}}),
                                json!({"type":"assistant", "uuid":"assistant", "sessionId": target, "timestamp":"2020-01-01T00:00:00Z", "message":{"role":"assistant", "content":"assistantorchid"}})
                            );
                            write!(
                                std::fs::OpenOptions::new()
                                    .append(true)
                                    .open(&path)
                                    .unwrap(),
                                "{foreign}"
                            )
                            .unwrap();
                        }
                    }
                    if state == State::Symlink {
                        std::fs::create_dir_all(refused.parent().unwrap()).unwrap();
                        std::os::unix::fs::symlink(&path, &refused).unwrap();
                    }
                    if matches!(
                        state,
                        State::Refused | State::Vanished | State::Mixed | State::MixedVanished
                    ) {
                        std::fs::create_dir_all(&refused).unwrap();
                    }
                    let root = dir.path().join("root");
                    std::fs::create_dir(&root).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                            .unwrap();
                    }
                    let ctx = Context::at(root.clone(), Some(projects.clone()))
                        .with_codex_home(Some(codex_home));
                    // Stored IDs also resolve when the file is absent; the second
                    // stored id makes ambiguity span disk and store (ORB-27295).
                    let stored: SessionId = target.parse().unwrap();
                    ctx.store()
                        .update_control(|control| {
                            switch(
                                control,
                                &stored,
                                &Switch::On,
                                time::OffsetDateTime::now_utc(),
                            )
                        })
                        .unwrap();
                    if matches!(form, IdForm::Ambiguous | IdForm::ShortExact) {
                        let collision: SessionId = if form == IdForm::ShortExact {
                            "abcdef"
                        } else {
                            COLLISION
                        }
                        .parse()
                        .unwrap();
                        ctx.store()
                            .update_control(|control| {
                                switch(
                                    control,
                                    &collision,
                                    &Switch::On,
                                    time::OffsetDateTime::now_utc(),
                                )
                            })
                            .unwrap();
                    }
                    let input_id = match form {
                        IdForm::Full | IdForm::ShortExact | IdForm::StoredOnly => target,
                        IdForm::Prefix => "12345678-1234",
                        IdForm::Ambiguous => "12345678",
                    };
                    let mut ids = input_id.to_owned();
                    if timing == Timing::During {
                        write_private(
                            &projects.join("sentinel").join(format!("{SENTINEL}.jsonl")),
                            &claude_turn_line("s1", "sentinelorchid"),
                        );
                        ids.push_str(&format!(" {SENTINEL}"));
                    }
                    if state == State::Regular && timing == Timing::Before {
                        write!(
                            std::fs::OpenOptions::new()
                                .append(true)
                                .open(&path)
                                .unwrap(),
                            "{late}"
                        )
                        .unwrap();
                    }
                    let watched = refused.clone();
                    let append_path = path.clone();
                    let late_for_read = late.clone();
                    BEFORE_READ.with(|slot| {
                        *slot.borrow_mut() = Some(Box::new(move |opened| {
                            if matches!(state, State::Vanished | State::MixedVanished)
                                && opened == watched
                            {
                                std::fs::remove_dir(opened).unwrap();
                            }
                            if state == State::Replaced && opened == append_path {
                                std::fs::remove_file(opened).unwrap();
                                std::os::unix::fs::symlink("missing", opened).unwrap();
                            }
                            if timing == Timing::During
                                && opened.file_name().unwrap().to_string_lossy()
                                    == format!("{SENTINEL}.jsonl")
                            {
                                write!(
                                    std::fs::OpenOptions::new()
                                        .append(true)
                                        .open(&append_path)
                                        .unwrap(),
                                    "{late_for_read}"
                                )
                                .unwrap();
                            }
                        }))
                    });
                    let output = submit(
                        &ctx,
                        json!({"session_id": A, "transcript_path": projects.join("caller").join(format!("{A}.jsonl")), "cwd": "/work", "prompt": format!("/scribe group add planning {ids}")}),
                    );
                    BEFORE_READ.with(|slot| *slot.borrow_mut() = None);
                    if form != IdForm::Ambiguous {
                        if matches!(state, State::Vanished | State::MixedVanished) {
                            assert!(!refused.exists(), "{row}: refused entry was not removed");
                        }
                        if state == State::Replaced {
                            assert!(
                                std::fs::symlink_metadata(&path)
                                    .unwrap()
                                    .file_type()
                                    .is_symlink(),
                                "{row}: transcript was not replaced"
                            );
                        }
                    }
                    let Output::Hook(reply) = output else {
                        panic!("{row}: no hook reply")
                    };
                    let expected_error = if form == IdForm::Ambiguous {
                        Some("id_ambiguous")
                    } else if matches!(
                        state,
                        State::Refused
                            | State::Symlink
                            | State::Replaced
                            | State::Vanished
                            | State::Mixed
                            | State::MixedVanished
                    ) {
                        Some("provenance_failed")
                    } else {
                        None
                    };
                    assert_eq!(reply.outcomes[0].error_code, expected_error, "{row}");
                    let db = rusqlite::Connection::open(root.join("scribe.db")).unwrap();
                    let audits: Vec<(String, String, Option<String>, Option<String>)> = db
                        .prepare(
                            "SELECT command, status, target, error_code FROM audit ORDER BY id",
                        )
                        .unwrap()
                        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                        .unwrap()
                        .map(Result::unwrap)
                        .collect();
                    let mut expected = vec![(
                        "hook user-prompt-submit".to_owned(),
                        if expected_error.is_some() {
                            "failure"
                        } else {
                            "success"
                        }
                        .to_owned(),
                        Some(format!(
                            "session:{}",
                            if expected_error.is_some() { A } else { target }
                        )),
                        expected_error.map(str::to_owned),
                    )];
                    if timing == Timing::During {
                        expected.push((
                            "hook user-prompt-submit".into(),
                            "success".into(),
                            Some(format!("session:{SENTINEL}")),
                            None,
                        ));
                    }
                    assert_eq!(audits, expected, "{row}");
                    let groups = ctx.store().groups().unwrap();
                    if expected_error.is_some() {
                        assert!(groups.is_empty(), "{row}");
                    } else {
                        assert_eq!(groups.len(), 1, "{row}");
                        let mut expected_members =
                            std::collections::BTreeSet::from([stored.clone()]);
                        if timing == Timing::During {
                            expected_members.insert(SENTINEL.parse().unwrap());
                        }
                        assert_eq!(groups[0].members(), expected_members, "{row}");
                        let cursor: Option<String> = db
                            .query_row(
                                "SELECT turn_cursor FROM group_members WHERE session = ?1",
                                [target],
                                |r| r.get(0),
                            )
                            .unwrap();
                        let expected_cursor = if state != State::Regular {
                            None
                        } else if timing == Timing::Before {
                            Some(if source == Source::Claude { "b2" } else { "u2" })
                        } else {
                            Some(if source == Source::Claude { "b1" } else { "u1" })
                        };
                        assert_eq!(cursor.as_deref(), expected_cursor, "{row}");
                    }
                    let count = |word: &str| {
                        ctx.store()
                            .search_messages(&[word.to_owned()])
                            .unwrap()
                            .matches
                            .len()
                    };
                    assert_eq!(count("foreignorchid"), 0, "{row}");
                    assert_eq!(count("assistantorchid"), 0, "{row}");
                    assert_eq!(
                        count("matrixbaseorchid"),
                        usize::from(expected_error.is_none() && state == State::Regular),
                        "{row}"
                    );
                    assert_eq!(
                        count("matrixlateorchid"),
                        usize::from(
                            expected_error.is_none()
                                && state == State::Regular
                                && timing == Timing::Before
                        ),
                        "{row}"
                    );
                    if expected_error.is_none() && state == State::Regular {
                        if timing == Timing::After {
                            write!(
                                std::fs::OpenOptions::new()
                                    .append(true)
                                    .open(&path)
                                    .unwrap(),
                                "{late}"
                            )
                            .unwrap();
                        }
                        let provider = if source == Source::Claude {
                            Provider::ClaudeCode
                        } else {
                            Provider::Codex
                        };
                        let turns = group_turns::past(&ctx.with_provider(provider), &stored, &path)
                            .unwrap()
                            .unwrap();
                        let added = ctx
                            .store()
                            .capture_group_turns(&stored, &turns, &Redactor::default())
                            .unwrap();
                        assert_eq!(added, usize::from(timing != Timing::Before), "{row}");
                        assert_eq!(
                            ctx.store()
                                .capture_group_turns(&stored, &turns, &Redactor::default())
                                .unwrap(),
                            0,
                            "{row}"
                        );
                        assert_eq!(count("matrixlateorchid"), 1, "{row}");
                    }
                }
            }
        }
    }
}
