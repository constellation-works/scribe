use crate::composition::{Env as AppEnv, resolve_root, transcripts_dir};
use crate::runtime::errors::CommandError as CliError;
use std::path::{Path, PathBuf};

fn env(root: Option<&str>, home: Option<&str>) -> AppEnv {
    AppEnv {
        root: root.map(Into::into),
        home: home.map(Into::into),
        claude_config_dir: None,
        codex_home: None,
        claude_entrypoint: None,
        ..Default::default()
    }
}

#[test]
fn the_flag_beats_the_environment_which_beats_home() {
    let all = env(Some("/from-env"), Some("/home/u"));
    assert_eq!(
        resolve_root(Some(Path::new("/from-flag")), &all).unwrap(),
        PathBuf::from("/from-flag")
    );
    assert_eq!(
        resolve_root(None, &all).unwrap(),
        PathBuf::from("/from-env")
    );
    assert_eq!(
        resolve_root(None, &env(None, Some("/home/u"))).unwrap(),
        PathBuf::from("/home/u/.scribe")
    );
}

#[test]
fn empty_variables_count_as_unset() {
    assert_eq!(
        resolve_root(None, &env(Some(""), Some("/home/u"))).unwrap(),
        PathBuf::from("/home/u/.scribe")
    );
    assert!(matches!(
        resolve_root(None, &env(Some(""), Some(""))),
        Err(CliError::NoDataDir)
    ));
}

#[test]
fn transcripts_are_under_claude_config_dir_else_home() {
    let mut both = env(None, Some("/home/u"));
    assert_eq!(
        transcripts_dir(&both),
        Some(PathBuf::from("/home/u/.claude/projects"))
    );
    both.claude_config_dir = Some("/elsewhere/claude".into());
    assert_eq!(
        transcripts_dir(&both),
        Some(PathBuf::from("/elsewhere/claude/projects"))
    );
    both.claude_config_dir = Some("".into());
    assert_eq!(
        transcripts_dir(&both),
        Some(PathBuf::from("/home/u/.claude/projects")),
        "empty counts as unset"
    );
    assert_eq!(transcripts_dir(&env(None, None)), None);
}
