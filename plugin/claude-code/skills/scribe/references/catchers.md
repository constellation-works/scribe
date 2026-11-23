# Catchers and incident lookup

Read a catcher's rules with `scribe tag show <tag>`. The
`[tags.<tag>.capture]` config gives whose turns the session pass reads
(`from`), its triggers, and whether a catch is proposed for approval or
written at once (`route`). `preferences` and `standard` ship. Other
catchers, such as `issue` for incidents, are configured separately.
See `docs/design/scribe/specs/catchers.md` in the scribe source repository
for the capture configuration.

1. Before diagnosing a failure the human asks about, run
   `scribe search '<symptoms or error text>' --kind entry --json`.
2. Say what a matching `issue-<n>` hit found before diagnosing again.
   Attribute the note to its agent and quote the human's request as theirs.
3. To record your own finding as an ordinary agent note, use a tag the
   store has and your own author.

   ```sh
   scribe entry add --tag <tag> --author <name> --related session:<id> '<words>'
   ```
