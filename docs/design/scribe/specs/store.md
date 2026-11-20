---
type: design
summary: "Spec: store — layout, record files with YAML frontmatter (format 8: action entries' quote and transcript window, pickups.json; format 7: agent provenance, write metadata and the rebuilding author; session header and append-only log, active flag, authors), JSON documents, the tags store, config, private modes, links, format versioning, corrupt records, redaction, locking and the audit log"
last_validated: 2026-11-10
---

# Spec: Store

The store never exposes a torn file, never rewrites a file a newer build
wrote, never trusts state another user could have written, and never serves
a record under an id its file does not carry. The design is
[2_design.md §3](../2_design.md#3-storage); this spec is what the code does.
The code writes format 8 and reads formats 1–8. Format 8 adds an entry's
`quote` and `window` and the `pickups.json` document (action tags,
[2_design.md §4.6](../2_design.md#46-action-tags)). Format 7 adds `provider` to each
session control and `codex_cursors` to `control.json`; missing older values
upgrade in memory to `claude-code` and an empty cursor map. Format 6 adds the session
`rebuilt_by` field (ORB-24003); S12 is built;
other future-slice markers describe work outside this slice.

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
├── tags.json                            0600, optional (format 5, S12)
├── pickups.json                         0600, optional (format 8)
├── audit.jsonl                          0600, created by the first audited command
└── .lock                                0600, created by the first writer, never deleted
```

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`. A missing root, a
missing subdirectory or a missing optional file is empty, not an error.
Names starting with `.` are never records (a writer's temp file, an
editor's swap file, a rebuild checkpoint `.<id>.rebuild.md`), nor is a file without the `.md` extension; under
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
  (`preferences-12`; a lowercase kebab-case tag of at most 32 characters; at
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

Format 4 adds `author` (`{kind: daniel|agent, name}`; `name` lowercase
`[a-z0-9._-]`, 1–40 characters) and `updated_by` (an author or `null`);
`#scribe` capture writes `{kind: daniel, name: daniel}`. (S12) An agent entry (`author.kind: agent`) has `said: ""`,
`content_from: {kind: agent}` and `cite: null`; a Daniel entry always has a
cite. Any other combination is `store_corrupt`. Format 5 adds
`retirement_reason` (redacted text or `null`) and enables this agent
provenance; old entries upgrade with a null retirement reason.

Format 8 adds `quote` and `window`, both nullable and both upgraded to
`null`. `quote` is the human's correction an agent quoted under a tag
with `require_quote` (redacted text); a Daniel entry's is always `null`.
`window` is the transcript window a `#scribe` line under an action tag
keeps: `{turns: [{role: user|assistant, message, at, text, cut}],
truncated, omitted}`, where `message` is the turn's uuid or `null` for the
prompt itself, `text` is redacted, `cut` says the turn was shortened to its
cap, and `truncated`/`omitted` say whether and how many of the oldest turns
were dropped. An agent entry's `window` is always `null`. Any other
combination is `store_corrupt`.

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
`next_open`, and from format 4 `active` (bool), `keep_active` (bool),
`active_change` (`null` or `{at, by, route, reason, cite}`: `by` an author
of any kind, `route` `age`, `removed` or `manual`, `reason` at most 300
characters, `cite` a reference or `null`), `cataloged_by` (`null` or `{at,
by}`), `rebuilt_by` (`null` or `{kind, name}`: the agent whose `--author` ran
the `session rebuild` that wrote the record; from format 6) and `log`, a list
of segments `{n, heading, first_message,
last_message, from, to, state, windows, origin, topics, related}`: `n`
counts from 1 without gaps; `heading` at most 80 characters with no line
break; `first_message` and `last_message` message ids, `null` only when
`origin` is `upgrade`; `from` and `to` RFC 3339; `state` `open` or
`sealed`, and only the last segment may be `open` (none when `status` is
`final`); `windows` a positive integer; `origin` `pass`, `rebuild`,
`catalog` or `upgrade`; `topics` and `related` as for the record. The body
is exactly `## Summary`, `## Goals`, `## Next` and `## Log`, in that order;
under `## Log`, one `### <n>. <heading>` line per `log` item, in order and
equal to it, each followed by its text. Under `## Log`, text before the
first `### ` line, a heading that disagrees with `log`, a `### ` line
that starts no segment, or more or fewer segments than `log` items is
`store_corrupt`, naming the file; a segment's text is every other line up
to the next `### ` line. Header text is every line up to the next `##`
heading of the four; a format-4 record with `## Outcome` is
`store_corrupt`. The writer refuses (rather than writes) a header text
holding one of those heading lines or a segment text holding a `### `
line, so a written record always reads back the same.

A format 1–3 record's body is exactly the headings `## Summary`, `## Goals`,
`## Outcome` and `## Next`, in that order, each followed by its text; any
other line, another `##` heading included, is section text, and blank lines
around a section's text are dropped. Text before `## Summary` or a missing
heading is `store_corrupt`. It reads as format 4 (see **Format 4** below).

## JSON documents

- `control.json`: `{"format": 4, "sessions": {"<session-id>": {"on",
  "since", "due", "final", "consecutive_failures", "retry_after",
  "pending_links", "cataloged_by"}}}`. `pending_links` holds `{type, session}` links
  Daniel asserted before the session had a record; the pass that writes
  the record moves them onto it, then clears them, in one locked call.
  `since` and `retry_after` are RFC 3339 timestamps or `null`; the failure
  count is a saturating unsigned integer. A failed pass stays due; its next
  automatic attempt waits 5, 10, 20, 40, then at most 60 minutes. Success
  or switching off resets both retry fields. Explicit `--session` bypasses
  the delay.
- `proposals.json`: `{"format": 4, "proposals": [{"id": "p-<n>", "kind":
  "supersedes"|"stale", "older", "newer", "reason", "quotes": [{"entry",
  "text"}], "state": "open"|"approved"|"rejected", "opened_at",
  "settled_at", "settled_by"}]}`. `newer`, `settled_at` and `settled_by`
  (a cite) may be `null`. Proposals are returned sorted by number.
  Each proposal also has `author` (`{kind: scribe|agent|daniel, name}`;
  the consistency routine writes `{kind: scribe, name: consistency}`) and
  `settled_author` (an author or `null`; `#scribe approve|reject|retire|forget`
  sets Daniel's). (S12) An agent's settle leaves `settled_by` `null`. Format 5 adds
  `updated_by` (agent author or `null`) to preserve who last changed a
  proposal reason without replacing its original `author`.
- `cataloged_by` on a `control.json` switch is `null` or `{at, by}`;
  `/scribe on` keeps it. *(S14)* `session catalog` sets it and the pass
  copies it onto the record.
- (S12) `tags.json`: `{"format": 5, "tags": [{"name",
  "aliases", "description", "stale_after_days", "author", "updated_by", "created",
  "updated"}]}`, sorted by name; `name` and aliases as for config tags,
  `description` accepts 1–300 characters before redaction, `stale_after_days` a positive
  integer or `null`, `author` of kind `agent`; `updated_by` is the last updating agent or
  `null`. Absent is no store tags.
- (format 8) `pickups.json`: `{"format": 8, "pickups": [{"entry", "author",
  "at", "result", "rejected"}]}`, oldest first: a consumer's note that it
  picked up the action entry `entry` (`author` of kind `agent`, `result` a
  reference or `null`), or with `rejected: true`, that the human rejected
  its result. Written only by `entry pickup`, under the lock; an identical
  note is not written twice. Absent is no pickups. Entries are never
  edited by a consumer.

Both are strict in the same way as frontmatter: format first, then no
unknown, missing or repeated key.

## Config

`config.toml` is Daniel's; scribe reads it and never writes it. Absent, or
without a table, it is the defaults. Every key is optional:

```toml
[tags.rituals]                   # a [tags] table replaces the default tags
aliases = ["ritual"]
description = "how Daniel starts a day"
stale_after_days = 180           # omit: entries never go stale

[tags.idea]
action = true                    # an action tag: work items for a consumer
require_quote = false            # agents' entries must quote Daniel (action tags only)
ask_first = false                # agents ask before writing (action tags only)

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
segment_windows = 3               # windows before scribe seals an open segment

[redact]
env = []                         # more variables whose values are masked

[sessions]
inactive_after_days = 120        # days since a session's last message

# Accepted by the parser (S14). None by default.
[projects.orbit]
paths = ["codebases/orbit"]      # path fragments counted by transcript list
```

The default memory tags are `preferences` (aliases `preference`,
`pref`; stale after 180 days) and `finding`. An entry under a tag no config
or `tags.json` names (such as a `decisions` entry from before that tag
stopped shipping) stays listed, shown, searched, cited by `ask` and
settled. The
action tags `standard` (`require_quote`), `remind` and `idea` ship too,
with no aliases, and stay when a config has its own `[tags]`, unless it
claims their names; a config tag of the same name starts from the shipped
settings and overrides what it sets. `require_quote` or `ask_first` on a tag
without `action = true` is `config_invalid`. A tag
name must already be lowercase kebab-case, since it is a directory; aliases
are matched case-insensitively. A word that two tags claim, an unknown key,
a zero cadence/window bound or an invalid variable name is `config_invalid`, naming the
file and, where the parser knows it, the line. So is a zero
`segment_windows` or `inactive_after_days`, a project slug that is not
lowercase kebab-case, or an empty `paths`.

`tag list` reports every configured tag and every directory under
`entries/` with its entry count; a directory no configured tag names is
listed with `configured: false`. *(format 5, S12)* The tag table is the
config's tags merged with `tags.json`'s; config tags win
([2_design.md §3](../2_design.md#3-storage)), and a store tag's word a
config tag claims is dropped with a warning naming both.

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
  call. Before each window's write (a `checkpoint_pass` or the final
  `settle_pass`), the pass compares, under the same lock, the `updated`
  revision of the record it loaded with the record on disk; on a mismatch
  it refuses that write with `record_changed`, writes nothing, leaves the
  record on disk unchanged and keeps the session due, so the next run
  starts from the record on disk. A session with no record loaded expects
  no record on disk. A link keeps `updated`, so a record linked while the
  model ran still matches and its links are merged as before. Each window rewrites the header and open segment, appends up to
  three new segments, and derives message ranges and dates from the new
  segments' `starts_at` ids. A segment seals when another starts, after
  `segment_windows` windows, after repairing an overlong open text without
  a successor, or in the final pass's last window. The
  header budgets are title 120, summary 1200, goals 600 and next 1000;
  each segment has a 1–80-character heading and 1–900-character text.
  Refused answers get one retry naming the refusal. Remaining header/text
  budget overruns and misplaced starts can be repaired deterministically
  ([design §5.5](../2_design.md#55-the-log-segments-and-sealing)); all other
  shape rules must pass on the uncut answer, and the repaired answer is
  revalidated before redaction and writing. Unrepairable answers fail with
  `session_answer_invalid`. `active`, `keep_active`, `active_change` and
  `cataloged_by` are carried from the record on
  disk. `Store::checkpoint_pass` writes intermediate windows with the same
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
  open to group or others, any group or other mode bit read included
  (`store_permissions`, naming the `chmod 700` or `chmod 600` that fixes it).
  Nothing is repaired automatically. On non-unix platforms these checks are
  no-ops.
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
- **Format.** One `FORMAT` (5) covers every document. A shape change bumps
  it and appends one step to `UPGRADES` in `store/format.rs`, which is
  given the document type it upgrades; shipped steps never change. Reads
  upgrade an older document in memory; a newer one is refused and left
  untouched. The format-1-to-2 step adds zero failures and a null retry
  timestamp to control switches; other document shapes are unchanged. The
  format-2-to-3 step adds an empty `links` to each session record and an
  empty `pending_links` to each control switch; entries and proposals are
  unchanged. The old shape is validated strictly before upgrade, and reads
  never rewrite it.
- **Format 4**. The format-3-to-4 step converts in memory: a
  session record's `## Summary`, `## Goals` and `## Next` become the
  header, and one sealed segment `{n: 1, heading: <title>, first_message:
  null, last_message: <scribed_through>, from: <started>, to: <updated>,
  windows: <passes, at least 1>, origin: upgrade}` holds the summary, a blank line and
  the outcome; `active: true`, `keep_active: false`, `active_change: null`,
  `cataloged_by: null`. Entries gain `author: {kind: daniel, name:
  daniel}` and `updated_by: null`; proposals `author: {kind: scribe, name:
  consistency}` and `settled_author` (Daniel's for a settled one, else
  `null`); control switches `cataloged_by: null`. The step is the only
  place an upgraded segment may exceed 900 characters. The first write of
  an upgraded document now persists it at format 5; a build older than S8
  refuses format 4 (`store_too_new`).
- **Format 8**. The format-7-to-8 step adds `quote: null` and
  `window: null` to each entry; `pickups.json` is new and has no older
  form. Other document shapes are unchanged. An older build refuses format
  8 (`store_too_new`).
- **Format 6**. The format-5-to-6 step adds `rebuilt_by: null` to each session
  record. A rebuild sets it to its `--author` as it finishes (`finish_rebuild`
  writes the record it is given); a pass carries it forward unchanged, so it
  names the rebuild that wrote the record now on disk. Checkpoints are not
  visible and carry none. An older build refuses format 6 (`store_too_new`).
  `control.json` and `proposals.json` change no shape at format 6; a format-5
  file of either is checked against its current shape, as format 4 is.
- **The log is append-only** (S9). Every session write except `session
  rebuild` compares each sealed segment it writes with the one on disk;
  any difference in metadata or text refuses the write
  (`sealed_segment_changed`) and keeps the record. An ordinary pass first
  compares the `updated` revision it loaded with the one on disk under
  the lock; a mismatch refuses the write with `record_changed` before the
  sealed-segment check, so a pass that loaded its record before a rebuild
  finished cannot overwrite the rebuilt record. A rebuild writes its
  windows to `.<id>.rebuild.md` beside the record and replaces the record
  under the lock only if the record's `updated` is unchanged since the
  rebuild began. Checkpoints use the same format-4 session shape; their
  `updated` pins that visible revision rather than the checkpoint write time.
  No new persisted field or store version is needed. Each rebuilt segment
  has `origin: rebuild`. A per-session `.<id>.rebuild.lock` serializes
  rebuilders without holding the store lock over model calls. The lock file
  remains after release; ordinary reads skip it. A changed visible revision
  causes rebuild to read the transcript again and fold in newer messages
  before retrying replacement. A successful replacement removes its
  checkpoint. It records `rebuilt_by` (format 6) with the `--author` that ran it. Identity, creation time, links and activity/catalog fields
  come from the latest visible record; the control switch stays unchanged.
  `--confirm` is mandatory, missing transcripts refuse `transcript_missing`,
  and on-and-due sessions refuse `session_busy`. Sealed segments are checked
  before redaction and retained verbatim, so a later redaction setting does
  not rewrite already sealed history.
- **Activity** *(S10)*. `Store::set_active` changes `active` or
  `keep_active` and `active_change` under the lock, and nothing else on
  the record (`updated` included); it writes only when something changed.
  `active_change.reason` and `cite` pass the redactor. The consistency
  scan's deactivations go through `Store::set_active_many`, one lock for
  the batch, each checked again against the record on disk: a session
  pinned or already inactive by then is left as it is, and a missing or
  unreadable record fails only its own request so the remaining requests
  proceed and the scan reports the failure. A pinned session
  (`keep_active: true`) is never deactivated by `age` or `removed`; a
  `manual` deactivation clears `keep_active` too. Age counts from the
  last segment's `to`, or for a record with no segment from the later of
  `started` and `on_since`.
- **Format 5** (S12). Append-only step 4→5 adds null entry
  `retirement_reason` and proposal `updated_by`. Existing citations and
  authors are preserved. New agent notes have a null cite and
  `content_from: {kind: agent}`. Agent tags are a new optional document.
  A format-4 reader refuses format-5 records rather than misreading them.
- **Agent writes** (S12). Each write verb takes the lock, checks the
  author guards (a reserved `--author` name — `daniel`, `scribe`,
  `consistency` — is refused `reserved_author`; a Daniel entry's content,
  retirement and the settling of a proposal whose `older` is a Daniel
  entry are refused `daniel_only`; author names between agents are
  attribution only, so any agent may update or retire an active agent
  entry, update an open agent proposal or settle one; a changed `--reason` when settling a
  proposal Daniel or scribe opened is refused `proposal_not_agent`),
  redacts, and writes the entry, `proposals.json` or `tags.json` through
  `write_atomic`. `entry add` numbers like capture. An agent settle uses the
  same two-write order as `#scribe approve`; both and recovery call the
  same entry-effect function. Recovery preserves an agent settler in
  `updated_by`. Entry content updates retain the original author and set
  `updated_by`; metadata-only updates of Daniel entries do likewise.
  Direct retirement stores its redacted reason. Proposal reasons accept
  1–300 characters before redaction; direct retirement uses the same bound.
  Repeating an unchanged update or settlement prints the unchanged record
  and `nothing changed` on stderr, without an audit event (the lock holder
  diagnostic may change). Config/store collisions are resolved in memory:
  `tag list` warns; it never rewrites either file.
- **Transcripts** *(S14, built)*. `transcript list` and `transcript show`
  read Claude Code transcripts through the hooks' provenance check and
  write nothing, take no lock and create nothing. `[projects.<slug>] paths`
  is parsed: a kebab-case slug and at least one non-empty path. `session
  catalog` sets `cataloged_by` on the switch and copies it onto the record.
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
  `session.next`, `session.terms.term`, `session.terms.said`,
  `session.log.heading`, `session.log.text`,
  `session.active_change.reason`, `proposal.reason` and
  `proposal.quotes.text`; (S12) also `tag.description` and
  `entry.retirement_reason`; (format 8) also `entry.quote` and
  `entry.window.text`. A masked segment
  heading is cut back to 80 characters. Values shorter than 8 characters are not masked;
  identifiers, timestamps and paths are not free text. A `[redact] env`
  name with no value in the writer's environment refuses the write
  (`redact_env_unset`) before anything is stored; already-stored records
  are not rewritten.
- **Audit.** Lines are appended in one write under `.lock` and `fsync`ed;
  never rewritten. A line is `{"format": 2, "at", "command", "status",
  "target", "error_code", "author", "duration_ms"}`: ids and codes, no argument
  values or error text. `target` is the resolved id of the record a write names,
  on refusal as well as success (`null` when it does not resolve). `author` is
  the `--author` name of a command that takes one, on every line it writes
  (STD-05 R2: a self-reported identity, kept in its own field); `null`
  otherwise. A format-1 line, with no `author`, still reads. A line that cannot be written is a warning and never
  changes the command's result.

## Known gaps

- `session list` lists session records. A session switched on in
  `control.json` before its first pass has no record yet and is not listed;
  `/scribe status` in that session reports it as on with no record yet.
- `entry list --tag` matches a tag's name, not its aliases.

## Codex ingestion state (format 7)

`control.json` has `codex_cursors: {"<session-id>": "<host-message-id>"}`.
Each session switch also stores `provider: claude-code|codex`. Provider stamping
and switch mutation share one locked atomic control write. The Codex ingestion
lock `.codex-<session-id>.lock` serializes lifecycle events independently of the
store lock; actual records and cursors still use the store lock and atomic writes.
The cursor advances only after effects, so an interruption can replay an
idempotent capture/settlement/control; it cannot silently lose a pending write.
Only ids are queued, never raw prompt text. A missing cursor in a rewritten
transcript refuses replay. Bootstrap starts in the current verified hook turn,
not at the beginning of a historical transcript. An earlier typed opt-in alone
does not activate a newly installed hook in a different turn.

Config adds `[codex.models]` (all uses default `gpt-6-luna`, transport `codex`).
Both model tables accept an explicit `provider` and per-use ids. Existing
`[models]` defaults and persisted entry/session shapes remain compatible.
