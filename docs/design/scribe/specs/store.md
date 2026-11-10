---
type: design
summary: "Spec: store — layout, record files with YAML frontmatter, JSON documents, config, private modes, links, format versioning, corrupt records, redaction, locking and the audit log"
last_validated: 2026-11-02
---

# Spec: Store

The store never exposes a torn file, never rewrites a file a newer build
wrote, never trusts state another user could have written, and never serves
a record under an id its file does not carry. The design is
[2_design.md §3](../2_design.md#3-storage); this spec is what the code does.

## Why This Exists

Hooks in concurrent sessions write to one store that `ask` and the CLI read
from any workspace. Without these rules two writers could lose one another's
work, a crash could leave unparseable state, and a hand edit or another
user's file could pass for one of Daniel's records.

## Layout

```
<root>/                                  0700
├── config.toml                          0600, optional
├── entries/<tag>/<tag>-<n>.md           0600, one file per entry
├── sessions/<yyyy>/<mm>/<session-id>.md 0600, one file per session record
├── control.json                         0600, optional
├── proposals.json                       0600, optional
├── audit.jsonl                          0600, created by the first audited command
└── .lock                                0600, created by the first writer, never deleted
```

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`. A missing root, a
missing subdirectory or a missing optional file is empty, not an error.
Names starting with `.` are never records (a writer's temp file, an
editor's swap file), nor is a file without the `.md` extension; under
`sessions/`, only four-digit year and two-digit month directories are
walked.

## Record files

An entry or session record is a Markdown file: a first line `---`, YAML
frontmatter, a closing `---` line, then the body. One trailing line break
is not part of the body.

Frontmatter is read strictly:

- `format` is read first. Greater than this build's is `store_too_new`;
  `0` or missing is `store_corrupt`.
- `kind` must be `entry` or `session`, matching the directory.
- Every other key is the record's own. An unknown key, a missing key or a
  repeated key is `store_corrupt`, naming the file and the key. A key whose
  value may be `null` (`superseded_by`, `review_after`, a cite's `message`,
  `scribed_through`) must still be present.
- Values are typed: timestamps are RFC 3339; `review_after` is
  `YYYY-MM-DD`; ids, tags and topics are validated as in the domain types
  (`decisions-12`; a lowercase kebab-case tag of at most 32 characters; at
  most five distinct kebab-case topics of at most 40 characters each;
  `related` items `task:`, `pr:`, `repo:` or `session:` plus a target).
- An entry's `id` and `tag` must match `entries/<tag>/<id>.md`; a session's
  `id` must match its file name and its `started` month the `<yyyy>/<mm>`
  directories.

**Entry keys:** `format`, `kind`, `id`, `tag`, `status` (`active`,
`superseded`, `retired`, `forgotten`), `project`, `topics`, `related`,
`provider`, `host`, `created`, `updated`, `said`, `content_from`
(`{kind: instruction}` or `{kind: message, role: user|assistant, message,
at}`), `cite` (`{session, message, at, prompt_sha256, line, cwd}`),
`superseded_by`, `review_after`. The body is the entry's content, verbatim.

**Session keys:** `format`, `kind`, `id`, `status` (`open`, `final`),
`project`, `topics`, `related` (model-extracted references, unchanged by
links), `links` (a list, possibly empty, of `{type, session}`: `type` is
`spawned_from` or `continues`, `session` a session id; the links Daniel
asserted for this session, stored once, on the child; a parent's children
are derived at read time and its record is never rewritten for them),
`terms` (a list, possibly empty, of
`{term, said, message}`: a phrase Daniel coined or defined, the verbatim
span of his turn that does so, and that message's uuid), `provider`, `host`, `created`, `updated`,
`title`, `cwd`, `started`, `on_since`, `scribed_through`, `passes`,
`next_open`. The body is exactly the headings `## Summary`, `## Goals`,
`## Outcome` and `## Next`, in that order, each followed by its text; any
other line, another `##` heading included, is section text, and blank lines
around a section's text are dropped. Text before `## Summary` or a missing
heading is `store_corrupt`.

## JSON documents

- `control.json`: `{"format": 3, "sessions": {"<session-id>": {"on",
  "since", "due", "final", "consecutive_failures", "retry_after",
  "pending_links"}}}`. `pending_links` holds `{type, session}` links
  Daniel asserted before the session had a record; the pass that writes
  the record moves them onto it, then clears them, in one locked call.
  `since` and `retry_after` are RFC 3339 timestamps or `null`; the failure
  count is a saturating unsigned integer. A failed pass stays due; its next
  automatic attempt waits 5, 10, 20, 40, then at most 60 minutes. Success
  or switching off resets both retry fields. Explicit `--session` bypasses
  the delay.
- `proposals.json`: `{"format": 3, "proposals": [{"id": "p-<n>", "kind":
  "supersedes"|"stale", "older", "newer", "reason", "quotes": [{"entry",
  "text"}], "state": "open"|"approved"|"rejected", "opened_at",
  "settled_at", "settled_by"}]}`. `newer`, `settled_at` and `settled_by`
  (a cite) may be `null`. Proposals are returned sorted by number.

Both are strict in the same way as frontmatter: format first, then no
unknown, missing or repeated key.

## Config

`config.toml` is Daniel's; scribe reads it and never writes it. Absent, or
without a table, it is the defaults. Every key is optional:

```toml
[tags.decisions]                 # a [tags] table replaces the default tags
aliases = ["decision", "dec"]
description = "choices Daniel has made"
stale_after_days = 180           # omit: entries never go stale

[models]
ask = "claude-sonnet-5-5"
session = "claude-haiku-5-5"
consistency = "claude-haiku-5-5"
referent = "claude-haiku-5-5"
credentials = ["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]

[catalog]
every_minutes = 20
every_turns = 10
window_messages = 40              # max messages per model call
window_chars = 15000              # max content characters per call

[redact]
env = []                         # more variables whose values are masked
```

The default tags are `decisions` (aliases `decision`, `dec`) and
`preferences` (aliases `preference`, `pref`; stale after 180 days). A tag
name must already be lowercase kebab-case, since it is a directory; aliases
are matched case-insensitively. A word that two tags claim, an unknown key,
a zero cadence/window bound or an invalid variable name is `config_invalid`, naming the
file and, where the parser knows it, the line.

`tag list` reports every configured tag and every directory under
`entries/` with its entry count; a directory no configured tag names is
listed with `configured: false`.

## Invariants

- **Reads write nothing.** Read-only commands take no lock, create no
  directory or audit line, and work on a read-only root.
- **Capture** (S2). `Store::capture` takes the drafts of one prompt, and
  under `.lock` finds an entry with the same `cite` session,
  `prompt_sha256` and `line` before writing; a match is returned as it is
  and nothing is written. A new entry gets the next number of its tag:
  one more than the largest number among the tag's parsed entries and the
  stems of its corrupt files, so a number is never reused while its file
  exists. The root and `entries/<tag>/` are created `0700` when absent and
  a new file is `0600`; writing over an existing entry file is refused.
- **Cite fill** (S2). `Store::has_unfilled_cites` reads only.
  `Store::fill_cite_messages` rewrites, under `.lock`, each entry of the
  session whose `cite.message` is `null` and for which the caller finds a
  message, setting `cite.message` and `updated`; it does nothing when the
  root does not exist.
- **Control** (S4). `Store::update_control` changes `control.json` under
  `.lock` and writes it only when the change reports one, so `/scribe on`
  twice, `off` for a session that is not on, and a `Stop` or `SessionEnd`
  for a session never switched on write nothing. A session never switched
  on has no key in `control.json`.
- **Session records** (S4). `Store::settle_pass` writes a session record
  (redacted, `sessions/<yyyy>/<mm>/` from its `started`, created `0700`)
  and settles its switch (`due` cleared, `final` too when the final pass
  ran; a `final` set while a non-final pass ran is kept) in one locked
  call. `Store::checkpoint_pass` writes intermediate windows with the same
  lock, redaction and atomic replacement, keeping `due` set. An explicit
  pass starting while not due publishes that obligation before its first
  intermediate checkpoint. A crash after the last record write but before
  settling leaves it due; the next pass sees nothing new and settles it.
  With no record `settle_pass` only settles. A failure is recorded through
  `update_control` under the lock. A pass never writes an entry.
- **Proposals** (S6). `Store::open_proposals` numbers new proposals
  `p-<max+1>` under `.lock`, skipping any whose pair or stale entry already
  has a proposal in any state or whose entries are no longer all `active`,
  redacts them, and replaces `proposals.json` only when one opened.
  `Store::settle` applies one `#scribe approve|reject|retire|forget` under
  `.lock`, writing `proposals.json` first and then each entry it changed,
  so a crash between them leaves an approved proposal whose older entry is
  still `active`; `Store::reapply_approved` (the scan's first step)
  finishes it. A forget blanks the entry's `said` and body and, in
  `proposals.json`, the `text` of its quotes and the `reason` of every
  proposal about it. Nothing is written when nothing changed.
- **Modes and owners.** Every load refuses the root, a subdirectory, a
  record file or a document owned by another user (`store_foreign_owner`) or
  writable by group or others (`store_permissions`, naming the `chmod 700`
  or `chmod 600` that fixes it). Nothing is repaired automatically. On
  non-unix platforms these checks are no-ops.
- **Links.** A subdirectory, record file, document, config or lock file that
  is a symbolic link is `store_symlink` and is not followed. The root itself
  may be a link.
- **Corrupt records.** A record file that does not parse is skipped by the
  entry and session lists and by `tag list`'s counts, and reported as a
  warning naming the file (stderr, and the JSON envelope's `warnings`);
  `show` of it fails with `store_corrupt`. A corrupt `control.json`,
  `proposals.json` or `config.toml` fails the command that reads it. Any
  other refusal (a link, loose modes, a newer format, an I/O error) fails
  the whole command rather than skipping the file
  ([4_decisions.md](../4_decisions.md#one-malformed-record-makes-its-file-corrupt)).
- **Format.** One `FORMAT` (3) covers every document. A shape change bumps
  it and appends one step to `UPGRADES` in `store/format.rs`, which is
  given the document type it upgrades; shipped steps never change. Reads
  upgrade an older document in memory; a newer one is refused and left
  untouched. The format-1-to-2 step adds zero failures and a null retry
  timestamp to control switches; other document shapes are unchanged. The
  format-2-to-3 step adds an empty `links` to each session record and an
  empty `pending_links` to each control switch; entries and proposals are
  unchanged. The old shape is validated strictly before upgrade, and reads
  never rewrite it.
- **Links** (§5.4). A link is written only by `/scribe on --from` or
  `/scribe link`, under the store lock: onto the child's record when it has
  one (no other field, `updated` included, changes), else onto its switch's
  `pending_links`. Every pass write merges the record's links on disk and
  the pending ones into the record it writes, so a link added while the
  model ran is never lost.
- **Writes** (S2). Every file except `audit.jsonl` is replaced whole by
  `fsio::write_atomic`: a temp file in the same directory, `fsync`, rename,
  `fsync` of the directory, under `.lock`. Lock acquisition gives up after
  10 seconds with `store_locked`, naming the holder.
- **Redaction** (S2). Before a write, `Redactor` replaces each live
  value of the variables in `[redact] env` and `[models] credentials` with
  `[redacted:<NAME>]`, and each high-confidence credential shape
  (`Authorization: Bearer …`, `ghp_`/`github_pat_`, `sk-ant-`/`sk-`,
  `AKIA…`, `xox[bp]-`, PEM private-key blocks) with a labelled marker, in
  every field of `REDACTED_FIELDS`: `entry.said`,
  `entry.content`, `session.title`, `session.summary`, `session.goals`,
  `session.outcome`, `session.next`, `session.terms.term`,
  `session.terms.said`, `proposal.reason` and
  `proposal.quotes.text`. Values shorter than 8 characters are not masked;
  identifiers, timestamps and paths are not free text. A `[redact] env`
  name with no value in the writer's environment refuses the write
  (`redact_env_unset`) before anything is stored; already-stored records
  are not rewritten.
- **Audit.** Lines are appended in one write under `.lock` and `fsync`ed;
  never rewritten. A line is `{"format": 1, "at", "command", "status",
  "target", "error_code", "duration_ms"}`: ids and codes, no argument values
  or error text. A line that cannot be written is a warning and never
  changes the command's result.

## Known gaps

- `session list` lists session records. A session switched on in
  `control.json` before its first pass has no record yet and is not listed;
  `/scribe status` in that session reports it as on with no record yet.
- `entry list --tag` matches a tag's name, not its aliases.
