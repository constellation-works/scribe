---
type: design
summary: "Spec: output contract — streams, modes, plain columns, the JSON list envelope and record fields (segments, activity, agent authors and direct writes, search hits; future transcripts), warnings, errors and exit codes"
last_validated: 2026-11-23
---

# Spec: Output contract

Every data command renders one typed payload in the mode the sink resolved;
stdout carries only that payload; warnings, notices and errors go to stderr;
the exit code says whether it worked. The JSON form is a public contract,
pinned by the output goldens in `crates/scribe/tests/goldens/output/`.
S11's read surface, S12's writes, S13's skill and tmux popup (now the `scribe tmux` subcommand), and S14's
transcript list and session catalog are built. Parts marked *(S14)* add
fields, columns, commands and codes and do not rename or retype earlier
ones.

## Why This Exists

**Format 23 (contract change, 2_design.md §15.8).** The user's author
channel is `user`, not `daniel`: an author's `kind` (and the user's `name`)
in `--json`, the plain `author` column, `--author-kind user` (`daniel`
still accepted), the error code `user_only` (was `daniel_only`), and the
labels `user_turns`, `user_first_turns`, `USER_TURNS` and `user_asked`
(were `daniel_turns`, `daniel_first_turns`, `DANIEL_TURNS` and
`daniel_asked`). The only readers, the skill and the tmux popup, ship in
the same package. The text below keeps the older tokens where it
describes formats before 23.

Three readers — Daniel at a terminal, a script in a pipe, an agent through
a shell tool — need different bytes from the same result, and scripts and
agents break silently when the machine form drifts.

## Modes

| Mode | Chosen by | Shape |
|------|-----------|-------|
| `table` | `--format table`, or auto on a terminal | header row + one line per record; `show` is one field per line, then the full text |
| `plain` | auto off a terminal | one tab-separated line per record, no header; `show` prints its record's list line only, so a script or agent that wants a `show`'s detail from a pipe passes `--json` or `--format table` |
| `json` | `--json`, `--format json`, or `SCRIBE_FORMAT=json` | one document: the list envelope for `list`, one record object for `show` |

Precedence: flag > `SCRIBE_FORMAT` > auto. An unrecognized environment
value means auto. Off a terminal there are no escapes and no truncation; on
a terminal only the last free-text column is cut to the width, and `show`
or `--json` has it whole. The width is `COLUMNS` when it holds a positive
number, else the terminal's own size (`TIOCGWINSZ` on stdout); both are read
in the output sink only, and with neither no table is cut. In plain and table
rows, free text is flattened to one line (every whitespace run becomes one
space), an empty value or list is `-`, lists are comma-joined, and timestamps
are RFC 3339, except in table mode, where every timestamp cell of a human list
table shows the UTC date as `YYYY-MM-DD` when representable: `entry list`
`created`, `session list` `updated` (flat, `--tree` or `--include-inactive`),
`proposal list` `opened_at`, `scribe search` `updated`, `transcript list`
`started` and `ended`, and `group list` `opened` and `closed`. A present
timestamp whose UTC date is outside the supported range shows `-`. Table mode
is the default on a terminal and `--format table` selects it on a pipe too.
Plain and JSON list output keep the full instant, as does `show` (`session
show`, `group show` and the other records) in every mode. The table date is
display-only. After filtering, `session list`
sorts matches by full `updated` instant, newest first (IDs ascending on ties),
before applying `--limit`; `--tree` then traverses limited rows depth first,
parent before child, preserving selection order among roots and siblings. A
session whose parent is not in the limited rows is a root. An absent cell is
`-` in every table, never
blank; the only blank cells are the layout cells of a continuation row (a
proposal's quote line).

## Plain columns

| Command | Columns |
|---------|---------|
| `entry list` | `id status project topics created content author age` (`author` is `user` or `agent:<name>`; `age` is an action entry's age, `45s`, `12m`, `3h` or `9d`, and `-` for any other entry). When any returned entry is open, a final `state` column follows `age`: open entries show `open`, and other entries in the mixed result show `-`. With no open entries, the ordinary eight-column shape above is unchanged. |
| `session list` | `id status due project topics updated title` (`due` is `yes` or `no`) |
| `session list --tree` | `id parent status due project topics updated title` (`parent` is the session the row nests under, its `spawned_from` link else its first `continues` link, or `-`) |
| `session list --include-inactive` | `id status active due project topics updated title` (`active` is `yes` or `no`); without the flag the columns are unchanged, inactive sessions are left out, and when any matched the notice `<n> inactive session(s) also match and are hidden; --include-inactive lists them` goes to stderr. With `--tree` too, `active` follows `status` |
| `proposal list` | `id kind state older newer opened_at reason admitted closed_reason` (format 11: `admitted` is `trigger "<phrase>" matched "<words>"; criteria: <criteria or ->` for a candidate a capture rule admitted, else `-`; `closed_reason` is `tag_retired` when scribe closed it without Daniel, `instructed` when an agent rejected it on Daniel's checked instruction (format 13), `covered` when an approved candidate's entry was missing and an active entry covers it (format 15), `withdrawn` when a typed session forget erased the source, else `-`; format 14: for a candidate with a statement, `reason` is the statement, and the table prints `Daniel's words: "<quote>"` on a row under it in the same column, while plain joins them as `<statement> \| Daniel's words: "<quote>"`; the single-proposal detail a proposal write prints (`proposal add`, `update`, `settle`) and `proposal show <id>` prints adds `statement` and `subject` fields before `quote` in the table, and its JSON carries both on `candidate`; format 24 (2_design.md §15.3): for a candidate with a context, `reason` is the context (plain `<context> \| The user's words: "<quote>"`), and an older claim's (a statement and no context) is `older claim: <statement>`; the detail prints `subject` and then `context` (the context, or `older claim: <statement>`) before `quote`, and for an open candidate a `settle` field, `#scribe approve <id> or #scribe reject <id>`, so a claim is settled from `proposal show` without opening its session; (ORB-26246) a candidate an agent settled on Daniel's instruction adds `<approved\|rejected> by <agent> on your instruction "<quote>" (<session>, <date>)`: plain joins it after the reason with ` \| `, the table prints it on a row under, in the same column, and a proposal write's detail adds it as an `instruction` field) |
| `tag list` | `name entries stale_after_days aliases description`, (S12) `source` (`config` or `store`), then `action` (`free` or `ask-first`, with `,quote` when the tag requires a quote), `retired` and `rejected` (counts of agent entries); the last three are `-` for a tag that is not an action tag; (format 11) then `route` (the tag's capture route, `propose` or `write`, or `-` when it has no capture rules) |
| `tag show <tag>` (format 11) | one `<field>\t<value>\t<origin>` line each for `tag`, `source`, `aliases`, `description`, `stale_after_days`, `action` (`free`/`ask-first`, with `,quote`, or `-`), then the capture rules `from`, `route`, `triggers` (quoted, comma separated), `criteria`, `not` and (phase 2) `max_open`; `origin` is `shipped`, `config <file>:<line>` or `none` (an unset rule's value is `-`), and `-` for the first six. The table prints `<field> <value>  (<origin>)` |
| `scribe search` (S11) | `cite kind score updated title snippet` (`score` to three decimals; the table cuts `title` to 40 characters and fits `snippet` to the width; (ORB-26246) an entry an agent approved on Daniel's instruction ends its snippet, in every mode, with ` \| approved by <agent> on your instruction "<quote>" (<session>, <date>)`) |
| `memory show <id>` (format 25) | `<id>\t<kind>\t<date or ->\t<title or ->` (`kind` `profile`, `now` or `milestone`), then one `text\t<line>` line per line of its text; the table prints the item's fields one per line (`id kind date now_kind title text sources updated raised`, `-` when empty). `memory show` with no id prints the memory block (below), not rows |
| `transcript list` *(S14)* | `session project user_turns bytes started ended first_prompt` |

Search hits are best first; on a tie entries come before sessions, then
the newest first (as `ask` ranks); transcripts newest first by
`started`. Entries are oldest first (by `created`, then `started`, ties by
id). Proposals are ordered by number; tags by name. Table output indents the
title two spaces per level; plain output does not indent, and the `parent`
column carries the nesting.

## JSON list envelope

Every list is
`{"<key>": [<record>, ...], "total": <integer>, "truncated": <boolean>,
"warnings": [<warning>, ...]}`, with `<key>` `entries`, `sessions`,
`proposals` or `tags`, (S11) `hits` for `scribe search`, and *(S14)*
`transcripts` for `transcript list`. `total` counts the records that matched the filters;
`truncated` is true when `--limit` returned fewer. No matches is the same
envelope with an empty array. A warning is `{"path", "code", "error"}` for a
record file the list skipped because it did not parse (`code` is
`store_corrupt`); each is also a `warning: skipped …` line on stderr in
every mode.

stderr also carries, in every mode, one notice when a list is empty (where
records come from, or `no <noun> match <filters>` naming the flags) or cut
(`showing N of M matching <noun>; raise or drop --limit to see the rest`).
(S11) `scribe search` names its terms first among the filters (`no records
match "<terms>" --kind session`; with `--tag` the tag follows the kind, as
`no records match "<terms>" --tag <tag>`), and when hidden records also
matched it adds `<n> hidden record(s) also match: superseded or retired entries, or
inactive sessions; --include-hidden shows them`.
(ORB-27628) A default `scribe search` (no `--kind`) that finds no entry or
session, while stored messages match the same filters, ends its stderr, in
every mode, with the last line `<n> stored message(s) match, and no entry or
session does: scribe --root <dir> search --kind message [filters] -- <terms>
lists them`. The command repeats the effective store root and the filters
(`--project`, `--group`, `--topic`, the dates and `--include-hidden`), so
`<n>` counts what it lists. The `--` before the terms preserves a literal term
that starts with a dash. Its `--json` object gains `message_matches` (`<n>`)
after `warnings`. Only that case adds the line or the key: with no matching
message the output is unchanged, and the default kinds do not change.

(S11) List filters: `entry list` and `session list` take `--since <date>`
and `--until <date>` (`YYYY-MM-DD`, both days included), `--status`,
repeated `--topic` (every one must match) and repeated `--related
<kind:target>` (any may match); `entry list` also `--author <name>` and
`--author-kind user|agent|scribe` (format 16: `scribe` lists the ideas
the session pass wrote itself; `user` never lists them); `proposal list` takes `--state`, `--since`,
`--until`, `--entry <id>` (the older or the newer entry) and
`--author-kind user|agent|scribe`. An entry's day is its `created`, a
proposal's its `opened_at`; a session matches when its span, `started` to
`updated`, has a day in the range, and its topics and references are the
record's and its segments'. A date that does not parse, or `--until`
before `--since`, is `usage_error` (exit 2). The filters change no
record shape.

## JSON fields

Absent values are `null`, never omitted; empty lists are `[]`. Timestamps
are RFC 3339 with offset.

**Entry** (`entries[]`, `entry show`): `id`, `tag`, `status` (`active`,
`superseded`, `retired`, `forgotten`), `project`, `topics` (strings),
`related` (strings `kind:target`), `provider`, `host`, `created`,
`updated`, `said`, `content`, `content_from` (`{kind, role, message, at}`;
`kind` is `instruction` or `message`, the other three `null` for an
instruction), `cite` (`{session, message, at, prompt_sha256, line, cwd}`;
`message` may be `null`), `superseded_by` (an id or `null`),
`review_after` (`YYYY-MM-DD` or `null`), `author` (`{kind, name}`, `kind`
`user` or `agent`), `updated_by` (an author or `null`) and
`retirement_reason` (text or `null`), and `handoff_mode` (`continue`, `new`
or `null`). (Format 11) `entry show` adds
`admitted` (the `{trigger, matched, criteria}` of the approved candidate
proposal that created the entry, or `null`); plain prints it as an
`admitted\t<trigger>\t<matched>\t<criteria>` line, and the table detail
as an `admitted` field. (S12) An
agent entry has `said` `""`, `content_from` `{kind: "agent",
role: null, message: null, at: null}` and `cite` `null`; a Daniel entry's
`cite` is never `null`, so a reader of Daniel's entries sees no change.

Store format 8 adds `quote` (the human's words an agent quoted, or
`null`), `window_turns` (how many turns the entry's transcript window
holds, or `null` when it has none), `window` (`{turns: [{role, message,
at, text, cut}], truncated, omitted}`, or `null`; only `entry list
--window` and `entry show --window` fill it) and `action` (`null` except
for an entry under an action tag in `entry list`, `entry show` and the
entry write verbs: `{age_seconds, cancel_window_ends, past_cancel_window,
picked_up, rejected, pickups: [{author, at, result, rejected}]}`;
`cancel_window_ends` is `null` for Daniel's entries, which have no cancel
window). The table detail view adds `author`, and for such an entry
`quote`, `age`, `cancel` (when the window ends, and whether it has),
`pickups` and `window` (its turns with `--window`, else `<n> turns (show
with --window)`). Store format 13 adds `approved_by` (`{kind, name}` of kind
`agent`, or `null`) and `instruction` (`{session, message, at, quote}` or
`null`, where `quote` is `null` once forgotten). An entry an agent approved
on Daniel's instruction (format 15) has `approved_by` and `instruction` set
and `said` the command it ran, `scribe proposal settle p-<n> --approve
--author <name> --session <id> --instruction '<his words>'`. Store format 18
adds `removal_instruction` (`{session, message, at, quote}` or `null`, last
in the object): the instruction an agent retired or forgot the entry on,
kept apart from `instruction`, which is then only the approval's and never
replaced by a later retirement or forgetting. (ORB-26246) For an entry an agent approved on his instruction, `entry show` prints `approved by <agent> on your instruction "<quote>" (<session>, <date>)` (the quote left out once forgotten): as an `approved` field under `cite` in the table detail, and plain as an `approved\t<line>` line after any `stated_by` line; an entry he approved by typing prints neither. Store format 14 adds
`stated_by` (`{kind, name}`, `{kind: "scribe", name: "session-pass"}` when
`content` is scribe's statement of Daniel's turn and `quote` his verbatim
words, else `null`). The table detail prints `stated_by
scribe:session-pass (content is scribe's statement of Daniel's turn)` before
`quote`; plain prints a `stated_by\t<kind>:<name>\t<quote>` line after the
entry row. Store format 16 adds `outcome` (`{state, reason, task}`, `state`
one of `open`, `parked`, `task`, or `null`), set only on an idea the session
pass wrote itself (author and `stated_by` both `scribe:session-pass`). For
such an entry the table detail's `stated_by` reads `scribe:session-pass
(content is scribe's statement of Daniel's turn, captured by the session pass
(not typed by Daniel))`, an `outcome` field follows `quote` (`open`,
`parked, in Daniel's words: "<reason>"` or `task <id>`), and `cancel` reads
`none (written by scribe, not an agent)`; plain adds an
`outcome\t<state>\t<reason, task or ->` line after the `stated_by` line.
Store format 22 adds `{kind: "scribe", name: "annotation"}`: an entry Stop
wrote for a `#scribe` marker inside a sentence of a turn Daniel typed
(2_design.md §14.4), authored by Daniel, with no proposal. It prints as a
stated entry: the table detail's `stated_by` reads `scribe:annotation
(content is scribe's statement of Daniel's turn)` before `quote`, and plain
prints `stated_by\tscribe:annotation\t<quote>`.
`entry add --spoken` (2_design.md §16.6) writes a spoken-instruction entry
under an eligible non-action memory tag with `author: {kind: "agent", name}`,
`stated_by: {kind: "agent", name}`, `content_from: {kind: "agent"}`, `cite: null`,
`said: ""`, the agent's statement as `content`, and the user's verbatim turn
span in `quote`. It prints as a stated entry with the statement labelled as
the agent's: table detail's `stated_by` reads `agent:<name> (content is agent
(<name>)'s statement of the user's turn)` before `quote`, plain prints
`stated_by\tagent:<name>\t<quote>`, `search` snippets open with `agent
(<name>)'s statement of the user's turn: <statement> | The user's words:
"<quote>"`, and `ask` attributes quotes inside `quote` to the user and quotes
in `content` to `agent (<name>)'s statement of the user's turn`.

Store format 28 adds `handoff_mode` to entry JSON. It is `continue` or `new`
for a handoff that sets a mode, and `null` for other entries and legacy
handoffs.

Store format 31 adds `decision` and `item` to the entry JSON shared by
`entry show`, `entry list` and entry writes. Each field is null for other
entries and for the trial's prose-only records. A decision object has
`decided_by`, `rationale`, `reason_quote`, `options`, `recommended`,
`supersedes` and `settles`. `quote` keeps the user's decision words. An
item object has `asked_by`, `options`, `recommended`, `why`, `blocked`,
`answer_in`, `answer` and `closed`. `answer` and `closed` are null on a new
item. Options are an array of strings and `recommended` is a one-based
integer or null. `answer_in` is `any`, `hand` or `session:<id>`.
`answer` is `{words, session, at, recorded_by}`: the user's checked words,
the session and time of the typed turn that holds them, and the agent that
recorded them. `closed` is `{at, by, decision, note}`, `decision` the
settling decision's id or null. Every entry also has `state`: for an entry
under `open-item` its derived state, `open` (active, no answer),
`answered` (active, with an answer), `closed` (retired with `closed`, or a
retired trial entry) or `withdrawn` (retired without `closed`, superseded
or forgotten); `null` for every other entry.

For `entry show` and entry writes, table detail adds labelled fields for
the supplied values. Plain output keeps the existing first row and
appends labelled, tab-separated lines, including `quote` for a user
decision. In both modes, each option has an `option` field with its
one-based number. An open item adds `state` before its fields and, once
set, `answer` and `answered` (time, session and recorder), then `closed`
(time and closer), `closed_decision` and `closed_note`. Plain and table
list rows retain their existing columns; a list holding an open item adds
a last `STATE` column (`-` for other entries).
The `related` array can contain `doc:<repo>/<path>#<anchor>` references.
When `decisions` resolves to the shipped `decision` tag, `ask --tag decisions`
also includes entries filed under the historical `decisions` name.

**Session** (`sessions[]`, `session show`): `id` (the provider's session
id), (phase 6) `short_id` (`s-<n>`, or `null` for a session with no number;
the table detail prints it as a `short id` field under `id`), `status` (`open`,
`final`), `forgotten` (when `#scribe forget s-<n>` erased the session, else
`null`; the table detail prints a `forgotten` field under `status`, and plain
a `forgotten<TAB><time>` line after the row, only when set), `project`, `topics`, `related` (unchanged: model-extracted
`kind:target` strings), `links` (`[{type, session, title}]`: the links the
record stores, `type` `spawned_from` or `continues`, `title` the linked
session's record title or `null` when it has none), `children`
(`[{type, session, title}]`: the sessions whose links point at this one,
derived at read time from every record, oldest first; `type` is the
child's link type), `terms` (`[{term, said,
message}]`, Daniel's own words), `provider`, `host`, `created`,
`updated`, `title`, `cwd`, `started`, `on_since`, `scribed_through` (a
message id or `null`), `passes`, `next_open`, `summary`, `goals`,
`outcome`, `next`, and from its switch `on` and `due` (both `false` when the
session has no switch; the switch is read from `scribe.db` once the sessions
have moved, from `control.json` before). The `outcome` field is an empty string
for compatibility; `summary`, `goals`, and `next` are model-written.
`links` and `children` were added with session links (store format 3);
every other field is unchanged.

Store format 6 adds `rebuilt_by` (`null` or `{kind, name}`): the agent whose
`--author` ran the `session rebuild` that wrote the record. It is in `--json`
only; the table view does not print it.

Store format 4 adds `active`, `keep_active`, `active_change` (`null` or
`{at, by, route, reason, cite}`), `cataloged_by` (`null` or `{at, by}`)
and `log`: in `session show` the segments `[{n, heading, first_message,
last_message, from, to, state, windows, origin, topics, related, text}]`,
oldest first; a `session list` row has no `log` key (use `show`).
(§16, ORB-26826) `session show` also adds `handoffs`: active agent-written
`handoff` entries referencing `session:<id>`, newest first, each `{id,
author, created, project, quote, content, handoff_mode}`; `handoff_mode` is
`continue`, `new` or `null`. A `session list` row has no `handoffs` key.
When the session has active handoff(s), the table view prints `## Handoffs`
(each as `### <id> · <author> · <created>`, an optional `mode: <mode>`
line, then its body) before `## Summary`, and plain prints one
`handoff\t<id>\t<author>\t<created>\t<content>` line per handoff before
the segments, prefixing the content with `mode: <mode> ` when a mode is set.
For a session with active handoff(s) and no session record,
`session show <id>` prints `{id, has_record: false, project, on, due,
summary: "", goals: "", next: "", handoffs}` in JSON (with `on` and `due`
from its switch, or `false` when it has none), `record: none (handoff
only; session was not switched on)` (or `none (handoff only; no catalog
record yet)` when switched on/due), `on`, `due` and `## Handoffs` in
table mode, and `<id>\thandoff-only\t<yes|no>\t<project>\t-\t<created>\tno
session record` followed by the `handoff\t...` lines in plain mode. `outcome` stays and
is `""` for every format-4 record; what it held is in `log`. `--segment
<n>` and `session show session:<id>#<n>` print the record with `log`
holding that one segment; `--segment <n>` with a cite naming another
segment is `usage_error`, and a segment the log lacks is
`segment_not_found`. The table detail view adds the lines `active`, `keep
active`, `change` (`<at> <route> by <kind>:<name> (<reason>)`, then `,
cite <cite>` when there is one, or `-`) and `cataloged` (`<at> by
<kind>:<name>` or `-`), prints the header (`## Summary`, `## Goals`, `##
Next`), then `## Log` and a table of contents (`n state from to heading`),
then with `--log` or one segment each segment as `### <n>. <heading>` and
its text; plain prints the record's
list line, then one `segment\t<n>\t<state>\t<from>\t<to>\t<heading>`
line per segment. `session list --tree --json` is the same
envelope with the same objects, in tree order. The table detail view of
`session show` prints `links` as `<type> <id> (<title>)` and `children`
as `<id> <type> (<title>)`, one per line, or `-`.

**Proposal** (`proposals[]`): `id` (`p-<n>`), `kind` (`supersedes`,
`stale`), `older`, `newer` (or `null`), `reason`, `quotes` (`[{entry,
text}]`), `state` (`open`, `approved`, `rejected`), `opened_at`,
`settled_at` (or `null`), `settled_by` (a cite or `null`), `author` and
`settled_author` (`{kind, name}` or `null`), and `updated_by` (the last
agent who revised the reason, or `null`). Store format 11 adds
`closed_reason` (`tag_retired` when scribe closed it without Daniel,
`instructed` when an agent rejected it on Daniel's checked instruction in format
13, `covered` in format 15 when an approved candidate's reserved entry was
missing and an active entry already covers it, keeping its settlement,
else `null`) and, on a `candidate` object, `admitted` (`{trigger, matched,
criteria}`: the capture-rule trigger as configured, the turn's words it
matched, verbatim and at most 120 characters, and the tag's `criteria`
then in force or `null`; `null` on a candidate opened before format 11). Store
format 13 adds `instruction` (`{session, message, at, quote}` or `null`, where
`quote` is `null` once forgotten). Store format 14 adds, on a `candidate`
object, `statement` (scribe's statement of the claim, at most 300
characters) and `subject` (at most 80 characters), both `null` on a
candidate opened before format 14 or forgotten. Store format 24 adds, on a
`candidate` object, `context` (scribe's paragraph of the claim, at least
two sentences and at most 600 characters), `null` on a candidate opened
before format 24 or forgotten; a new claim's `statement` is `null`.
`proposal show <id> --json` is that one proposal object.

**Tag rules** (`tag show <tag> --json`, format 11): `tag`, `source`
(`config` or `store`), `aliases`, `description` (or `null`),
`stale_after_days` (or `null`), `action` (`{mode, require_quote}` or
`null`) and `capture` (`{from, route, triggers, criteria, not, max_open,
origin}`). Each capture field is `null` when the tag has no such rule;
`from` is `user`, `assistant` or `both`, `route` `propose` or `write`, and
(phase 2) `max_open` the most open claims the tag may hold: its config
value, else 10 (origin `shipped`) for a tag with a route, else `null`
(docs/design/scribe/specs/catchers.md); `origin` has
one `{source, file, line}` per field, `source` `shipped`, `config` or
`none`, and `file` and `line` set only for `config` (the config file and
the 1-based line of the key). The lookup takes a name or alias and is
read-only; an unknown word is refused `tag_not_found`.

**Tag** (`tags[]`): `name`, `aliases`, `description` (or `null`),
`stale_after_days` (integer or `null`), `configured` (false for a directory
no configured tag names), `entries` (count, any status). (S12) Adds
`source` (`config`, `store`, or `null` for a directory no tag names) and
`author` (`null` for a config or orphaned tag). Format 8 adds `action`:
`null` for a tag that is not an action tag, else `{mode, require_quote,
retired, rejected}` (`mode` `free` or `ask-first`; `retired` counts the
agent entries under it that were retired or forgotten, `rejected` those a
consumer recorded as rejected). A shipped action tag reports `source`
`config`. Format 11 adds `route` (the capture route, or `null` when the
tag has no capture rules). `configured` is true only
for config tags, even though a store tag is also routable.

**Hit** (S11, `hits[]`): `kind` (`entry` or `session`; (phase 4)
`message`; *(S7)* `discussion`), `cite` (an entry id, `session:<id>` for a
header or a message, `session:<id>#<n>` for a segment; *(S7)* a discussion
slug), `id` (the entry's or session's id), `segment` (integer or `null`),
(phase 4) `message` (a stored message's provider id, `null` for any other
kind), `score` (a
number, three decimals), `title` (a session's title, a segment's heading,
or for an entry `<id> (<tag>)`), `snippet` (up to 240 characters on one
line, starting a little before the first matching word, with `…` where it
was cut; format 14: for an entry with `stated_by`, `scribe's statement of
Daniel's turn: <statement> | Daniel's words: "<quote>"`, each part cut to
240 characters, and format 24 for an approved assistant claim with
context: `scribe's statement of the assistant's turn: <context> | The
assistant's words: "<quote>"`; format 16: for an idea the pass wrote itself, `scribe's
statement of Daniel's turn, captured by the session pass (not typed by
Daniel): <statement> | Daniel's words: "<quote>" | outcome: <outcome>`,
and (phase 2) for one the pass caught from an assistant turn, `scribe's
statement of the assistant's turn, captured by the session pass (not the
user's words): <statement> | The assistant's words: "<quote>" | outcome:
<outcome>`; format 28 appends ` | handoff mode: <continue|new>` when set),
`project`, `topics` (a segment's are its session's plus its
own), `author` (an entry's `{kind, name}`, `null` for a session), `active`
(a session's flag; `null` for an entry), `updated` (an entry's or
session's `updated`, a segment's `to`, a message's time), and `status`
(added beside the designed fields: an entry's `active`, `superseded` or
`retired`, a session's `open` or `final`, which a message takes from its
session when it has a record; otherwise a group-only message is `stored`).

(Phase 4, 2_design.md §15.7) `scribe search --kind message` searches the
messages in `scribe.db` instead of the entries and sessions: ranked by
SQLite FTS5's `bm25()` (`score` is its negation, so higher is better),
each search token matched as a word prefix. Both unscoped and group-scoped
search include group-only turns without a session record. A record supplies
the title, project, topics and active status when present; otherwise a
group-only turn uses the session id as its title, the group's project, empty
topics, null `active` and `stored` status. The role is appended to the title
(`(<role>)`); dates are the message's, and an inactive session's messages
are hidden unless `--include-hidden` is set. Without
`--kind`, a search ranks entries and sessions only: the two scores are not
comparable. A store whose database had to be opened as a read-only copy
adds the notice `read-only copy: <path> has no -wal or -shm file and its
directory is read-only, so it was opened immutable; …` on stderr.

**Store migrate** (phase 6, `store migrate`): `{"sessions", "switches",
"cursors", "moved_to"}`, the counts imported and the `pre-sqlite-<date>`
directory the files went to (or `null`). Plain is one line
`<sessions>\t<switches>\t<cursors>\t<moved_to or ->`; the table prints one
field per line. It is a write: audited as `store migrate`.

**Export** (phase 6, `export <dir>`): `{"dir", "format", "sessions",
"files", "skipped"}`, `format` `markdown` or `jsonl`, `files` the paths
written relative to `dir`, `skipped` the records that did not read. Plain
prints one path per line; the table prints `dir`, `format`, `sessions`,
`files` (a count) and `skipped`. It reads only and is not audited.

**Issue** (format 17) is removed in phase 2 (2_design.md §15.9) with the
`issue` noun: `issue list|show|add|occur|update|move|search|stats` exit 2
as unknown commands, `scribe search --kind issue` exits 2, and the codes
`issue_not_found`, `occurrence_not_found`, `issue_forgotten` and
`reserved_tag` are no longer emitted. Stored issues become `issue` entries
(specs/catchers.md).

**Transcript** *(S14, `transcripts[]`, `transcript show`)*: `session`,
`project` (the attributed slug), `project_mentions` (`{"<slug>":
<count>}`), `cwd`, `started`, `ended` (the last message's time), `bytes`,
`user_turns`, `first_prompt` (at most 300 characters); `transcript show`
adds `user_first_turns` (up to five strings of at most 300 characters).

**Write verbs** (S12 built; S10 separately): `entry add`, `entry update`, `entry
retire`, `entry pickup` (format 8; prints the entry it noted), `entry answer`,
`entry close`, `tag add`, `tag update`, `proposal add`, `proposal update`,
`proposal settle`, `session activate` and `session deactivate` print the
record they wrote in its `show` shape (`--json`: the record object). A
write that changed nothing prints the record unchanged and `nothing
changed` on stderr. `session rebuild` and `session catalog` print the
session pass report. *(S10, as built)* `session deactivate` and `session
activate` print the session in `session show`'s shape (`--json`: the
object with every segment); a change writes one audit line (`session
deactivate` or `session activate`, target `session:<id>`) and an
unchanged write none, with `nothing changed: <id> is already <state>` on
stderr; deactivating a pinned session also says on stderr that it
unpinned it. A blank `--reason`, one over 300 characters, a `--cite`
that is not one token of at most 200 characters or names no record
(`session:<id>`, `session:<id>#<n>` or an entry id that exists), a
segment cite as the id, or `--pin` with `--unpin` is a usage error (exit 2).

S12 tag writes return `name`, `aliases`, `description`, `stale_after_days`,
`author`, `updated_by`, `created` and `updated`. Plain tag writes are one
line (`name description author aliases`, tab-separated); table mode shows
the full definition and provenance. Plain proposal writes use the proposal
list columns; table mode shows settlement metadata. Proposal writes use the
proposal-list item shape, including the additive `updated_by` author.
Entry writes use the entry-show shape, with `cite: null` and
`content_from.kind: agent` for an agent note. Every write requires
`--author`, and a reserved name (`user`, `daniel`, `scribe`, `consistency`,
`session-pass`, `tag-retired`) is refused.

For a new decision, `--decided-by user` takes a checked `--quote` and can
take a checked `--reason-quote`. Either quote may be shorter than eight
characters when it is a whole typed turn. An agent decision takes
`--rationale`. Each decision names a `task:`, `pr:` or `repo:` reference,
along with its source `session:` reference for a user decision. Use
`--option` for each of two to six options and `--recommended` for a
recommendation. `--supersedes` names the earlier active decision and
`--settles` names an open item. For a new open item, `--asked-by` and
`--blocked` are required. A choice also requires `--recommended` and
`--why`. An action item has zero options. `--answer-in` defaults to `any`.

`entry answer <id> --quote '<words>' --related session:<id>` records the
user's answer on an open item; the quote is checked as a user decision's
is, and the latest typed turn holding it gives `answer.at`. A later answer
replaces an earlier one until the item closes; the same words from the same
session change nothing. When the item's `answer_in` names another session,
stderr says the user must also type it there. `entry close <id>` takes
`--decision <id>`, an active decision whose `settles` names the item, or
`--note` (at most 300 characters), or both; without `--note` the note reads
`settled by <decision>`. It retires the item with `retirement_reason`
`closed: <note>`. A choice closes only once answered; an action item, or a
trial entry, closes either way. Both verbs print the entry in its `show`
shape and are audited as `entry answer` and `entry close`, by id. `entry
list --state open|answered|closed|withdrawn` (repeatable) keeps open items
in those states.

To correct a decision, `entry update --option` replaces its options and
sets a new recommendation when supplied. `--clear-options` clears both.
`--recommended` alone updates the existing recommendation.
`--rationale` replaces an agent decision's rationale. `--content`
replaces the decision statement.

For retrieval with any agent evidence, checked quotes and source labels
form the answer and claim text; model-written attribution prose is not
forwarded. Conflicts retain their checked cites with a fixed attribution
notice. Daniel-only retrieval keeps the existing answer behavior.

Adding a field is compatible. Renaming, removing or retyping one is a
breaking change and shows up as a golden diff.

## Errors and exit codes

| Exit | Meaning | stderr |
|------|---------|--------|
| 0 | success, including an empty result and a closed stdout | warnings and at most one notice |
| 1 | the command failed | `error: <message>` or `{"error": "...", "code": "..."}` |
| 2 | usage error, including an invalid or too-short id, tag, topic, project or status | clap's rendering, or `{"error": "...", "code": "usage_error"}`; with `--json` the message keeps clap's detail (the argument name) on one line |

(S11) Every command that takes an entry, proposal or session id (`entry
show|update|retire|answer|close`, `proposal add --older|--newer`, `proposal
update|settle`, `proposal list --entry`, `session show|deactivate|activate|rebuild`
and `session pass --session`) also takes a unique prefix of it of at least
four characters. An exact id wins over a prefix; several matches are
`id_ambiguous` (exit 2); an input that is not an id and is shorter than
four characters is `usage_error` (exit 2); none is the noun's not-found
(`entry_not_found`, `proposal_not_found`, `session_not_found`, exit 1),
whose message names the id and the store path searched. A session's prefix is
matched against its record and its switch (in `scribe.db` once the sessions
have moved, in `control.json` before), and the
audit line of a write names the full id; a write that takes `--author` also
names that author in `author`, and a refused write names both its resolved
target and its author. A write given `--session` and `--instruction`
(`proposal settle`, `entry retire`, `entry forget`) also names
`instruction_session` and, once the check found his turn,
`instruction_message` (audit format 3), never the instruction's words. A
refusal after his turn was found (`not_interactive`, `instruction_stale`,
`instruction_mismatch`, or a store refusal) names its message too;
`instruction_message` is `null` only when no turn of his held the
instruction, or the turn holding it has no message id.

Decision and open-item refusals use `decided_by_required`,
`asked_by_required`, `record_fields` (flags for the wrong tag),
`invalid_options`, `invalid_recommendation`, `recommendation_why`
(missing or misplaced `--why`), `record_required` (missing task, PR or
repository reference), `rationale_required`, `rationale_not_allowed`,
`decision_quote_not_allowed` (user quote on an agent decision),
`decision_session` (editing a user decision's source sessions),
`decision_too_long`, `invalid_supersedes`, `supersede_user_decision`
(including trial decisions with no typed decider), and `invalid_settles`.
`entry answer` and `entry close` refuse an entry not under `open-item`
(`not_open_item`) and a closed or withdrawn item (`item_settled`); `entry
answer` refuses words not found (`quote_not_found`), no `session:`
reference (`quote_required`) and words redaction would alter
(`needs_redaction`); `entry close` refuses an unanswered choice
(`not_answered`), a `--decision` that is not an active decision settling
the item (`invalid_decision`), and a blank or long note (`invalid_text`).
The marker hook records `unknown_tag` for a refused decision or open-item
marker.

Stable codes: `entry_not_found`, `session_not_found`, `io_error`,
`store_corrupt`, `config_invalid`, `store_too_new`, `store_locked`,
(phase 4) `store_database`,
`store_symlink`, `store_foreign_owner`, `store_permissions`, `no_data_dir`,
`output_error`, `usage_error`, `hook_payload_invalid`,
`segment_not_found`, `sealed_segment_changed`, `confirm_required`,
`transcript_missing`, `session_busy` (exit 1), (S11) `id_ambiguous` (exit 2;
the message lists up to ten candidates, sorted, and how many more), `user_only`, `reserved_author`, `tag_not_found`, `tag_conflict`, `tag_configured`,
`proposal_not_found`, `proposal_settled`, `entry_not_active`,
(S12) `invalid_text`, `invalid_topics`, (format 8) `quote_required`,
`quote_not_found`, `not_action_tag`, `cancel_window`, (§12, `--session` with
`--instruction`) `instruction_not_found`, `instruction_stale`,
`instruction_mismatch` (including negation or exception markers in the instruction or answered reply), `not_interactive`, `already_covered`,
`proposal_invalid`, `proposal_not_agent`,
`session_cataloged`, `transcript_not_found`, (format 19) `memory_not_found`,
`memory_not_now`, (format 25) `memory_changed`, `revision_not_found`,
`profile_shape`, `profile_pinned`, `profile_too_long`,
`profile_unknown_link`, `profile_sensitive`, `profile_milestone`, (phase 6)
`migrate_blocked`, `export_target_exists`, `export_needs_migrate`,
`session_forgotten` (a record write to a forgotten session, or switching it
on), `handoff_shape` and `handoff_too_long` (a `handoff` entry whose body
violates the six fixed headings in order or exceeds 3,000 characters),
`handoff_session` (a `handoff` entry with an invalid or multiple distinct
`session:` references, or an `entry update` that adds, removes or replaces
a handoff's `session:` reference),
`not_memory_tag` (`entry add --spoken` on an action tag, `handoff`, `decision`,
`open-item` or a `require_evidence` tag), `needs_redaction` (a checked
decision, spoken-instruction or annotation quote that redaction would alter), `not_agent_note`
(`entry update --content` on an approved claim rather than a direct agent
note), for `tmux` `fzf_missing` and
`tmux_failed` (exit 1; `tmux` renders no payload, its popup owns the
terminal), for `session pass`
`session_pass_failed` and `session_not_on`, and for `question
ask` the model codes `model_timeout`, `model_unavailable`, `model_failed`,
`model_malformed`. A hook's refusals
are not errors and appear only in its reply and audit lines:
`unknown_tag`, `unparsed_instruction`, `too_many_marks`,
`quote_not_verbatim`, `annotation_out_of_bounds`, `covered`,
`needs_redaction`, `model_failed`, `provenance_failed`,
and `unknown_scribe_command`. A session
pass's failures appear in its report and audit lines, and the run's
`session_pass_failed` error names each with its code:
`provenance_failed`, `io_error`, `session_answer_invalid` (budgets or
shape), `model_timeout`, `model_unavailable`, `model_failed`,
`model_malformed`, and the store codes. A not-found error names the path it looked
for and the `list` command that shows the existing ids.

## Hook replies

`scribe hook <event>` writes Claude Code hook JSON, the same in every mode
(`--json` and `--plain` change nothing): one compact line,
`{"systemMessage": "...", "hookSpecificOutput": {"hookEventName":
"UserPromptSubmit", "additionalContext": "..."}}`, or nothing at all when
there is nothing to say (a prompt without `#scribe`, a `Stop` with
nothing cataloged or refused, every `SessionEnd`, and a `SessionStart` with no open proposal, no stuck
action entry and no continuation to suggest). `SessionStart` with open proposals prints
`{"systemMessage": "scribe: <n> proposals to review — scribe proposal
list"}` (`1 proposal`), and nothing for the agent. (Format 8) When active
action entries older than one hour have no pickup it prints, as its own
line of the same `systemMessage` (after the proposals line when there is
one), `scribe: <n> action entries not picked up after 1 hour (<tag> <n>,
…) — scribe entry list --tag <tag> --status active` (`1 action entry`).
When this session is not opted in and an active session in the same
project was updated within the last day, it adds one more line of that
`systemMessage` (after the stuck-entries line when there is one),
`scribe: this may continue <id> (recorded) — type /scribe on --from <id> to keep recording`,
when just one record matches. With several matches it names their ids. No line when this session's
switch is on or it already has a record, when no record in the project is
that recent, or when the sessions cannot be read. (Format 19) In an
interactive session (`CLAUDE_CODE_ENTRYPOINT` is `cli` or `claude-desktop`) with memory, a
last group, a session or a claim to show, `SessionStart` also gives the agent the brief
(as `scribe brief` prints it) as `hookSpecificOutput.additionalContext`
(`hookEventName` `SessionStart`); a headless run (any other entrypoint, or
none) gets none. A verified clear with a single predecessor and a current
`continue` handoff prepends its body to the brief and writes the link.
A `new`, missing or legacy handoff leaves the ordinary brief alone.
Multiple clear predecessors are named in `systemMessage`. When the single
clear predecessor is the newest transcript of an open group seat, an
interactive Claude Code `SessionStart` whose source is `startup`, `clear`, or
omitted appends the starting session to that seat's chain (audited against the
group id) and notes the held seat in `additionalContext`. The same source
conditions apply to clear-handoff restoration; `resume`, `compact`, and other
source values do not use clear ancestry. When the transcript is not written at an eligible start, its context says ancestry will be checked at the first prompt. The first unblocked typed prompt whose transcript is readable (or whose session already has a continuation link or seat) consumes that deferred check and may receive the handoff and seat context once, provided it has no continuation link or seat; blocked `/scribe` prompts and prompts while the transcript is still missing keep the check pending. A session that already holds a seat
is told which seat it holds, while an unseated session with no open groups
gets no seat context.
When open groups exist and the starting session holds no seat for another
reason (no predecessor line, multiple predecessors, or a predecessor holding
no open seat), it joins nothing and notes in `additionalContext` that
`scribe group join` must be re-run. A session a later group member replaced
on its seat holds none: its chain row stays as history, it joins nothing, and
`additionalContext` says it was replaced on its seat, with no join
recommendation. A seat-lookup failure appears in `systemMessage`;
`additionalContext` also recommends the command. A typed
`/scribe continue <handoff-id>` supplies the same body in the prompt’s
`hookSpecificOutput.additionalContext` and lets that turn reach the agent.
It
exits 0 whenever the payload parsed, including when every line was refused
or the store could not be used, so the prompt is never blocked; a payload
that is not valid JSON, lacks a field or has an invalid `session_id` exits 1
with `hook_payload_invalid`.
The `UserPromptSubmit` reply has one echo line per settling line (form 4,
a whole line; 2_design.md §4.4), then the marker echoes below. A settling line
echoes `scribe: approved p-<n> — <older> is superseded by <newer>; ask
leaves it out of answers` (`<older> is retired` for a stale proposal),
`scribe: approved p-<n> — already covered by <entry>; no new entry` when an
active entry of the same tag and project already covers the candidate (the
proposal becomes `rejected`, `newer` stays empty, and the covering id is
named only in the reply; when the approval was already recorded and its
entry is missing, format 15 also keeps its settlement and sets
`closed_reason: covered`),
`scribe: rejected p-<n> — its entries stay as they are, and it is not
proposed again`, `scribe: retired <id> — ask leaves it out of answers;
scribe entry show <id> still has it`, `scribe: forgot <id> — entry text and
evidence, proposal quotes of it, and related candidate quotes, evidence and
reasons are cleared; session records stay`, `scribe: forgot s-<n> (<full id>) — the
session record and <k> stored messages are erased[; it is switched off for
good]; the provider transcript was left untouched`, `scribe: s-<n> (<full id>) is
already forgotten — cleared remaining scribe data, including any stale
checkpoint; the provider transcript was left untouched` on a repair, or
`scribe: <id> is already <state>
— nothing changed`, `scribe: refused "<line>" — <reason>` (codes
`proposal_not_found`, `entry_not_found`, `proposal_settled`,
`entry_not_active`) or `scribe: could not apply "<line>" — <error>`. A
change writes one audit line whose `target` is the proposal or entry id (a
session forget's is `s-<n>`); a
refusal one line with `target` `session:<id>` and its code; a repeat or an
already-covered approval none. A covered approval is a change: one audit
line, target the proposal id, and no error code. Replaying it is a refusal.
A forgotten session is absent from session listings. `session show`
returns its ids and `forgotten` timestamp with empty content and no log.
An approved entry with an erased source has `cite.session: "deleted"`
in JSON and `session:deleted` in the table detail.
The text is for Daniel and the agent to read; the audit line, not the
echo, is the record. Daniel's `systemMessage` carries the plain echoes;
the agent's `additionalContext` carries each echo with a per-line note:
` (recorded by scribe; no action needed)` after a settled line, and
` (not recorded by scribe; relay this to Daniel)` after a refused line
(`scribe: refused …`), a settling line whose transcript failed the
provenance check (`scribe: could not record "<line>" — the hook payload
failed the transcript check: <reason>`, code `provenance_failed`) or one
that could not be applied. A refused or failed line therefore never says
no action is needed, even beside a settled one.

(Format 22, 2_design.md §14.) A `#scribe` marker, inside a sentence or
opening a line that is not a settling command (§14.6; outside inline code,
double quotes, fences, blockquotes, indented code and pasted content),
writes nothing at submit and calls no model.
The `UserPromptSubmit` reply echoes `scribe: marked — cataloged after the
reply` (`marked <n> times` for several markers), with the agent note
` (scribe tries to catalog this #scribe marker after your reply and tells
you if it gives up; record it yourself only then)`, and `scribe: refused #scribe in "<line>" — <reason>` with the
relay note for a marker refused before any call (a settling verb after it
or a `#scribe/<word>` other than `global`: `unparsed_instruction`; a fourth
marker: `too_many_marks`). The same turn's `Stop` reply then prints, in
its `systemMessage`, one line per marker:
`scribe: added <id> to <tag> — "<statement>" · your words: "<quote>"`,
`scribe: nothing recorded for #scribe in your message — <reason>` (the
model found nothing, a check refused the answer, an active entry already
covers it — `already covered by <id> in <tag>` — or redaction would change
the quote), or `scribe: could not catalog #scribe in your message —
<error>; will retry at the next reply (<n> of 3)` and, at the third
failure, `…; gave up after 3 attempts — scribe mark retry <session> tries
again`. A given-up marker reaches the agent as well: the `Stop` reply's
`hookSpecificOutput` (`hookEventName` `Stop`) carries in
`additionalContext` the echo, the marked line and the note `(not cataloged
by scribe; if the user wants it kept, record it with their exact words:
scribe entry add --tag <tag> --quote '<their words>' --spoken
'<statement>' --author <name>, or ask them to run scribe mark retry
<session> from a shell with model credentials)`. When `claude` fails and
none of `[models] credentials` is set in scribe's environment, `<error>`
names them: `claude ended with exit status: 1 (none of
CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY is set in scribe's
environment)`. A refusal the prompt hook echoed
is not echoed again. Audit lines (`hook stop`): an entry's id as `target`;
`session:<id>` with no code for nothing found; `session:<id>` with the
refusal code (`unparsed_instruction`, `too_many_marks`,
`quote_not_verbatim`, `unknown_tag`, `annotation_out_of_bounds`,
`covered`, `needs_redaction`, `model_failed`) for a refusal; none for a
pending retry or a marker already settled. A `hook stop` or
`hook user-prompt-submit` run that stores a group member's new typed turns
writes one content-free success line for the run (`target` `session:<id>`, no
code), and a run that stores none writes none. A marker on a line an older
store recorded verbatim (an entry cites that prompt and line) is not
cataloged again. When the transcript fails the provenance check, `Stop`
echoes `scribe: could not check for #scribe markers — <reason>`, audits
`provenance_failed` and does nothing else. Under `--provider codex` the
same echoes and audit lines hold, line-start forms included: the prompt
hook echoes a marker once the typed turn is in the rollout (before that it
refuses with `provenance_failed`, `typed input not yet verified …; Stop will
retry`), and Stop catalogs from the rollout.

`scribe mark list [--session <id>]` lists the markers not cataloged yet,
oldest first: `pending` ones and given-up ones (`refused`, `model_failed`).
`--json` is the list envelope with `marks`, each record in `marks.json`'s
shape (store.md): `session`, `message`, `prompt_sha256`, `marker`,
`outcome`, `entry`, `code`, `attempts`, `at`. The table columns are
`SESSION MESSAGE MARKER OUTCOME CODE ATTEMPTS AT`; piped lines are
tab-separated with timestamps. `SESSION` in both forms is the session's `s-<n>`
alias when it is cataloged, otherwise its shortest unique prefix (`ShortIds`,
at least eight characters); either form is accepted back as a session id.
`--json` keeps the provider id in `session`. An empty list prints nothing on
stdout and `no uncataloged #scribe markers: every marker has an entry or an
answer` on stderr. It is read-only and not audited.
`scribe mark retry <session>` resolves the session (a cataloged id, its
`s-<n>` alias, a transcript's id or a unique prefix), turns its given-up
markers back to `pending` with no attempts, and catalogs every unsettled marker of the
session's transcript as `Stop` does, with no per-run turn bound. It prints
the markers it wrote, in `mark list`'s forms, and the `Stop` echoes on
stderr. When a call fails again it exits 1 with `model_failed` (`<n> #scribe
marker(s) of <session> still not cataloged — <error>`) and the marker stays
`pending`. It writes one audit line (`mark retry`, `target`
`session:<id>`, the code on failure); a missing transcript is
`transcript_missing` and one failing the provenance check
`provenance_failed`.

The single-line commands `/scribe group move <session-id> <slug>`, `/scribe group remove <session-id>`, and `/scribe session update <session-id> project|title <value>` have provider-specific replies. Claude Code blocks with `scribe: session <id> moved|removed|updated` on success, or `scribe: <verb> refused — <reason>` on refusal. Codex returns a verified command's reply as `systemMessage` and `hookSpecificOutput.additionalContext`, without `decision: "block"`. Their audit targets contain session ids and error codes only.

A prompt whose first line is `/scribe on`, `/scribe off`, `/scribe status`,
`/scribe on --from <id> [--type <type>]`, `/scribe link <id> [--type
<type>]`, `/scribe catalog <id>`, `/scribe group <slug>`, `/scribe group add <slug>
<session-id>...` or a bare `/scribe`
(also `/scribe:scribe …`, the plugin-namespaced form; the control verb is
case-insensitive, including `group` and `add`; group slugs are validated as
typed and must be lowercase) is not scanned for `#scribe` lines. Under
Claude Code, the reply is `{"decision": "block", "reason": "<status>"}`;
the prompt does not reach the model, and Daniel sees the reason. Under
Codex, once the typed turn is present and verified in the rollout, the reply
uses `systemMessage` and `hookSpecificOutput.additionalContext` with no
`decision: "block"`; the command text still reaches the model. If the turn
is not yet in the rollout, Codex says `scribe: not recorded — typed input
not yet verified in the host transcript; nothing recorded yet; Stop will
retry. Agent or skill-injected text cannot activate scribe` and does not
advance the cursor. For `/scribe group` and `/scribe group add`, the first
line selects the command even when later prompt lines follow. Status,
unknown-command and failed-transcript replies use these one-line forms:
`scribe: on — …`, `scribe: already on since <time> — nothing changed`,
`scribe: off — …`, `scribe: this session is not on — nothing changed`,
`scribe: on since <time>[; a pass is due]; <record status>` or `scribe: off
— this session was never switched on …` for a status, `scribe: unknown
command "/scribe <arg>" — use /scribe on, /scribe off, /scribe status,
/scribe on --from <id>, /scribe link <id> [--type spawned_from|continues],
/scribe catalog <id>, /scribe group <slug>, /scribe group add <slug> <session-id>..., /scribe group move <session-id> <slug>, /scribe group remove <session-id>, /scribe session update <session-id> project|title <value> or /scribe continue <handoff-id>`,
or `scribe: /scribe <arg> not applied — …` when the payload fails the
transcript check. `/scribe group <slug>` replies `scribe: joined group
<slug> (g-<n>)` (audited against `session:<id>`) or `scribe: group refused —
<reason>` with code `group_membership`, `invalid_text`, or
`provenance_failed`. A slug changed by the configured redactor is refused
with `invalid_text` and is not echoed. `/scribe group add <slug>
<session-id>...` replies `scribe: group <slug> (g-<n>) opened|open`, then one
line per named session: `<id>: added; transcript on disk, <n> typed turns
stored`, `<id>: added; membership only, no transcript on disk` or `<id>:
already a member; nothing changed`. For example, a one-session add with no
transcript gives Claude Code:

```json
{"decision":"block","reason":"scribe: group planning (g-1) opened\n  <id>: added; membership only, no transcript on disk"}
```

Codex returns the same reply text as context, without blocking:

```json
{"systemMessage":"scribe: group planning (g-1) opened\n  <id>: added; membership only, no transcript on disk","hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"scribe: group planning (g-1) opened\n  <id>: added; membership only, no transcript on disk"}}
```

Both providers audit one line per added session, against `session:<id>`.
Codex additionally audits one content-free success outcome against the
typing session (`session:<caller-id>`) when the verified typed turn advances
its rollout cursor. A refusal is `scribe: group add refused — <reason>` with code
`group_membership` (a session already in another group, one `session <id>
already belongs to group <group>` per session, `; nothing changed`; or more
than 32 sessions), `invalid_text`, `session_forgotten`, `provenance_failed`
(the named transcript's check), `id_ambiguous` or `usage_error` (a non-exact
id shorter than four characters), audited against the typing session, and
changes nothing. If the caller's transcript fails its check before dispatch,
the hook instead replies `scribe: group refused — <reason>` with
`provenance_failed`, audited against the caller. Codex checks its rollout
before dispatch; a caller transcript failure is reported as `scribe: not
recorded — <reason>` with `provenance_failed`, no audit target, and no
group-add or cursor advance. An explicitly agent- or harness-originated
turn, or one opening with a markup wrapper, that contains a group command replies
`scribe: group refused — the prompt is not a typed user turn` with
`group_membership`, audited against the caller. Ordinary, unmarked prose or
pasted text that mentions `/scribe group add` later in the prompt is not a
command and receives no group refusal; an actionable command must parse
from the first line of a typed prompt. A status of a linked
session ends `; linked: <type>
<id>, …`. A link replies `scribe: on — this session is cataloged from now
on and linked: <type> <id>; …`, `scribe: on — … it was already linked:
<type> <id>`, `scribe: linked — this session is <type> <id>`, `scribe:
already linked: <type> <id> — nothing changed`, or `scribe: /scribe
<command> refused — cannot link this session: <reason>` with code
`link_unknown_session` (no record and no switch for the target),
`link_self`, `link_cycle` (the target's links lead back here),
`link_conflict` (a second `spawned_from` parent) or `link_session_off`
(`/scribe link` in a session neither switched on nor recorded). A catalog
replies `scribe: cataloging <id> as yours — its windows run at the next
session pass, …` (the rest names `scribe session catalog <id> --author
<name>` for running them now), `scribe: <id> is already being cataloged — nothing changed; …`
(a catalog switch still due), or `scribe: /scribe catalog <id> refused —
<message>` with the code `session catalog` would give
(`session_cataloged`, `transcript_not_found`, `id_ambiguous`, …). A switch,
a new link or a new catalog writes one audit line (`target` `session:<id>`); a refusal
one line with its code; a no-op or a status writes none.

## Session pass report

`scribe session pass --json` prints `{"passes": [<pass>, ...],
"remaining": <integer>}`: one pass per due session handled this run (at
most five, by session id, or explicitly named sessions) and the due sessions
left for the next run, including those waiting for retry backoff and a
partly processed session stopped between windows by the routine deadline. A
pass is `{"session", "result", "error_code", "passes", "dropped_terms",
"gave_up", "windows", "warnings", "error"}`: `result` is `written`, `unchanged`
(nothing new; `due` cleared without a model call), `waiting` (no transcript
or message yet; still due) or `failed` (the previous record kept, still due);
`error_code` is a stable code or `null`; `error` is present only on a failed
pass and contains its store-redacted reason, flattened to one line and cut to
at most 300 characters; `passes` is the record's pass
count (successful record updates, one per window), or `null` on failure
or without a record; `windows` is the number of successfully persisted
windows for this session in this invocation (including progress before a
later failure; validation retries do not add a window). `dropped_terms`
counts terms dropped across persisted windows whose
`said` was not verbatim from its named user turn, whose message id was
unknown or future, or whose lengths were invalid; `gave_up` is always
`false` (a failed pass is never given up; the field is kept for the
shape). A pass also carries `segments_sealed` (segments sealed in this
invocation) and `segments` (the latest persisted record/checkpoint's segment
count, or `null`). `session rebuild` prints this report with its one session,
`remaining: 0` and an additional `author: {kind: agent, name: <name>}` naming
its requester (`author` is `null` for ordinary passes). A failed rebuild keeps its private checkpoint and exits 1
with `session_pass_failed`, naming the failure and the rebuild command to
resume; it does not change the control switch. `--confirm` missing refuses
with `confirm_required`, naming the current segment and pass counts;
`transcript_missing` and `session_busy` also refuse before model calls.
One audit line names `session:<id>` for success or refusal. (Format 19) A
pass also carries `memory_written` (format 25: `now` and milestone files
written across persisted windows), (format 25) `memory_pending` (undated
`now` candidates kept), `memory_promoted` and `memory_proposed` (both
always `0` since [2_design.md §15.9](../2_design.md#159-phases) phase 5;
kept because field names are a contract) and `memory_refused`
(`{"<code>": <count>}` of elements refused: `memory_invalid`,
`memory_unverified`, `memory_sensitive`, `memory_inferred`,
`memory_undated`, `memory_not_stated`, `memory_unknown`,
`memory_pending_full`); a refusal is not a failure, and the pass's audit
line names each `memory:now/<stem>` and `memory:milestones/<stem>` it wrote
in `targets`. *(S14, built)*
`session catalog` prints the same report with its one session and
`remaining: 0`. `author` stays `null` (the catalog flag is not a JSON
field). A failure exits 1 with `session_pass_failed` and names
`session catalog` to resume. A session that already has a record or a
switch catalog did not write is refused `session_cataloged` before a
model call. When this run wrote the switch, the audit log gains a
success line for that switch ahead of the pass line; both name
`session:<id>`.
`warnings` is an array of strings, empty when no deterministic repair was
persisted in this invocation. After the existing one model retry, scribe
repairs header budget overruns (sentence or bullet boundary, else a word
boundary with `…`), segment texts over 900 (cut and seal at window end when
no new segment follows), and new starts outside the window or at/before the
open segment's last message (continue the open segment when it fits, else
start at the first window message after the open segment's last message when
that start precedes every start the model kept; if no such start exists, cut
the continuation and seal the open segment at window end; with no open
segment, start at the window's first message). New segments whose own text is
cut are kept; content merged into the open segment remains part of it.
sealed segments stay unchanged. Other shape defects still fail the window,
and the entire repaired answer is revalidated before redaction and writing
([design §5.5](../2_design.md#55-the-log-segments-and-sealing)).
Warnings name the window (1-based within this invocation), field or new
segment index and action, such as `window 1: repaired next: cut at bullet
boundary (budget 1000 characters)` or `window 2: repaired new segment 1
starts_at: continued the open segment`. They contain no model prose or
message ids. They accumulate in persisted-window order, including progress
before a later failure; failed/unwritten repairs are not reported. Rebuild
reports only repairs persisted by this invocation, not earlier checkpoints.
Each warning also prints on stderr in every CLI mode as `warning: session
<id>: <warning>`; the plugin reply includes the same `warnings` array.
Plain and table
print one row per pass (`session result error_code passes dropped_terms`);
with nothing due stdout is empty, stderr says `no session pass is due` and
the exit is 0. If all due sessions are waiting for backoff, `passes` is
empty, `remaining` counts them, and stderr names the retry wait. Automatic
retries wait 5, 10, 20, 40, then at most 60 minutes; `--session <id>` ignores
backoff. When any pass failed, the report is printed as usual and
the run then exits 1 with `session_pass_failed` on stderr, `error: <n>
session pass(es) failed: <id> <code>: <reason>, ...; each stays due with retry
backoff; use 'scribe session pass --session <id>' to retry now` (or that message in the JSON error object
under `--json`). `--session <id>` naming a session that is not on exits 1
with `session_not_on` before any pass. Each pass except a `waiting`
one writes one audit line (`target` `session:<id>`, with its code only; the
reason is never added to the audit line).

## Memory

(Format 25, [2_design.md §15.4–§15.6](../2_design.md#154-the-brief-one-session-start-page).)
`scribe memory show` with no id prints the memory block, read-only and
unaudited: plain and table the text

```text
## Profile (as of <YYYY-MM-DD>)
<memory/profile.md in full>
## Now
- <stem> · due <YYYY-MM-DD> · <title>
## Milestones
- <stem> · <YYYY-MM-DD> · <title>
```

(`## Profile` without a date when no revision wrote the file; every `now`
item, nearest date first; the last 5 milestones, newest first; an empty
part prints `(none)`), JSON `{"profile": {"text", "as_of"} or null, "now":
[{"id", "date", "title"}], "milestones": [{"id", "date", "title"}]}`.
Warnings go to stderr: `memory_pending` (a write was cut short; the profile
shown is its latest committed revision until the next memory write), a
skipped file that does not parse, a `read-only copy`, and format-24 items
not yet migrated.

`memory show <id>` prints one file: `profile`, or a `now` or milestone
stem. JSON is `{"id", "kind", "date", "now_kind", "title", "text",
"sources", "updated", "raised", "as_of"}` (`kind` `profile`, `now` or
`milestone`; `now_kind` `deadline`, `event` or `plan` for a `now` item;
`raised` `null` or `{"at", "session"}`; `as_of` the profile's revision
date; fields that do not apply are `null`). `memory show --revision <n>`
prints a stored profile revision: plain and table its text, JSON
`{"revision", "created", "text"}`; an unknown one is `revision_not_found`.
`--page` is a deprecated alias of the block.

`memory ack <stem> --session <id> --author <name>` marks a `now` item
raised in that session (`memory_not_now` for a milestone); the same
session again changes nothing. `memory forget <stem> --author <name>
--confirm` removes a `now` item or milestone for good with its history
and the messages it cites, which `scribe search --kind message` then no
longer finds (without `--confirm`, `confirm_required`; `profile` is `usage_error`, as
the profile is corrected by editing it). `memory restore <n>` writes
revision `n` back as a new revision. Each prints the item and writes one
audit line.

`memory refresh` rebuilds the profile (§15.6). JSON is `{"result",
"revision", "sessions", "backlog", "milestones", "removed"}`: `result`
`written` or `nothing_new` (no model call, nothing written), `revision` the
new profile revision or `null` when the profile did not change, `sessions`
the sessions read, `backlog` those left for later runs, `milestones` the
stems written, `removed` the passed `now` stems removed. Plain is one
`<result>\t<revision or ->\t<sessions>\t<backlog>\t<milestones>\t<removed>`
line (the last two counts); the table prints the fields one per line. A
refused answer exits 1 with its `profile_*` code and writes nothing;
`memory_changed` when memory changed while the model ran. The Orbit tool
`scribe.memory-refresh` returns the same JSON.

`memory rules` prints memory's fixed rules, read-only and unaudited. JSON
is `{"rules": [{"rule", "value", "origin": {"source", "file", "line"},
"meaning"}]}` in a fixed order: `max_per_pass` (3), `max_pending` (5),
`now_keep_days` (14), `milestone_lines` (5), `profile_chars` (6000) and
`user_says` (the phrases). `origin.source` is always `shipped`. Plain
prints `<rule>\t<value>\t<origin>` lines; the table adds `meaning`.

## Brief

`brief [--sessions <n>] [--project <p>]` (n 0 to 20, default 5; the
project defaults to the current directory's) reads and writes nothing.
Plain and table print the same text, which the `SessionStart` hook also
injects: `# scribe brief`; the memory block (as `memory show` prints it);
`## Last group` with the most recently opened group in the current project,
`g-<n> · <title>`, followed by its retained close summary when present or
`Still open.` when the group is open without a summary; `## Recent sessions`
with one `- <id> · <YYYY-MM-DD HH:MMZ> · <project> · <title>` line
per session, `<p>`'s first, each part newest first (or
`(none)`): a recorded session with an active handoff ends in
`[handoff: <entry-id>]`, or `[handoff: <entry-id>, mode: <continue|new>]`
when its newest handoff has a mode. An off session with a handoff and no
session record appears as `- <id> · <YYYY-MM-DD HH:MMZ> · <project> · no
session record [handoff: <entry-id>]`, or with the same mode suffix when set;
`## Open claims` with one `- <p-id> · <YYYY-MM-DD HH:MMZ> ·
<tag> · "<quote>"` line per claim, by tag then id, and the line naming how
to settle one (or `(none)`). A quote is on one line and cut at 200
characters, ending in `…`. JSON:

```json
{"memory": {"profile", "now", "milestones"}, "warnings": ["<memory warning>"],
 "project": "<p or null>",
 "last_group": {"id", "title", "closed", "summary"} or omitted,
 "sessions": [{"id", "project", "title", "at", "has_record", "handoff", "handoffs", "handoff_mode"}],
 "claim_count": 1,
 "claims": [{"tag": "<tag>", "claims": [{"id": "p-<n>", "kind": "candidate|supersedes|stale",
   "quote", "statement", "subject", "older", "newer", "opened_at"}]}]}
```

`last_group` is omitted when the project has no group. When present, its
`closed` and `summary` fields are strings or `null`.

(Format 25) `memory` is the memory block's JSON (above); `warnings` holds
the memory block's warnings.

For each `sessions[]` row, `handoff_mode` is the newest handoff's mode or
`null` when there is no handoff or its newest handoff has no mode.

`at` is a session's last recorded activity, and the brief orders
`sessions[]` by it, newest first, the current project's first. For a
recorded session it is the later of its last segment's end (else its
`updated`) and its newest active handoff's `created`; an older handoff leaves
it at the record's time. A session with no record (handoff only) has `at`
equal to its newest active handoff's `created`. `quote` is a candidate's
verbatim words, or a consistency proposal's first quoted span (its reason
when it quotes none);
`statement` is a candidate's statement (its quote when it has none) or a
consistency proposal's reason; `subject` is `null` for a consistency
proposal, `older` and `newer` `null` for a candidate.

## Consistency scan report

`scribe consistency scan --json` and the Orbit tool
`scribe.consistency-scan` print `{"reapplied": [{"proposal", "entry",
"status"}], "opened": [<proposal>, ...], "closed": [<proposal>, ...],
"covered": [{"proposal", "by"}],
"judged_pairs", "pairs_left", "unrelated_pairs", "dropped_labels",
"stale_entries", "failed_batches": [{"batch", "error_code"}], "memory": {…}}`. `reapplied`
lists approved proposals the scan finished: a supersession or retirement
whose older entry was still `active` and is now `superseded` or `retired`
(its `status`), and an approved candidate whose reserved entry was missing
and is now written, with `status: active`; `opened` holds each proposal opened, in
`proposal list --json`'s shape, by id; `closed` holds each open candidate
proposal closed automatically (`tag_retired`), in the same shape; `covered`
holds each approved candidate this scan closed because an active entry
already covers its statement (`proposal` the candidate, `by` the entry) —
the proposal is `rejected` with `closed_reason: covered` (format 15),
keeping who approved it and any instruction, `newer` stays empty, and the
covering id is not stored on it; `judged_pairs` counts pairs the model labelled, `pairs_left` those past
the cap of 40 or not reached in this call's time, `unrelated_pairs` those
labelled `unrelated`, `dropped_labels` labels refused (a quote not verbatim,
an unknown or repeated pair, an unknown label); `stale_entries` counts
active entries past `review_after` without a stale proposal;
`failed_batches` names each batch of ten the model failed, from 1.
*(S10)* Adds `deactivated` (`[{session, route, reason, cite, quote}]`,
`route` `age` or `removed`; `cite` and `quote`, the later entry and its
verbatim words, are `null` for `age`), `removal_pairs_judged`,
`removal_pairs_left` (past the cap of 20 or not reached in this call's
time), `dropped_removal_labels` and `failed_removal_batches` (`[{batch,
error_code}]`, numbered from 1 apart from `failed_batches`), and
`failed_deactivations` (`[{session, route, error_code}]`): a failed
session write is reported and audited by session while the other
deactivations continue. Each
deactivation is also a plain row `deactivated <session> <route> <cite or
-> -`, and each failed removed-feature batch `failed removal-batch-<n>
<code> - -`. Each deactivation writes one audit line (target
`session:<id>`), each failed deactivation one (failure target
`session:<id>` with its error code), each failed removed-feature batch one (target
`removal-batch:<n>`, with its code); when removed-feature pairs are
left, stderr says how many. Plain and
table print one row per change: `reapplied p-<n> <status> <entry> -`,
`opened p-<n> <kind> <older> <newer or ->`, `closed p-<n> <closed_reason>
<older> <newer or ->`, `covered p-<n> <entry> - -` and `failed batch-<n> <code> - -` (columns `action
id what older newer`). When nothing was reapplied, opened, closed, covered or
failed, stdout is empty and stderr says `nothing to propose: …`; when pairs
are left, stderr says how many. The exit is 0 whenever the store could be
read, including failed batches. Each reapplied approval (target the entry
id), opened proposal (target the proposal id), closed proposal (target the
proposal id), covered candidate (target the proposal id) and failed batch (target `batch:<n>`, with its code) writes
one audit line under `consistency scan`.

(Format 20, [2_design.md §13.5](../2_design.md#135-consolidation-the-weekly-consistency-routine).)
Since §15.9 phase 5 the scan judges no memory pairs: `memory` keeps its
shape with zero counts and empty lists, and the rows and audit lines below
are no longer written. It was `{"pairs_judged", "pairs_left", "unrelated", "dropped_labels",
"failed_batches": [{"batch", "error_code"}], "merged": [{"item", "into"}],
"superseded": [{"item", "by"}], "promoted": [{"item", "to", "rule"}],
"completed": [<id>, ...]}`: memory pairs the model labelled, those past the
cap of 40 or not reached in this call's time, those labelled `unrelated`
(never offered again), labels refused (a quote not verbatim in one of that
side's quotes, an unknown or repeated pair, an unknown label), the batches
of ten the model failed (numbered from 1 apart from the others), each
newer item merged into an older one, each older item superseded, each
`now` item promoted by rule (b) after the merges, and each item changed to
complete a promotion a crash cut short. Pairs are judged only while
`[memory] enabled`. Plain and table add the rows `merged <item> memory
<into> <item>`, `superseded <item> memory <item> <by>`, `promoted <item>
<rule> <item> <to>`, `completed <id> memory - -` and `failed
memory-batch-<n> <code> - -`. Each item whose file changed writes one audit
line (target `memory:<id>`), and each failed memory batch one (target
`memory-batch:<n>`, with its code); when memory pairs are left, stderr says
how many.

## Ask

`scribe question ask "<question>" --json` and the Orbit tool `scribe.ask`
return the same object (2_design.md §6.1, §6.6):

```json
{
  "status": "answered | no_record | conflict | record",
  "answer": "…",
  "claims": [{"text": "…", "quote": "…", "cite": "preferences-2"}],
  "conflicts": [{"cites": ["preferences-1", "preferences-2"], "note": "…"}],
  "evidence_considered": 3,
  "dropped_quotes": 0,
  "hidden_matches": 1,
  "lookup": null,
  "records": [],
  "warnings": []
}
```

`cite` is an entry id, `session:<id>` (a session's header: title,
summary, goals and next), `session:<id>#<n>` (one log segment: heading and
text) or `task:<id>`; every `quote` was found verbatim in the item it
cites. Inactive sessions are left out of evidence and lookups unless
`include_hidden`; `hidden_matches` also counts inactive sessions that
retrieval matched and left out. A `session` record carries `log` with
every segment and its text; the table view prints the header and the
log's table of contents. `evidence_considered` is the number of
items the model read, or of records a lookup returned; `dropped_quotes`
counts claims dropped by the check (format 14: an entry with `stated_by` is
read as scribe's statement of Daniel's turn followed by his labelled words; a
claim whose `quote` lies in his words has `text` `Daniel`, one whose quote
lies only in the statement has `text` `scribe's statement of Daniel's turn`
(format 16: for an idea the pass wrote itself, `Daniel (captured by the
session pass)` and `scribe's statement of Daniel's turn, captured by the
session pass (not typed by Daniel)`, and the item the model reads also
carries its outcome; phase 2: for one caught from an assistant turn,
`assistant (captured by the session pass)` and `scribe's statement of the
assistant's turn, captured by the session pass (not the user's words)`), and the answer is built from those labels as it is with agent evidence); `hidden_matches` counts superseded or
retired entries that matched and were left out (0 with `include_hidden`).
`lookup` is `null` when retrieval ran, else `last_session`,
`named_session`, `next_steps`, `tag_period`, `term` or
`orchestrator_session`, and then `status` is `record` and `records` holds
the stored records, each `{"kind": "entry" | "session" | "task", "cite",
"entry", "session", "task", "terms"}` with exactly the one named by `kind`
set: `entry` is `entry show --json`'s object, `session` is `session show
--json`'s (with `on` and `due`), `task` is `{"id", "title", "status",
"tags", "updated", "execution_summary"}` with the summary verbatim, and
`terms` holds the terms a term lookup matched. `warnings` lists record
files skipped because they did not parse, as a list's do.

Plain prints `<status>\t<answer>`, then `claim\t<cite>\t<quote>\t<text>`,
`conflict\t<cites>\t<note>` and `record\t<kind>\t<cite>` lines; the table
is a detail view followed by each record in full. A model failure exits 1
with its code; nothing found is `no_record` and exits 0. `question ask`
reads only and writes no audit line. Handoff evidence is labelled `agent
note (<author>)`; when it has a mode, ` [handoff mode: <continue|new>]` is
appended in the model context and claim text.

## Plugin envelope

`scribe plugin call` reads Orbit's exec envelope (`{"schema_version": 1,
"tool": "scribe.<verb>", "input", "context": {"config": {"root",
"transcripts", "task_workspaces"}, …}}`) on stdin and prints exactly one
compact line, in every mode, then exits 0: `{"ok": true, "output":
<output>}`, where `<output>` is the ask object for `scribe.ask`, the
session pass report for `scribe.session-pass` and the consistency scan
report for `scribe.consistency-scan`; or `{"ok": false, "error": {"code",
"message", "retryable"}}` with `code` `invalid_envelope`, `unknown_tool`,
`invalid_input` (unknown keys included; the routine tools take `{}`),
`not_configured` (`root` missing, empty, relative or the manifest's
`/var/tmp` default; for `scribe.session-pass`, `transcripts` likewise), a
model code (`retryable` for `model_timeout` and `model_failed`) or a store
code (`retryable` for `store_locked` from a routine tool). For
`scribe.session-pass`, `session_pass_failed` when any pass in the report
failed (not `retryable`: each failed session stays due for the next run);
its message is the CLI's, naming each failed session with its code and short
reason, and the report is not returned. `scribe.ask`
writes no audit line; the routine tools write the lines their CLI commands
do, in the store `root` names.

## Session groups

`group backfill <id>` reports `group`, `members` (each with `seat`, `session`, `stored`, `skipped`, and `transcript_unavailable`), and total `stored` and `skipped` in JSON. Skipped counts are eligible turns already stored; an unavailable or refused transcript is named separately because its turn count is unknown. Terminal output gives the same counts and marks unavailable transcripts.

`group open`, `group join`, `group leave`, `group close` and `group show` return one group.
Its JSON fields are `id`, optional `slug` (present on project groups, omitted when
absent), `title`, `kind`, `project`, `opened`, `closed` (null while open),
`opener`, `related`, `seats` and optional `summary` (closing summary, omitted
when absent). Each seat has a `name`, a `role`, its `transcripts` in join order
and `newest_handoff` (an active entry id or null). Seats sort by name.
`group list --json` returns the same objects newest first in `items`, alongside
`total`, `truncated` and `warnings`.

`group list`'s table holds short values only: `ID TITLE KIND PROJECT OPENED
CLOSED SEATS`, where `SEATS` is the right-aligned number of seats and only
`TITLE` is cut to a terminal's width (`CLOSED` is `-` while open). Its plain
line keeps every field: `id title kind project opened closed opener related
seats summary`, with `seats` as `<name> (<role>): <session> -> <session>
[handoff: <id or ->]` joined by `; `, and `-` for an empty `related`, `seats`
or `summary`. Piped output, the brief and `group show` print `s-<n>` for
cataloged session ids, otherwise their shortest unique prefix (at least eight
characters); a terminal `group show` table does the same, while a terminal
`session list` table prints prefixes only. `--json` and stored references
retain full provider ids. This supersedes the earlier STD-01@2 §R15 reading for piped session ids; Daniel chose
“Yes, s-<n> alias for open-item-7” (2_design.md, Ids).
