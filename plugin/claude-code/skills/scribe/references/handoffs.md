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

- **Quote check.** When supplied, `--quote` must be a verbatim span (at
  least 8 characters) of a typed turn in that session. For a self-refresh,
  `--mode continue|new` may be used without a quote when the current
  `CLAUDE_CODE_SESSION_ID` matches the related session and a typed user turn
  is in its transcript or its verified clear or launch ancestry (at most 16
  predecessors). Peer messages and scheduled prompts do not count as typed
  turns.
- **Works when the session is off.** Writing a handoff needs no session
  record and does not switch the session on; no session pass reads an off
  session, and even in an on session the pass never reads handoffs.
- **Fixed headings and size limit.** The body must start with `## State`,
  carry all six `## ` headings in exact order (`State`, `In flight`,
  `Next`, `Waiting on you`, `Do not touch`, `Pointers`), and fit within
  3,000 characters (one screen). Write `(none)` under a heading that has
  nothing to report. Scribe refuses a missing heading, wrong order, extra
  heading or oversized body (`handoff_shape`, `handoff_too_long`).
- **Multiple handoffs and removal.** Each handoff is a new entry (`handoff-<n>`),
  and `scribe session show <id>` lists them newest first beside scribe's
  header. After writing one, echo its id and how to drop it:
  `scribe: wrote <id> (drop: #scribe retire <id>)`.
- **Attribution.** A handoff is an agent note, never the human's words;
  `ask` and `search` treat it as your account.
