# Capture controls

## Cataloging sessions

The human types `/scribe on`, `/scribe off` or `/scribe status` in chat.
Codex also supports an exact typed `$scribe` invocation (or a Codex skill
mention link `[$scribe](/<path>/scribe/SKILL.md) ...`) for `on`, `off`, `status`,
`on --from <id>`, `link <id>`, `catalog <id>`, `group <slug>`, and
`group add <slug> <session-id>...`. On enables session cataloging. Off stops
further passes and keeps the record.

The human types these lines (in Codex, `$scribe` in place of `/scribe`):

- `/scribe on --from <id>` or `/scribe link <id> --type continues` to link
  related sessions;
- `/scribe group <slug>` to create or join an open project group;
- `/scribe group add <slug> <session-id>...` to add past sessions to one;
- `/scribe catalog <id>` to catalog a past session as theirs. Its windows run
  on the next `scribe-session-pass` Orbit job run.

If they ask you to catalog a past session, run
`scribe --provider <provider> session catalog <id> --author <name>`
under your own author. It writes the switch and runs no window: they run at
the next session pass, or now with the `orbit job run scribe-session-catalog`
command it prints.

`scribe session rebuild <id> --author <name> --confirm` checks a rebuild's refusals (`confirm_required`, `session_busy`,
`session_forgotten`, `transcript_missing`) and prints the
`scribe-session-rebuild` job command; a failed run keeps its checkpoint, and
running the job again resumes it.

A catalog switch is an on switch: its passes write memory the same way
`/scribe on` passes do. A session that is off gets no pass and no memory. Its
`#scribe` lines are still recorded, and an open group may still store its typed
turns; those are group-only (`session-groups.md`).

## Annotating a turn

In Claude Code and Codex, every `#scribe` in the human's typed turn, inside a
sentence or opening a line (`#scribe idea: …`, `#scribe add this to preferences`),
marks it. After your reply, scribe hands the marked line to you in your
context, with the command that records it; it calls no model. Record nothing
for the marker before then.

When the line marks something to keep, record it as theirs with their exact
words, using the flags scribe gives: `scribe entry add --tag <tag> --project
<project> --author <name> --related session:<id> --quote '<their words>'
--spoken '<statement>'`. For an action tag such as `standard` the command
scribe gives has no `--spoken`, since `entry add` refuses it there.

For a `require_evidence` tag such as `finding`, the evidence-only command has
no `--quote` or `--spoken`: `--evidence '<pointer>' '<statement>'`. If
`require_quote = true`, scribe gives `--quote '<their words>' --evidence
'<pointer>' '<statement>'` (still no `--spoken`). Record your finding only
when you can point to evidence (a path, task, run, experiment or commit).

A tag named after the marker or at the end of its line is the `--tag`;
`#scribe/global` gives `--project global` for every repository. Quote only
the `quote` as theirs. When the line marks nothing, record nothing.

A queued line and a slash command's arguments are marked the same way.
`scribe mark list` shows the markers with no entry from scribe.

## Settlement and deletion by the human

The human types `#scribe approve|reject <proposal-id>` or
`#scribe retire|forget <entry-id>` to control existing records. These forms
use `#scribe` only. Forget clears the entry's text and evidence, proposal
quotes of it, and related candidate quotes, evidence and reasons. The
session record stays.

To erase a whole session, tell the human to type `#scribe forget s-<n>`
(or use the session's full provider id there). This erases scribe's session
record, stored turns and handoffs, then leaves an id-only forgotten marker.

Open claims from the session are withdrawn with their quotes cleared.
Approved entries keep their text with a deleted `session:<id>` citation; the
human can remove one with `#scribe forget <entry-id>`. The session cannot be
cataloged or switched back on. The provider transcript stays in place. Only
the human's typed line can forget a session.

## Hook status and model work

Codex verifies per-content host user provenance. Unsupported transcripts,
automation/subagents and missing metadata record
nothing. Before the host persists a prompt, recording may be pending until
Stop verifies it. Relay the hook's actual status. These checks prevent
accidents. They do not authenticate a process running as the same OS user.

Orbit chooses the session-pass, catalog and rebuild models by the
`session_distill_crew` and `session_header_crew` settings.

Stop and SessionEnd mark opted-in sessions due. `scribe session pass` settles
sessions with no new window and prints the job command when there is more work.
Run `orbit job run scribe-session-pass` from an authorized workspace. To select
a switched-on session explicitly, add `--input 'sessions=["<id>"]'`.
The Orbit routine ships disabled.
Agents must not bypass hook trust or change scheduling merely to make a
recording status look successful.
