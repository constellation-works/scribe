---
type: design
summary: "Spec: output contract — streams, modes, plain columns, the JSON list envelope and record fields (segments, activity, agent authors and direct writes, search hits; future transcripts), warnings, errors and exit codes"
last_validated: 2026-11-10
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
| `entry list` | `id status project topics created content author age` (`author` is `daniel` or `agent:<name>`; `age` is an action entry's age, `45s`, `12m`, `3h` or `9d`, and `-` for any other entry) |
| `session list` | `id status due project topics updated title` (`due` is `yes` or `no`) |
| `session list --tree` | `id parent status due project topics updated title` (`parent` is the session the row nests under, its `spawned_from` link else its first `continues` link, or `-`) |
| `session list --include-inactive` | `id status active due project topics updated title` (`active` is `yes` or `no`); without the flag the columns are unchanged, inactive sessions are left out, and when any matched the notice `<n> inactive session(s) also match and are hidden; --include-inactive lists them` goes to stderr. With `--tree` too, `active` follows `status` |
| `proposal list` | `id kind state older newer opened_at reason admitted closed_reason` (format 11: `admitted` is `trigger "<phrase>" matched "<words>"; criteria: <criteria or ->` for a candidate a capture rule admitted, else `-`; `closed_reason` is `tag_retired` when scribe closed it without Daniel, else `-`) |
| `tag list` | `name entries stale_after_days aliases description`, (S12) `source` (`config` or `store`), then `action` (`free` or `ask-first`, with `,quote` when the tag requires a quote), `retired` and `rejected` (counts of agent entries); the last three are `-` for a tag that is not an action tag; (format 11) then `route` (the tag's capture route, `propose`, `write` or `agent`, or `-` when it has no capture rules) |
| `tag show <tag>` (format 11) | one `<field>\t<value>\t<origin>` line each for `tag`, `source`, `aliases`, `description`, `stale_after_days`, `action` (`free`/`ask-first`, with `,quote`, or `-`), then the capture rules `from`, `route`, `triggers` (quoted, comma separated), `criteria` and `not`; `origin` is `shipped`, `config <file>:<line>` or `none` (an unset rule's value is `-`), and `-` for the first six. The table prints `<field> <value>  (<origin>)` |
| `record search` (S11) | `cite kind score updated title snippet` (`score` to three decimals; the table cuts `title` to 40 characters and fits `snippet` to the width) |
| `transcript list` *(S14)* | `session project daniel_turns bytes started ended first_prompt` |

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
`proposals` or `tags`, (S11) `hits` for `record search`, and *(S14)*
`transcripts` for `transcript list`. `total` counts the records that matched the filters;
`truncated` is true when `--limit` returned fewer. No matches is the same
envelope with an empty array. A warning is `{"path", "code", "error"}` for a
record file the list skipped because it did not parse (`code` is
`store_corrupt`); each is also a `warning: skipped …` line on stderr in
every mode.

stderr also carries, in every mode, one notice when a list is empty (where
records come from, or `no <noun> match <filters>` naming the flags) or cut
(`showing N of M matching <noun>; raise or drop --limit to see the rest`).
(S11) `record search` names its terms first among the filters (`no records
match "<terms>" --kind session`), and when hidden records also matched it
adds `<n> hidden record(s) also match: superseded or retired entries, or
inactive sessions; --include-hidden shows them`.

(S11) List filters: `entry list` and `session list` take `--since <date>`
and `--until <date>` (`YYYY-MM-DD`, both days included), `--status`,
repeated `--topic` (every one must match) and repeated `--related
<kind:target>` (any may match); `entry list` also `--author <name>` and
`--author-kind daniel|agent`; `proposal list` takes `--state`, `--since`,
`--until`, `--entry <id>` (the older or the newer entry) and
`--author-kind daniel|agent|scribe`. An entry's day is its `created`, a
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
`daniel` or `agent`), `updated_by` (an author or `null`) and
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
with --window)`).

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
`closed_reason` (`tag_retired` when scribe closed it without Daniel, else
`null`) and, on a `candidate` object, `admitted` (`{trigger, matched,
criteria}`: the capture-rule trigger as configured, the turn's words it
matched, verbatim and at most 120 characters, and the tag's `criteria`
then in force or `null`; `null` on a candidate opened before format 11).

**Tag rules** (`tag show <tag> --json`, format 11): `tag`, `source`
(`config` or `store`), `aliases`, `description` (or `null`),
`stale_after_days` (or `null`), `action` (`{mode, require_quote}` or
`null`) and `capture` (`{from, route, triggers, criteria, not, origin}`).
Each capture field is `null` when the tag has no such rule; `origin` has
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
was cut), `project`, `topics` (a segment's are its session's plus its
own), `author` (an entry's `{kind, name}`, `null` for a session), `active`
(a session's flag; `null` for an entry), `updated` (an entry's or
session's `updated`, a segment's `to`), and `status` (added beside the
designed fields: an entry's `active`, `superseded` or `retired`, a
session's `open` or `final`).

**Transcript** *(S14, `transcripts[]`, `transcript show`)*: `session`,
`project` (the attributed slug), `project_mentions` (`{"<slug>":
<count>}`), `cwd`, `started`, `ended` (the last message's time), `bytes`,
`daniel_turns`, `first_prompt` (at most 300 characters); `transcript show`
adds `daniel_first_turns` (up to five strings of at most 300 characters).

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
`--author`; the name `daniel` still records kind `agent`.

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
target and its author.

Stable codes: `entry_not_found`, `session_not_found`, `io_error`,
`store_corrupt`, `config_invalid`, `store_too_new`, `store_locked`,
`store_symlink`, `store_foreign_owner`, `store_permissions`, `no_data_dir`,
`output_error`, `usage_error`, `hook_payload_invalid`,
`segment_not_found`, `sealed_segment_changed`, `confirm_required`,
`transcript_missing`, `session_busy` (exit 1), (S11) `id_ambiguous` (exit 2;
the message lists up to ten candidates, sorted, and how many more), `daniel_only`, `reserved_author`, `tag_not_found`, `tag_conflict`, `tag_configured`,
`proposal_not_found`, `proposal_settled`, `entry_not_active`,
(S12) `invalid_text`, `invalid_topics`, (format 8) `quote_required`,
`quote_not_found`, `not_action_tag`, `cancel_window`,
`proposal_invalid`, `proposal_not_agent`, `already_covered`,
`session_cataloged`, `transcript_not_found`, for `tmux` `fzf_missing` and
`tmux_failed` (exit 1; `tmux` renders no payload, its popup owns the
terminal), for `session pass`
`session_pass_failed` and `session_not_on`, and for `question
ask` the model codes `model_timeout`, `model_unavailable`, `model_failed`,
`model_malformed`. A hook's refusals
are not errors and appear only in its reply and audit lines:
`unknown_tag`, `unparsed_instruction`, `unsupported_instruction`,
`no_referent`, `provenance_failed`, `unknown_scribe_command`. A session
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
there is nothing to say (a prompt without `#scribe`, every `Stop` and
`SessionEnd`, and a `SessionStart` with no open proposal, no stuck
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
that recent, or when the sessions cannot be read. It
exits 0 whenever the payload parsed, including when every line was refused
or the store could not be used, so the prompt is never blocked; a payload
that is not valid JSON, lacks a field or has an invalid `session_id` exits 1
with `hook_payload_invalid`.
The `UserPromptSubmit` reply has one echo line per `#scribe` line: `scribe:
added <id> to <tag> — "<content>"`, `scribe: already recorded <id> in <tag>
— "<content>"`, `scribe: already covered by <id> in <tag> — "<content>"`
(an active entry of the same tag and project already covers the statement;
nothing is written and no audit line is written), `scribe: refused "<line>" — <reason>` or `scribe: could not
record "<line>" — <error>`. A form-3 line whose referent the model did not
resolve is still added, and its echo puts `(guessed: <why>; recorded the
<role> message before your prompt instead)` after the tag. A settling line
echoes `scribe: approved p-<n> — <older> is superseded by <newer>; ask
leaves it out of answers` (`<older> is retired` for a stale proposal),
`scribe: approved p-<n> — already covered by <id>; no new entry` when an
active entry of the same tag and project already covers a candidate (the
proposal becomes `rejected`, its `newer` stays empty, and no entry is written),
`scribe: rejected p-<n> — its entries stay as they are, and it is not
proposed again`, `scribe: retired <id> — ask leaves it out of answers;
scribe entry show <id> still has it`, `scribe: forgot <id> — its text is
blanked for good; the id and cite stay`, `scribe: <id> is already <state>
— nothing changed`, `scribe: refused "<line>" — <reason>` (codes
`proposal_not_found`, `entry_not_found`, `proposal_settled`,
`entry_not_active`) or `scribe: could not apply "<line>" — <error>`. A
change writes one audit line whose `target` is the proposal or entry id; a
refusal one line with `target` `session:<id>` and its code; a repeat none.
The text is for Daniel and the agent to read; the audit line, not the
echo, is the record. Daniel's `systemMessage` carries the plain echoes;
the agent's `additionalContext` carries each echo with a per-line note:
` (recorded by scribe; no action needed)` after a recorded, already
recorded, already covered or settled line, and ` (not recorded by scribe; relay this to
Daniel)` after a refused line (`scribe: refused …`), a line that could
not be recorded (`scribe: could not record …`) or a settling line that
was refused or could not be applied. A refused or failed line therefore
never says no action is needed, even beside a recorded line.

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
One audit line names `session:<id>` for success or refusal. *(S14, built)*
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

## Consistency scan report

`scribe consistency scan --json` and the Orbit tool
`scribe.consistency-scan` print `{"reapplied": [{"proposal", "entry",
"status"}], "opened": [<proposal>, ...], "closed": [<proposal>, ...],
"judged_pairs", "pairs_left", "unrelated_pairs", "dropped_labels",
"stale_entries", "failed_batches": [{"batch", "error_code"}]}`. `reapplied`
lists approved proposals whose older entry was still `active` and is now
`superseded` or `retired`; `opened` holds each proposal opened, in
`proposal list --json`'s shape, by id; `closed` holds each open candidate
proposal closed automatically (`tag_retired`), in the same shape; `judged_pairs` counts pairs the model labelled, `pairs_left` those past
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
<older> <newer or ->` and `failed batch-<n> <code> - -` (columns `action
id what older newer`). When nothing was reapplied, opened, closed or
failed, stdout is empty and stderr says `nothing to propose: …`; when pairs
are left, stderr says how many. The exit is 0 whenever the store could be
read, including failed batches. Each reapplied approval (target the entry
id), opened proposal (target the proposal id), closed proposal (target the
proposal id) and failed batch (target `batch:<n>`, with its code) writes
one audit line under `consistency scan`.

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
counts claims dropped by the check; `hidden_matches` counts superseded or
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
