//! `user-prompt-submit` in process: a line-start `#scribe` line is a marker
//! (2_design.md §14.6), echoed and left for Stop, never recorded here and
//! never sent to a model.

use crate::app::{AppEnv, Context};
use crate::commands::hook::user_prompt_submit::run;
use crate::output::Output;
use scribe_core::EntryFilter;
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
        std::fs::write(projects.join(format!("{SESSION}.jsonl")), text).unwrap();
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
            context.contains("do not record it yourself"),
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
