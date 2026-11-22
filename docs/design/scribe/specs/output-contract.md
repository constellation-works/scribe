---
type: design
summary: "Spec: output contract — streams, modes, plain columns, the JSON list envelope and record fields (segments, activity, agent authors and direct writes, search hits; future transcripts), warnings, errors and exit codes"
last_validated: 2026-11-21
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
or `--json` has it whole. In plain and table rows, free text is flattened to
one line (every whitespace run becomes one space), an empty value or list
is `-`, lists are comma-joined, and timestamps are RFC 3339.

## Plain columns

| Command | Columns |
|---------|---------|
| `entry list` | `id status project topics created content author age` (`author` is `user` or `agent:<name>`; `age` is an action entry's age, `45s`, `12m`, `3h` or `9d`, and `-` for any other entry) |
| `session list` | `id status due project topics updated title` (`due` is `yes` or `no`) |
| `session list --tree` | `id parent status due project topics updated title` (`parent` is the session the row nests under, its `spawned_from` link else its first `continues` link, or `-`) |
| `session list --include-inactive` | `id status active due project topics updated title` (`active` is `yes` or `no`); without the flag the columns are unchanged, inactive sessions are left out, and when any matched the notice `<n> inactive session(s) also match and are hidden; --include-inactive lists them` goes to stderr. With `--tree` too, `active` follows `status` |
| `proposal list` | `id kind state older newer opened_at reason admitted closed_reason` (format 11: `admitted` is `trigger "<phrase>" matched "<words>"; criteria: <criteria or ->` for a candidate a capture rule admitted, else `-`; `closed_reason` is `tag_retired` when scribe closed it without Daniel, `instructed` when an agent rejected it on Daniel's checked instruction (format 13), `covered` when an approved candidate's entry was missing and an active entry covers it (format 15), else `-`; format 14: for a candidate with a statement, `reason` is the statement, and the table prints `Daniel's words: "<quote>"` on a row under it in the same column, while plain joins them as `<statement> \| Daniel's words: "<quote>"`; the single-proposal detail a proposal write prints (`proposal add`, `update`, `settle`) and `proposal show <id>` prints adds `statement` and `subject` fields before `quote` in the table, and its JSON carries both on `candidate`; format 24 (2_design.md §15.3): for a candidate with a context, `reason` is the context (plain `<context> \| The user's words: "<quote>"`), and an older claim's (a statement and no context) is `older claim: <statement>`; the detail prints `subject` and then `context` (the context, or `older claim: <statement>`) before `quote`, and for an open candidate a `settle` field, `#scribe approve <id> or #scribe reject <id>`, so a claim is settled from `proposal show` without opening its session; (ORB-26246) a candidate an agent settled on Daniel's instruction adds `<approved\|rejected> by <agent> on your instruction "<quote>" (<session>, <date>)`: plain joins it after the reason with ` \| `, the table prints it on a row under, in the same column, and a proposal write's detail adds it as an `instruction` field) |
| `tag list` | `name entries stale_after_days aliases description`, (S12) `source` (`config` or `store`), then `action` (`free` or `ask-first`, with `,quote` when the tag requires a quote), `retired` and `rejected` (counts of agent entries); the last three are `-` for a tag that is not an action tag; (format 11) then `route` (the tag's capture route, `propose`, `write` or `agent`, or `-` when it has no capture rules) |
| `tag show <tag>` (format 11) | one `<field>\t<value>\t<origin>` line each for `tag`, `source`, `aliases`, `description`, `stale_after_days`, `action` (`free`/`ask-first`, with `,quote`, or `-`), then the capture rules `from`, `route`, `triggers` (quoted, comma separated), `criteria`, `not` and (phase 2) `max_open`; `origin` is `shipped`, `config <file>:<line>` or `none` (an unset rule's value is `-`), and `-` for the first six. The table prints `<field> <value>  (<origin>)` |
| `scribe search` (S11) | `cite kind score updated title snippet` (`score` to three decimals; the table cuts `title` to 40 characters and fits `snippet` to the width; (ORB-26246) an entry an agent approved on Daniel's instruction ends its snippet, in every mode, with ` \| approved by <agent> on your instruction "<quote>" (<session>, <date>)`) |
| `memory show` (format 19) | `id section status page seen latest statement` (`page` is `on` or why the item is off it: `merged`, `superseded`, `promoted` (format 20), `retired`, `forgotten`, `section_off`, `decayed`, `past_window`; `seen` counts occurrences; `latest` is the newest occurrence's date; a forgotten statement is `-`). Sorted by section in page order (`now`, `milestones`, `profile`, `projects`, `working`), then id. `memory show <id>`, `ack`, `retire` and `forget` print the row, then one `occurrence\t<n>\t<date>\t<session>\t<message>\t<quote>` line each, newest first (`-` for a forgotten quote); the table prints the item's fields one per line |
| `transcript list` *(S14)* | `session project user_turns bytes started ended first_prompt` |

Search hits are best first; on a tie entries come before sessions, then
the newest first (as `ask` ranks); transcripts newest first by
`started`. Entries and sessions are oldest first (by `created`, then `started`, ties by
id); proposals by number; tags by name. `session list --tree` keeps that
order among siblings but lists each session right after the one it nests
under, depth first; a session whose parent is not listed is a root. The
table indents the title two spaces per level; the piped form does not
indent, and the `parent` column carries the nesting.

## JSON list envelope

Every list is
`{"<key>": [<record>, ...], "total": <integer>, "truncated": <boolean>,
"warnings": [<warning>, ...]}`, with `<key>` `entries`, `sessions`,
`proposals` or `tags`, (S11) `hits` for `scribe search`, (format 19)
`items` for `memory show`, and *(S14)*
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
match "<terms>" --kind session`), and when hidden records also matched it
adds `<n> hidden record(s) also match: superseded or retired entries, or
inactive sessions; --include-hidden shows them`.

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
`retirement_reason` (text or `null`). (Format 11) `entry show` adds
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

**Session** (`sessions[]`, `session show`): `id`, `status` (`open`,
`final`), `project`, `topics`, `related` (unchanged: model-extracted
`kind:target` strings), `links` (`[{type, session, title}]`: the links the
record stores, `type` `spawned_from` or `continues`, `title` the linked
session's record title or `null` when it has none), `children`
(`[{type, session, title}]`: the sessions whose links point at this one,
derived at read time from every record, oldest first; `type` is the
child's link type), `terms` (`[{term, said,
message}]`, Daniel's own words), `provider`, `host`, `created`,
`updated`, `title`, `cwd`, `started`, `on_since`, `scribed_through` (a
message id or `null`), `passes`, `next_open`, `summary`, `goals`,
`outcome`, `next`, and from `control.json` `on` and `due` (both `false`
when the session has no switch). The four text fields are model-written.
`links` and `children` were added with session links (store format 3);
every other field is unchanged.

Store format 6 adds `rebuilt_by` (`null` or `{kind, name}`): the agent whose
`--author` ran the `session rebuild` that wrote the record. It is in `--json`
only; the table view does not print it.

Store format 4 adds `active`, `keep_active`, `active_change` (`null` or
`{at, by, route, reason, cite}`), `cataloged_by` (`null` or `{at, by}`)
and `log`: in `session show` the segments `[{n, heading, first_message,
last_message, from, to, state, windows, origin, topics, related, text}]`,
oldest first; a `session list` row has no `log` key (use `show`). `outcome` stays and
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

**Hit** (S11, `hits[]`): `kind` (`entry` or `session`; *(S7)*
`discussion`), `cite` (an entry id, `session:<id>` for a header,
`session:<id>#<n>` for a segment; *(S7)* a discussion slug), `id` (the
entry's or session's id), `segment` (integer or `null`), `score` (a
number, three decimals), `title` (a session's title, a segment's heading,
or for an entry `<id> (<tag>)`), `snippet` (up to 240 characters on one
line, starting a little before the first matching word, with `…` where it
was cut; format 14: for an entry with `stated_by`, `scribe's statement of
Daniel's turn: <statement> | Daniel's words: "<quote>"`, each part cut to
240 characters; format 16: for an idea the pass wrote itself, `scribe's
statement of Daniel's turn, captured by the session pass (not typed by
Daniel): <statement> | Daniel's words: "<quote>" | outcome: <outcome>`,
and (phase 2) for one the pass caught from an assistant turn, `scribe's
statement of the assistant's turn, captured by the session pass (not the
user's words): <statement> | The assistant's words: "<quote>" | outcome:
<outcome>`),
`project`, `topics` (a segment's are its session's plus its
own), `author` (an entry's `{kind, name}`, `null` for a session), `active`
(a session's flag; `null` for an entry), `updated` (an entry's or
session's `updated`, a segment's `to`), and `status` (added beside the
designed fields: an entry's `active`, `superseded` or `retired`, a
session's `open` or `final`).

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
retire`, `entry pickup` (format 8; prints the entry it noted), `tag add`, `tag update`, `proposal add`, `proposal update`,
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
show|update|retire`, `proposal add --older|--newer`, `proposal
update|settle`, `proposal list --entry`, `session show|deactivate|activate|rebuild`
and `session pass --session`) also takes a unique prefix of it of at least
four characters. An exact id wins over a prefix; several matches are
`id_ambiguous` (exit 2); an input that is not an id and is shorter than
four characters is `usage_error` (exit 2); none is the noun's not-found
(`entry_not_found`, `proposal_not_found`, `session_not_found`, exit 1),
whose message names the id and the store path searched. A session's prefix is
matched against its record and its switch in `control.json`, and the
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

Stable codes: `entry_not_found`, `session_not_found`, `io_error`,
`store_corrupt`, `config_invalid`, `store_too_new`, `store_locked`,
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
`memory_not_active`, `memory_not_now`, for `tmux` `fzf_missing` and
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
naming the record updated most recently. No line when this session's
switch is on or it already has a record, when no record in the project is
that recent, or when the sessions cannot be read. (Format 19) In an
interactive session (`CLAUDE_CODE_ENTRYPOINT` is `cli`) with memory to
show, `SessionStart` also gives the agent the memory page (as `memory show
--page` prints it) as `hookSpecificOutput.additionalContext`
(`hookEventName` `SessionStart`); a headless run (any other entrypoint, or
none) gets no page. It
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
reasons are cleared; session records stay`, `scribe: <id> is already <state>
— nothing changed`, `scribe: refused "<line>" — <reason>` (codes
`proposal_not_found`, `entry_not_found`, `proposal_settled`,
`entry_not_active`) or `scribe: could not apply "<line>" — <error>`. A
change writes one audit line whose `target` is the proposal or entry id; a
refusal one line with `target` `session:<id>` and its code; a repeat or an
already-covered approval none. A covered approval is a change: one audit
line, target the proposal id, and no error code. Replaying it is a refusal.
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
` (scribe catalogs this #scribe marker after your reply; do not record it
yourself)`, and `scribe: refused #scribe in "<line>" — <reason>` with the
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
failure, `…; gave up after 3 attempts`. A refusal the prompt hook echoed
is not echoed again. Audit lines (`hook stop`): an entry's id as `target`;
`session:<id>` with no code for nothing found; `session:<id>` with the
refusal code (`unparsed_instruction`, `too_many_marks`,
`quote_not_verbatim`, `unknown_tag`, `annotation_out_of_bounds`,
`covered`, `needs_redaction`, `model_failed`) for a refusal; none for a
pending retry or a marker already settled. A marker on a line an older
store recorded verbatim (an entry cites that prompt and line) is not
cataloged again. When the transcript fails the provenance check, `Stop`
echoes `scribe: could not check for #scribe markers — <reason>`, audits
`provenance_failed` and does nothing else. Under `--provider codex` the
same echoes and audit lines hold, line-start forms included: the prompt
hook echoes a marker once the typed turn is in the rollout (before that it
refuses with `provenance_failed`, `typed input not yet verified …; Stop will
retry`), and Stop catalogs from the rollout.

A prompt whose first line is `/scribe on`, `/scribe off`, `/scribe status`,
`/scribe on --from <id> [--type <type>]`, `/scribe link <id> [--type
<type>]`, `/scribe catalog <id>` or a bare `/scribe` (also `/scribe:scribe …`, the plugin-namespaced form;
the argument is case-insensitive) is not scanned for `#scribe` lines: the
reply is `{"decision": "block", "reason": "<status>"}`, which keeps the
prompt from the model and shows Daniel the reason. The reason is one line:
`scribe: on — …`, `scribe: already on since <time> — nothing changed`,
`scribe: off — …`, `scribe: this session is not on — nothing changed`,
`scribe: on since <time>[; a pass is due]; <record status>` or `scribe: off
— this session was never switched on …` for a status, `scribe: unknown
command "/scribe <arg>" — use /scribe on, /scribe off, /scribe status,
/scribe on --from <id>, /scribe link <id> [--type spawned_from|continues] or
/scribe catalog <id>`,
or `scribe: /scribe <arg> not applied — …` when the payload fails the
transcript check. A status of a linked session ends `; linked: <type>
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
pass also carries `memory_written` (memory items created or given an
occurrence across persisted windows), (format 20) `memory_promoted` (`now`
items promoted to `milestones` and milestones written directly on rule (a)
or (c), including rule (b)'s recount), (format 21) `memory_proposed`
(always `0` since [2_design.md §15.9](../2_design.md#159-phases) phase 1
removed the §13.7 proposal; kept because field names are a contract) and
`memory_refused` (`{"<code>": <count>}` of elements refused:
`memory_invalid`, `memory_unverified`, `memory_section_not_written` (a
`working` or `projects` element, which the pass no longer writes),
`memory_section_off`, `memory_sensitive`, `memory_personal_off`,
`memory_inferred`); a refusal is not a failure, and the pass's audit line
names each `memory:<id>` it wrote in `targets`, a promotion's milestone and
profile items included. *(S14, built)*
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

(Format 19, [2_design.md §13](../2_design.md#13-memory-profile-projects-working-now-and-milestones-proposed).)
`scribe memory show --json` prints the list envelope with key `items`; by
default the items on the page, with `--include-off-page` every item, with
`--section <s>` and `--project <p>` (a `projects` item's project) as filters.
An item is `{"id", "section", "status", "statement", "project",
"event_date", "sensitive", "sensitive_kind", "on_page", "off_page",
"not_yet_raised", "acknowledged", "occurrence_count", "sessions", "latest",
"retirement_reason", "promoted_from", "promotion", "merged_into",
"superseded_by", "preference_proposal", "author", "updated_by", "created",
"updated", "occurrences"}`: `status` is `active`, `merged`, `superseded`, `promoted`,
`retired` or `forgotten`; `off_page` is `null` or the reason it is off the
page (as the plain `page` column); (format 20) `promoted_from` is a
milestone's `now` item, `promotion` `null` or `{"rule", "to",
"profile_from", "profile_to"}` (`rule` `profile_change`,
`recurs_in_sessions` or `user_says`) on a milestone and its promoted `now`
item, `merged_into` and `superseded_by` the item a merged or superseded
one went to, each `null` otherwise; (format 21) `preference_proposal` is
the `preferences` candidate (`p-<n>`) a `working` item opened when it
recurred across sessions, else `null` (none is opened since §15.9 phase 1);
`not_yet_raised` is true for a `now` item no agent acknowledged, or whose
date passed since it was; `acknowledged` is `null` or `{"at", "session",
"author"}`; `sessions` counts distinct sessions among the occurrences; an
occurrence is `{"n", "date", "session", "message", "host", "project",
"quote", "author"}`, newest first. A forgotten item's `statement`, quotes
and `retirement_reason` are empty or `null`.

`memory show <id>` (an id or a unique prefix) prints one item.
`memory ack <id> --session <id> --author <name>` marks a `now` item raised
in that session (`memory_not_now` for another section,
`memory_not_active` for any status but `active`); `memory retire <id>
--author <name> --reason <text>` takes it off the page with its redacted
reason; `memory forget <id>[#<n>] --author <name> --confirm` blanks its
text for good, or one occurrence's quote (without `--confirm`,
`confirm_required`); forgetting a whole item also blanks the items merged
into it, directly or through another merged item, and every other side of
a promotion (the `now` item, the milestone and rule (a)'s new profile item
`profile_to`), leaving a superseded profile item. Each prints the item
and writes one audit line naming `memory:<id>`; the same write again
changes nothing and still prints it.

(Format 20.) `memory rules` prints the promotion rules and page windows in
effect, read-only and unaudited. JSON is `{"rules": [{"rule", "value",
"origin": {"source", "file", "line"}, "meaning"}]}` in a fixed order:
`promote.profile_change` (a boolean), `promote.recurs_in_sessions` (a
number; 0 is off), `promote.user_says` (a list of phrases), `decay_days`,
`now_undated_days`, `now_after_days` (numbers of days). `origin` is as in
`tag show`: `source` `shipped` (file and line `null`) or `config` with the
config file and the key's 1-based line. Plain prints `<rule>\t<value>\t<origin>`
lines (a list as quoted phrases joined by `, `; origin `shipped` or
`config <file>:<line>`); the table adds the `meaning` column, cut to fit.

`memory show --page [--project <p>]` prints the session-start page:
plain and table the page text, JSON `{"page": <text or null>, "chars",
"over_budget"}`. The page is the header line, then `Now:` (at most 8
lines, by date), `Milestones:` (never cut), `Profile:`, `Project <p> (as
of <date>):` (the current project's items only), `Working:`, and `Recent
sessions:` (the last `[memory] recent_sessions` active records other than
this session), then the footer line. A part over its budget drops whole
items from its end and says `(+<k> more)`; within `[memory] page_chars`
(2400) apart from milestones; `over_budget` is true when milestones pass
600 characters, and the hook logs `memory_page_over_budget`.

## Brief

`brief [--sessions <n>] [--project <p>]` (n 0 to 20, default 5; the
project defaults to the current directory's) reads and writes nothing.
Plain and table print the same text, which the `SessionStart` hook also
injects: `# scribe brief`; the memory block (today the page `memory show
--page --project <p>` prints, left out when there is none); `## Recent
sessions` with one `- <id> · <YYYY-MM-DD HH:MMZ> · <project> · <title>` line
per session, `<p>`'s first, each part newest first (or
`(none)`); `## Open claims` with one `- <p-id> · <YYYY-MM-DD HH:MMZ> ·
<tag> · "<quote>"` line per claim, by tag then id, and the line naming how
to settle one (or `(none)`). A quote is on one line and cut at 200
characters, ending in `…`. JSON:

```json
{"memory": "<page or null>", "project": "<p or null>",
 "sessions": [{"id", "project", "title", "at"}],
 "claim_count": 1,
 "claims": [{"tag": "<tag>", "claims": [{"id": "p-<n>", "kind": "candidate|supersedes|stale",
   "quote", "statement", "subject", "older", "newer", "opened_at"}]}]}
```

`at` is a session's last activity (its last segment's end, else its
`updated`). `quote` is a candidate's verbatim words, or a consistency
proposal's first quoted span (its reason when it quotes none);
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
`memory` is `{"pairs_judged", "pairs_left", "unrelated", "dropped_labels",
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
reads only and writes no audit line.

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
