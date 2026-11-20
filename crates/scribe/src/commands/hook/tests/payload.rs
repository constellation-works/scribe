//! The transcript read (`payload::read_checked`, `payload::read_for`): one
//! handle, opened without following links, and checked on that handle
//! (STD-03, STD-05 R7).

use crate::app::{AppEnv, Context};
use crate::commands::hook::payload::{self, Transcript, Unreadable};
use scribe_core::SessionId;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const ID: &str = "0a1b2c3d-0000-4000-8000-0000000000d1";

struct Setup {
    dir: tempfile::TempDir,
    ctx: Context,
}

impl Setup {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".claude/projects/-work")).unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let env = AppEnv {
            root: None,
            home: Some(home.into_os_string()),
            claude_config_dir: None,
            codex_home: None,
        };
        let ctx = Context::resolve(Some(&root), &env).unwrap();
        Self { dir, ctx }
    }

    fn project(&self) -> PathBuf {
        self.dir.path().join("home/.claude/projects/-work")
    }

    fn transcript(&self) -> PathBuf {
        self.project().join(format!("{ID}.jsonl"))
    }

    fn id(&self) -> SessionId {
        ID.parse().unwrap()
    }

    /// A file outside the transcripts directory, holding `text`.
    fn secret(&self, text: &str) -> PathBuf {
        let path = self.dir.path().join("secret.txt");
        std::fs::write(&path, text).unwrap();
        path
    }
}

#[test]
fn a_regular_transcript_is_read_and_a_missing_one_is_none() {
    let setup = Setup::new();
    assert!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id())
            .unwrap()
            .is_none()
    );
    std::fs::write(setup.transcript(), "line\n").unwrap();
    let bytes = payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id())
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"line\n");
}

#[test]
fn a_transcript_swapped_for_a_link_after_the_check_is_not_followed() {
    let setup = Setup::new();
    std::fs::write(setup.transcript(), "real\n").unwrap();
    let path = setup.transcript();
    assert_eq!(
        payload::check(&setup.ctx, &path, &setup.id()),
        Ok(Transcript::Present)
    );

    // Between the check and the read, the same user swaps in a link to a
    // file outside the transcripts directory.
    let secret = setup.secret("not a transcript\n");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&secret, &path).unwrap();

    let err = payload::read_for(&setup.ctx, &path).unwrap_err();
    assert!(
        !err.to_string().contains("not a transcript"),
        "the link was followed: {err}"
    );
    match payload::read_checked(&setup.ctx, &path, &setup.id()) {
        Err(Unreadable::Refused(reason)) => {
            assert_eq!(reason, "the transcript is not a regular file");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_linked_directory_above_the_transcript_is_not_followed() {
    let setup = Setup::new();
    let outside = setup.dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join(format!("{ID}.jsonl")), "elsewhere\n").unwrap();
    let linked = setup.dir.path().join("home/.claude/projects/-linked");
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    let path = linked.join(format!("{ID}.jsonl"));
    assert!(matches!(
        payload::read_checked(&setup.ctx, &path, &setup.id()),
        Err(Unreadable::Refused(_))
    ));
}

#[test]
fn a_pipe_in_place_of_the_transcript_is_refused_without_blocking() {
    let setup = Setup::new();
    rustix::fs::mknodat(
        rustix::fs::CWD,
        setup.transcript(),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(matches!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id()),
        Err(Unreadable::Refused(_))
    ));
}

#[test]
fn a_transcript_named_for_another_session_is_refused_before_it_is_opened() {
    let setup = Setup::new();
    let other: SessionId = "0a1b2c3d-0000-4000-8000-0000000000d2".parse().unwrap();
    std::fs::write(setup.transcript(), "x\n").unwrap();
    assert!(matches!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &other),
        Err(Unreadable::Refused(_))
    ));
}
