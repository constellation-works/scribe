# Memory

The brief opens with the memory block: `## Profile (as of <date>)`, the
human's profile in full; `## Now`, every dated thing they plan, attend or
must meet, nearest first, as `- <id> · due <date> · <title>`; and
`## Milestones`, the last five, as `- <id> · <date> · <title>`. In Codex,
or when the brief is missing, run `scribe memory show --json` to read the
same block. Use the human's preferences for rules.

Run `scribe memory show <id> --json` to read one item and its source sessions.

- **Now.** Raise a `now` item at most once before its date and once after
  it, when the conversation goes near it, and each time mark it raised for
  this session: `scribe memory ack <id> --session <this session id>
  --author <name>`.
- **Milestones.** Raise one only when the conversation goes there.
- **The profile** is rebuilt weekly from the switched-on sessions by
  `scribe memory refresh`. When the human says something in it is wrong,
  tell them to edit `memory/profile.md` (the store root's): the next
  refresh takes the edit as their correction. Lines under `## Pinned` are
  theirs and are never rewritten.
- **Wrong or forgotten.** When the human asks scribe to forget a `now`
  item or milestone, run `scribe memory forget <id> --author <name>
  --confirm`; it cannot be undone.
- Never write memory yourself: only the session pass writes `now` items
  and milestones, only from sessions switched on with `/scribe on` or named
  with `session catalog`, and only the refresh writes the profile. A session
  that is off gets no memory pass. Memory evidence is the human's own typed
  words; the assistant's turns are context only. `scribe memory rules`
  prints the fixed rules.
