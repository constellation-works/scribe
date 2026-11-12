//! Distribution scaffolding through the installed binary surface, on temp projects.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;
use serde_json::{Value, json};
use support::{Fixture, text};

#[test]
fn init_embeds_skill_preserves_unrelated_config_and_is_idempotent() {
    for provider in ["codex", "claude-code"] {
        let f = Fixture::new();
        let target = f.elsewhere("project");
        std::fs::create_dir(&target).unwrap();
        let program = if provider == "codex" {
            "codex"
        } else {
            "claude"
        };
        let path = f.fake_program(program, "#!/bin/sh\nexit 0\n");
        let config = target.join(if provider == "codex" {
            ".codex"
        } else {
            ".claude"
        });
        std::fs::create_dir(&config).unwrap();
        let hooks = config.join(if provider == "codex" {
            "hooks.json"
        } else {
            "settings.json"
        });
        let unrelated = json!({"type":"command","command":"echo unrelated"});
        std::fs::write(
            &hooks,
            json!({"other_setting":42,"hooks":{"Stop":[{"hooks":[unrelated.clone()]}]}})
                .to_string(),
        )
        .unwrap();
        let run = || {
            f.cmd(&[
                "init",
                "--provider",
                provider,
                "--target",
                target.to_str().unwrap(),
                "--json",
            ])
            .env("PATH", &path)
            .output()
            .unwrap()
        };
        let first = run();
        assert!(first.status.success(), "{}", text(&first.stderr));
        let report: Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(report["changed"], true);
        assert_eq!(report["provider"], provider);
        assert!(
            report["next"].as_array().unwrap().iter().any(|s| s
                .as_str()
                .unwrap()
                .contains("review")
                || s.as_str().unwrap().contains("Review"))
        );
        let skill = target.join(if provider == "codex" {
            ".agents/skills/scribe/SKILL.md"
        } else {
            ".claude/skills/scribe/SKILL.md"
        });
        assert!(skill.is_file());
        let contents = std::fs::read_to_string(&skill).unwrap();
        assert_eq!(contents, include_str!("../../../skills/scribe/SKILL.md"));
        let merged: Value = serde_json::from_slice(&std::fs::read(&hooks).unwrap()).unwrap();
        assert_eq!(merged["other_setting"], 42);
        assert_eq!(merged["hooks"]["Stop"][0]["hooks"][0], unrelated);
        let before = support::snapshot(&target)
            .into_iter()
            .filter(|(p, _, _)| p.file_name().unwrap() != ".scribe-init.lock")
            .collect::<Vec<_>>();
        let again = run();
        assert!(again.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&again.stdout).unwrap()["changed"],
            false
        );
        assert_eq!(
            support::snapshot(&target)
                .into_iter()
                .filter(|(p, _, _)| p.file_name().unwrap() != ".scribe-init.lock")
                .collect::<Vec<_>>(),
            before
        );
        std::fs::write(&skill, "user edited skill\n").unwrap();
        let before = support::snapshot(&target)
            .into_iter()
            .filter(|(p, _, _)| p.file_name().unwrap() != ".scribe-init.lock")
            .collect::<Vec<_>>();
        let conflict = run();
        assert_eq!(conflict.status.code(), Some(1));
        assert!(text(&conflict.stderr).contains("init_conflict"));
        assert_eq!(
            support::snapshot(&target)
                .into_iter()
                .filter(|(p, _, _)| p.file_name().unwrap() != ".scribe-init.lock")
                .collect::<Vec<_>>(),
            before
        );
    }
}

#[test]
fn init_refuses_missing_runtime_unknown_provider_and_symlinked_artifacts() {
    let f = Fixture::new();
    let target = f.elsewhere("project");
    std::fs::create_dir(&target).unwrap();
    let missing = f.run(&[
        "init",
        "--provider",
        "codex",
        "--target",
        target.to_str().unwrap(),
    ]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(text(&missing.stderr).contains("not executable on PATH"));
    assert!(!target.join(".codex").exists());
    let invalid = f.run(&[
        "init",
        "--provider",
        "unknown",
        "--target",
        target.to_str().unwrap(),
    ]);
    assert_eq!(invalid.status.code(), Some(2));
    let path = f.fake_program("codex", "#!/bin/sh\nexit 0\n");
    let outside = f.elsewhere("outside");
    std::fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, target.join(".agents")).unwrap();
    let denied = f
        .cmd(&[
            "init",
            "--provider",
            "codex",
            "--target",
            target.to_str().unwrap(),
        ])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(1));
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
}
