//! Behaviour of `scribe tmux` (S13, embedded in the binary since S14):
//! driven through a fake `fzf` (and fake `tmux`, `pbcopy` and pager) on
//! `PATH` against the real binary and a seeded fixture store, so no test
//! touches the real store, a real tmux server or the host clipboard.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use support::Fixture;

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

/// A fixture with the fakes and a capture directory wired up; the returned
/// command runs `scribe tmux` from the started binary.
fn popup(fx: &Fixture) -> (Command, PathBuf) {
    let path = fx.fake_program("fzf", FAKE_FZF);
    fx.fake_program("pager", FAKE_PAGER);
    fx.fake_program("tmux", FAKE_TMUX);
    fx.fake_program("pbcopy", FAKE_PBCOPY);
    let cap = fx.elsewhere("cap");
    std::fs::create_dir_all(&cap).expect("create capture dir");
    let mut cmd = Command::new(support::BIN);
    cmd.arg("tmux")
        .env_clear()
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
    // A PATH holding nothing: no fzf anywhere, whatever the host has.
    let bin = fx.elsewhere("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    let out: Output = Command::new(support::BIN)
        .arg("tmux")
        .env_clear()
        .env("HOME", fx.home())
        .env("SCRIBE_ROOT", fx.root())
        .env("PATH", &bin)
        .output()
        .expect("run tmux");
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("fzf is not installed"), "{stderr}");
    assert!(stderr.contains("install fzf"), "{stderr}");
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
        .expect("run tmux");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The list fzf received is the sessions list, kind-prefixed.
    assert!(read(&cap, "fzf-input.txt").contains("session\t7c2b9d14"));
    // The preview is the session's header and summary, not only its list line.
    let preview = read(&cap, "preview.txt");
    assert!(
        preview.contains("7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"),
        "{preview}"
    );
    assert!(preview.contains("## Summary"), "{preview}");
    assert!(
        preview.contains("Discussed how agent PRs are merged"),
        "{preview}"
    );
    // Enter opened the record with --log in the pager: the log text itself.
    let pager = read(&cap, "pager.txt");
    assert!(
        pager.contains("### 1. Merge policy for agent PRs"),
        "{pager}"
    );
    assert!(
        pager.contains("Squash-merge agent PRs; releases keep merge commits."),
        "{pager}"
    );
    assert!(pager.contains("### 2. Telling the release crew"), "{pager}");
}

/// Regression: `show` prints only its list line when stdout is not a
/// terminal, and the popup previews through a pipe. The popup must ask for
/// the detailed format, so an entry shows its full detail and a plain,
/// non-tty `show` stays a single line (the output contract).
#[test]
fn the_entry_preview_and_open_show_full_detail_from_a_pipe() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-e")
        .env("FAKE_PICK", "decisions-2")
        .env("FAKE_FZF_PREVIEW", "1")
        .output()
        .expect("run tmux");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for (name, text) in [
        ("preview", read(&cap, "preview.txt")),
        ("pager", read(&cap, "pager.txt")),
    ] {
        assert!(
            text.contains("said        #scribe add this to"),
            "{name}: {text}"
        );
        assert!(
            text.contains("keep merge commits for releases"),
            "{name}: {text}"
        );
        assert!(text.contains("cite        session"), "{name}: {text}");
    }
    // The contract is unchanged: plain non-tty `show` is still one line.
    let plain = fx.ok(&["entry", "show", "decisions-2"]);
    assert_eq!(String::from_utf8_lossy(&plain.stdout).lines().count(), 1);
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
        .expect("run tmux");
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
        .expect("run tmux");
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
        .expect("run tmux");
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
        .expect("run tmux");
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

#[test]
fn the_popups_commands_run_the_started_binary_not_the_one_on_path() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    // A different `scribe` first on PATH: it records that it ran.
    fx.fake_program(
        "scribe",
        "#!/bin/sh\nprintf 'shadow ran\\n' >> \"$FAKE_CAP/shadow.txt\"\nexit 1\n",
    );
    let out = cmd
        .env("FAKE_PICK", "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02")
        .env("FAKE_FZF_PREVIEW", "1")
        .output()
        .expect("run tmux");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        read(&cap, "shadow.txt").is_empty(),
        "the scribe on PATH ran: {}",
        read(&cap, "shadow.txt")
    );
    // The started binary listed, previewed and opened the record.
    assert!(read(&cap, "fzf-input.txt").contains("session\t7c2b9d14"));
    assert!(read(&cap, "preview.txt").contains("## Summary"));
    assert!(read(&cap, "pager.txt").contains("### 1. Merge policy"));
}

#[test]
fn tmux_writes_nothing() {
    let fx = Fixture::new();
    fx.seed();
    let before = support::snapshot(&fx.root());
    let (mut cmd, _cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-e")
        .env("FAKE_PICK", "decisions-2")
        .env("FAKE_FZF_PREVIEW", "1")
        .env("FAKE_FZF_COPY", "1")
        .output()
        .expect("run tmux");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(support::snapshot(&fx.root()), before, "the store changed");
    // The snapshot covers the whole root, so an audit line (a new file or
    // an appended one) would show too.
}
