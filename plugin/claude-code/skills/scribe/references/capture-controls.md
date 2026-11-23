# Capture controls

## Cataloging sessions

The human types `/scribe on`, `/scribe off` or `/scribe status` in chat.
Codex also supports an exact typed `$scribe` invocation (or a Codex skill
mention link `[$scribe](.../scribe/SKILL.md) ...`) for `on`, `off`, `status`,
`on --from <id>`, `link <id>`, `catalog <id>`, and `group <slug>`. On enables
session cataloging. Off stops further passes and keeps the record.

The human types `/scribe on --from <id>` or `/scribe link <id> --type continues`
to link related sessions, `/scribe group <slug>` (or `$scribe group <slug>` in
Codex) to create or join an open project group, and `/scribe catalog <id>` to
catalog a past session as theirs. Its windows run at the next
`scribe session pass`. If they ask you to catalog a past session, run
`scribe --provider <provider> session catalog <id> --author <name>`
under your own author.

## Annotating a turn

In Claude Code and Codex, every `#scribe` in the human's typed turn, inside a
sentence or opening a line (`#scribe idea: …`, `#scribe add this to preferences`),
marks it. After your reply, scribe catalogs it as their entry in the working
directory's project. A tag named after the marker or at the end of its line
is the tag. `#scribe/global` records it with `project: global` for every
repository.

Let scribe capture the turn. Its content is scribe's context paragraph
(`stated_by: scribe:annotation`) and its `quote` is the human's exact words.
Quote only the `quote` as theirs.

## Settlement and deletion by the human

The human types `#scribe approve|reject <proposal-id>` or
`#scribe retire|forget <entry-id>` to control existing records. These forms
use `#scribe` only. Forget clears the entry's text and evidence, proposal
quotes of it, and related candidate quotes, evidence and reasons. The
session record stays.

To erase a whole session, tell the human to type `#scribe forget s-<n>`
(or use the session's full provider id there). This is the human's command.
It erases stored messages and log text and switches cataloging off for good.
Its id stays as forgotten. Entries, memory and claims citing it keep their
own text.

## Hook status and model work

Codex verifies per-content host user provenance. Unsupported transcripts,
automation/subagents and missing metadata record
nothing. Before the host persists a prompt, recording may be pending until
Stop verifies it. Relay the hook's actual status. These checks prevent
accidents. They do not authenticate a process running as the same OS user.

Codex-origin model work defaults to **Luna (`gpt-6-luna`)** through the Codex
CLI, with no fallback. Configure `[codex.models]` for explicit model or
transport overrides. Claude defaults remain under `[models]`. Global asks and
consistency jobs use `[models]` unless the caller selects `--provider codex`.

Stop and SessionEnd mark opted-in sessions due; catalog passes run through
`scribe session pass` from an authorized scheduler or shell. Init enables no
scheduler. The CLI must be authenticated and the selected model available.
Agents must not bypass hook trust or change scheduling merely to make a
recording status look successful.
