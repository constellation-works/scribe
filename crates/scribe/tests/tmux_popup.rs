//! Behaviour of `plugin/tmux/scribe-tmux` (S13): driven through a fake
//! `fzf` (and fake `tmux`, `pbcopy` and pager) on `PATH` against the real
//! binary and a seeded fixture store, so no test touches the real store,
//! a real tmux server or the host clipboard.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use support::Fixture;

/// The script under test, in the plugin tree the change ships.
fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin/tmux/scribe-tmux")
}

const FAKE_FZF: &str = r#"#!/bin/sh
cap=$FAKE_CAP
printf '%s\n' "$@" > "$cap/fzf-args.txt"
cat > "$cap/fzf-input.txt"
# Simulate one key press: run the bind's execute-silent command, then its
# reload command, whose output replaces the list fzf would show.
if [ -n "${FAKE_FZF_PRESS:-}" ]; then
  for a in "$@"; do
    case $a in
    "$FAKE_FZF_PRESS":execute-silent\(*\)+reload\(*\))
      cmds=$(printf '%s' "$a" | sed 's/.*execute-silent(\(.*\))+reload(\(.*\))/\1\
\2/')
      printf '%s\n' "$cmds" | while IFS= read -r c; do [ -n "$c" ] && sh -c "$c"; done > "$cap/fzf-reload.txt"
      cp "$cap/fzf-reload.txt" "$cap/fzf-input.txt" ;;
    "$FAKE_FZF_PRESS":execute-silent\(*\))
      cmd=$(printf '%s' "$a" | sed 's/.*execute-silent(\(.*\))/\1/')
      sh -c "$cmd" >/dev/null 2>&1 || true ;;
    esac
  done
fi
row=$(awk -F '\t' -v id="${FAKE_PICK:-}" 'id == "" { print; exit } $2 == id { print; exit }' "$cap/fzf-input.txt")
[ -n "$row" ] || exit 1
kind=$(printf '%s' "$row" | cut -f1)
id=$(printf '%s' "$row" | cut -f2)
if [ -n "${FAKE_FZF_PREVIEW:-}" ]; then
  prev=
  for a in "$@"; do
    if [ "$take" = 1 ]; then prev=$a; take=; fi
    [ "$a" = --preview ] && take=1
  done
  tmpl=$(printf '%s' "$prev" | sed "s/{1}/$kind/g; s/{2}/$id/g")
  sh -c "$tmpl" > "$cap/preview.txt" 2>&1 || true
fi
if [ -n "${FAKE_FZF_COPY:-}" ]; then
  for a in "$@"; do
    case $a in
    ctrl-y:execute-silent\(*\))
      cmd=$(printf '%s' "$a" | sed "s/.*execute-silent(\(.*\))/\1/; s/{2}/$id/g")
      sh -c "$cmd" >/dev/null 2>&1 || true ;;
    esac
  done
fi
printf '%s\n' "$row"
"#;

const FAKE_PAGER: &str = "#!/bin/sh\ncat > \"$FAKE_CAP/pager.txt\"\n";
const FAKE_TMUX: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$FAKE_CAP/tmux-args.txt\"\n";
const FAKE_PBCOPY: &str = "#!/bin/sh\ncat > \"$FAKE_CAP/pbcopy.txt\"\n";

/// A fixture with the script, the real binary (as `scribe`), the fakes and
/// a capture directory wired up; the returned command runs the script.
fn popup(fx: &Fixture) -> (Command, PathBuf) {
    let path = fx.fake_program("fzf", FAKE_FZF);
    fx.fake_program("pager", FAKE_PAGER);
    fx.fake_program("tmux", FAKE_TMUX);
    fx.fake_program("pbcopy", FAKE_PBCOPY);
    // The real binary, under the name the script looks up on PATH.
    let bin = fx.elsewhere("bin");
    #[cfg(unix)]
    std::os::unix::fs::symlink(support::BIN, bin.join("scribe")).expect("link scribe");
    let cap = fx.elsewhere("cap");
    std::fs::create_dir_all(&cap).expect("create capture dir");
    let mut cmd = Command::new(script());
    cmd.env_clear()
        .env("HOME", fx.home())
        .env("SCRIBE_ROOT", fx.root())
        .env("PATH", &path)
        .env("PAGER", "pager")
        .env("FAKE_CAP", &cap);
    (cmd, cap)
}

fn read(cap: &Path, name: &str) -> String {
    std::fs::read_to_string(cap.join(name)).unwrap_or_default()
}

#[test]
fn without_fzf_it_exits_1_with_an_install_hint() {
    let fx = Fixture::new();
    // A PATH holding only the (symlinked) real binary: no fzf anywhere,
    // whatever the host has installed.
    let bin = fx.elsewhere("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    #[cfg(unix)]
    std::os::unix::fs::symlink(support::BIN, bin.join("scribe")).expect("link scribe");
    let out: Output = Command::new(script())
        .env_clear()
        .env("HOME", fx.home())
        .env("SCRIBE_ROOT", fx.root())
        .env("PATH", &bin)
        .output()
        .expect("run script");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("fzf is not installed"), "{stderr}");
    assert!(stderr.contains("Install fzf"), "{stderr}");
}

#[test]
fn enter_opens_the_picked_session_with_its_log_in_the_pager() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_PICK", "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02")
        .env("FAKE_FZF_PREVIEW", "1")
        .output()
        .expect("run script");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The list fzf received is the sessions list, kind-prefixed.
    assert!(read(&cap, "fzf-input.txt").contains("session\t7c2b9d14"));
    // The preview is `session show` (no --log): header and table of contents.
    let preview = read(&cap, "preview.txt");
    assert!(
        preview.contains("7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"),
        "{preview}"
    );
    // Enter opened the record with --log in the pager: both segments listed.
    let pager = read(&cap, "pager.txt");
    assert!(pager.contains("segment\t1\tsealed"), "{pager}");
    assert!(pager.contains("segment\t2\topen"), "{pager}");
}

#[test]
fn switching_to_entries_and_opening_one_shows_it_in_the_pager() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-e")
        .env("FAKE_PICK", "decisions-2")
        .output()
        .expect("run script");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The reload produced active entries only (decisions-1 is superseded).
    let reload = read(&cap, "fzf-reload.txt");
    assert!(reload.contains("entry\tdecisions-2"), "{reload}");
    assert!(!reload.contains("decisions-1"), "{reload}");
    let pager = read(&cap, "pager.txt");
    assert!(pager.contains("squash-merge agent PRs"), "{pager}");
}

#[test]
fn ctrl_y_copies_the_picked_id() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_PICK", "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02")
        .env("FAKE_FZF_COPY", "1")
        .output()
        .expect("run script");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(read(&cap, "tmux-args.txt").contains("7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"));
    assert_eq!(
        read(&cap, "pbcopy.txt"),
        "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"
    );
}

#[test]
fn the_proposals_view_previews_the_proposal() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-p")
        .env("FAKE_PICK", "p-1")
        .env("FAKE_FZF_PREVIEW", "1")
        .output()
        .expect("run script");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(read(&cap, "fzf-reload.txt").contains("proposal\tp-1"));
    let preview = read(&cap, "preview.txt");
    assert!(preview.contains("older: decisions-1"), "{preview}");
    assert!(preview.contains("state: open"), "{preview}");
}

#[test]
fn the_transcripts_view_says_when_nothing_is_uncataloged() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    // The fixture has no Claude Code transcripts, so the list succeeds
    // empty and the view shows that notice. Enter on it does not page.
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-b")
        .output()
        .expect("run script");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reload = read(&cap, "fzf-reload.txt");
    assert!(
        reload.contains("no uncataloged transcripts in the last 30 days"),
        "{reload}"
    );
    assert!(reload.contains("transcript\t-"), "{reload}");
    assert!(read(&cap, "pager.txt").is_empty());
}
