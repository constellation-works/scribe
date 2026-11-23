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
        f.create_private_dir_all(&target);
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
        f.create_private_dir_all(&config);
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
        support::make_private(&hooks);
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
        let canonical =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/scribe");
        let installed = skill.parent().unwrap();
        let skill_tree = |root: &std::path::Path| {
            support::snapshot(root)
                .into_iter()
                .filter(|(path, _, _)| path.is_file())
                .map(|(path, contents, _)| (path.strip_prefix(root).unwrap().to_owned(), contents))
                .collect::<Vec<_>>()
        };
        let expected_files = skill_tree(&canonical);
        assert_eq!(skill_tree(installed), expected_files);
        let installed = installed.canonicalize().unwrap();
        let reported = report["files"].as_array().unwrap();
        for (relative, _) in expected_files {
            let path = installed.join(relative);
            assert!(reported.iter().any(|file| {
                file["path"].as_str() == path.to_str() && file["status"] == "created"
            }));
        }
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
        support::make_private(&skill);
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
fn init_upgrades_a_single_file_receipt_and_preflights_reference_conflicts() {
    let f = Fixture::new();
    let target = f.elsewhere("project");
    let skill_dir = target.join(".agents/skills/scribe");
    let config = target.join(".codex");
    f.create_private_dir_all(&skill_dir.join("references"));
    f.create_private_dir_all(&config);
    let skill = skill_dir.join("SKILL.md");
    let receipt = config.join("scribe-init.json");
    let reference = skill_dir.join("references/session-groups.md");
    let old_skill = "old embedded skill\n";
    std::fs::write(&skill, old_skill).unwrap();
    support::make_private(&skill);
    let legacy = json!({
        "format": 1,
        "skill_sha256": scribe_core::capture::prompt_sha256(old_skill),
        "hooks": {},
    });
    std::fs::write(&receipt, legacy.to_string()).unwrap();
    support::make_private(&receipt);
    let path = f.fake_program("codex", "#!/bin/sh\nexit 0\n");
    let run = || {
        f.cmd(&[
            "init",
            "--provider",
            "codex",
            "--target",
            target.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", &path)
        .output()
        .unwrap()
    };
    let snapshot = || {
        support::snapshot(&target)
            .into_iter()
            .filter(|(path, _, _)| !path.ends_with(".scribe-init.lock"))
            .collect::<Vec<_>>()
    };
    std::fs::write(&reference, "unowned reference\n").unwrap();
    support::make_private(&reference);
    let before = snapshot();
    let denied = run();
    assert_eq!(denied.status.code(), Some(1));
    assert!(text(&denied.stderr).contains("init_conflict"));
    assert_eq!(snapshot(), before);

    std::fs::remove_file(&reference).unwrap();
    let upgraded = run();
    assert!(upgraded.status.success(), "{}", text(&upgraded.stderr));
    assert_eq!(
        std::fs::read_to_string(&skill).unwrap(),
        include_str!("../../../skills/scribe/SKILL.md")
    );
    assert_eq!(
        std::fs::read_to_string(&reference).unwrap(),
        include_str!("../../../skills/scribe/references/session-groups.md")
    );

    std::fs::write(&reference, "user edited reference\n").unwrap();
    let before = snapshot();
    let denied = run();
    assert_eq!(denied.status.code(), Some(1));
    assert!(text(&denied.stderr).contains("init_conflict"));
    assert_eq!(snapshot(), before);
}

#[test]
fn init_refuses_missing_runtime_unknown_provider_and_symlinked_artifacts() {
    let f = Fixture::new();
    let target = f.elsewhere("project");
    f.create_private_dir_all(&target);
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
    f.create_private_dir_all(&outside);
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
