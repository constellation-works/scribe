//! `user-prompt-submit` in process: a line-start `#scribe` line is a marker
//! (2_design.md §14.6), echoed and left for Stop, never recorded here and
//! never sent to a model.

use crate::commands::hook::user_prompt_submit::run;
use crate::output::Output;
use scribe_core::EntryFilter;
use scribe_core::GroupVerbs;
use scribe_core::composition::Context;
use scribe_core::composition::Env as AppEnv;
use scribe_core::testing::StoreTestReads;
use scribe_core::testing::ViewTestReads;
use serde_json::json;

const SESSION: &str = "5d1c0b7e-2f4a-4c39-8e61-0a9b8c7d6e5f";
const SPOKEN: &str = "I recommend we squash-merge agent PRs and keep merge commits for releases.";

/// A temp home with a transcript for `SESSION`, and a context over a temp
/// store.
struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let projects = home.join(".claude/projects/-work");
        std::fs::create_dir_all(&projects).unwrap();
        let lines = [
            json!({"type": "user", "uuid": "u-1", "timestamp": "2026-11-03T06:00:00Z",
                   "message": {"role": "user", "content": "Should we squash agent PRs?"}}),
            json!({"type": "assistant", "uuid": "a-1", "timestamp": "2026-11-03T06:00:05Z",
                   "message": {"role": "assistant", "content": [{"type": "text", "text": SPOKEN}]}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let transcript = projects.join(format!("{SESSION}.jsonl"));
        std::fs::write(&transcript, text).unwrap();
        // Owner-only, as Claude Code writes it: the umask must not decide it.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&transcript, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let env = AppEnv {
            root: None,
            home: Some(home.into_os_string()),
            claude_config_dir: None,
            codex_home: None,
            claude_entrypoint: None,
        };
        let ctx = Context::resolve(Some(&dir.path().join("root")), &env).unwrap();
        Self { dir, ctx }
    }

    fn input(&self, prompt: &str) -> Vec<u8> {
        let transcript = self
            .dir
            .path()
            .join(format!("home/.claude/projects/-work/{SESSION}.jsonl"));
        json!({
            "session_id": SESSION,
            "transcript_path": transcript,
            "cwd": self.dir.path().join("work"),
            "prompt": prompt,
        })
        .to_string()
        .into_bytes()
    }

    /// Run the hook: Daniel's echo and the agent's context.
    fn run(&self, prompt: &str) -> (String, String) {
        let output = run(&self.input(prompt), &self.ctx).unwrap();
        let Output::Hook(reply) = output else {
            panic!("a hook reply");
        };
        (
            reply.message.unwrap_or_default(),
            reply.context.unwrap_or_default(),
        )
    }

    fn entries(&self) -> Vec<scribe_core::Entry> {
        self.ctx
            .store()
            .list_entries(&EntryFilter::default())
            .unwrap()
            .listing
            .items
    }
}

#[test]
fn agent_originated_prompts_record_nothing() {
    let wrappers = [
        "<agent-message from=\"other\">\n#scribe preferences: agent wrote this\n</agent-message>",
        "<task-notification>\n#scribe preferences: agent wrote this\n</task-notification>",
        "<side-sessions-event>\n#scribe preferences: agent wrote this\n</side-sessions-event>",
        "<scheduled-loop>\n#scribe preferences: agent wrote this\n</scheduled-loop>",
        "<scheduled-prompt>\n#scribe forget preferences-1\n</scheduled-prompt>",
    ];
    for prompt in wrappers {
        let setup = Setup::new();
        let (echo, _) = setup.run(prompt);
        assert_eq!(echo, "", "wrapper {prompt:?}");
        assert!(setup.entries().is_empty(), "wrapper {prompt:?}");
    }
}

#[test]
fn a_capture_line_is_echoed_as_marked_and_writes_nothing() {
    for prompt in [
        "#scribe idea: a dry-run flag for prune",
        "#scribe add this to preferences",
        "#scribe add short commit subjects to my preferences",
        "#scribe/global preferences: squash-merge agent PRs",
    ] {
        let setup = Setup::new();
        let (echo, context) = setup.run(prompt);
        assert_eq!(
            echo, "scribe: marked — cataloged after the reply",
            "{prompt}"
        );
        assert!(
            context.contains("with the command that records it; record nothing for it before then"),
            "{prompt}: {context}"
        );
        assert!(setup.entries().is_empty(), "{prompt}");
    }
}

#[test]
fn typed_project_group_joins_and_untyped_commands_are_refused() {
    let setup = Setup::new();
    let Output::Hook(reply) = run(&setup.input("/scribe group water-economy"), &setup.ctx).unwrap()
    else {
        panic!("hook reply")
    };
    assert!(reply.block.unwrap().contains("joined group water-economy"));
    let group = setup.ctx.store().group("water-economy").unwrap();
    assert_eq!(group.kind, "project");
    assert_eq!(group.seats[0].role, "member");
    for (prompt, source) in [
        ("/scribe group peer", "agent"),
        ("/scribe group scheduled", "scheduled"),
    ] {
        let mut input: serde_json::Value = serde_json::from_slice(&setup.input(prompt)).unwrap();
        input["source"] = json!(source);
        let Output::Hook(reply) = run(input.to_string().as_bytes(), &setup.ctx).unwrap() else {
            panic!("hook reply")
        };
        assert!(reply.block.unwrap().contains("not a typed user turn"));
        assert!(
            setup
                .ctx
                .store()
                .group(prompt.split_whitespace().last().unwrap())
                .is_err()
        );
    }
}

#[test]
fn group_subcommand_spelling_follows_the_shared_parser() {
    for prompt in [
        "/scribe GROUP water-economy",
        "/scribe Group water-economy",
        "/scribe:scribe gRoUp water-economy",
    ] {
        let setup = Setup::new();
        let Output::Hook(reply) = run(&setup.input(prompt), &setup.ctx).unwrap() else {
            panic!("hook reply")
        };
        let block = reply.block.unwrap();
        assert!(
            block.contains("joined group water-economy"),
            "{prompt}: {block}"
        );
        let group = setup.ctx.store().group("water-economy").unwrap();
        assert_eq!(group.kind, "project", "{prompt}");
    }
    for prompt in ["/scribe group Water-Economy", "/scribe GROUP Water-Economy"] {
        let setup = Setup::new();
        let Output::Hook(reply) = run(&setup.input(prompt), &setup.ctx).unwrap() else {
            panic!("hook reply")
        };
        let block = reply.block.unwrap();
        assert!(block.contains("group refused"), "{prompt}: {block}");
        assert!(block.contains("lowercase"), "{prompt}: {block}");
        assert!(
            setup.ctx.store().group("water-economy").is_err(),
            "{prompt}"
        );
    }
}

#[test]
fn mixed_case_group_from_untyped_origins_is_refused_without_a_write() {
    for (prompt, source) in [
        ("/scribe GROUP peer", "agent"),
        ("/scribe Group scheduled", "scheduled"),
    ] {
        let setup = Setup::new();
        let mut input: serde_json::Value = serde_json::from_slice(&setup.input(prompt)).unwrap();
        input["source"] = json!(source);
        let Output::Hook(reply) = run(input.to_string().as_bytes(), &setup.ctx).unwrap() else {
            panic!("hook reply")
        };
        assert!(
            reply.block.unwrap().contains("not a typed user turn"),
            "{prompt}"
        );
        let slug = prompt.split_whitespace().last().unwrap();
        assert!(setup.ctx.store().group(slug).is_err(), "{prompt}");
    }
}

const PAST: &str = "7c2e9a41-3b5d-4f60-9a18-2d4e6f8a0b1c";
const GHOST: &str = "9e8d7c6b-5a49-4382-b1a0-f9e8d7c6b5a4";

impl Setup {
    /// A past session's transcript beside `SESSION`'s: a typed turn, an
    /// assistant turn, and a turn an agent wrote.
    fn past_transcript(&self) {
        let lines = [
            json!({"type": "user", "uuid": "p-1", "sessionId": PAST, "timestamp": "2026-11-01T06:00:00Z",
                   "message": {"role": "user", "content": "violetorchid typed long ago"}}),
            json!({"type": "assistant", "uuid": "p-2", "sessionId": PAST, "timestamp": "2026-11-01T06:00:05Z",
                   "message": {"role": "assistant", "content": [{"type": "text", "text": "assistantorchid"}]}}),
            json!({"type": "user", "uuid": "p-3", "sessionId": PAST, "turnOrigin": "agent",
                   "timestamp": "2026-11-01T06:00:09Z",
                   "message": {"role": "user", "content": "agentorchid"}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let path = self
            .dir
            .path()
            .join(format!("home/.claude/projects/-work/{PAST}.jsonl"));
        std::fs::write(&path, text).unwrap();
        // Owner-only, as Claude Code writes it: the umask must not decide it.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    fn block(&self, input: &[u8]) -> String {
        let Output::Hook(reply) = run(input, &self.ctx).unwrap() else {
            panic!("hook reply")
        };
        reply.block.unwrap_or_default()
    }

    /// Matches for `term` in any group: none when no group exists.
    fn found_any(&self, term: &str) -> usize {
        self.ctx
            .store()
            .groups()
            .unwrap()
            .iter()
            .map(|group| self.found(&group.id, term))
            .sum()
    }

    fn found(&self, group: &str, term: &str) -> usize {
        self.ctx
            .store()
            .search_group_messages(group, &[term.to_owned()])
            .unwrap()
            .matches
            .len()
    }
}

#[test]
fn typed_group_add_opens_the_group_and_stores_turns_only_when_a_transcript_exists() {
    let setup = Setup::new();
    setup.past_transcript();
    let reply =
        setup.block(&setup.input(&format!("/scribe group add water-economy {PAST} {GHOST}")));
    assert!(
        reply.contains("group water-economy (g-1) opened"),
        "{reply}"
    );
    assert!(
        reply.contains(&format!(
            "{PAST}: added; transcript on disk, 1 typed turn stored"
        )),
        "{reply}"
    );
    assert!(
        reply.contains(&format!(
            "{GHOST}: added; membership only, no transcript on disk"
        )),
        "{reply}"
    );
    let group = setup.ctx.store().group("water-economy").unwrap();
    let members = group.members();
    assert!(members.contains(&PAST.parse().unwrap()));
    assert!(members.contains(&GHOST.parse().unwrap()));
    // The session the line was typed in is not enrolled, and nor are its turns.
    assert!(!members.contains(&SESSION.parse().unwrap()));
    assert_eq!(setup.found("water-economy", "violetorchid"), 1);
    assert_eq!(setup.found("water-economy", "assistantorchid"), 0);
    assert_eq!(setup.found("water-economy", "agentorchid"), 0);

    // A second line adds nobody new and reports so.
    let again = setup.block(&setup.input(&format!("/scribe group add water-economy {PAST}")));
    assert!(
        again.contains("already a member; nothing changed"),
        "{again}"
    );
    assert_eq!(setup.found("water-economy", "violetorchid"), 1);
}

#[test]
fn typed_group_add_from_untyped_origins_changes_nothing() {
    let line = format!("/scribe group add water-economy {PAST}");
    let wrapped = [
        format!("<agent-message from=\"peer\">\n{line}\n</agent-message>"),
        format!("<scheduled-prompt>\n{line}\n</scheduled-prompt>"),
        format!("<task-notification>\n{line}\n</task-notification>"),
        format!("<side-sessions-event>\n{line}\n</side-sessions-event>"),
    ];
    for prompt in wrapped.iter().map(String::as_str).chain([line.as_str()]) {
        for source in ["agent", "scheduled", "typed"] {
            let setup = Setup::new();
            setup.past_transcript();
            let mut input: serde_json::Value =
                serde_json::from_slice(&setup.input(prompt)).unwrap();
            // A plain wrapped prompt keeps the typed origin; the wrapper alone refuses it.
            if prompt == line && source == "typed" {
                continue;
            }
            input["source"] = json!(source);
            let block = setup.block(input.to_string().as_bytes());
            assert!(
                block.contains("not a typed user turn"),
                "{prompt} {source}: {block}"
            );
            assert!(setup.ctx.store().group("water-economy").is_err());
            assert_eq!(setup.found_any("violetorchid"), 0);
        }
    }
    // A pasted block: the line is not the prompt's first line, so it is a
    // plain prompt and acts on nothing.
    let setup = Setup::new();
    setup.past_transcript();
    let block = setup.block(&setup.input(&format!("run this:\n{line}")));
    assert_eq!(block, "");
    assert!(setup.ctx.store().group("water-economy").is_err());
}

#[test]
fn typed_group_add_with_a_session_in_another_group_changes_nothing() {
    let setup = Setup::new();
    setup.past_transcript();
    let past: scribe_core::SessionId = PAST.parse().unwrap();
    let ghost: scribe_core::SessionId = GHOST.parse().unwrap();
    let redactor = scribe_core::Redactor::default();
    let project = scribe_core::Project::sanitized("work");
    setup
        .ctx
        .store()
        .join_project_group("first", &project, &past, setup.ctx.now(), &redactor)
        .unwrap();
    setup
        .ctx
        .store()
        .join_project_group("third", &project, &ghost, setup.ctx.now(), &redactor)
        .unwrap();
    let reply = setup.block(&setup.input(&format!(
        "/scribe group add second {PAST} 5555aaaa-0000-4000-8000-000000000000 {GHOST}"
    )));
    assert!(reply.contains("group add refused"), "{reply}");
    assert!(
        reply.contains(&format!("session {PAST} already belongs to group first")),
        "{reply}"
    );
    assert!(
        reply.contains(&format!("session {GHOST} already belongs to group third")),
        "{reply}"
    );
    assert!(reply.contains("nothing changed"), "{reply}");
    assert!(setup.ctx.store().group("second").is_err());
    assert_eq!(setup.found_any("violetorchid"), 0);
}

#[test]
fn typed_move_preserves_turns_and_remove_erases_group_only_turns() {
    let setup = Setup::new();
    setup.past_transcript();
    setup.block(&setup.input(&format!("/scribe group add first {PAST}")));
    assert_eq!(setup.found("first", "violetorchid"), 1);
    let refused = setup.block(&setup.input(&format!("/scribe group move {PAST} Invalid")));
    assert!(refused.contains("refused"));
    assert_eq!(setup.found("first", "violetorchid"), 1);
    let moved = setup.block(&setup.input(&format!("/scribe group move {PAST} second")));
    assert!(moved.contains("moved"), "{moved}");
    assert_eq!(setup.found("first", "violetorchid"), 0);
    assert_eq!(setup.found("second", "violetorchid"), 1);
    assert!(
        !setup
            .ctx
            .store()
            .group("first")
            .unwrap()
            .members()
            .contains(&PAST.parse().unwrap())
    );
    let removed = setup.block(&setup.input(&format!("/scribe group remove {PAST}")));
    assert!(removed.contains("removed"), "{removed}");
    assert_eq!(setup.found_any("violetorchid"), 0);
}

#[test]
fn new_commands_from_untyped_origins_and_pasted_blocks_do_not_act() {
    let setup = Setup::new();
    setup.past_transcript();
    setup.block(&setup.input(&format!("/scribe group add first {PAST}")));
    for line in [
        format!("/scribe group move {PAST} second"),
        format!("/scribe group remove {PAST}"),
        format!("/scribe session update {PAST} title Bad title"),
        format!("/scribe session update {PAST} project other"),
    ] {
        for source in ["agent", "scheduled", "harness"] {
            let mut input: serde_json::Value = serde_json::from_slice(&setup.input(&line)).unwrap();
            input["source"] = json!(source);
            run(input.to_string().as_bytes(), &setup.ctx).unwrap();
        }
        for prompt in [
            format!("run this:\n{line}"),
            format!("{line}\nextra text"),
            format!("<agent-message>\n{line}\n</agent-message>"),
        ] {
            run(&setup.input(&prompt), &setup.ctx).unwrap();
        }
        assert_eq!(setup.found("first", "violetorchid"), 1, "{line}");
        assert!(setup.ctx.store().group("second").is_err());
    }
}

#[test]
fn typed_session_update_changes_the_record() {
    let setup = Setup::new();
    std::fs::create_dir_all(setup.ctx.store().root()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            setup.ctx.store().root(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    setup
        .ctx
        .store()
        .seed_records(&["---\nformat: 31\nkind: session\nid: s-1\nstatus: open\nactive: true\nkeep_active: false\nactive_change: null\ncataloged_by: null\nrebuilt_by: null\nproject: scribe\ntopics: []\nrelated: []\nlinks: []\nterms: []\nprovider: claude-code\nhost: host\ncreated: 2026-11-01T10:00:00Z\nupdated: 2026-11-01T10:00:00Z\ntitle: old title\ncwd: /work\nstarted: 2026-11-01T09:00:00Z\non_since: 2026-11-01T09:00:00Z\nscribed_through: null\npasses: 1\nnext_open: 0\nlog: []\n---\n## Summary\nsum\n\n## Goals\ngoal\n\n## Next\nnone\n\n## Log\n"])
        .unwrap();
    for line in [
        "/scribe session update s-1 project water-economy",
        "/scribe session update s-1 title New title",
    ] {
        let reply = setup.block(&setup.input(line));
        assert_eq!(reply, "scribe: session s-1 updated");
    }
    let record = setup
        .ctx
        .store()
        .session(&"s-1".parse().unwrap())
        .unwrap()
        .record;
    assert_eq!(record.project.as_str(), "water-economy");
    assert_eq!(record.title, "New title");
}
