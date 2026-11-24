# Looking up evidence

To look something up, use `scribe search "<words>" --json`,
`scribe entry list --json`, `scribe session list --json`, and
`scribe proposal list --json`. Read full evidence with
`scribe entry show <id> --json` or `scribe session show <id> --log --json`.
A citation `session:<id>#<n>` names one segment.
A session id may also be its short `s-<n>` (`short_id` in JSON); cite with
the full `id`.

`scribe search "<words>" --kind message --json` searches stored turns from
switched-on sessions and group members instead (each hit cites its session).

A question about what the human typed or said searches `--kind message`. The
default kinds are entries and sessions, so a line the human typed is found
only with `--kind message`.

An empty default search is not evidence that a line was never said: check
`--kind message` before saying it was not. When a default search finds no
entry or session but stored messages match, its last stderr line gives their
count and the `--kind message` command that lists them, and `--json` carries
the count as `message_matches`.

Most are the human's own typed turns. An assistant turn is stored only when a
session pass drafts a claim from it, which needs a tag whose config `from` is
`assistant` or `both` (the shipped tags read only the user), and never when it
holds a finance or sensitive-event word. Each hit's `title` ends with its
speaker, `(user)` or `(assistant)`; its `message` is the provider's message id.
Attribute each assistant hit to the assistant.

Group consent captures eligible typed turns even when cataloging is off. An
explicit `group open`, `group join` or `/scribe group add` also captures the
session's earlier typed turns; the typed `/scribe group <slug>` join and
clear-time inheritance capture from the time the session joined. Search finds
them with or without `--group <id>`. Group
turns are always the human's. Leaving removes turns held only on group consent.
They feed no memory and make no catalog entry.

Piped session IDs, the session-start brief and `group show` use `s-<n>` when cataloged, otherwise a unique prefix of at least eight characters. Both can be passed back; JSON and stored references retain the full provider ID.
