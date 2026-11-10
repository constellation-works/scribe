# scribe-tmux — the tmux popup

`scribe-tmux` is a POSIX shell script that browses scribe in a tmux
popup: sessions, entries, proposals and backfill candidates in one `fzf`
list, each record previewed with `scribe … show`. It only reads — every
command it runs is a list or a show — and it adds no Rust dependency.

## Install

Put the script on `PATH`, with `scribe` and `fzf`:

```sh
install -m 755 plugin/tmux/scribe-tmux ~/.local/bin/scribe-tmux
```

Bind it to a key in `~/.tmux.conf`:

```tmux
bind-key S display-popup -E -w 90% -h 85% scribe-tmux
```

Reload tmux (`tmux source-file ~/.tmux.conf`) and press `prefix` then
`S`. Without `fzf` the script exits 1 and says how to install it.

## Keys

| Key | Does |
|---|---|
| `enter` | Open the record in `$PAGER` (else `less`): a session with `--log`, an entry with `entry show`, a proposal as its `proposal list` row (there is no `proposal show`). On a backfill candidate, put `scribe session catalog <id> --author daniel` in the tmux paste buffer and say so — cataloging runs from a shell, where its progress is visible. |
| `ctrl-y` | Copy the record's id: `tmux set-buffer`, and `pbcopy` when present. |
| `ctrl-s` / `ctrl-e` / `ctrl-p` / `ctrl-b` | Switch the list to sessions, active entries, open proposals or backfill candidates (uncataloged transcripts from the last 30 days). |
| `alt-a` | Toggle the hidden records in the list: inactive sessions join `session list --include-inactive`, and the entry list drops `--status active`. |

The preview pane is `scribe session show <id>`, `scribe entry show <id>`,
the proposal's list row formatted field by field, or `scribe transcript
show <id>` for a candidate.

Backfill candidates come from `scribe transcript list` over the last 30
days. An empty list, or a list that fails, shows one notice row instead
of failing the popup, and its preview says which case that row is.
