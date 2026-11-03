use crate::commands::hook::project::{of, remote_url, repo_name};

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
    std::fs::write(
        main.join(".git/config"),
        "[remote \"origin\"]\n\turl = git@host:owner/the-repo.git\n",
    )
    .unwrap();
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
