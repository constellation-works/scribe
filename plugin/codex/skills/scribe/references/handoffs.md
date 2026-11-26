# Session handoffs

Write a handoff when the human explicitly asks for one in this session, or
as the first step of a session refresh. Do not write one by default
or from routines, scheduled or background jobs, Orbit runs or subagents.

When they ask for a handoff, write it under your own `--author` with the
shipped `handoff` tag, naming this session in `--related` and quoting their
request word for word from one of their typed turns in this session:

```sh
scribe entry add --tag handoff --author <name> --project <project> \
  --related session:<this session id> [--mode continue|new] --quote '<their request>' \
  '## State
...
## In flight
...
## Next
...
## Waiting on you
...
## Do not touch
...
## Pointers
...'
```

- **Quote check.** A supplied `--quote` must quote a typed turn verbatim for
  at least 8 characters. For self-refresh, `--mode continue|new` may be used
  without a quote when `CLAUDE_CODE_SESSION_ID` matches the related session
  and a typed user turn is in its transcript or verified clear or launch
  ancestry, within a shared total of 16 transcripts across all launch branches
  (the current session plus up to 15 predecessors). Peer and scheduled prompts
  are not typed turns.
- **Works when the session is off.** Writing a handoff needs no session
  record and does not switch the session on; no session pass reads an off
  session, and even in an on session the pass never reads handoffs.
- **Fixed headings and size limit.** The body must start with `## State`,
  carry all six `## ` headings in exact order (`State`, `In flight`,
  `Next`, `Waiting on you`, `Do not touch`, `Pointers`), and fit within
  3,000 characters (one screen). Write `(none)` under a heading that has
  nothing to report. Scribe refuses a missing heading, wrong order, extra
  heading or oversized body (`handoff_shape`, `handoff_too_long`).
- **Multiple handoffs.** Each is a new `handoff-<n>`. In the same project, a
  new handoff supersedes earlier active handoffs for its session and any
  ancestor reached through recorded `continues` links, including pending
  links. Superseded handoffs are hidden from `scribe session show <id>` and
  `brief`. Read them with `scribe entry show <id>` or `scribe entry list --tag
  handoff --status superseded`; omitting `--status` also includes them.
- **Removal.** After writing a handoff, echo its id and how to drop it:
  `scribe: wrote <id> (drop: #scribe retire <id>)`.
- **Attribution.** A handoff is an agent note, never the human's words;
  `search` and `entry show` treat it as your account.
