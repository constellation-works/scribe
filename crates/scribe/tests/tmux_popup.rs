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
    // The preview includes the session's header, summary and log text.
    let preview = read(&cap, "preview.txt");
    assert!(
        preview.contains("7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02"),
        "{preview}"
    );
    assert!(
        preview.starts_with("Merge policy for agent PRs\n"),
        "{preview}"
    );
    assert!(
        preview.contains("Discussed how agent PRs are merged"),
        "{preview}"
    );
    assert!(
        preview.contains("Squash-merge agent PRs; releases keep merge commits."),
        "{preview}"
    );
    assert!(
        preview.contains("Drafted the note to the release crew about the new merge policy."),
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

/// The popup shows the full entry in both piped views. Plain non-tty
/// `entry show` still follows the single-line output contract.
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
    // The preview leads with the text, then the metadata.
    let preview = read(&cap, "preview.txt");
    assert!(preview.starts_with("decisions · active\n"), "{preview}");
    assert!(
        preview.contains("keep merge commits for releases"),
        "{preview}"
    );
    assert!(
        preview.contains("said        #scribe add this to"),
        "{preview}"
    );
    assert!(preview.contains("cite        session"), "{preview}");
    // Enter keeps the full record.
    let pager = read(&cap, "pager.txt");
    assert!(pager.contains("said        #scribe add this to"), "{pager}");
    assert!(pager.contains("keep merge commits for releases"), "{pager}");
    assert!(pager.contains("cite        session"), "{pager}");
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
    assert!(preview.contains("older      decisions-1"), "{preview}");
    assert!(preview.contains("state      open"), "{preview}");
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
    assert!(read(&cap, "preview.txt").contains("Discussed how agent PRs"));
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

// ---- list rows (date, status, title, project; the id stays hidden) ----

const SESSION_A: &str = "7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02";
const SESSION_B: &str = "0f8e7a52-3c1d-4b6e-9a2f-5d4c3b2a1e01";
const SESSION_INACTIVE: &str = "3d5e7f90-1a2b-4c3d-8e9f-0a1b2c3d4e05";

/// The `kind`, `id` and display fields of every row.
fn fields(rows: &str) -> Vec<(String, String, String)> {
    rows.lines()
        .map(|l| {
            let mut f = l.splitn(3, '\t');
            let kind = f.next().unwrap_or_default().to_owned();
            let id = f.next().unwrap_or_default().to_owned();
            let display = f.next().unwrap_or_default().to_owned();
            (kind, id, display)
        })
        .collect()
}

/// The visible part of a display field: before the searchable-only text.
fn visible(display: &str) -> &str {
    match display.find(&" ".repeat(100)) {
        Some(at) => &display[..at],
        None => display,
    }
}

/// `scribe tmux rows <view> …` against the seeded fixture.
fn rows(fx: &Fixture, args: &[&str], envs: &[(&str, &str)]) -> String {
    let mut cmd = fx.cmd(&["tmux", "rows"]);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run rows");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    support::text(&out.stdout)
}

fn strip_escapes(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn the_sessions_view_shows_date_status_title_project_newest_first_without_the_id() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    let out = cmd.output().expect("run tmux");
    assert_eq!(out.status.code(), Some(0));
    let input = read(&cap, "fzf-input.txt");
    let rows = fields(&input);
    assert_eq!(rows.len(), 2, "{input}");
    // The id rides on the row, second, and fzf is told to show field 3 only.
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("session", SESSION_A)
    );
    assert_eq!(rows[1].1, SESSION_B);
    assert!(
        read(&cap, "fzf-args.txt")
            .lines()
            .any(|a| a == "--with-nth=3")
    );
    let first = visible(&rows[0].2);
    let second = visible(&rows[1].2);
    // Date, status, title, project, in that order, newest started first.
    let date = |d: &str| d.len() >= 5 && d.as_bytes()[2] == b'-' || d.as_bytes()[4] == b'-';
    assert!(date(first), "{first}");
    assert!(
        first.contains("  open   Merge policy for agent PRs  "),
        "{first}"
    );
    assert!(first.ends_with("constellation-works/scribe"), "{first}");
    assert!(second.contains("  final  Scribe store layout"), "{second}");
    // Neither visible part holds the 36-character id.
    assert!(!first.contains(SESSION_A) && !second.contains(SESSION_B));
    // The project column lines up.
    assert_eq!(
        first.find("constellation-works"),
        second.find("constellation-works"),
        "{first}\n{second}"
    );
}

#[test]
fn a_date_from_another_year_shows_its_year_and_color_off_has_no_escapes() {
    let fx = Fixture::new();
    fx.seed();
    let this = rows(&fx, &["sessions", "--year", "2026"], &[]);
    let other = rows(&fx, &["sessions", "--year", "2027"], &[]);
    let (this, other) = (fields(&this), fields(&other));
    assert!(
        this[0].2.starts_with("11-02  open   Merge policy"),
        "{}",
        this[0].2
    );
    assert!(
        other[0].2.starts_with("2026-11-02  open   Merge policy"),
        "{}",
        other[0].2
    );
    for envs in [
        &[("TERM", "xterm")][..],
        &[("TERM", "xterm"), ("NO_COLOR", "1")][..],
    ] {
        // Without --color, or under NO_COLOR, the rows are plain text.
        let plain = rows(&fx, &["sessions", "--year", "2026"], envs);
        assert!(!plain.contains('\x1b'), "{plain}");
    }
    let no_color = rows(
        &fx,
        &["sessions", "--year", "2026", "--color"],
        &[("TERM", "xterm"), ("NO_COLOR", "1")],
    );
    assert!(!no_color.contains('\x1b'), "{no_color}");
}

#[test]
fn color_wraps_only_the_text_and_keeps_the_columns_aligned() {
    let fx = Fixture::new();
    fx.seed();
    let plain = rows(&fx, &["sessions", "--year", "2026"], &[]);
    let colored = rows(
        &fx,
        &["sessions", "--year", "2026", "--color"],
        &[("TERM", "xterm")],
    );
    assert!(
        colored.contains("\x1b[32mopen\x1b[0m"),
        "open is green: {colored}"
    );
    assert!(
        colored.contains("\x1b[90m"),
        "the project is dimmed: {colored}"
    );
    assert_eq!(strip_escapes(&colored), plain);
}

#[test]
fn inactive_sessions_appear_after_alt_a_dimmed() {
    let fx = Fixture::new();
    fx.seed();
    let shown = rows(&fx, &["sessions", "--year", "2026"], &[]);
    assert!(!shown.contains(SESSION_INACTIVE));
    let all = rows(
        &fx,
        &["sessions", "--hidden", "--year", "2026", "--color"],
        &[("TERM", "xterm")],
    );
    let row = all
        .lines()
        .find(|l| l.contains(SESSION_INACTIVE))
        .expect("inactive row");
    assert!(row.contains("\x1b[90mMerge commits for agent PRs"), "{row}");
    assert!(row.contains("\x1b[90mfinal"), "{row}");
}

#[test]
fn topics_are_searchable_but_not_displayed() {
    let fx = Fixture::new();
    fx.seed();
    let (mut cmd, cap) = popup(&fx);
    cmd.output().expect("run tmux");
    let args = read(&cap, "fzf-args.txt");
    // fzf shows field 3 only and never scrolls the hidden tail into view.
    assert!(args.lines().any(|a| a == "--no-hscroll"), "{args}");
    let rows = fields(&read(&cap, "fzf-input.txt"));
    let display = &rows[0].2;
    assert!(!visible(display).contains("merge-policy"), "{display}");
    assert!(
        display.contains("release merge-policy"),
        "the topics follow the columns"
    );
}

#[test]
fn entries_proposals_and_backfill_rows_have_their_shapes() {
    let fx = Fixture::new();
    fx.seed();
    let entries = fields(&rows(&fx, &["entries", "--year", "2026"], &[]));
    assert_eq!(entries[0].0, "entry");
    assert_eq!(entries[0].1, "decisions-2");
    assert!(
        visible(&entries[0].2).starts_with("11-02  decisions    squash-merge agent PRs;"),
        "{}",
        entries[0].2
    );
    // Superseded entries are hidden until alt-a.
    assert!(entries.iter().all(|e| e.1 != "decisions-1"));
    let proposals = fields(&rows(&fx, &["proposals", "--year", "2026"], &[]));
    assert_eq!(proposals[0].1, "p-1");
    assert!(
        visible(&proposals[0].2).starts_with("11-02  open  decisions-2 supersedes decisions-1:"),
        "{}",
        proposals[0].2
    );

    // A recent transcript with enough turns is a backfill candidate: date,
    // project, first prompt.
    let now = time::OffsetDateTime::now_utc();
    let stamp = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let dir = fx.home().join(".claude/projects/-work");
    std::fs::create_dir_all(&dir).unwrap();
    let lines: Vec<String> = (0..20)
        .map(|n| {
            serde_json::json!({
                "type": "user", "uuid": format!("u-{n}"), "timestamp": stamp,
                "cwd": "/work/constellation",
                "message": {"role": "user", "content": format!("turn {n}: tidy the popup rows")}
            })
            .to_string()
        })
        .collect();
    std::fs::write(
        dir.join("c0ffee00-1111-4222-8333-444455556666.jsonl"),
        lines.join("\n") + "\n",
    )
    .unwrap();
    let year = now.year().to_string();
    let transcripts = fields(&rows(&fx, &["transcripts", "--year", &year], &[]));
    assert_eq!(transcripts[0].0, "transcript");
    assert_eq!(transcripts[0].1, "c0ffee00-1111-4222-8333-444455556666");
    let shown = visible(&transcripts[0].2);
    let date = format!("{:02}-{:02}", u8::from(now.month()), now.day());
    assert!(shown.starts_with(&date), "{shown}");
    assert!(shown.ends_with("turn 0: tidy the popup rows"), "{shown}");
}

#[test]
fn a_failed_or_empty_list_is_one_notice_row() {
    let fx = Fixture::new();
    fx.seed();
    // The backfill list is empty (no transcripts under the fixture home).
    let empty = fields(&rows(&fx, &["transcripts"], &[]));
    assert_eq!(empty.len(), 1);
    assert_eq!(&empty[0].1, "-");
    assert!(
        empty[0].2.contains("no uncataloged transcripts"),
        "{}",
        empty[0].2
    );
    // A store the list refuses (open to other users) fails the list.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(fx.root(), std::fs::Permissions::from_mode(0o755)).unwrap();
        for (view, kind) in [
            ("sessions", "session"),
            ("entries", "entry"),
            ("proposals", "proposal"),
        ] {
            let failed = fields(&rows(&fx, &[view], &[]));
            assert_eq!(failed.len(), 1, "{view}");
            assert_eq!((failed[0].0.as_str(), failed[0].1.as_str()), (kind, "-"));
            assert!(failed[0].2.contains("unavailable"), "{}", failed[0].2);
        }
    }
}

#[test]
fn the_session_preview_leads_with_title_and_summary_and_puts_the_ids_last() {
    let fx = Fixture::new();
    fx.seed();
    let out = fx.ok(&["tmux", "preview", "session", SESSION_A]);
    let preview = support::text(&out.stdout);
    let at = |needle: &str| {
        preview
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}: {preview}"))
    };
    assert!(
        preview.starts_with("Merge policy for agent PRs\n\nDiscussed how"),
        "{preview}"
    );
    assert!(at("Discussed how") < at("## Goals"));
    assert!(at("## Goals") < at("## Next"));
    assert!(at("## Next") < at("id         7c2b9d14"));
    assert!(at("id         7c2b9d14") < at("host       dk-server-1"));
    assert!(at("host       dk-server-1") < at("cwd        /home/daniel/workspace/scribe"));
}

#[test]
fn enter_preview_and_ctrl_y_act_on_the_full_id_in_every_view() {
    let fx = Fixture::new();
    fx.seed();
    let cases = [
        ("ctrl-s", SESSION_A),
        ("ctrl-e", "decisions-2"),
        ("ctrl-p", "p-1"),
    ];
    for (key, id) in cases {
        let (mut cmd, cap) = popup(&fx);
        let out = cmd
            .env("FAKE_FZF_PRESS", key)
            .env("FAKE_PICK", id)
            .env("FAKE_FZF_PREVIEW", "1")
            .env("FAKE_FZF_COPY", "1")
            .output()
            .expect("run tmux");
        assert_eq!(out.status.code(), Some(0), "{key}");
        // ctrl-y copied the hidden id, not what is displayed.
        assert_eq!(read(&cap, "pbcopy.txt"), id, "{key}");
        assert!(read(&cap, "tmux-args.txt").contains(id), "{key}");
        assert!(!read(&cap, "preview.txt").is_empty(), "{key}");
        assert!(!read(&cap, "pager.txt").is_empty(), "{key}");
    }
}

#[test]
fn an_empty_view_is_one_notice_row_worded_for_the_view_and_its_filter() {
    // A store with nothing in it: every list succeeds empty.
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.root()).unwrap();
    support::make_private(&fx.root());
    for (args, kind, text) in [
        (
            &["sessions"][..],
            "session",
            "no active sessions; alt-a shows inactive ones",
        ),
        (&["sessions", "--hidden"][..], "session", "no sessions"),
        (
            &["entries"][..],
            "entry",
            "no active entries; alt-a shows every status",
        ),
        (&["entries", "--hidden"][..], "entry", "no entries"),
        (&["proposals"][..], "proposal", "no open proposals"),
    ] {
        let got = fields(&rows(&fx, args, &[]));
        assert_eq!(got.len(), 1, "{args:?}");
        assert_eq!(
            (got[0].0.as_str(), got[0].1.as_str(), got[0].2.as_str()),
            (kind, "-", text),
            "{args:?}"
        );
    }
}

#[test]
fn enter_and_ctrl_y_on_a_notice_row_do_nothing_and_its_preview_says_so() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.root()).unwrap();
    support::make_private(&fx.root());
    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PREVIEW", "1")
        .env("FAKE_FZF_COPY", "1")
        .output()
        .expect("run tmux");
    assert_eq!(out.status.code(), Some(0));
    assert!(read(&cap, "fzf-input.txt").contains("session\t-\tno active sessions"));
    assert!(read(&cap, "preview.txt").contains("this row is a notice"));
    assert!(read(&cap, "pager.txt").is_empty());
    assert!(read(&cap, "pbcopy.txt").is_empty());
    assert!(read(&cap, "tmux-args.txt").is_empty());
}

/// Daniel's raw words behind the stated candidate.
const STATED_QUOTE: &str = "yeah, for the popup just show the newest five, nothing older.";

/// Replace the fixture's proposals with one open pass candidate carrying
/// scribe's statement beside Daniel's raw quote (format 14).
fn seed_stated_candidate(fx: &Fixture) {
    let path = fx.root().join("proposals.json");
    let cite = serde_json::json!({"session":"7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02",
        "message":"user-popup", "at":"2026-11-02T00:00:00Z",
        "prompt_sha256":"0".repeat(64), "line":1, "cwd":"/work/scribe"});
    let quote = STATED_QUOTE;
    let proposal = serde_json::json!({
        "id":"p-6", "kind":"candidate", "older":"preferences-3", "newer":null,
        "candidate": {
            "tag":"preferences", "role":"user", "quote":quote,
            "statement":"Daniel prefers that the scribe tmux popup lists only the five newest records.",
            "subject":"the scribe tmux popup", "evidence":null, "cite":cite,
            "project":"constellation-works/scribe", "provider":"claude-code",
            "host":"dk-server-1", "admitted":null
        },
        "reason":format!("Unmarked preferences statement: {quote}"), "quotes":[],
        "state":"open", "opened_at":"2026-11-02T00:06:00Z", "settled_at":null,
        "settled_by":null, "author":{"kind":"scribe", "name":"session-pass"},
        "settled_author":null, "updated_by":null, "closed_reason":null, "instruction":null
    });
    std::fs::write(
        &path,
        serde_json::json!({"format":14, "proposals":[proposal]}).to_string(),
    )
    .unwrap();
    support::make_private(&path);
}

#[test]
fn a_stated_candidate_shows_scribes_statement_in_its_row_and_preview() {
    let fx = Fixture::new();
    fx.seed();
    seed_stated_candidate(&fx);
    let proposals = fields(&rows(&fx, &["proposals", "--year", "2026"], &[]));
    assert_eq!(proposals[0].1, "p-6");
    let shown = visible(&proposals[0].2);
    assert!(
        shown.ends_with(
            "record preferences-3 after approval: Daniel prefers that the scribe tmux popup lists only the five newest records."
        ),
        "{shown}"
    );

    let (mut cmd, cap) = popup(&fx);
    let out = cmd
        .env("FAKE_FZF_PRESS", "ctrl-p")
        .env("FAKE_PICK", "p-6")
        .env("FAKE_FZF_PREVIEW", "1")
        .output()
        .expect("run tmux");
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let preview = read(&cap, "preview.txt");
    let head = preview.split("## Details").next().unwrap_or_default();
    assert!(
        head.contains("## Statement\nDaniel prefers that the scribe tmux popup lists only the five newest records."),
        "{preview}"
    );
    assert!(
        !head.contains("Unmarked preferences statement"),
        "{preview}"
    );
    // Daniel's own words stay in the details.
    assert!(preview.contains(STATED_QUOTE), "{preview}");
}
