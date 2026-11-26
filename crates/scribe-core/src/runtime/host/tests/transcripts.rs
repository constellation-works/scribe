//! The transcript read (`payload::read_checked`, `payload::read_for`): one
//! handle, opened without following links, and checked on that handle
//! (STD-03, STD-05 R7).

use crate::composition::{Context, Env as AppEnv};
use crate::runtime::host::transcripts::{self as payload, Transcript, Unreadable};
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
            claude_entrypoint: None,
            ..Default::default()
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

/// Write `text` as the session's transcript at exactly `mode`: the process
/// umask must not decide what a test checks.
fn write_transcript(setup: &Setup, text: &str, mode: u32) -> PathBuf {
    let path = setup.transcript();
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

/// Modes that another user can write: a group or an other write bit.
const WRITABLE_BY_OTHERS: [u32; 4] = [0o664, 0o646, 0o622, 0o666];
const WRITE_REFUSAL: &str = "writable by another user";

#[test]
fn a_regular_transcript_is_read_and_a_missing_one_is_none() {
    let setup = Setup::new();
    assert!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id())
            .unwrap()
            .is_none()
    );
    write_transcript(&setup, "line\n", 0o600);
    let bytes = payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id())
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"line\n");
}

#[test]
fn a_transcript_swapped_for_a_link_after_the_check_is_not_followed() {
    let setup = Setup::new();
    let path = write_transcript(&setup, "real\n", 0o600);
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
    let made = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(setup.transcript())
        .status()
        .unwrap();
    assert!(made.success());
    assert!(matches!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &setup.id()),
        Err(Unreadable::Refused(_))
    ));
}

#[test]
fn a_transcript_named_for_another_session_is_refused_before_it_is_opened() {
    let setup = Setup::new();
    let other: SessionId = "0a1b2c3d-0000-4000-8000-0000000000d2".parse().unwrap();
    write_transcript(&setup, "x\n", 0o600);
    assert!(matches!(
        payload::read_checked(&setup.ctx, &setup.transcript(), &other),
        Err(Unreadable::Refused(_))
    ));
}

#[test]
fn a_transcript_only_its_owner_can_write_is_accepted_whatever_its_read_bits() {
    for mode in [0o600, 0o640, 0o644, 0o400] {
        let setup = Setup::new();
        let path = write_transcript(&setup, "line\n", mode);
        let id = setup.id();
        assert_eq!(
            payload::check(&setup.ctx, &path, &id),
            Ok(Transcript::Present),
            "mode {mode:o}"
        );
        let bytes = payload::read_checked(&setup.ctx, &path, &id)
            .unwrap()
            .unwrap();
        assert_eq!(bytes, b"line\n", "mode {mode:o}");
    }
}

#[test]
fn a_transcript_writable_by_another_user_is_refused_by_the_path_check() {
    for mode in WRITABLE_BY_OTHERS {
        let setup = Setup::new();
        let path = write_transcript(&setup, "line\n", mode);
        let checked = payload::check(&setup.ctx, &path, &setup.id());
        assert!(
            matches!(&checked, Err(why) if why.contains(WRITE_REFUSAL)),
            "mode {mode:o}: {checked:?}"
        );
    }
}

#[test]
fn a_transcript_writable_by_another_user_is_refused_by_the_open_handle_and_every_read() {
    for mode in WRITABLE_BY_OTHERS {
        let setup = Setup::new();
        let path = write_transcript(&setup, "line\n", mode);
        let id = setup.id();
        let ctx = &setup.ctx;

        let opened = payload::open_checked(ctx, &path, &id);
        assert!(
            matches!(&opened, Err(Unreadable::Refused(why)) if why.contains(WRITE_REFUSAL)),
            "open_checked, mode {mode:o}: {opened:?}"
        );
        let read = payload::read_checked(ctx, &path, &id);
        assert!(
            matches!(&read, Err(Unreadable::Refused(why)) if why.contains(WRITE_REFUSAL)),
            "read_checked, mode {mode:o}: {read:?}"
        );
        let bytes = payload::read_checked_bytes(ctx, &path, &id);
        assert!(
            matches!(&bytes, Err(Unreadable::Refused(why)) if why.contains(WRITE_REFUSAL)),
            "read_checked_bytes, mode {mode:o}: {bytes:?}"
        );
        let header = payload::claude_header(ctx, &path, &id);
        assert!(
            matches!(&header, Err(Unreadable::Refused(why)) if why.contains(WRITE_REFUSAL)),
            "claude_header, mode {mode:o}: {header:?}"
        );
        let unchecked = payload::read_for(ctx, &path);
        assert!(
            matches!(&unchecked, Err(e) if e.to_string().contains(WRITE_REFUSAL)),
            "read_for, mode {mode:o}: {unchecked:?}"
        );
    }
}

#[test]
fn a_transcript_made_writable_after_its_check_is_refused_on_read() {
    let setup = Setup::new();
    let path = write_transcript(&setup, "line\n", 0o600);
    let id = setup.id();
    assert_eq!(
        payload::check(&setup.ctx, &path, &id),
        Ok(Transcript::Present)
    );

    // Another user's write bit appears between the path check and the read.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
    let read = payload::read_checked(&setup.ctx, &path, &id);
    assert!(
        matches!(&read, Err(Unreadable::Refused(why)) if why.contains(WRITE_REFUSAL)),
        "{read:?}"
    );
}

#[test]
fn a_codex_rollout_writable_by_another_user_is_refused_by_the_path_check() {
    let setup = Setup::new();
    let ctx = setup.ctx.with_provider(scribe_core::Provider::Codex);
    let sessions = setup.dir.path().join("home/.codex/sessions/2026/11/23");
    std::fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join(format!("rollout-2026-11-23T00-00-00-{ID}.jsonl"));
    std::fs::write(&path, "x\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
    // The preflight's own reason: the Codex read guard's wording says "Codex
    // transcript", so this cannot pass by the read refusal alone.
    let checked = payload::check(&ctx, &path, &setup.id());
    assert!(
        matches!(&checked, Err(why) if why.starts_with("the transcript is writable by another user")),
        "{checked:?}"
    );
}
