---
name: scribe
description: Look up Daniel's recorded decisions and preferences, find and read cataloged session records, and record agent notes, tags and proposals through the `scribe` CLI. Use when a task turns on what Daniel decided or prefers, what an earlier session did, or when you need to leave a note, tag or proposal in his store. For a vague question that needs a synthesized, cited answer, use `question ask` (or the `scribe.ask` MCP tool) instead of the list and search commands.
---

# scribe

`scribe` is a CLI on `PATH` that reads and writes Daniel's store of
decisions, preferences and session records. Run it from a shell. It is
read-only unless a command below says it writes; the writing commands are
audited and record who wrote.

The store is on this Mac (`--root <dir>`, else `$SCRIBE_ROOT`, else
`~/.scribe`). The skill therefore only works in sessions running on the
Mac; a worker on another host cannot read the store and should say so
rather than guess from memory.

## Which command answers the question

| You have | Use |
|---|---|
| Words you expect to appear in a record | `scribe record search "<terms>"` |
| A date range, topic, status, project, task id or author | `scribe entry list`, `scribe session list` or `scribe proposal list` with the filters below |
| One record's id (or the first four or more characters of it) | `scribe entry show <id>` or `scribe session show <id>` |
| A vague question, or one that needs weighing several records into an answer | `scribe question ask "<question>"` (the same answer as the `scribe.ask` MCP tool) |

`record search` and the lists call no model and answer at once; reach for
them first when you know a word, a date or a topic. `question ask` calls
a model and is for the questions the others cannot answer. Do not hand
Daniel a session id to look up: find it with `session list` or
`record search`.

## Reading

```sh
scribe record search "<terms>" [--kind entry|session] [--project <p>] [--topic <t>]… [--since <date>] [--until <date>] [--include-hidden] [--limit <n>]
scribe entry list [--tag <tag>] [--status active|superseded|retired|forgotten] [--project <p>] [--topic <t>]… [--related <ref>]… [--since <date>] [--until <date>] [--author <name>] [--author-kind daniel|agent] [--limit <n>]
scribe session list [--project <p>] [--topic <t>]… [--related <ref>]… [--status open|final] [--since <date>] [--until <date>] [--limit <n>] [--tree] [--include-inactive]
scribe proposal list [--state open|approved|rejected] [--since <date>] [--until <date>] [--entry <id>] [--author-kind daniel|agent|scribe]
scribe tag list
scribe entry show <id>
scribe session show <id> [--log] [--segment <n>]
scribe transcript list --since <date> [--until <date>] [--project <slug>]… [--min-turns <n>] [--limit <n>]
scribe transcript show <id>
scribe question ask "<question>" [--tag <tag>]… [--topic <t>]… [--project <p>] [--kind entry|session] [--since <date>] [--include-hidden]
```

- Dates are `YYYY-MM-DD`; `--since` and `--until` both include their day.
  A repeated `--topic` requires every topic to match; a repeated
  `--related` accepts any of them (a reference looks like `task:<id>`,
  `pr:<owner>/<repo>#<n>`, `repo:<slug>` or `session:<id>`).
- Inactive sessions and hidden (superseded, retired or forgotten) entries
  are left out by default: `--include-inactive` on `session list`,
  `--include-hidden` on `record search` and `question ask`. `entry list`
  shows every status unless `--status` narrows it.
- Every command that takes an id accepts a unique prefix of at least four
  characters. An exact id wins. An ambiguous prefix fails with exit 2 and
  `id_ambiguous`, listing the candidates; pick the full id and retry.
- `session show <id>` prints the record's header (summary, goals, next
  steps) and its log's table of contents. `--log` prints every segment's
  text, `--segment <n>` only one segment, and
  `scribe session show 'session:<id>#<n>'` shows one segment by its
  citation. Cite a segment as `session:<id>#<n>` when you rely on it.
- Add `--json` (or `--format json`) to any command for the stable machine
  form: lists print one envelope (`entries`, `sessions`, `proposals`,
  `tags`, `hits` or `transcripts`, with `total`, `truncated` and
  `warnings`) and `show` prints the record object. Prefer `--json`
  whenever a script or a later step will read the output; in a pipe the
  plain form is tab-separated lines with no header.
- `transcript list` and `transcript show` only read. They list Claude
  Code transcripts that have no session record and no switch, so Daniel
  can pick one to catalog. They call no model. `scribe session catalog`
  is Daniel's: it writes a session record and calls the model. Do not
  run it.

## What the records are

- An **entry** is a decision or preference filed under a tag. Daniel's
  entries are his verbatim words from a typed `#scribe` line.
- A **session record** is a model-written summary: its header and log
  segments describe what a session did. They are labelled summaries,
  never Daniel's own words — cite them as `session:<id>#<n>`, and never
  quote a summary as something Daniel said.
- An **agent entry** (below) is a note an agent wrote through the CLI.
  `question ask` labels it `agent note (<name>)` and never presents it
  as Daniel's words.

## Writing (agents)

Every write verb requires `--author <name>`: lowercase letters, digits,
`.`, `_` or `-`, 1–40 characters — by convention your crew or model name,
such as `opus`. It records `author: {kind: agent, name: <name>}`, because
`kind` names the channel, not a claimed identity. The names `daniel`,
`scribe` and `consistency` are reserved for Daniel's and scribe's own
channels: an agent write with one is refused `reserved_author` (reads
still accept them as filters). `scribe session catalog --author daniel`
is Daniel's alone: it records `kind: daniel` only from an interactive
terminal, and the skill rule stands — do not run `session catalog`; if
you must, under your own name it is recorded `kind: agent`. Each write
prints the record it wrote, so read the id back from that output. A write
that changes nothing prints the unchanged record, says `nothing changed`
on stderr and writes no audit line.

```sh
scribe entry add --tag <tag> --author <name> [--project <p>] [--topic <t>]… [--related <ref>]… <content>
scribe entry update <id> --author <name> [--content <text>] [--add-topic <t>]… [--remove-topic <t>]… [--add-related <ref>]… [--remove-related <ref>]…
scribe entry retire <id> --author <name> --reason <text>
scribe tag add <name> --author <name> --description <text> [--alias <a>]… [--stale-after-days <n>]
scribe tag update <name> --author <name> [--description <text>] [--add-alias <a>]… [--remove-alias <a>]… [--stale-after-days <n>]
scribe proposal add --kind supersedes|stale --older <id> [--newer <id>] --reason <text> --author <name>
scribe proposal update <id> --author <name> --reason <text>
scribe proposal settle <id> --approve|--reject --author <name> [--reason <text>]
scribe session deactivate <id> --author <name> --reason <text> [--cite <ref>]
scribe session activate <id> --author <name> [--pin|--unpin] [--reason <text>]
```

`--newer` is required for a `supersedes` proposal. `tag add` creates a
store tag; tags Daniel declared in his config win, and `tag update` of a
configured tag is refused. `session deactivate` hides a session whose
content no longer describes something current (check that first; scribe
takes your reason as given), and `session activate` shows it again, with
`--pin` exempting it from automatic deactivation and `--unpin` lifting
that exemption.

### The guard on Daniel's entries

Daniel's own entries are settled only by Daniel:

- `entry update` of a Daniel entry may change only its topics and related
  references; changing its content is refused `daniel_only`.
- `entry retire` of a Daniel entry, and `proposal settle` of a proposal
  whose older entry is Daniel's, are refused `daniel_only` and change
  nothing.
- You may open a proposal against a Daniel entry (`proposal add`), and
  update the reason of an open proposal you opened (`proposal update`);
  Daniel settles it with `#scribe approve|reject`. `#scribe forget` is
  Daniel's alone.

### Your own work, and other agents'

An agent edits only its own work:

- `entry update` and `entry retire` of an agent entry are allowed only
  under that entry's `--author` name; another name is refused
  `not_author`. A retired, superseded or forgotten agent entry refuses
  any update (`entry_not_active`). (Daniel entries keep the
  metadata-only rule above for every agent.)
- `proposal update` is allowed only to the proposal's author
  (`not_author` otherwise). `proposal settle` is the mirror image: the
  proposal's author cannot settle it (`not_author`); another agent
  settles a proposal about an agent entry.
- Settling with `--reason` replaces the proposal's reason, so it is
  allowed only on an agent-opened proposal. A proposal Daniel or the
  consistency routine opened keeps its reason: settle it without
  `--reason`, or the write is refused `proposal_not_agent`.
- Tags are shared vocabulary: any agent may `tag update` a store tag,
  recorded in its `updated_by`.

Record what you learn as an agent entry with your own `--author` instead
of editing Daniel's words or another agent's note.
