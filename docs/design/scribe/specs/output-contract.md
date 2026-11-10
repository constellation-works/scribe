---
type: design
summary: "Spec: output contract — streams, modes, plain columns, the JSON list envelope and record fields, warnings, errors and exit codes"
last_validated: 2026-11-02
---

# Spec: Output contract

Every data command renders one typed payload in the mode the sink resolved;
stdout carries only that payload; warnings, notices and errors go to stderr;
the exit code says whether it worked. The JSON form is a public contract,
pinned by the output goldens in `crates/scribe/tests/goldens/output/`.

## Why This Exists

Three readers — Daniel at a terminal, a script in a pipe, an agent through
a shell tool — need different bytes from the same result, and scripts and
agents break silently when the machine form drifts.

## Modes

| Mode | Chosen by | Shape |
|------|-----------|-------|
| `table` | `--format table`, or auto on a terminal | header row + one line per record; `show` is one field per line, then the full text |
| `plain` | auto off a terminal | one tab-separated line per record, no header; `show` prints its record's list line |
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
| `entry list` | `id status project topics created content` |
| `session list` | `id status due project topics updated title` (`due` is `yes` or `no`) |
| `proposal list` | `id kind state older newer opened_at reason` |
| `tag list` | `name entries stale_after_days aliases description` |

Entries and sessions are oldest first (by `created`, then `started`, ties by
id); proposals by number; tags by name.

## JSON list envelope

Every list is
`{"<key>": [<record>, ...], "total": <integer>, "truncated": <boolean>,
"warnings": [<warning>, ...]}`, with `<key>` `entries`, `sessions`,
`proposals` or `tags`. `total` counts the records that matched the filters;
`truncated` is true when `--limit` returned fewer. No matches is the same
envelope with an empty array. A warning is `{"path", "code", "error"}` for a
record file the list skipped because it did not parse (`code` is
`store_corrupt`); each is also a `warning: skipped …` line on stderr in
every mode.

stderr also carries, in every mode, one notice when a list is empty (where
records come from, or `no <noun> match <filters>` naming the flags) or cut
(`showing N of M matching <noun>; raise or drop --limit to see the rest`).

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
`review_after` (`YYYY-MM-DD` or `null`).

**Session** (`sessions[]`, `session show`): `id`, `status` (`open`,
`final`), `project`, `topics`, `related`, `terms` (`[{term, said,
message}]`, Daniel's own words), `provider`, `host`, `created`,
`updated`, `title`, `cwd`, `started`, `on_since`, `scribed_through` (a
message id or `null`), `passes`, `next_open`, `summary`, `goals`,
`outcome`, `next`, and from `control.json` `on` and `due` (both `false`
when the session has no switch). The four text fields are model-written.

**Proposal** (`proposals[]`): `id` (`p-<n>`), `kind` (`supersedes`,
`stale`), `older`, `newer` (or `null`), `reason`, `quotes` (`[{entry,
text}]`), `state` (`open`, `approved`, `rejected`), `opened_at`,
`settled_at` (or `null`), `settled_by` (a cite or `null`).

**Tag** (`tags[]`): `name`, `aliases`, `description` (or `null`),
`stale_after_days` (integer or `null`), `configured` (false for a directory
no configured tag names), `entries` (count, any status).

Adding a field is compatible. Renaming, removing or retyping one is a
breaking change and shows up as a golden diff.

## Errors and exit codes

| Exit | Meaning | stderr |
|------|---------|--------|
| 0 | success, including an empty result and a closed stdout | warnings and at most one notice |
| 1 | the command failed | `error: <message>` or `{"error": "...", "code": "..."}` |
| 2 | usage error, including an invalid id, tag, topic, project or status | clap's rendering, or `{"error": "...", "code": "usage_error"}` |

Stable codes: `entry_not_found`, `session_not_found`, `io_error`,
`store_corrupt`, `config_invalid`, `store_too_new`, `store_locked`,
`store_symlink`, `store_foreign_owner`, `store_permissions`, `no_data_dir`,
`output_error`, `usage_error`, `hook_payload_invalid`, and for `question
ask` the model codes `model_timeout`, `model_unavailable`, `model_failed`,
`model_malformed`. A hook's refusals
are not errors and appear only in its reply and audit lines:
`unknown_tag`, `unparsed_instruction`, `unsupported_instruction`,
`no_referent`, `provenance_failed`, `unknown_scribe_command`. A session
pass's failures appear only in its report and audit lines:
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
`SessionEnd`, and a `SessionStart` with no open proposal). `SessionStart`
with open proposals prints `{"systemMessage": "scribe: <n> proposals to
review — scribe proposal list"}` (`1 proposal`), and nothing for the agent. It
exits 0 whenever the payload parsed, including when every line was refused
or the store could not be used, so the prompt is never blocked; a payload
that is not valid JSON, lacks a field or has an invalid `session_id` exits 1
with `hook_payload_invalid`.
The `UserPromptSubmit` reply has one echo line per `#scribe` line: `scribe:
added <id> to <tag> — "<content>"`, `scribe: already recorded <id> in <tag>
— "<content>"`, `scribe: refused "<line>" — <reason>` or `scribe: could not
record "<line>" — <error>`. A form-3 line whose referent the model did not
resolve is still added, and its echo puts `(guessed: <why>; recorded the
<role> message before your prompt instead)` after the tag. A settling line
echoes `scribe: approved p-<n> — <older> is superseded by <newer>; ask
leaves it out of answers` (`<older> is retired` for a stale proposal),
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
echo, is the record.

A prompt whose first line is `/scribe on`, `/scribe off`, `/scribe status`
or a bare `/scribe` (also `/scribe:scribe …`, the plugin-namespaced form;
the argument is case-insensitive) is not scanned for `#scribe` lines: the
reply is `{"decision": "block", "reason": "<status>"}`, which keeps the
prompt from the model and shows Daniel the reason. The reason is one line:
`scribe: on — …`, `scribe: already on since <time> — nothing changed`,
`scribe: off — …`, `scribe: this session is not on — nothing changed`,
`scribe: on since <time>[; a pass is due]; <record status>` or `scribe: off
— this session was never switched on …` for a status, `scribe: unknown
command "/scribe <arg>" — use /scribe on, /scribe off or /scribe status`,
or `scribe: /scribe <arg> not applied — …` when the payload fails the
transcript check. A switch writes one audit line (`target`
`session:<id>`); a no-op or a status writes none.

## Session pass report

`scribe session pass --json` prints `{"passes": [<pass>, ...],
"remaining": <integer>}`: one pass per due session handled this run (at
most five, by session id) and the due sessions left for the next run. A
pass is `{"session", "result", "error_code", "passes", "dropped_terms",
"gave_up"}`: `result` is `written`, `unchanged` (nothing new; `due`
cleared without a model call), `waiting` (no transcript or message yet;
still due) or `failed` (the previous record kept, still due);
`error_code` is a stable code or `null`; `passes` is the record's pass
count or `null` without a record; `dropped_terms` counts terms whose
`said` was not verbatim from one of Daniel's turns; `gave_up` is true when
this failure cleared `due`: the third in a row, or for a retryable code
(`model_unavailable`, `model_timeout`) the 96th. Plain and table
print one row per pass (`session result error_code passes dropped_terms`);
with nothing due stdout is empty, stderr says `no session pass is due` and
the exit is 0. A failed pass still exits 0; each pass except a `waiting`
one writes one audit line (`target` `session:<id>`, with its code).

## Consistency scan report

`scribe consistency scan --json` and the Orbit tool
`scribe.consistency-scan` print `{"reapplied": [{"proposal", "entry",
"status"}], "opened": [<proposal>, ...], "judged_pairs", "pairs_left",
"unrelated_pairs", "dropped_labels", "stale_entries", "failed_batches":
[{"batch", "error_code"}]}`. `reapplied` lists approved proposals whose
older entry was still `active` and is now `superseded` or `retired`;
`opened` holds each proposal opened, in `proposal list --json`'s shape, by
id; `judged_pairs` counts pairs the model labelled, `pairs_left` those past
the cap of 40 or not reached in this call's time, `unrelated_pairs` those
labelled `unrelated`, `dropped_labels` labels refused (a quote not verbatim,
an unknown or repeated pair, an unknown label); `stale_entries` counts
active entries past `review_after` without a stale proposal;
`failed_batches` names each batch of ten the model failed, from 1. Plain and
table print one row per change: `reapplied p-<n> <status> <entry> -`,
`opened p-<n> <kind> <older> <newer or ->` and `failed batch-<n> <code> -
-` (columns `action id what older newer`). When nothing was reapplied,
opened or failed, stdout is empty and stderr says `nothing to propose: …`;
when pairs are left, stderr says how many. The exit is 0 whenever the store
could be read, including failed batches. Each reapplied approval (target the
entry id), opened proposal (target the proposal id) and failed batch
(target `batch:<n>`, with its code) writes one audit line under
`consistency scan`.

## Ask

`scribe question ask "<question>" --json` and the Orbit tool `scribe.ask`
return the same object (2_design.md §6.1, §6.6):

```json
{
  "status": "answered | no_record | conflict | record",
  "answer": "…",
  "claims": [{"text": "…", "quote": "…", "cite": "decisions-2"}],
  "conflicts": [{"cites": ["decisions-1", "decisions-2"], "note": "…"}],
  "evidence_considered": 3,
  "dropped_quotes": 0,
  "hidden_matches": 1,
  "lookup": null,
  "records": [],
  "warnings": []
}
```

`cite` is an entry id, `session:<id>` or `task:<id>`; every `quote` was
found verbatim in the item it cites. `evidence_considered` is the number of
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
failed (`retryable` when every failure was `model_unavailable` or
`model_timeout`); its message names each failed session, its code and the
model program, and the report is not returned. `scribe.ask`
writes no audit line; the routine tools write the lines their CLI commands
do, in the store `root` names.
