use crate::commands::hook::project::{MAX_METADATA_BYTES, of, remote_url, repo_name};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ORIGIN: &str = "[remote \"origin\"]\n\turl = git@host:owner/the-repo.git\n";

#[test]
fn the_repository_name_is_the_last_segment_of_the_remote_url() {
    for url in [
        "https://github.com/constellation-works/scribe.git",
        "https://github.com/constellation-works/scribe/",
        "git@github.com:constellation-works/scribe.git",
        "ssh://git@host/scribe",
        "/srv/git/scribe.git",
    ] {
        assert_eq!(repo_name(url).as_deref(), Some("scribe"), "{url}");
    }
    assert_eq!(repo_name(""), None);
}

#[test]
fn origin_wins_over_the_first_remote() {
    let config = "[core]\n\turl = not-a-remote\n[remote \"upstream\"]\n\turl = https://h/up/first.git\n[remote \"origin\"]\n\turl = https://h/me/mine.git\n";
    assert_eq!(remote_url(config).as_deref(), Some("https://h/me/mine.git"));
    let no_origin = "[remote \"upstream\"]\n\turl = https://h/up/first.git\n";
    assert_eq!(
        remote_url(no_origin).as_deref(),
        Some("https://h/up/first.git")
    );
    assert_eq!(remote_url("[core]\n\tbare = false\n"), None);
}

#[test]
fn a_worktree_finds_the_shared_config_and_a_plain_directory_its_basename() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    std::fs::write(main.join(".git/config"), ORIGIN).unwrap();
    std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
    let worktree = dir.path().join("elsewhere/wt-dir");
    std::fs::create_dir_all(worktree.join("sub")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    )
    .unwrap();
    assert_eq!(of(&worktree.join("sub")).as_str(), "the-repo");
    assert_eq!(of(&main).as_str(), "the-repo");
    let plain = dir.path().join("plain dir");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(of(&plain).as_str(), "plain-dir");
}

/// A checkout at `root` whose `.git` directory holds `config`.
fn checkout(root: &Path, config: &str) -> PathBuf {
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/config"), config).unwrap();
    root.to_path_buf()
}

/// `text` followed by `fill` until it is exactly `len` bytes long.
fn padded(text: &str, len: u64, fill: char) -> String {
    let len = usize::try_from(len).unwrap();
    assert!(text.len() <= len, "the text is longer than {len} bytes");
    format!("{text}{}", fill.to_string().repeat(len - text.len()))
}

#[test]
fn a_config_at_the_size_limit_is_read_and_one_past_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let at_limit = checkout(
        &dir.path().join("at-limit"),
        &padded(ORIGIN, MAX_METADATA_BYTES, '#'),
    );
    let past_limit = checkout(
        &dir.path().join("past-limit"),
        &padded(ORIGIN, MAX_METADATA_BYTES + 1, '#'),
    );
    assert_eq!(of(&at_limit).as_str(), "the-repo");
    assert_eq!(of(&past_limit).as_str(), "past-limit");
}

#[test]
fn an_oversized_worktree_pointer_or_common_dir_falls_back_to_the_basename() {
    let dir = tempfile::tempdir().unwrap();
    let main = checkout(&dir.path().join("main"), ORIGIN);
    let gitdir = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&gitdir).unwrap();
    let worktree = dir.path().join("wt-dir");
    std::fs::create_dir_all(&worktree).unwrap();

    // The common-directory pointer names the real directory once trimmed, so
    // only the size check tells this apart from a valid pointer.
    std::fs::write(
        gitdir.join("commondir"),
        padded("../..", MAX_METADATA_BYTES + 1, ' '),
    )
    .unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    assert_eq!(of(&worktree).as_str(), "wt-dir");

    std::fs::write(gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        worktree.join(".git"),
        padded(
            &format!("gitdir: {}\n", gitdir.display()),
            MAX_METADATA_BYTES + 1,
            ' ',
        ),
    )
    .unwrap();
    assert_eq!(of(&worktree).as_str(), "wt-dir");
}

/// A FIFO at `path`, made by `mkfifo(1)`: rustix does not offer `mkfifoat` on
/// Apple targets, and the CI macOS job builds these tests.
#[cfg(unix)]
fn fifo(path: &Path) {
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo {}", path.display());
}

/// Project discovery for `cwd`, asked on another thread. A read that waits
/// on a FIFO fails the test here instead of hanging the run.
#[cfg(unix)]
fn discovered_promptly(cwd: &Path) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    let cwd = cwd.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(of(&cwd).as_str().to_owned());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("project discovery waited on a Git metadata file")
}

#[cfg(unix)]
#[test]
fn a_fifo_config_falls_back_to_the_basename_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fifo-config");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    fifo(&root.join(".git/config"));
    assert_eq!(discovered_promptly(&root), "fifo-config");
}

#[cfg(unix)]
#[test]
fn a_fifo_worktree_pointer_or_common_dir_falls_back_to_the_basename_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let pointer = dir.path().join("fifo-pointer");
    std::fs::create_dir_all(&pointer).unwrap();
    fifo(&pointer.join(".git"));
    assert_eq!(discovered_promptly(&pointer), "fifo-pointer");

    let main = checkout(&dir.path().join("main"), ORIGIN);
    let gitdir = main.join(".git/worktrees/wt");
    std::fs::create_dir_all(&gitdir).unwrap();
    fifo(&gitdir.join("commondir"));
    let worktree = dir.path().join("fifo-common");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    assert_eq!(discovered_promptly(&worktree), "fifo-common");
}

#[cfg(unix)]
#[test]
fn a_linked_config_is_refused_rather_than_followed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("linked");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let real = dir.path().join("real-config");
    std::fs::write(&real, ORIGIN).unwrap();
    std::os::unix::fs::symlink(&real, root.join(".git/config")).unwrap();
    assert_eq!(of(&root).as_str(), "linked");
}
