#[test]
fn typed_group_add_audits_ids_only_and_a_later_capture_resumes_after_the_stored_turns() {
    let f = Fixture::new();
    let typing = transcript(
        &f,
        A,
        &[row(A, "t", "user", "/scribe group add planning B C")],
    );
    let past = transcript(
        &f,
        B,
        &[row(B, "b1", "user", "pastorchid typed before the add")],
    );
    let line = format!("/scribe group add planning {B} {C}");
    let out = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": A, "transcript_path": typing, "cwd": f.home(), "prompt": line}),
        &[],
    );
    let reply = text(&out.stdout);
    assert!(reply.contains("group planning (g-1) opened"), "{reply}");
    assert!(reply.contains("1 typed turn stored"), "{reply}");
    assert!(
        reply.contains("membership only, no transcript on disk"),
        "{reply}"
    );

    let group = show_group(&f, A, "planning");
    let members: Vec<&str> = group["seats"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["transcripts"].as_array().unwrap())
        .map(|t| t.as_str().unwrap())
        .collect();
    assert_eq!(members.len(), 2, "{members:?}");
    assert!(members.contains(&B) && members.contains(&C), "{members:?}");
    assert!(!members.contains(&A), "{members:?}");

    let audit = support::audit_lines(&f.root());
    let targets: Vec<&str> = audit
        .iter()
        .filter(|l| l["command"] == "hook user-prompt-submit")
        .map(|l| l["target"].as_str().unwrap())
        .collect();
    assert_eq!(targets, [format!("session:{B}"), format!("session:{C}")]);
    let log = support::audit_text(&f.root());
    assert!(
        !log.contains("pastorchid") && !log.contains("planning"),
        "{log}"
    );

    // The session later resumes: its stored turn is not stored twice, and its
    // new typed turn is.
    let mut more = row(B, "b2", "user", "lateorchid typed after the add");
    more["timestamp"] = json!("2090-11-21T01:00:00Z");
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&past)
        .unwrap();
    writeln!(append, "{more}").unwrap();
    hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": B, "transcript_path": past, "cwd": f.home(), "prompt": "lateorchid typed after the add"}),
        &[],
    );
    let total = |term| {
        invoke(
            &f,
            B,
            &["search", term, "--kind", "message", "--group", "g-1"],
            0,
        )["total"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(total("pastorchid"), 1);
    assert_eq!(total("lateorchid"), 1);
}

#[test]
fn typed_group_add_includes_the_typing_session_when_its_id_is_named() {
    let f = Fixture::new();
    let typing = transcript(
        &f,
        A,
        &[row(A, "t", "user", "/scribe group add planning A")],
    );
    let line = format!("/scribe group add planning {A}");
    let out = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id": A, "transcript_path": typing, "cwd": f.home(), "prompt": line}),
        &[],
    );
    let reply = text(&out.stdout);
    assert!(
        reply.contains(&format!(
            "{A}: added; transcript on disk, 0 typed turns stored"
        )),
        "{reply}"
    );

    let group = show_group(&f, A, "planning");
    let members: Vec<&str> = group["seats"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["transcripts"].as_array().unwrap())
        .map(|t| t.as_str().unwrap())
        .collect();
    assert_eq!(members, [A]);
}

#[cfg(unix)]
#[test]
fn typed_group_add_refuses_a_linked_transcript_but_keeps_an_absent_one_membership_only() {
    let f = Fixture::new();
    let typing = transcript(&f, A, &[row(A, "t", "user", "/scribe group add planning")]);
    // B's transcript was replaced by a link to a file elsewhere.
    let outside = f.elsewhere("outside.jsonl");
    std::fs::write(
        &outside,
        format!("{}\n", row(B, "b1", "user", "linkedorchid typed")),
    )
    .unwrap();
    support::make_private(&outside);
    std::os::unix::fs::symlink(&outside, transcript_path(&f, B)).unwrap();
    let submit = |named: &str| {
        text(
            &hook(
                &f,
                &["hook", "user-prompt-submit"],
                json!({"session_id": A, "transcript_path": typing, "cwd": f.home(),
                    "prompt": format!("/scribe group add planning {named}")}),
                &[],
            )
            .stdout,
        )
    };
    let store = scribe_core::Store::open(f.root());

    // The linked entry refuses the whole line, even beside an absent session.
    let reply = submit(&format!("{B} {C}"));
    assert!(reply.contains("group add refused"), "{reply}");
    assert!(reply.contains("nothing changed"), "{reply}");
    assert!(!reply.contains("membership only"), "{reply}");
    assert!(store.groups().unwrap().is_empty());
    assert_eq!(
        invoke(&f, A, &["search", "linkedorchid", "--kind", "message"], 0)["total"],
        0
    );
    let log = support::audit_text(&f.root());
    assert!(!log.contains("linkedorchid"), "{log}");

    // A genuinely absent transcript is still membership only.
    let reply = submit(C);
    assert!(reply.contains("group planning (g-1) opened"), "{reply}");
    assert!(
        reply.contains("membership only, no transcript on disk"),
        "{reply}"
    );
}

#[cfg(unix)]
#[test]
fn typed_group_add_refuses_a_session_with_a_regular_and_a_refused_transcript() {
    let f = Fixture::new();
    let typing = transcript(&f, A, &[row(A, "t", "user", "/scribe group add planning")]);
    let rollout = |text: &str| -> String {
        [
            json!({"type":"session_meta","payload":{"id":B,"source":"vscode","thread_source":"user","cwd":f.home()}}),
            json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
                "type":"message","role":"user","id":"mixed",
                "content":[{"type":"input_text","text":text}],
                "internal_chat_message_metadata_passthrough":{"turn_id":"mixed-turn","content_item_kinds":["user.text"],"content_item_metadata":[{"provenance":{"type":"user"}}]}
            }}),
        ]
        .iter()
        .map(|r| format!("{r}\n"))
        .collect()
    };
    // B has a valid Codex rollout in the active root and, in the archived
    // root, a link named for it: discovery reports both entries.
    let active = f.home().join(".codex/sessions/2090/11/20");
    f.create_private_dir_all(&active);
    let regular = active.join(format!("rollout-2090-11-20T01-00-00-{B}.jsonl"));
    std::fs::write(&regular, rollout("mixedorchid typed")).unwrap();
    support::make_private(&regular);
    let outside = f.elsewhere("outside.jsonl");
    std::fs::write(&outside, rollout("linkedmixedorchid typed")).unwrap();
    support::make_private(&outside);
    let archived = f.home().join(".codex/archived_sessions");
    f.create_private_dir_all(&archived);
    std::os::unix::fs::symlink(
        &outside,
        archived.join(format!("rollout-2090-11-19T01-00-00-{B}.jsonl")),
    )
    .unwrap();
    let store = scribe_core::Store::open(f.root());
    let submit = |named: &str| {
        text(
            &hook(
                &f,
                &["hook", "user-prompt-submit"],
                json!({"session_id": A, "transcript_path": typing, "cwd": f.home(),
                    "prompt": format!("/scribe group add planning {named}")}),
                &[],
            )
            .stdout,
        )
    };

    // The refused entry refuses the whole line, even beside an absent session:
    // nothing is joined, stored or audited.
    let reply = submit(&format!("{B} {C}"));
    assert!(reply.contains("group add refused"), "{reply}");
    assert!(reply.contains("nothing changed"), "{reply}");
    assert!(!reply.contains("membership only"), "{reply}");
    assert!(store.groups().unwrap().is_empty());
    assert_eq!(
        invoke(&f, A, &["search", "mixedorchid", "--kind", "message"], 0)["total"],
        0
    );
    let log = support::audit_text(&f.root());
    assert!(!log.contains("mixedorchid"), "{log}");

    // A lone valid transcript still backfills, and a genuinely absent session
    // is membership only.
    transcript(&f, D, &[row(D, "d1", "user", "lonevalidorchid typed")]);
    let reply = submit(D);
    assert!(reply.contains("group planning (g-1) opened"), "{reply}");
    assert!(reply.contains("1 typed turn stored"), "{reply}");
    let reply = submit(C);
    assert!(
        reply.contains("membership only, no transcript on disk"),
        "{reply}"
    );

    // B still refuses its own line once the group exists, and is not a member.
    let reply = submit(B);
    assert!(reply.contains("group add refused"), "{reply}");
    let groups = store.groups().unwrap();
    assert_eq!(groups.len(), 1);
    let members: Vec<String> = groups[0]
        .members()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(members, [C, D]);
}

#[test]
fn typed_group_add_resolves_prefixes_and_exact_ids_across_disk_and_store() {
    let f = Fixture::new();
    let typing = transcript(&f, A, &[row(A, "t", "user", "/scribe group add planning")]);
    let submit = |prompt: String| {
        hook(
            &f,
            &["hook", "user-prompt-submit"],
            json!({"session_id": A, "transcript_path": typing, "cwd": f.home(), "prompt": prompt}),
            &[],
        )
    };
    let store = scribe_core::Store::open(f.root());

    // Shared prefix across a disk transcript (`abcd-one`), a catalog-only
    // switched session (`abcd-two`), and a valid earlier session (`B`) refuses
    // the whole line before any group, membership, or turn writes.
    transcript(
        &f,
        "abcd-one",
        &[row("abcd-one", "d1", "user", "diskorchid")],
    );
    transcript(&f, B, &[row(B, "b1", "user", "validorchid")]);
    let stored_only: scribe_core::SessionId = "abcd-two".parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &stored_only,
                &scribe_core::control::Switch::On,
                time::OffsetDateTime::now_utc(),
            )
        })
        .unwrap();

    let ambig = submit(format!("/scribe group add planning {B} abcd"));
    let reply = text(&ambig.stdout);
    assert!(reply.contains("group add refused"), "{reply}");
    assert!(reply.contains("ambiguous"), "{reply}");
    assert!(
        reply.contains("abcd-one") && reply.contains("abcd-two"),
        "{reply}"
    );
    assert!(store.groups().unwrap().is_empty());
    assert_eq!(
        invoke(&f, A, &["search", "diskorchid", "--kind", "message"], 0)["total"],
        0
    );
    assert_eq!(
        invoke(&f, A, &["search", "validorchid", "--kind", "message"], 0)["total"],
        0
    );
    let last_audit = support::audit_lines(&f.root()).pop().unwrap();
    assert_eq!(last_audit["command"], "hook user-prompt-submit");
    assert_eq!(last_audit["status"], "failure");
    assert_eq!(last_audit["target"], format!("session:{A}"));
    assert_eq!(last_audit["error_code"], "id_ambiguous");

    // Unique prefix for the disk session resolves and stores its typed turn;
    // unique prefix for the stored session adds membership only.
    let disambig = submit(String::from("/scribe group add planning abcd-o abcd-t"));
    let disambig_reply = text(&disambig.stdout);
    assert!(
        disambig_reply.contains("abcd-one: added; transcript on disk, 1 typed turn stored"),
        "{disambig_reply}"
    );
    assert!(
        disambig_reply.contains("abcd-two: added; membership only, no transcript on disk"),
        "{disambig_reply}"
    );

    // A membership-only session already in a group (`memb-two`) also collides
    // with a disk transcript (`memb-one`) on prefix `memb` before writes.
    let memb_first = submit(String::from("/scribe group add first memb-two"));
    assert!(
        text(&memb_first.stdout).contains("memb-two: added; membership only"),
        "{}",
        text(&memb_first.stdout)
    );
    transcript(
        &f,
        "memb-one",
        &[row("memb-one", "m1", "user", "memberorchid")],
    );
    let memb_ambig = submit(String::from("/scribe group add second memb"));
    let memb_reply = text(&memb_ambig.stdout);
    assert!(memb_reply.contains("group add refused"), "{memb_reply}");
    assert!(
        memb_reply.contains("memb-one") && memb_reply.contains("memb-two"),
        "{memb_reply}"
    );
    assert!(store.group("second").is_err());

    // Exact known IDs win across both sources, including IDs shorter than four
    // characters and when a longer ID in the other source shares the prefix.
    // 1. Exact stored ID (`abc`, <4 chars) wins over longer disk ID (`abc-one`).
    transcript(
        &f,
        "abc-one",
        &[row("abc-one", "x1", "user", "longdiskorchid")],
    );
    let exact_short_stored: scribe_core::SessionId = "abc".parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &exact_short_stored,
                &scribe_core::control::Switch::On,
                time::OffsetDateTime::now_utc(),
            )
        })
        .unwrap();
    let short_stored = submit(String::from("/scribe group add exact-wins abc"));
    let short_stored_reply = text(&short_stored.stdout);
    assert!(
        short_stored_reply.contains("  abc: added; membership only, no transcript on disk"),
        "{short_stored_reply}"
    );
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "longdiskorchid",
                "--kind",
                "message",
                "--group",
                "exact-wins"
            ],
            0
        )["total"],
        0
    );

    // 2. Exact disk ID (`def`, <4 chars) wins over longer stored ID (`def-two`).
    transcript(&f, "def", &[row("def", "d2", "user", "shortdiskorchid")]);
    let longer_stored: scribe_core::SessionId = "def-two".parse().unwrap();
    store
        .update_control(|state| {
            scribe_core::control::switch(
                state,
                &longer_stored,
                &scribe_core::control::Switch::On,
                time::OffsetDateTime::now_utc(),
            )
        })
        .unwrap();
    let short_disk = submit(String::from("/scribe group add exact-wins def"));
    let short_disk_reply = text(&short_disk.stdout);
    assert!(
        short_disk_reply.contains("  def: added; transcript on disk, 1 typed turn stored"),
        "{short_disk_reply}"
    );
    assert_eq!(
        invoke(
            &f,
            A,
            &[
                "search",
                "shortdiskorchid",
                "--kind",
                "message",
                "--group",
                "exact-wins"
            ],
            0
        )["total"],
        1
    );
}

#[test]
fn typed_group_add_backfills_other_provider_including_archived_codex() {
    let f = Fixture::new();
    let claude_prompt = format!("/scribe group add cross {B}");
    let claude_path = transcript(&f, A, &[row(A, "a", "user", &claude_prompt)]);
    let archived = f.home().join(".codex/archived_sessions");
    f.create_private_dir_all(&archived);
    let codex_past = archived.join(format!("rollout-2090-11-20T01-00-00-{B}.jsonl"));
    let codex_rows = [
        json!({"type":"session_meta","payload":{"id":B,"source":"vscode","thread_source":"user","cwd":f.home()}}),
        json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
            "type":"message","role":"user","id":"past-codex",
            "content":[{"type":"input_text","text":"archivedcodexorchid"},{"type":"input_text","text":"untrustedcodexorchid"}],
            "internal_chat_message_metadata_passthrough":{"turn_id":"past-turn","content_item_kinds":["user.text","skill"],"content_item_metadata":[{"provenance":{"type":"user"}},{"provenance":{"type":"skill"}}]}
        }}),
    ];
    std::fs::write(
        &codex_past,
        codex_rows
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>(),
    )
    .unwrap();
    support::make_private(&codex_past);
    let out = hook(
        &f,
        &["hook", "user-prompt-submit"],
        json!({"session_id":A,"transcript_path":claude_path,"cwd":f.home(),"prompt":claude_prompt}),
        &[],
    );
    assert!(
        text(&out.stdout).contains("1 typed turn stored"),
        "{}",
        text(&out.stdout)
    );

    let claude_past = transcript(
        &f,
        D,
        &[
            row(D, "past-claude", "user", "pastclaudeorchid"),
            row(C, "copied", "user", "copiedclaudeorchid"),
        ],
    );
    assert!(claude_past.exists());
    let codex_prompt = format!("/scribe group add cross {D}");
    let dir = f.home().join(".codex/sessions/2090/11/20");
    f.create_private_dir_all(&dir);
    let codex_caller = dir.join(format!("rollout-2090-11-20T01-00-00-{C}.jsonl"));
    let caller_rows = [
        json!({"type":"session_meta","payload":{"id":C,"source":"vscode","thread_source":"user","cwd":f.home()}}),
        json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
            "type":"message","role":"user","id":"caller",
            "content":[{"type":"input_text","text":codex_prompt}],
            "internal_chat_message_metadata_passthrough":{"turn_id":"caller-turn","content_item_kinds":["user.text"],"content_item_metadata":[{"provenance":{"type":"user"}}]}
        }}),
    ];
    std::fs::write(
        &codex_caller,
        caller_rows
            .iter()
            .map(|r| format!("{r}\n"))
            .collect::<String>(),
    )
    .unwrap();
    support::make_private(&codex_caller);
    let out = hook(
        &f,
        &["--provider", "codex", "hook", "user-prompt-submit"],
        json!({"session_id":C,"transcript_path":codex_caller,"cwd":f.home(),"hook_event_name":"UserPromptSubmit","turn_id":"caller-turn","prompt":codex_prompt}),
        &[],
    );
    assert!(
        text(&out.stdout).contains("1 typed turn stored"),
        "{}",
        text(&out.stdout)
    );
    for (term, expected) in [
        ("archivedcodexorchid", 1),
        ("untrustedcodexorchid", 0),
        ("pastclaudeorchid", 1),
        ("copiedclaudeorchid", 0),
    ] {
        assert_eq!(
            invoke(
                &f,
                A,
                &["search", term, "--kind", "message", "--group", "cross"],
                0
            )["total"],
            expected,
            "{term}"
        );
    }
}

#[test]
fn typed_group_add_scopes_cross_provider_collision_to_named_sessions() {
    const E: &str = "55555555-5555-4555-8555-555555555555";
    let f = Fixture::new();
    let typing = transcript(&f, A, &[row(A, "a", "user", "/scribe group add planning")]);
    let write_codex = |id: &str, body: &str| {
        let dir = f.home().join(".codex/sessions/2090/11/20");
        f.create_private_dir_all(&dir);
        let path = dir.join(format!("rollout-2090-11-20T01-00-00-{id}.jsonl"));
        let rows = [
            json!({"type":"session_meta","payload":{"id":id,"source":"vscode","thread_source":"user","cwd":f.home()}}),
            json!({"timestamp":"2090-11-20T01:00:00Z","type":"response_item","payload":{
                "type":"message","role":"user","id":"u1",
                "content":[{"type":"input_text","text":body}],
                "internal_chat_message_metadata_passthrough":{"turn_id":"t1","content_item_kinds":["user.text"],"content_item_metadata":[{"provenance":{"type":"user"}}]}
            }}),
        ];
        std::fs::write(
            &path,
            rows.iter().map(|r| format!("{r}\n")).collect::<String>(),
        )
        .unwrap();
        support::make_private(&path);
    };

    // B collides across Claude Code and Codex; C, D, and E do not.
    transcript(&f, B, &[row(B, "b1", "user", "collidingclaudeorchid")]);
    write_codex(B, "collidingcodexorchid");
    transcript(&f, C, &[row(C, "c1", "user", "unrelatedclaudeorchid")]);
    write_codex(D, "unrelatedcodexorchid");
    transcript(&f, E, &[row(E, "e1", "user", "partialenrollorchid")]);

    let submit = |group_cmd: &str, named: &str| {
        text(
            &hook(
                &f,
                &["hook", "user-prompt-submit"],
                json!({"session_id": A, "transcript_path": typing, "cwd": f.home(),
                    "prompt": format!("/scribe group add {group_cmd} {named}")}),
                &[],
            )
            .stdout,
        )
    };
    let store = scribe_core::Store::open(f.root());

    // An unrelated exact named session (in Claude or Codex) succeeds despite B colliding.
    let reply = submit("planning", &format!("{C} {D}"));
    assert!(reply.contains("group planning (g-1) opened"), "{reply}");
    assert!(
        reply.contains(&format!(
            "{C}: added; transcript on disk, 1 typed turn stored"
        )),
        "{reply}"
    );
    assert!(
        reply.contains(&format!(
            "{D}: added; transcript on disk, 1 typed turn stored"
        )),
        "{reply}"
    );
    let members: Vec<String> = store
        .group("planning")
        .unwrap()
        .members()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(members, [C, D]);

    // Naming the colliding ID alongside an eligible session refuses the whole
    // line for both a new group and an existing group, without partial enrollment.
    for (group_cmd, named) in [
        ("other", format!("{E} {B}")),
        ("planning", format!("{E} {B}")),
        ("other", format!("{E} {}", &B[..8])),
    ] {
        let reply = submit(group_cmd, &named);
        assert!(reply.contains("group add refused"), "{reply}");
        assert!(
            reply.contains("exists in multiple transcript providers"),
            "{reply}"
        );
        assert!(reply.contains(B), "{reply}");
        assert!(reply.contains("nothing changed"), "{reply}");
        let last_audit = support::audit_lines(&f.root()).pop().unwrap();
        assert_eq!(last_audit["command"], "hook user-prompt-submit");
        assert_eq!(last_audit["status"], "failure");
        assert_eq!(last_audit["target"], format!("session:{A}"));
        assert_eq!(last_audit["error_code"], "usage_error");
    }

    assert!(store.group("other").is_err());
    let members: Vec<String> = store
        .group("planning")
        .unwrap()
        .members()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(members, [C, D]);
    for term in [
        "partialenrollorchid",
        "collidingclaudeorchid",
        "collidingcodexorchid",
    ] {
        assert_eq!(
            invoke(&f, A, &["search", term, "--kind", "message"], 0)["total"],
            0,
            "{term}"
        );
    }
}

#[test]
fn typed_multiline_group_controls_dispatch_through_hook_and_ignore_later_lines() {
    let add_line = format!("/scribe group add water-economy {B}");
    let add_member_msg = format!("{B}: added; membership only, no transcript on disk");
    let cases = [
        (
            "/scribe group water-economy",
            A,
            vec!["joined group water-economy"],
        ),
        (
            add_line.as_str(),
            B,
            vec!["group water-economy (g-1) opened", add_member_msg.as_str()],
        ),
    ];

    for (first_line, expected_member, expected_reason_parts) in cases {
        let f = Fixture::seeded();
        f.config("");
        let audit_before = support::audit_lines(&f.root());
        let marks_before = support::exported(&f.root(), "marks.json");
        let prompt = format!(
            "{first_line}\n#scribe retire preferences-1\n#scribe idea: multiline later line marker\n/scribe on"
        );
        let path = transcript(&f, A, &[row(A, "t", "user", &prompt)]);
        let out = hook(
            &f,
            &["hook", "user-prompt-submit"],
            json!({
                "session_id": A,
                "transcript_path": path,
                "cwd": f.home(),
                "prompt": prompt,
            }),
            &[],
        );

        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{first_line}: {reply}");
        assert!(
            reply.get("systemMessage").is_none(),
            "{first_line}: {reply}"
        );
        assert!(
            reply.get("hookSpecificOutput").is_none(),
            "{first_line}: {reply}"
        );
        let reason = reply["reason"].as_str().unwrap();
        for part in expected_reason_parts {
            assert!(reason.contains(part), "{first_line}: {reason}");
        }
        assert!(!reason.contains("scribe: off"), "{first_line}: {reason}");

        let group = show_group(&f, A, "water-economy");
        assert_eq!(group["kind"], "project", "{first_line}: {group}");
        assert_eq!(
            group["seats"],
            json!([{
                "name": &expected_member[..8],
                "role": "member",
                "transcripts": [expected_member],
                "newest_handoff": null
            }]),
            "{first_line}: {group}"
        );

        // Later prompt lines produce no independent settling, marking, or switch effect.
        assert_eq!(
            invoke(&f, A, &["entry", "show", "preferences-1"], 0)["status"],
            "active",
            "{first_line}"
        );
        assert!(
            support::control(&f.root())["sessions"][A].is_null(),
            "{first_line}"
        );
        assert_eq!(
            support::exported(&f.root(), "marks.json"),
            marks_before,
            "{first_line}"
        );
        let audit_after = support::audit_lines(&f.root());
        assert_eq!(
            audit_after.len(),
            audit_before.len() + 1,
            "{first_line}: {audit_after:?}"
        );
        let line = audit_after.last().unwrap();
        assert_eq!(line["command"], "hook user-prompt-submit");
        assert_eq!(line["status"], "success");
        assert_eq!(line["target"], format!("session:{expected_member}"));
        assert!(line["error_code"].is_null(), "{line}");
    }
}

// Backfill on open and join: a member's capture starts at its own transcript's
// first line, not at its join time.
