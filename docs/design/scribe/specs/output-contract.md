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
`output_error`, `usage_error`. A not-found error names the path it looked
for and the `list` command that shows the existing ids.
