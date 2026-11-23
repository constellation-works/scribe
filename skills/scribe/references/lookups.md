# Looking up evidence

For a synthesized answer, use the current chat's provider:

```sh
scribe --provider <provider> question ask "<question>" --json
```

For focused lookup, use `scribe search "<words>" --json`,
`scribe entry list --json`, `scribe session list --json`, and
`scribe proposal list --json`. Read full evidence with
`scribe entry show <id> --json` or `scribe session show <id> --log --json`.
A citation `session:<id>#<n>` names one segment.
A session id may also be its short `s-<n>` (`short_id` in JSON); cite with
the full `id`.

`scribe search "<words>" --kind message --json` searches stored turns from
switched-on sessions and group members instead (each hit cites its session).

Most are the human's own typed turns. An assistant turn is stored only when a
session pass drafts a claim from it, which needs a tag whose config `from` is
`assistant` or `both` (the shipped tags read only the user), and never when it
holds a finance or sensitive-event word. Each hit's `title` ends with its
speaker, `(user)` or `(assistant)`; its `message` is the provider's message id.
Attribute each assistant hit to the assistant.

Group consent captures eligible typed turns after joining an open group even when
cataloging is off. Search finds them with or without `--group <id>`. Group
turns are always the human's. Leaving removes turns held only on group consent.
