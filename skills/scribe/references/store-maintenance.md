# Keep the store useful

At natural pauses (the brief or the end of a task), group a few suggestions
at a time and keep the work moving.

## Pending memory claims

When the hook acknowledged `/scribe on` or
`scribe session show <id> --json` reports `on: true`, let the session pass
propose claims instead of offering ready-to-type preference lines. It proposes
from the human's own turns. A candidate holds scribe's context paragraph
(two to four sentences on the discussion and what the turn settles), its
subject, the exact quote, source turn and suggested memory tag.

Run `scribe proposal show <proposal-id>` for the full claim so the human can
settle it without opening the session. If the output marks it `older claim`,
read the one-line statement in place of a context paragraph. The pass offers
up to three claims per session pass, with at most twenty open candidates in
the store.

Mention open candidates in the brief. The human types
`#scribe approve <proposal-id>` to keep one or `#scribe reject <proposal-id>`
to drop it for good. If they ask you to settle it, use the settlement
procedure linked from the skill.

Before approval there is only a claim. Approval writes an entry as theirs
citing the original turn. Its content is scribe's context
(`stated_by: scribe:session-pass`), so quote only the `quote` as theirs.
If an active entry of the same tag and project already covers the claim,
approval writes nothing and names the existing entry.

## Explicit spoken instructions to record an entry

When the human explicitly tells you to record something under a memory tag,
for example “add that to my preferences” or “remember this in my preferences”,
write under your own author. Use this session's id and their exact words
from one typed turn. Write your one-sentence statement as the content.
Pass `--project global` for every repository or `--project <project>` for
this one.

```sh
scribe entry add --tag preferences --author <name> --project <project> \
  --related session:<this session id> --quote '<their exact words>' \
  --spoken '<your one-sentence statement>'
```

- Use `--spoken` only for `preferences` or a configured or store memory tag.
  It is refused for `handoff`, `require_evidence` or action tags. It works
  in off sessions without switching them on.
- Use a verbatim span of at least 8 characters from one of their typed turns
  in this session. A refusal reports `quote_required` or `quote_not_found`.
- Keep their exact words as `quote`. The statement is yours
  (`stated_by: agent:<name>`). Echo the id and drop line
  `scribe: wrote <id> (drop: #scribe retire <id>)`.
- When the session is off or its state is unknown and the human states a
  preference without asking you to record it, offer to record it once they
  confirm in a phrase of at least 8 characters, or mention `/scribe on` for
  automatic proposals. Don't ask them to retype with `#scribe`.
- Something you established yourself can become an agent note under your
  own `--author` once they agree.
- Record a choice about work on its Orbit task with the human's exact words,
  or in the feature's `4_decisions.md` for a design decision. Skip offering
  an entry for it.

## Tags and capture rules

- When `scribe tag list --json` has no fitting tag for a recurring theme
  in entries or sessions, propose a name, a one-line description and any
  aliases. Once the human agrees, run
  `scribe tag add <tag> --author <name> --description '<description>' [--alias <alias>]`
  under your own author. Repeat `--alias` for each alias.
- Run `scribe tag show <tag>` to read each capture rule and its source
  (`shipped`, or `config` with its file and line). The session pass proposes
  a claim only when a human turn fires a trigger. The candidate's
  `admitted` names the trigger, matched words and criteria.
- To change what is caught, suggest a `[tags.<tag>.capture]` edit in config.
  Leave the store intact.

## Stale sessions

Treat a session as stale when a feature was removed, replaced or renamed, or
a later session superseded its plan. Keep a finished session about a feature
that still exists. The consistency routine handles age.

Check the claim against the repository or task state and propose the
deactivation with the reason and the supporting record. On the human's
explicit yes for that session, run the command under your own name and name
their approval in the reason.

```sh
scribe session deactivate <id> --author <name> --reason "<why>" [--cite <ref>]
```
