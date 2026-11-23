---
type: design
summary: "Spec: store — layout, scribe.db (phase 4: messages with FTS5, WAL, read-only copies; phase 5: the memory journal and pass signals; phase 6: sessions, switches, Codex cursors and rebuild checkpoints as rows, s-<n> ids, store migrate and export), memory files (profile, now items, milestones), record files with YAML frontmatter (format 30: withdrawn claims and deleted source markers; format 25: memory files migrated from memory items; format 24: a claim's context paragraph; format 23: the user author channel; format 22: marks.json and the annotation entry; format 21: a working item's preference_proposal; format 20: memory consolidation and promotion; format 19: memory items; format 18: entry removal_instruction apart from the approval instruction; format 17: issue records; format 16: entry outcome on a pass-written idea; format 15: instructed approval said and closed_reason covered; format 14: candidate statement and subject, entry stated_by; format 13: entry approved_by and instruction, proposal instruction, closed_reason instructed; format 12: entry inactive_since; format 11: proposal admitted and closed_reason; format 10: entry evidence and as_of, candidate role; format 9: proposal candidate; format 8: action entries' quote and transcript window, pickups.json; format 7: agent provenance, write metadata and the rebuilding author; session header and append-only log, active flag, authors), JSON documents, the tags store, config, private modes, links, format versioning, corrupt records, redaction, locking and the audit log"
last_validated: 2026-11-22
---

# Spec: Store

The store never exposes a torn file, never rewrites a file a newer build
wrote, never trusts state another user could have written, and never serves
a record under an id its file does not carry. The design is
[2_design.md §3](../2_design.md#3-storage); this spec is what the code does.
The code writes format 30 and reads formats 1–30. Format 30 permits withdrawn claims, deleted citations and legacy memory items retired with their sources erased. Format 29 adds groups and seats in SQLite schema 9. SQLite schema 11 adds group turn capture and closing summaries. SQLite schema 12 adds optional `groups.slug` and a unique partial index for open slugs. Format 28 adds an optional `handoff_mode` (`continue` or `new`) to entries; SQLite schema 7 adds its nullable column, and older databases read it as null until the next write migrates them. SQLite schema 8 removes stale rebuild checkpoints for forgotten sessions; export omits them before that migration runs. Schema 10 records completion of the required WAL cleanup, retrying after a busy reader, a busy writer, or interruption, including on stores already at schema 9. Format 26 moves the
sessions into `scribe.db`
([2_design.md §15.7](../2_design.md#157-storage), phase 6): records,
switches, Codex cursors and rebuild checkpoints become rows (schema version
3); the first write that touches a session moves the session files to
`pre-sqlite-<date>/` (a write that touches no session leaves them in files),
and `control.json` stays as the format stamp alone; see
[Sessions in scribe.db](#sessions-in-scribedb-phase-6). Format 27 (phase 7)
introduced moving the other kinds into `scribe.db`; the format-27 release
wrote the format stamps, including `control.json`, at 27. The current
format-30 build retains that migration and stamps at 30 after its first
eligible store write transaction moves every kind still in files, sessions
first. A standalone audit line does not start that move: it is inserted
without waiting, or spooled under `audit-spool/` while the store is still in
files or another writer holds the database, and the next eligible
transaction drains the spool; see
[Documents in scribe.db](#documents-in-scribedb-phase-7). Format 25
changes no stored document: it adds the memory files of
[2_design.md §15.5](../2_design.md#155-memory-one-fixed-shape) (`memory/profile.md`,
`memory/now/<stem>.md`, `memory/milestones/<stem>.md`), and the first
memory write migrates every `memory/m-<n>.md` item into them and removes it
(§15.11). A memory write stamps `control.json` at the format it writes (25
when these files arrived), so a format-24 build refuses the store rather
than reading it without its memory. Format 24 adds
`context` to each proposal candidate
([2_design.md §15.3](../2_design.md#153-claims-with-context)): scribe's
paragraph of two to four sentences, at most 600 characters before
redaction, saying what was being discussed and what the turn settles, so a
claim is settled without opening its session. A new claim carries it in
place of the one-line `statement`, which stays readable and is no longer
written; a format-23 candidate reads with `context: null` and, when it has
a statement, is an older claim. Approval writes the context as the entry's
content, the turn's words as `quote` and `stated_by: {kind: scribe, name:
session-pass}`, for an assistant claim too (author `{kind: agent, name:
assistant}`), which a format-23 build refuses. A collected entry and an
annotation take their content from the context the same way. Format 23 renames the
user's author channel ([2_design.md §15.8](../2_design.md#158-generic-and-configurable)):
the value stored before it, `{kind: daniel, name: daniel}`, reads as
`{kind: user, name: user}` in every document (entries, sessions,
`control.json`, `proposals.json`, issues, `marks.json`, `pickups.json` and
`audit.jsonl`), and the next write persists `user` at format 23, which a
format-22 build refuses as too new. `user` is a reserved `--author` name,
and `daniel` stays one. A tag's `[tags.<name>.capture] from` is `user`
(phase 2: or `assistant` or `both`, specs/catchers.md); `daniel` is still
read as `user`. Format 22 changes no
stored value: it adds `marks.json`, each `#scribe` annotation marker's
outcome, and admits an entry with `stated_by: {kind: scribe, name:
annotation}`, scribe's statement of a turn Daniel marked
([2_design.md §14](../2_design.md#14-scribe-as-an-annotation)); such an
entry is written at format 22, so a format-21 build refuses it as too new.
Format 21 adds to each
memory item `preference_proposal`: the `preferences` candidate a `working`
item opened when it recurred across sessions
([2_design.md §13.7](../2_design.md#137-from-working-to-preferences));
a memory write rewrites `control.json` at format 21, so a format-20 build
refuses the store. Format 20 adds to each
memory item the consolidation and promotion keys (`promoted_from`,
`promotion`, `merged_into`, `superseded_by`, `unrelated_to`) and the
statuses `merged`, `superseded` and `promoted`
([2_design.md §13.4, §13.5](../2_design.md#135-consolidation-the-weekly-consistency-routine));
a memory write rewrites `control.json` at format 20, so a format-19 build
refuses the store instead of misreading it. Format 19 adds the
`memory/` directory of memory items, the things the session pass notices
about Daniel in switched-on sessions
([2_design.md §13](../2_design.md#13-memory-profile-projects-working-now-and-milestones-proposed)); a memory write rewrites
`control.json` at format 19, so an older build refuses the store instead of
reading it without its memory. Format 18 adds
`removal_instruction` to each entry: the checked instruction that retired or
forgot it, kept apart from `instruction`, which is now only the instruction
an agent approved the entry on
([2_design.md §12.7](../2_design.md#127-store-format)). Format 17 changes no
stored value: it adds the issue record kind, `issues/<id>.md`, which has no
older form ([2_design.md §11.4](../2_design.md#114-issues); **Issue files**
below). Format 16 adds an entry's `outcome`, set only on an idea the session
pass wrote itself
([2_design.md §11.6](../2_design.md#116-idea-capture-after-finding-is-gone)).
Format 15 changes no
stored value: it admits an entry approved on Daniel's checked instruction,
whose `said` is the `scribe proposal settle` command the agent ran, and
`"covered"` in `closed_reason`
([2_design.md §12.7](../2_design.md#127-store-format)). Format 14 adds
`statement` and `subject` to each proposal candidate and `stated_by` to each
entry ([2_design.md §11.1](../2_design.md#111-a-claim-carries-a-statement)).
Format 13 adds
`approved_by` and `instruction` to each entry, adds `instruction` to each
proposal, and adds `"instructed"` to `closed_reason`
([2_design.md §12.7](../2_design.md#127-store-format)). Format 12 adds an
entry's `inactive_since`. Format 11 adds `admitted`
on a proposal candidate and `closed_reason` on a proposal. Format 10 adds an
entry's `evidence` and `as_of` and a candidate's `role`. Format 9 adds a
proposal's `candidate`. Format 8 adds an entry's `quote` and `window` and the
`pickups.json` document (action tags,
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
├── entries/<tag>/<tag>-<n>.md           0600, one file per entry, until the move (formats 1–26)
├── issues/issue-<n>.md                  0600, one file per issue (formats 17–23; phase 2 converts each to an entry)
├── pre-issue-<yyyy-mm-dd>/issue-<n>.md   0600, an issue file after its conversion (phase 2)
├── sessions/<yyyy>/<mm>/<session-id>.md 0600, one file per session record, until the move (formats 1–25)
├── control.json                         0600, optional; after the move only the format stamp, `{"format": 28, "sessions": {}, "codex_cursors": {}}`
├── pre-sqlite-<yyyy-mm-dd>/             0700, the files the move took in (`sessions/`, `entries/`, the JSON documents, `audit.jsonl`, `.codex-<id>.lock`), kept except when a forget erases a session or an entry from them
├── locks/codex-<id>.lock                0600, the Codex ingestion lock of a session (phase 6)
├── locks/rebuild-<id>.lock              0600, the rebuild lock of a session (phase 6)
├── locks/archive-forget.lock            0600, serializes `pre-sqlite-*` rewrites during entry and session forget
├── proposals.json                       0600, optional until the move (formats 1–26); after it an empty format stamp
├── tags.json                            0600, optional (introduced at format 5, S12); after the move an empty format stamp
├── pickups.json                         0600, optional (introduced at format 8); after the move an empty format stamp
├── memory/profile.md                     0600, the profile, rebuilt by `memory refresh` (format 25)
├── memory/now/<date>-<slug>.md          0600, one file per now item (format 25)
├── memory/milestones/<date>-<slug>.md   0600, one file per milestone (format 25)
├── memory/m-<n>.md                      0600, a memory item (formats 19–24), migrated and removed by the first memory write
├── marks.json                           0600, optional (introduced at format 22); after the move an empty format stamp
├── scribe.db                            0600, SQLite in WAL mode, created by the first write that needs it (phase 4)
├── scribe.db-wal, scribe.db-shm         0600, kept after the last connection (PERSIST_WAL)
├── audit-spool/<nanos>-<pid>-<n>.json   0600, one audit line a busy database could not take yet (phase 7), drained by the next write
├── audit.jsonl                          0600, the audit log until the move (formats 1–26)
└── .lock                                0600, the store lock before phase 7; an existing file stays at the root (the move neither archives nor removes it)
```

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`. A missing root, a
missing subdirectory or a missing optional file is empty, not an error.
Names starting with `.` are never records (a writer's temp file, an
editor's swap file, a rebuild checkpoint `.<id>.rebuild.md`), nor is a file without the `.md` extension; under
`sessions/`, only four-digit year and two-digit month directories are
walked.

## scribe.db (phase 4)

The SQLite database of [2_design.md §15.7](../2_design.md#157-storage),
built with `rusqlite`'s `bundled` feature (SQLite compiled in, FTS5 on).
Only `store/db.rs` and `store/db/` open it. It holds the session pass's
messages (phase 4) and memory's rows (phase 5): the pending `now`
candidates, removed items' history, the profile revisions and cursor, the
memory journal and each pass window's signal; the sessions (phase 6); and
entries, claims, agent tags, marks, pickups and the audit log (phase 7).
Only `config.toml` and `memory/`'s files remain as primary data files outside the database.

- **Privacy.** `scribe.db`, `scribe.db-wal` and `scribe.db-shm` are checked
  like any store file on every open, read or write: a link, another user's
  file or any group or other mode bit is refused (`store_symlink`,
  `store_foreign_owner`, `store_permissions`). The database is created
  `0600` before SQLite opens it, and SQLite gives the `-wal` and `-shm`
  files the database's mode.
- **Writers** open read-write and set `busy_timeout` to the lock wait
  (10 seconds), `SQLITE_FCNTL_PERSIST_WAL`, `journal_mode = WAL`,
  `synchronous = FULL` and `secure_delete = ON`. SQLite does not call the
  busy handler for the journal-mode change, so a writer racing another on
  a new database retries it every 20 ms within the same wait. Every write
  is one `BEGIN IMMEDIATE` transaction; a busy database past the wait is
  `store_locked` (holder `another scribe.db writer`). Writers do not take
  `.lock` (phase 7 retires the lock protocol).
- **Readers** take no lock and write nothing: they open
  `SQLITE_OPEN_READ_ONLY`. A missing database matches nothing. When SQLite
  cannot open the WAL read-only (`SQLITE_CANTOPEN` or `SQLITE_READONLY`:
  the `-wal` or `-shm` file is missing and the directory is not writable,
  as in a copied or mounted store), the reader opens `file:<path>?immutable=1`
  and the output says `read-only copy`.
- **Schema.** `PRAGMA user_version` is the schema version; migrations are a
  numbered list, each run in its own `BEGIN IMMEDIATE` transaction that
  re-reads the version, so racing writers run each step once. A version
  above this build's is `store_too_new` for readers and writers. Version 1
  is phase 4's; version 2 (phase 5) adds `memory_journal`,
  `memory_messages` and `pass_signals`; version 3 (phase 6) adds
  `sessions`, `segments`, `session_links`, `session_terms`,
  `codex_cursors`, `rebuild_checkpoints` and `record_kinds`; version 4 adds
  `sessions.forgotten`; version 5 (phase 7) adds `entries`, `claims`,
  `agent_tags`, `marks`, `pickups` and `audit`, and rebuilds `record_kinds`
  to accept their kinds; version 6 adds `pending_archive_forgets` for
  durable post-commit replay of entry-forget archive erasure; version 7
  adds the nullable `entries.handoff_mode` column; version 8 deletes
  `rebuild_checkpoints` rows whose session is forgotten (a
  checkpoint an older build could write after the forget) and adds no
  table. `scribe export --as jsonl` leaves those rows out even before that
  migration runs. Version 9 adds the group, seat and membership tables. Before advancing past versions 8 or 9, a writer truncates the WAL; if another reader or writer connection keeps the checkpoint from completing, the write fails and leaves the version available for retry. Version 10 records that cleanup without changing tables. Version 11 adds summary, membership capture time and cursor, and group-only message provenance. Because old memberships have no join time, their capture starts at migration. Messages that already exist retain their independent session-pass provenance. Version 12 adds the nullable `groups.slug` column and the unique partial index `group_open_slug` on `groups(slug) WHERE slug IS NOT NULL AND closed IS NULL`. Version 13 adds `deferred_ancestry` for pending first-prompt ancestry checks when a session starts before transcript creation. Version 14 permits the `withdrawn` reason on claims and reserves deleted handoff ids. Every application table is `STRICT`; the one virtual table, `messages_fts`, is an FTS5 index (`CREATE VIRTUAL TABLE`), and a virtual table takes no `STRICT` option. Times are RFC 3339 UTC text.
- **Errors.** Contention is `store_locked`: a write that waits past the lock
  wait (`SQLITE_BUSY` or `SQLITE_LOCKED`) fails with holder
  `another scribe.db writer`, and a schema 8 or 9 WAL cleanup blocked by an
  active reader or writer fails with holder `another scribe.db connection`,
  leaving the version for retry. A file SQLite cannot read as a database, or one it
  reports as corrupt (`SQLITE_NOTADB` or `SQLITE_CORRUPT`), is
  `store_corrupt`, naming the file. Any other SQLite failure is
  `store_database`; readers set no busy timeout, so a read SQLite reports
  busy is one too.

Schema version 15 adds the local Claude group capture checkpoint and
first-seen message index. Existing membership cursors carry forward and seed
the checkpoint on the next transcript replay that finds them. Both tables
are derived from the checked local transcript and are omitted from exports.

### Tables (version 15)

| Table | Columns (type, constraint) | Written by |
|---|---|---|
| `meta` | `id INTEGER PRIMARY KEY CHECK (id = 1)`; `store_id TEXT NOT NULL`, 32 hex characters; `created TEXT NOT NULL` | the first migration; one row |
| `groups` | `id INTEGER PRIMARY KEY AUTOINCREMENT` (shown as `g-<n>`), `title` and `kind` nonblank redacted text, `project` slug, `opened` timestamp, nullable `closed` timestamp and redacted `summary`, `opener` provider session id, `related` JSON array of task, PR and repository references, nullable `slug TEXT` with unique partial index `group_open_slug` on `slug WHERE slug IS NOT NULL AND closed IS NULL` (version 12) | `group open` on a verified typed quote, `/scribe group <slug>` (or Codex `$scribe group <slug>`), `group close` by the opener or the newest transcript of the opener's seat, or with `--quote` by a session whose verified typed turn names the group |
| `group_seats` | `group_id INTEGER` referencing the group, `name TEXT`, `role TEXT` (redacted free label), primary key `(group_id, name)` | `group join` creates the seat, `group leave` removes an empty seat |
| `group_members` | `position INTEGER PRIMARY KEY AUTOINCREMENT`, foreign key `(group_id, seat)` to the seat, `session TEXT UNIQUE` (provider id), `author TEXT` (joining agent), `joined` (capture start), nullable `turn_cursor` (provider message id). Index on `(group_id, seat, position)` orders the chain. | `group join` or clear inheritance appends, capture advances the cursor, `group leave` removes the caller’s row |
| `group_claude_progress` | `position INTEGER PRIMARY KEY` referencing a membership, `file_id TEXT` (device and inode), `offset INTEGER` (next byte), `modified TEXT` (file mtime) and `boundary TEXT` (digest of bounded head and tail samples). | Claude group capture, with its message and cursor updates |
| `group_claude_seen` | Primary key `(position, id_hash)` referencing the checkpoint by position. `id_hash TEXT` is the hashed provider message id, `permitted INTEGER` the nullable first origin and entry-point verdict, and `message INTEGER` records whether the parser saw a message for that id. | Claude group capture advances the index with its checkpoint. Replaying a transcript replaces the index, and deleting a membership cascades to both tables. |
| `messages` | `id INTEGER PRIMARY KEY`; `session TEXT NOT NULL`, 1–128 characters (the session record's id); `message TEXT NOT NULL`, non-empty (the provider's message id); `role TEXT NOT NULL CHECK (role IN ('user', 'assistant'))`; `at TEXT NOT NULL`; `text TEXT NOT NULL`, redacted; `sha256 TEXT NOT NULL`, 64 hex characters of the redacted text; `group_only INTEGER NOT NULL DEFAULT 0` (0 or 1); `UNIQUE (session, message)` | the session pass and group hooks |
| `messages_fts` | FTS5 over `messages.text`, external content (`content = 'messages'`, `content_rowid = 'id'`), `tokenize = 'unicode61 remove_diacritics 2'`; kept by `AFTER INSERT`, `AFTER DELETE` and `AFTER UPDATE OF text` triggers | the triggers |
| `now_pending` | `id INTEGER PRIMARY KEY`; `session TEXT NOT NULL`, 1–128 characters; `message TEXT NOT NULL`, non-empty; `subject TEXT NOT NULL`, non-empty; `text TEXT NOT NULL`; `created TEXT NOT NULL`; `UNIQUE (session, subject)`; a `BEFORE INSERT` trigger aborts a sixth row for one session | the session pass; dropped by the session's final pass or when a dated element completes one |
| `memory_history` | `id INTEGER PRIMARY KEY`; `item TEXT NOT NULL`, non-empty (the removed item's id or file stem); `kind TEXT NOT NULL`, non-empty; `text TEXT NOT NULL`; `sources TEXT NOT NULL`, a JSON array; `removed_at TEXT NOT NULL` | the migration, `memory refresh` (a passed `now` item it does not promote); `memory forget` deletes an item's rows |
| `profile_revisions` | `n INTEGER PRIMARY KEY AUTOINCREMENT` (the revision); `created TEXT NOT NULL`; `content TEXT NOT NULL`; `file_sha256 TEXT`, `NULL` until the journal closes, else 64 hex characters | `memory refresh` and `memory restore` |
| `profile_cursor` | `id INTEGER PRIMARY KEY CHECK (id = 1)`; `through_at TEXT NOT NULL`; `through_session TEXT NOT NULL`, 1–128 characters | `memory refresh`; one row |
| `memory_journal` | `id INTEGER PRIMARY KEY`; `created TEXT NOT NULL`; `ops TEXT NOT NULL`, a JSON array of `{path, content}` (content `null` removes the file); `done INTEGER NOT NULL CHECK (done IN (0, 1))`; `revision INTEGER REFERENCES profile_revisions(n)`; a partial index on the open rows | every memory write (version 2) |
| `memory_messages` | `item TEXT NOT NULL`, non-empty (a `now` or milestone stem, or a migrated `m-<n>`); `session TEXT NOT NULL`, 1–128 characters; `message TEXT NOT NULL`, non-empty; `PRIMARY KEY (item, session, message)` | the session pass (the turn an element quotes, and a completed candidate's turn), the migration (each occurrence's turn), `memory refresh` (a promoted `now` item's links copied to its milestone); `memory forget` deletes an item's rows (version 2) |
| `pass_signals` | `session TEXT NOT NULL`, 1–128 characters; `through TEXT NOT NULL`, non-empty (the window's last message); `signal INTEGER NOT NULL CHECK (signal BETWEEN 0 AND 3)`; `PRIMARY KEY (session, through)` | the session pass (version 2) |
| `sessions` | `id INTEGER PRIMARY KEY AUTOINCREMENT`, shown as `s-<id>`; `provider_session TEXT NOT NULL UNIQUE`, 1–128 characters (the provider's session id, the key cites and messages use); `has_record` and `has_switch`, each `0` or `1`; the record's header as columns (`status` `open` or `final`, `active`, `keep_active`, `project`, `provider`, `host`, `created`, `updated`, `title`, `cwd`, `started`, `on_since`, `scribed_through`, `passes`, `next_open`; `active_change`, `cataloged_by`, `rebuilt_by`, `topics` and `related` as JSON text) and its `summary`, `goals` and `next`; the switch as `sw_provider`, `sw_on`, `sw_since`, `sw_due`, `sw_final`, `sw_consecutive_failures`, `sw_retry_after`, `sw_pending_links` (JSON) and `sw_cataloged_by` (JSON); `forgotten TEXT`, when `#scribe forget` erased the session, else `NULL` (version 4). A row with `has_record = 1` has every header column; one with `has_switch = 1` every switch column the switch requires | every session write (version 3) |
| `segments` | `session INTEGER NOT NULL` (`sessions.id`); `n INTEGER NOT NULL CHECK (n >= 1)`; `heading`, `first_message`, `last_message`, `"from"`, `"to"`, `state` (`open` or `sealed`), `windows`, `origin`, `topics` and `related` (JSON), `text`; `PRIMARY KEY (session, n)` | the record's writer, replacing the session's rows (version 3) |
| `session_links` | `session INTEGER NOT NULL`; `position INTEGER NOT NULL`; `type TEXT NOT NULL CHECK (type IN ('spawned_from', 'continues'))`; `target TEXT NOT NULL` (a provider session id); `PRIMARY KEY (session, position)` | the record's writer (version 3) |
| `session_terms` | `session INTEGER NOT NULL`; `position INTEGER NOT NULL`; `term`, `said`, `message`; `PRIMARY KEY (session, position)` | the record's writer (version 3) |
| `codex_cursors` | `session TEXT PRIMARY KEY`, 1–128 characters; `message TEXT NOT NULL` | Codex ingestion (version 3) |
| `rebuild_checkpoints` | `session INTEGER NOT NULL PRIMARY KEY` (`sessions.id`); `record TEXT NOT NULL`, the checkpoint as a record file's text; `created TEXT NOT NULL` | `session rebuild`; dropped in the transaction that writes the rebuilt record, and by `#scribe forget` (version 3). A forgotten session keeps none: export omits a row an older build left, and version 8 deletes it. A checkpoint of a session that was not forgotten stays |
| `record_kinds` | `kind TEXT PRIMARY KEY CHECK (kind IN ('sessions', 'entries', 'claims', 'agent_tags', 'marks', 'pickups', 'audit'))`; `moved_at TEXT NOT NULL`; `moved_to TEXT`, the `pre-sqlite-<date>` directory the kind's files moved to, `NULL` when that kind had no files | the move; a row says that kind is rows (version 3; the kinds past `sessions` version 5) |
| `entries` | `id TEXT PRIMARY KEY` (`<tag>-<n>`); `tag TEXT NOT NULL`; `n INTEGER NOT NULL`, `UNIQUE (tag, n)`; every frontmatter field of an entry as a column (`"window"` quoted; lists, cites, authors and other objects as JSON text); `content TEXT NOT NULL`, the body | every entry write (version 5) |
| `claims` | `n INTEGER PRIMARY KEY` (`p-<n>`); every field of a proposal as a column, objects as JSON text; `has_candidate` `0` or `1` and the candidate's fields as `c_<field>` columns | every claim write: only the claims a write changed (version 5) |
| `agent_tags` | `name TEXT PRIMARY KEY`; the fields `tags.json` held | `tag add`, `tag update` (version 5) |
| `marks` | `position INTEGER PRIMARY KEY`; the fields of one `marks.json` record | the `Stop` hook's marker outcomes (version 5) |
| `pickups` | `position INTEGER PRIMARY KEY`; the fields of one `pickups.json` record | `entry pickup` (version 5) |
| `audit` | `id INTEGER PRIMARY KEY`; `format INTEGER NOT NULL`, the line's own format (1–5); the event's fields (`issue` as JSON text, `NULL` for none); `spool TEXT UNIQUE`, the spool file a line was drained from or the `audit.jsonl:<offset>:<sha256>` key of an imported log line, else `NULL` | every audited command; the move; the spool drain (version 5) |
| `deleted_handoffs` | `id TEXT PRIMARY KEY NOT NULL` | session forget reserves erased handoff ids (version 14) |
| `pending_archive_forgets` | `id INTEGER PRIMARY KEY`; `entry TEXT NOT NULL` (`<tag>-<n>`); `salt TEXT NOT NULL`, 64 hex characters; `digests TEXT NOT NULL`, JSON array of `{len, sha256}` salted SHA-256 digests of erased texts (never cleartext); `created TEXT NOT NULL` | committed in the entry forget's transaction when `pre-sqlite-*` archives exist or are pending move-aside; drained after commit by that write or any restarted write (version 6) |

Group turns use the same `messages` and FTS5 rows, with `group_only = 1` for
turns kept solely through membership. A session pass that writes the same
message clears that flag. Capture rechecks open membership, that the membership
is still its seat's newest position (a replaced member holds none), and the
cursor in the message write transaction. Capture skips replay if the transcript no
longer contains its cursor. A header-only `sessions` row gives an uncataloged
member's messages a forget target. A profile refresh reads only turns with
`group_only = 0`, so no derived profile holds text kept solely on group consent.

Leaving removes the membership, its group-only turns and any derived summary
in one transaction. Session forget erases the session's turns and invalidates
its group's summary. Deleting a stored turn or forgetting a handoff also
invalidates the affected summary. Affected writes use the normal FTS cleanup
and WAL checkpoint after deleting text.

Close takes a per-group lock, reads the snapshot, then makes one bounded model
call outside the SQLite write transaction. The group lock precedes the SQLite
write lock on every close. It checks the input again before
committing `closed` and `summary` together. A model error or changed evidence
leaves the group open for retry.

### Messages

- **What is kept.** For each pass window, the user's turns with their own
  typed words (pasted blocks out, as today), less fenced code (a line
  opening with three or more backticks or tildes, through its closing
  line or the end); a turn left empty is not kept. The only assistant
  turns kept are those a claim or collected entry of the window cites,
  whole. Every text passes the `Redactor` before it is written.
- **Not kept.** A turn holding one of the memory gate's finance words or
  sensitive-event words (loss, health, relationship; `memory::finance_word`,
  `memory::sensitive_word`). A memory item may not hold them outside `now`
  and `milestones`, and the profile job is the reader these messages are
  for. An off session receives no catalog pass; an open group membership
  can still capture eligible typed turns from the time it joins, without a
  session record or switch.
- **When.** The pass upserts a window's messages on (`session`,
  `message`) in the transaction that writes the window's record (phase 6;
  before it, the messages committed first and the record file followed).
  The record's `scribed_through` stays the cursor, and a rerun window
  writes the same rows (a row changes only when its text, role or time
  does).
- **Forget.** `#scribe forget <entry-id>` (and `entry forget`) deletes the
  messages the entry and its candidate proposals cite; `memory forget`
  deletes those its item cites (`memory_messages`: the turn each element
  quoted, a completed candidate's turn, a migrated item's occurrences),
  in the same transaction as the item's history and links. The rows go before
  the files are written; `secure_delete` overwrites their pages, an FTS5
  `optimize` rebuilds the index without their terms, and a `TRUNCATE`
  checkpoint empties the WAL. A forget on a store without a database
  creates none. `#scribe forget s-<n>` erases a whole session; see
  [Forgetting a session](#forgetting-a-session).
- **Search.** `scribe search --kind message` matches each search token as
  a word prefix (`"<token>"*`, any of them) and orders by `bm25()`, then
  time. Unscoped and group-scoped search include group-only turns even
  without a session record. A record supplies project, topics, title and
  active status when present; otherwise group-only turns use the group's
  project, have no topics and cite the session id. Inactive records are
  hidden unless requested; date filters use the message time.

### Forgetting a session

Daniel types `#scribe forget <session-id>` with `s-<n>`, the full provider
session id or a unique prefix of it of at least four characters (2_design.md
§15.7). An ambiguous prefix is refused `id_ambiguous` and erases nothing. The
verified typed hook is the deletion route.

- The session transaction deletes its segments, terms, links, switch, cursor,
  rebuild checkpoint, pending candidates and indexed message rows, including
  group-only turns. `secure_delete` and FTS5 `optimize` discard old contents.
  Only the short id, provider id and forget timestamp survive in the tombstone.
- The transaction deletes handoffs related to the session and reserves their
  ids in `deleted_handoffs`. It removes the member from its seat, removes an
  empty seat, invalidates the group's derived summary and drops successor
  `continues` links from records and pending switches.
- Open source claims become `rejected` with `closed_reason: withdrawn`.
  Their reason, quote and other text evidence are cleared. An approved
  entry remains readable, with `cite.session: deleted` and its source
  message unset. `session:deleted` also replaces a matching explicit
  citation. A claim with an erased source cannot recreate a missing entry.
- Memory occurrences and pending candidates for the session are removed.
  A current item without any source is retired from the live files, and
  source-free history is removed. An unmigrated `m-<n>` item can retain a
  retired header with no statement or occurrences. The memory journal
  applies file changes after commit and drops old copies of those files.
- The hook's single audit line commits with the changes and uses `s-<n>`
  as its target. Post-commit cleanup truncates the WAL and removes session
  record and checkpoint files and matching document data in each
  `pre-sqlite-*` archive. The forgotten row drives retry after interrupted
  cleanup and after migration moves legacy files. A repeated forget can
  also repair residue from an earlier version.

The provider transcript stays at its original path. New exports omit erased
content. A previously delivered export in the caller's directory is a
separate backup. Markdown export omits tombstones, and JSONL retains the
id-only session row with its forget timestamp. JSONL leaves out stale
forgotten checkpoints even before a write removes them.

A forgotten session takes no record, message, group, rebuild or switch write.
Explicit passes, catalogs and `/scribe on` report `session_forgotten`.
Rebuild also checks after acquiring its per-session lock.

### Sessions in scribe.db (phase 6)

- **Where sessions are read from.** With a `record_kinds` row for
  `sessions`, every session read is a query and the files are not read;
  without one the store has not moved and reads `sessions/` and
  `control.json` as before. `control.json`'s format is checked either way,
  so a store stamped by a newer build is `store_too_new`.
- **The move.** In the format-27 release, the first eligible store write
  transaction moves every kind still in files, sessions first (see
  [Documents in scribe.db](#documents-in-scribedb-phase-7)). A session write
  (a switch, a pass, a rebuild, an activity change, a link) starts such a
  transaction. An audit line recorded outside a write transaction does not
  start the move: while the store is still file-backed, it can be spooled
  under `audit-spool/` (phase 7). In the transaction, scribe reads every
  session record (one that does not read refuses the move with
  `store_corrupt`, naming the file, and nothing moves), inserts them ordered
  by `created` then id, inserts every switch and Codex cursor, imports each
  `.<id>.rebuild.md` checkpoint as a row, checks the row count against the
  files read, and adds the `record_kinds` row. After the commit it moves
  `sessions/`, `control.json` and the root's `.codex-<id>.lock` files to
  `pre-sqlite-<yyyy-mm-dd>/`. If a move on the same date reuses an existing
  archive directory, each colliding moved child gets the next numeric suffix
  inside it (for example, `pre-sqlite-2026-10-15/entries.2/` and
  `pre-sqlite-2026-10-15/proposals.json.2`); scribe keeps the archive directory and cleans erasures within it.
  It then writes `control.json` as the format stamp. A move cut off after
  the commit finishes at the next write. A write that refuses leaves the
  files where they are; at format 26, a write transaction that touches no
  session (an entry or a tag) also leaves sessions in files. The next
  eligible transaction can move the records and drain any spooled audit
  lines. A store with no session files is marked moved by its first
  eligible transaction.
- **`scribe store migrate`** makes the move now, in its write's transaction. It is
  refused with `migrate_blocked` while any session is switched on, while
  any rebuild checkpoint waits, and once the sessions moved; the write's
  own move is never refused for these, since it imports switches and
  checkpoints as rows.
- **`s-<n>` ids.** A session's number is its `sessions.id`. Before the
  move a file store computes the numbers the move will give: records by
  `created` then id, then sessions with only a switch by id, so
  `session show --json` reads the same before and after. Every command
  taking a session id takes `s-<n>`: an exact provider id wins first, then
  `s-<n>`, then a unique prefix. JSON keeps `id` as the provider id and adds
  `short_id`. Cites (`session:<id>#<n>`), messages and memory links keep the
  provider id. A row is written by an UPDATE and, for a new session, an
  INSERT, so rewriting a record spends no number.
- **One pass, one transaction.** A pass window writes its record, its
  messages, its memory rows with the memory journal row, and its switch in
  one `BEGIN IMMEDIATE` transaction; the journal's file operations apply
  after the commit and close the journal (§15.6). A crash after the commit
  leaves the journal open for the next memory or pass write to replay; a
  failure before it writes nothing.
- **Locks.** The Codex ingestion lock and the rebuild lock are
  `locks/codex-<id>.lock` and `locks/rebuild-<id>.lock`, created `0600` in a
  `0700` directory and kept after release.
- **`scribe export <dir>`** writes every session to a missing or empty
  directory, created `0700`, reading the store only (no lock, no write):
  `--as markdown` (the default) as `sessions/<yyyy>/<mm>/<id>.md` record
  files at the current format plus `control.json` with the switches and
  cursors; `--as jsonl` as one `<table>.jsonl` per session table, one JSON
  object per row. A directory with anything in it is
  `export_target_exists`; JSONL of a store whose sessions are still files
  is `export_needs_migrate`. A record that does not read is left out and
  reported under `skipped`.

### Documents in scribe.db (phase 7)

- **Where they are read from.** With `record_kinds` rows for every kind,
  entries, claims, agent tags, marks, pickups and the audit log are
  queries, and `entries/`, `proposals.json`, `tags.json`, `marks.json`,
  `pickups.json` and `audit.jsonl` are not read; without them the store has
  not moved and reads the files as before. A row is checked as its file was
  (an entry's id and tag agree, a claim's fields agree with its state), and
  one that does not read is skipped and reported as a file was.
- **One write, one transaction.** The store lock is retired. Each
  transaction-backed store write is one `BEGIN IMMEDIATE` transaction whose
  reads see what it wrote so far; a write that refuses rolls back, so it
  changes nothing. A step inside a larger write is a savepoint. A busy
  database past the lock wait (10 seconds) is `store_locked`, waited for
  once.
- **One command, one transaction.** A command that calls no model (the
  direct write verbs, `#scribe` in the prompt hook, `session-end`, `init`,
  `store migrate`) runs whole as one transaction with its audit lines, so a
  write and its line commit together: an approval writes its claim, its
  entry and its line or none of them, and `reapply_approved` is left with
  nothing to repair. A command that calls a model (`session pass`,
  `catalog`, `rebuild`, `consistency scan`, the `Stop` hook,
  `memory refresh`, the routine tools) holds no transaction across the
  call, and commits each step with its own lines.
- **Audit lines that cannot wait.** A line recorded outside a write
  transaction is inserted without waiting. When another writer holds the
  database, or the store has not moved, it is written to `audit-spool/` as
  one file of its own, created `0600` under a name no other line has; every
  read merges the spool. If the store is still file-backed, this audit-only
  write can spool the line without starting migration. The next eligible
  store write transaction moves the file-backed records and drains the
  spool into `audit` (a drained file's name is kept in `spool`, so a crash
  before its removal never counts a line twice), then removes the spool
  files after commit. A refused command therefore waits once and loses no
  line, and writes no warning before its error.
- **The move.** The first eligible store write transaction moves every kind
  still in files, sessions first: every record must read (one that does not
  refuses the move with `store_corrupt` and nothing moves); each kind's row
  count is checked against the files read; audit lines keep their own
  format, and a torn line is counted and left in the moved file. After the
  commit the files go to `pre-sqlite-<yyyy-mm-dd>/`, and `control.json`,
  `proposals.json`, `tags.json`, `marks.json` and `pickups.json` are
  written as empty stamps at format 30, so a format-29 build refuses the
  store instead of writing files beside the database. The format-27 release
  wrote these stamps at 27, the format-28 release at 28, and the format-29
  release at 29. A
  standalone audit
  line does not trigger this move; a later eligible transaction imports the
  records and drains its spool. A write that refuses rolls the move back
  with it; a move cut off after the commit finishes at the next write, with
  `audit.jsonl` lines keyed in `spool` by byte offset and SHA-256 so
  replaying an interrupted move keeps one copy of each original line and
  still preserves new lines appended before the rename.
  `scribe store migrate` makes the move now, refused as in
  phase 6.
- **The memory journal** is staged in the write's transaction and applied
  after its commit, in a write of its own, so a rollback leaves no memory
  file the database does not record. A later step of the same command
  reads the memory files with the open journals laid over them.
- **Forget and redaction.** A write that changes or deletes an entry or a
  claim row ends with a `TRUNCATE` checkpoint, so with `secure_delete` no
  old text is left in the WAL or a free page. Forgetting an entry also
  deletes its moved-aside entry file from every
  `pre-sqlite-<date>/entries*/`, drops or rewrites the matching proposals in
  `pre-sqlite-<date>/proposals.json*` atomically (keeping every claim the
  database keeps), and drops any `pre-sqlite-<date>/audit.jsonl*` lines
  carrying the entry's text in a free-text value. The archive-erasure
  obligation commits in `pending_archive_forgets` inside the forget's own
  SQLite transaction (storing only the entry id, a per-obligation salt, and
  salted SHA-256 `{len, sha256}` digests — never the entry's cleartext), and
  is removed only after every `pre-sqlite-*` archive has been cleaned
  (audit lines first, then proposals, then entry files). A crash or I/O
  failure after commit leaves the obligation in `pending_archive_forgets`,
  and the next write (a retry of `entry forget` or `#scribe forget`, or any
  subsequent write) replays WAL truncation and archive erasure idempotently
  without needing the original entry contents in cleartext (STD-03 R8–R9).
  These post-commit `pre-sqlite-*` rewrites run under
  `locks/archive-forget.lock` (`flock` / `LockFileEx`, bounded by the lock
  wait) so concurrent forgets serialize their read-modify-writes and cannot
  restore one another's erased text. An audit line's structural
  fields (ids, command, status, author, codes, counts) never match, so an
  entry whose body is a common word such as `success` removes no unrelated
  event. A torn line, which the reader skips, is dropped if it contains the
  text. Other files in `pre-sqlite-<date>/` are the user's and are never
  rewritten.
- **Formats 27–30.** Format 27 introduced the move of every kind still in
  files into `scribe.db`, sessions first, and stamped `control.json` at 27.
  The format-28 release kept that migration path and stamped `control.json`
  at 28 after the move. Format 29 adds groups and seats. The current
  format-30 build keeps that migration path and stamps `control.json` at 30.
  Format 28 adds optional `handoff_mode`
  (`continue` or `new`) to entries; SQLite schema 7 adds its nullable column.
  The upgrade registry remains append-only for persisted document shapes; SQL
  schema changes use migrations. The existing steps stay so the move can
  read a store left at any older format for the support window; phase 8
  removes them with the file decoders.
- **`scribe export <dir>`** writes records into a separate directory. `--as markdown` writes the
  file store's files at format 30 (`sessions/`, `entries/<tag>/<id>.md`,
  `control.json`, `proposals.json`, `tags.json`, `marks.json`,
  `pickups.json`, `audit.jsonl`), which a build reads as a store and moves
  in again unchanged. Each exported audit line keeps its original audit
  format and decoded event. `--as jsonl` writes one `<table>.jsonl` per
  table with rows, including groups, seats and membership chains. Groups have no legacy Markdown file representation. JSONL needs every kind with records moved (a phase-6 store with no live
  record files exports its session tables), and on a moved store merges any
  pending `audit-spool/` events not yet drained into `audit.jsonl` (with
  `id: null` and `spool` set to the spool filename), skipping spool files
  already present in the `audit` table's `spool` column.

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
Format 12 adds `inactive_since` (RFC 3339 or `null`): when the entry left
`active`, `null` while it is active; any other combination is
`store_corrupt`. Stop recovery reads it to tell which turns the entry still
covered ([2_design.md §4.1](../2_design.md#41-detection)).

Format 4 adds `author` (`{kind: daniel|agent, name}`, `user` since format 23; `name` lowercase
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

Format 13 adds `approved_by` (`{kind: agent, name}` or `null`) and
`instruction` (`{session, message, at, quote}` or `null`, where `quote` is
redacted text or `null` once forgotten), both upgraded to `null`. An entry
with `approved_by` requires `instruction` to be non-null and `approved_by` to
have kind `agent`; a direct agent entry (`content_from: {kind: agent}`) may not
carry `approved_by`. Any contradictory combination is `store_corrupt`.
Format 15 lets an approved claim's entry (Daniel's with `stated_by`, or the
assistant's) that carries `approved_by` have as `said` the canonical command
`scribe proposal settle p-<n> --approve --author <name> --session <id>
--instruction '<his words>'` (the words redacted, shell-quoted, and left out
once the instruction's quote is forgotten) instead of `#scribe approve
p-<n>`; the command without `approved_by`, or in a file older than format
15, is `store_corrupt`.

Format 17 adds `removal_instruction` (`{session, message, at, quote}` or
`null`): the checked instruction that retired or forgot the entry, its quote
redacted, `null` once forgotten. `instruction` is from format 18 only the
instruction `approved_by` approved on: an `instruction` without `approved_by`,
a `removal_instruction` on an entry that is not `retired` or `forgotten`, and
a blank `message` or quote in either are `store_corrupt`. The step 17→18
moves a format-13 to format-17 entry's `instruction` to `removal_instruction`
when it has no `approved_by`; through format 17 a retirement on instruction
replaced an approval's `instruction`, and that one cannot be told apart, so
an entry with `approved_by` keeps it.

Format 14 adds `stated_by` (`{kind: scribe, name: session-pass}` or `null`),
upgraded to `null`. An entry approved from a candidate that carried a
statement holds scribe's statement as its content, Daniel's verbatim turn in
`quote`, `stated_by: {kind: scribe, name: session-pass}` and `author: {kind:
daniel}`; its `content_from` is his user turn, its `said` the `#scribe
approve p-<n>` line and its `window` `null`. Forgetting it blanks the content
and clears the quote; `stated_by` stays. A Daniel entry with a quote but no
`stated_by`, a `scribe` `stated_by` on a direct agent entry, or any other
author or source is `store_corrupt`. (2_design.md §16.6: a direct agent
entry written with `--spoken` carries `author: {kind: agent, name}`,
`stated_by: {kind: agent, name}`, `content_from: {kind: agent}`, `cite: null`,
`said: ""`, and the user's verbatim `quote` until forgotten; `stated_by`
updates if `entry update --content` rewrites the statement.) An entry
approved from an older candidate (no statement) keeps the quote as its
content with `stated_by: null`.

Format 22 admits `stated_by: {kind: scribe, name: annotation}`: an entry
Stop wrote for a `#scribe` marker inside a sentence of a turn Daniel typed
(2_design.md §14.4). It has `author: {kind: daniel}`, scribe's statement
as content, his verbatim span in `quote`, `content_from` his user turn
(`{kind: message, role: user, message, at}`), the marker's line in `said`,
a cite with the marked turn's message id and the marker's line, no
`approved_by` or `instruction`, and under an action tag the §4.6 `window`.
Forgetting it blanks the content, `said` and the quote; `stated_by`
stays. An annotation entry below format 22, one with another source, or an
active, retired or superseded one without its quote and `said` is
`store_corrupt`.

Format 16 adds `outcome` (`{state: open|parked|task, reason, task}` or
`null`), upgraded to `null`. It is set exactly on an entry the session pass
wrote itself under a `write`-route tag (2_design.md §11.6), and `null` on
every other entry. Such an entry has `author` and `stated_by` both `{kind:
scribe, name: session-pass}`, `said: ""`, `content_from` the turn it caught
(`{kind: message, role, message, at}`: Daniel's user turn, or from phase 2
an assistant turn under a catcher whose `from` reads it), a cite, scribe's
statement as content and that turn's verbatim span in `quote`, and `window`, `evidence` and
`approved_by` `null`. `open` has `reason` and `task` `null`; `parked` has
Daniel's parking words verbatim in `reason` (redacted text) and `task`
`null`; `task` has a task id (at most 64 characters) in `task` and `reason`
`null` (reserved: the pass never writes it; a pass drops an idea as
become-a-task only for an id-shaped task, one token with a digit, that a
session message writes as a whole token). Forgetting such an entry blanks
the content, clears the quote and the parked `reason`; `outcome.state`
stays. An `outcome` on any other entry, a pass-written entry without one, or
any other author or source on it is `store_corrupt`.

**Session keys:** `format`, `kind`, `id`, `status` (`open`, `final`),
`project`, `topics`, `related` (model-extracted references, unchanged by
links), `links` (a list, possibly empty, of `{type, session}`: `type` is
`spawned_from` or `continues`, `session` a session id; the links typed for this session or restored from a continue handoff, stored once, on the child; a parent's children
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
heading is `store_corrupt`. The format-3-to-4 step is where that body becomes
the current header and log (see **Format 4** below); later steps then apply.

## Issue files

Phase 2 (2_design.md §15.9) removed issue records. In a write's transaction,
the first write converts each `issues/issue-<n>.md` to one entry under the
tag `issue` with no model call and moves the file to
`pre-issue-<yyyy-mm-dd>/`; a file that does not parse stays and is tried
again ([specs/catchers.md](catchers.md#stored-issues-become-issue-entries)).
`issue` is no longer a reserved tag. The shape below is what that
conversion reads.

An issue (`issues/issue-<n>.md`, format 17 and later only; an older format
in one is `store_corrupt`) is the memory of an incident across projects and
what fixed it. It is a record file as above with `kind: issue`, written by
agents through the former `scribe issue add|occur|update|move` and never
approved; until phase 2 the tag `issue` was reserved for it.
Frontmatter keys, every one required and no other admitted: `id`, `title`
(1–120 characters), `status` (`open`, `mitigated`, `resolved` or
`forgotten`), `signature` (1–5 symptom strings, 1–200 characters each;
empty once forgotten), `projects`, `hosts`, `topics`, `related`, `author`
(an agent), `updated_by` (the agent that changed it last, `daniel` after
his `#scribe forget`, or `null`), `created`, `updated`,
`occurrences` (newest first) and `gone`. Each occurrence holds `n`, `date`
(`YYYY-MM-DD`), `host`, `project`, `session` (the session the request was
typed in), `request_message` (the uuid of Daniel's typed turn holding it),
`recurred` (`true` when it was added to a `resolved` issue, which reopens
it), `moved_from` (`issue-<n>#<m>` or `null`) and `author`. `gone` lists
occurrence numbers that left the issue, `{n, reason: moved|forgotten,
to}`, so a number is never reused.

The body is `## Cause`, the header's cause (≤600 characters, or nothing),
`## Fix`, its fix, then `## Occurrences`: one `### <n>. <date> — <host>,
<project>` section per frontmatter occurrence in the same order, each the
summary (≤1200 characters), `**The user asked:** "<request>"` (`**Daniel asked:**` before format 23, still read; their words,
checked verbatim against his typed turns of `session` when written,
≤1200 characters), `**Error lines (agent evidence):**` over at most 10
lines indented four spaces (≤300 characters each), `**Cause:** ` and
`**Fix:** ` (`cause not found` and `fix not found` when not given) and
`**Links:** session:<id>[, <link>…]` (at most 10 links beyond the session;
task, pr, repo and session kinds). A heading that disagrees with its
occurrence, a section without one, or a missing line is `store_corrupt`.

Occurrences are never edited, only moved (`issue move`, which writes the
target then the source under the lock, the source's `gone` naming `to`; a
move does not reopen) or forgotten. `#scribe forget <issue-id>` blanks the
title, signature, cause, fix and every occurrence (removing them into
`gone`) and sets `status: forgotten`, keeping the file and its id;
`#scribe forget <issue-id>#<n>` removes one occurrence into `gone`, and
takes its cause and fix out of every header that holds them: its own
issue's, and, following its moves back (a `moved` gone entry naming it),
each issue it was moved from, since `issue add` and `issue move --new` copy
an occurrence's cause and fix into the header. Such a header falls back to
the newest remaining occurrence's own cause or fix that does not hold them,
else to `cause not found` / `fix not found`. No word of a forgotten issue
or occurrence remains in any store file; the audit lines never held any.
`issue update` checks its cause and fix against their bounds after
redaction, which can lengthen them; one past its bound is refused
`invalid_text` naming the flag, and nothing is written.

## Memory files

Memory is three kinds of file under `memory/`
([2_design.md §15.5](../2_design.md#155-memory-one-fixed-shape)). None carries a
`format` or `kind` key: `control.json`'s format covers them.

- **`profile.md`** has no frontmatter: exactly seven `## ` headings in this
  order, `Role and work`, `Current focus`, `Projects`, `Goals`,
  `Environment`, `How they work` and `Pinned`, nothing before the first. A
  store with none reads as having none; the first refresh starts from the
  empty headings.
- **`now/<date>-<slug>.md`** and **`milestones/<date>-<slug>.md`**: the
  stem is the item's id, its date and the title's lowercase ASCII words
  joined by `-` (cut at 48 characters on a word; `item` when none). The
  body is `# <title>`, a blank line and the text.

```yaml
---
date: 2026-12-12           # required
kind: event                # now only: deadline | event | plan
sources:                   # the sessions it came from, at least one
- 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02
updated: 2026-11-03T06:00:00Z   # the turn's time, so a replayed window writes the same bytes
raised:                    # now only, optional: the last agent ack
  at: 2026-11-04T09:00:00Z
  session: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02
---
# Sister's wedding

The user said their sister's wedding is on 2026-12-12.
```

A file that does not parse, or whose stem is not a valid one, is skipped
with a warning and never served; an unknown key is `store_corrupt`.

**The journal.** Every memory write runs in its write's transaction
(phase 7; the store lock before it), in order: reads the memory files with
every open journal row's operations laid over them; migrates any `m-<n>`
items (below); computes its change; and stages its rows and one journal row
holding the file operations. After the transaction commits, a write of its
own applies every open row's operations (through `write_atomic`, or removes
the file) and closes the row (`done = 1`, and the profile revision's
`file_sha256`). A rollback writes no file; a crash between the commit and
the files is completed by the next write.
Reads take no lock and never replay: with an open journal row they print
the latest committed profile revision and warn `memory_pending`. A file
operation's path must stay under `memory/`.

**The pass** (§15.5). Each window's elements are checked without the lock
(shape and bounds; the quote a verbatim span of the user's own turn in the
window and in the original transcript; the redactor; the finance words;
the mood rule), then again under it. At most 3 are taken per invocation. A
milestone needs a real date and a turn that calls it one (one of
`user_says`, matched as a capture trigger). A dated `now` element writes
its file by stem, or rewrites the item `update` names, or dates the
pending candidate `completes` names and drops it. An undated one is kept
in `now_pending`, one per subject and at most 5 per session; the
session's final pass keeps none and drops the session's rows. A file whose
bytes would not change is not written. The window's `signal` (0–3, how
much it says about who the user is) is kept in `pass_signals`.

**The refresh** (§15.6, `scribe memory refresh`, weekly). Under the lock
it reads the sessions after `profile_cursor` in `(updated, id)` order,
active only, at most 40 (the rest are its `backlog`, taken by later runs);
each with its summary and goals, cut at 1,500 characters, and the three
with the highest summed signal (ties: the longer user text) also with
their kept user turns, cut at 12,000. It adds the hand edits since the last
revision as a line diff, the migrated `profile` and `projects` history
rows as hints while there is no revision, the `now` items, the ones past
their 14 days, the milestones and the preference entries. It calls
`models.profile` once without the lock, then writes under the lock only if
nothing changed meanwhile (the latest revision, the profile file's hash,
the cursor and the `now` stems; else `memory_changed`). The answer is
refused whole when the profile does not have the seven headings
(`profile_shape`), changes Pinned by a byte (`profile_pinned`), is over
6,000 characters (`profile_too_long`), links an entry id that does not
exist (`profile_unknown_link`) or holds a credential, finance or
sensitive word outside Pinned (`profile_sensitive`); a promoted milestone
naming no passed item or with no real date is `profile_milestone`. A
refused answer keeps the cursor. A changed profile is a new revision; an
unchanged one writes none. Each passed `now` item becomes the milestone
the answer names for it, or else a `memory_history` row, and its file is
removed. A run with no session after the cursor, no hand edit and no
passed item calls no model and writes nothing.

**Migration** (§15.11). The first memory write on a store holding
`m-<n>.md` items migrates them through one journal: an active `now` item
with a date not past its 14 days becomes a `now` file (kind `plan`, its
`acknowledged` as `raised`); an active milestone a milestone file (dated
by its event date, else its creation); every item except a forgotten one
leaves a `memory_history` row (its statement and quotes); an active
`working` item whose newest quote fires the `preferences` capture trigger
opens a claim first, cited by that quote's hash, `admitted.trigger`
`migrated working memory`; each item's occurrence turns are linked in
`memory_messages` under its `m-<n>` and, for an item that becomes a file,
its stem; and every `m-<n>.md` is removed. Reads leave the
items and warn how many await migration.

**Forget** (`memory forget <stem> --confirm`) removes the file and, in
the journal's transaction, deletes the item's `memory_history` rows, the
messages it cites and its `memory_messages` rows; when a message went, an
FTS5 `optimize` runs in the transaction and a `TRUNCATE` checkpoint after
it, as for an entry forget, so no trace of the words is left. The profile is not forgotten: it is
corrected by editing it outside Pinned.

## JSON documents

- `control.json` is the format stamp. Before the sessions moved (phase 6) it
  also held the switches and Codex cursors: `{"sessions": {"<session-id>":
  {"provider", "on", "since", "due", "final", "consecutive_failures",
  "retry_after", "pending_links", "cataloged_by"}}, "codex_cursors": {...}}`
  (an older file upgrades on read). Once the sessions moved, a switch is a
  row of `sessions` (its `sw_*` columns) and a cursor a row of
  `codex_cursors`, both in `scribe.db`, and the file holds only
  `{"format": 28, "sessions": {}, "codex_cursors": {}}`. A switch has the
  same fields in either home. `pending_links` holds `{type, session}` links
  established before the session had a record; the pass that writes
  the record moves them onto it, then clears them, in one locked call.
  `since` and `retry_after` are RFC 3339 timestamps or `null`; the failure
  count is a saturating unsigned integer. A failed pass stays due; its next
  automatic attempt waits 5, 10, 20, 40, then at most 60 minutes. Success
  or switching off resets both retry fields. Explicit `--session` bypasses
  the delay.
- `proposals.json` (written at format 22; an older file upgrades on read): `{"proposals": [{"id": "p-<n>", "kind":
  "supersedes"|"stale", "older", "newer", "reason", "quotes": [{"entry",
  "text"}], "state": "open"|"approved"|"rejected", "opened_at",
  "settled_at", "settled_by"}]}`. `newer`, `settled_at` and `settled_by`
  (a cite) may be `null`. Proposals are returned sorted by number.
  Each proposal also has `author` (`{kind: scribe|agent|user, name}`, `daniel` before format 23;
  the consistency routine writes `{kind: scribe, name: consistency}`) and
  `settled_author` (an author or `null`; `#scribe approve|reject|retire|forget`
  sets Daniel's). (S12) An agent's settle leaves `settled_by` `null`. Format 5 adds
  `updated_by` (agent author or `null`) to preserve who last changed a
  proposal reason without replacing its original `author`. Format 11 adds
  `closed_reason` (`null`, `"tag_retired"` on a `rejected` proposal
  scribe closed itself, or `"instructed"` on a `rejected` candidate proposal
  an agent closed on Daniel's checked instruction; a reason on any other
  state is refused as corrupt, and `"instructed"` on a proposal that is not a
  candidate is refused too. `"tag_retired"` stays readable on a rejected
  proposal of any kind, as format 11 accepted it; scribe itself closes only
  open candidates) and, on each `candidate`,
  `admitted` (`null`, or `{trigger, matched, criteria}`: the trigger that
  fired, the turn's words it matched, at most 120 characters, and the tag's
  `criteria` or `null`). Format 13 adds `instruction` (`null`, or
  `{session, message, at, quote}` where `quote` is redacted text or `null`
  once forgotten) on a settled `candidate` proposal with an agent
  `settled_author` and matching `settled_by` cite; an `instruction` on an
  open or non-candidate proposal, or `closed_reason: "instructed"` without
  an agent `settled_author`, `instruction` and matching `settled_by`, is
  `store_corrupt`. Format 15 adds `"covered"` to `closed_reason`: an
  approved candidate whose reserved entry was missing when an active entry
  already covered it, closed `rejected` by a repeated approval, the scan's
  recovery or an instructed approval, keeping its `settled_at`,
  `settled_by`, `settled_author` and `instruction`; a rejected candidate
  with an `instruction` carries `"instructed"` or `"covered"`, and
  `"covered"` on a proposal that is not a settled candidate, or in a file
  older than format 15, is `store_corrupt`. Format 14 adds, on each `candidate`, `statement` (scribe's
  one-sentence statement of the claim in third person, 1–300 characters
  before redaction, then redacted) and `subject` (what the claim is about,
  1–80 characters before redaction, then redacted; masking may lengthen
  either, and the masked value is kept), both `null` on a candidate opened before format 14 and both
  cleared by forgetting; one without the other, or a statement on a
  non-`user` candidate, is `store_corrupt`. The pass opens nothing for a
  claim with an empty or over-long statement or subject, and its duplicate
  check compares both the quote and the statement against what active
  entries and earlier candidates say. Format 24 adds, on each `candidate`,
  `context` (scribe's paragraph of the claim in third person, at least two
  sentences and 1–600 characters before redaction, then redacted), `null`
  on a candidate opened before format 24 and cleared by forgetting; a new
  claim carries `context` and `subject` and a `null` `statement`. A
  candidate with both a context and a statement, or either without its
  subject, is `store_corrupt`. The pass opens nothing for a claim with an
  empty subject or a context under two sentences or past its bound, and the
  duplicate check compares the quote and the context (an older claim's
  statement) as it compared statements.
- `cataloged_by` on a switch (in `control.json` before the move, a `sessions`
  row after it) is `null` or `{at, by}`;
  `/scribe on` keeps it. *(S14)* `session catalog` sets it and the pass
  copies it onto the record.
- (S12) `tags.json` (written at format 22; introduced at format 5): `{"tags": [{"name",
  "aliases", "description", "stale_after_days", "author", "updated_by", "created",
  "updated"}]}`, sorted by name; `name` and aliases as for config tags,
  `description` accepts 1–300 characters before redaction, `stale_after_days` a positive
  integer or `null`, `author` of kind `agent`; `updated_by` is the last updating agent or
  `null`. Absent is no store tags.
- `marks.json` (written at format 22; introduced at format 22, no older
  form): `{"marks": [{"session", "message", "prompt_sha256", "marker",
  "outcome", "entry", "code", "attempts", "at"}]}`, oldest first: one per
  `#scribe` marker inside a sentence of a typed turn, keyed by session,
  message uuid and marker number, holding ids and codes only, never words.
  `outcome` is `entry` (`entry` the id it wrote, `code` null), `none` (both
  null), `refused` (`code` the refusal: `unparsed_instruction`,
  `too_many_marks`, `quote_not_verbatim`, `unknown_tag`,
  `annotation_out_of_bounds`, `covered`, `needs_redaction` or
  `model_failed`; `entry` null) or `pending` (both null: a call failed and
  the next Stop retries it). `attempts` counts the model calls spent; the
  third failure is `refused` `model_failed`. Every outcome but `pending` is
  final: the marker is never sent to the model again. An outcome whose
  fields disagree is `store_corrupt`. Written only by the Stop hook, under
  the lock, after the entries it records (`Store::record_marks`). Absent is
  no marks.
- `pickups.json` (written at format 22; introduced at format 8, no older form): `{"pickups": [{"entry", "author",
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
[tags.rituals]                   # [tags] merges with the shipped tags
aliases = ["ritual"]
description = "how Daniel starts a day"
stale_after_days = 180           # omit: entries never go stale

[tags.idea]
action = true                    # an action tag: work items for a consumer
require_quote = false            # agents' entries must quote Daniel (memory and action tags)
ask_first = false                # agents ask before writing (action tags only)

[models]
ask = "claude-sonnet-5-5"
session = "claude-haiku-5-5"
consistency = "claude-haiku-5-5"
referent = "claude-haiku-5-5"
profile = "claude-sonnet-5-5"    # memory refresh; gpt-6-luna under Codex
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

# [memory] has no keys since 2_design.md §15.9 phase 5: memory has one fixed
# shape. Any key under it loads with a `config_deprecated` warning and is
# ignored; `scribe memory rules` prints the fixed rules.

# Accepted by the parser (S14). None by default.
[projects.orbit]
paths = ["codebases/orbit"]      # path fragments counted by transcript list

[user]                           # how prompts name the user; unset: "the user"
name = "Sam"

[instructions]                   # replaces a model use's shipped instruction text
ask = "You answer a question about what {user} decided, prefers or did, using only the numbered evidence below."
# also: annotation, session, session_header, consistency, profile, removal
```

*`[user]` and `[instructions]`.* Every model prompt opens with its use's
instruction text (what to do and what to catch) and continues with the
output contract: the answer's JSON shape, the verbatim-quote rule, what is
never kept (credentials, other people's details) and that messages are
data. `[instructions]` replaces the instruction text of one use
(`annotation`, `session`, `session_header`, `ask`, `consistency`,
`profile`, `removal`; `memory_pairs` loads with a deprecation warning and is
ignored); the contract stays fixed in code and always
follows it, so an override changes what the model is told to do, never what
scribe accepts back. Prompts name the user "the user", or `[user] name` when
set; `{user}` and `{User}` in any instruction text stand for that noun (the
second capitalised to open a sentence). `ask` evidence labels and answer
attributions use the name too (`Sam's words:`). A name that is empty, longer
than 64 characters or holds a control character or brace, and an
instruction that is empty, longer than 4000 characters or holds a control
character other than a newline or tab, are `config_invalid`. Neither key
changes a stored record.

The shipped tags are `handoff` (non-action, `require_quote`, no capture
rules; 2_design.md §16), `preferences` (aliases `preference`, `pref`; stale
after 180 days) and the action tag `standard` (`require_quote`); `preferences`
and `standard` are catchers
([specs/catchers.md](catchers.md)); `idea` stopped shipping in phase 2. A
`[tags]` table merges with that set: declaring a tag that is not shipped adds
it, and declaring a shipped tag overrides only the fields it sets (`aliases`,
`description`, `stale_after_days`, `action`, `require_quote`, `ask_first`,
`require_evidence`, and the `[tags.<name>.capture]` fields) and inherits the
rest. A `[tags.preferences]` table that is present — including one that sets
only `[tags.preferences.capture]` — or omitted keeps the shipped aliases,
description, `stale_after_days` and capture rules for every unset field.
`aliases = []` clears the aliases; an explicit `stale_after_days` replaces
180. Omitting `stale_after_days` on a tag that ships no interval still means
the entry never goes stale. There is no way to remove a shipped tag except by
claiming its name (`action = false` turns a shipped action tag into a memory
tag and keeps it). A name or alias the file sets that a shipped tag also
holds is Daniel's: the shipped tag loses that alias, or is left out when the
word is its name, and `tag list` prints a line naming both. The file stays
readable (`tag list`, `entry list` and `scribe search` still run). Omitting
`aliases` does not claim the inherited aliases. A word that two config
entries claim is `config_invalid`. An entry under a tag no config or
`tags.json` names (such as a `decisions`, `remind` or `finding` entry from
before that tag stopped shipping) stays listed, shown, searched, cited by
`ask` and settled. `ask_first` on a tag whose effective
action status is false is `config_invalid`; that status comes from configured
`action`, or from the shipped tag when `action` is unset (so `[tags.standard]`
inherits `action = true`). `require_quote` is accepted on any tag (action or
memory), and `[tags.handoff]` inherits `require_quote = true`. `require_evidence = true` on a tag whose effective
action status is true is also `config_invalid`. A tag name must already be
lowercase kebab-case, since it is a
directory; aliases are matched case-insensitively. An unknown key, a zero
cadence or window bound, or an invalid variable name is `config_invalid`,
naming the file and, where the parser knows it, the line. So is a zero
`segment_windows` or `inactive_after_days`, a project slug that is not
lowercase kebab-case, or an empty `paths`.

`tag list` reports every configured tag and every directory under
`entries/` with its entry count; a directory no configured tag names is
listed with `configured: false`. *(format 5, S12)* The tag table is the
config's tags, already merged with the shipped set, merged again with
`tags.json`; config tags win
([2_design.md §3](../2_design.md#3-storage)). A store tag's word a config
tag claims is dropped with a warning naming both, and so is a shipped name
or alias a config entry claims. Both lines go to stderr; the command still
exits 0.

## Invariants

- **Reads write nothing.** Read-only commands take no lock, create no
  directory or audit line, and work on a read-only root.
- **Capture** (S2). `Store::capture` takes the drafts of one prompt, and
  in its write's transaction finds an entry with the same `cite` session,
  `prompt_sha256` and `line` before writing; a match is returned as it is
  and nothing is written. A new entry gets the next number of its tag:
  one more than the largest number among the tag's parsed entries and the
  stems of its corrupt files, so a number is never reused while its file
  exists. The root and `entries/<tag>/` are created `0700` when absent and
  a new file is `0600`; writing over an existing entry file is refused.
- **Cite fill** (S2). `Store::has_unfilled_cites` reads only.
  `Store::fill_cite_messages` rewrites, in its write's transaction, each entry of the
  session whose `cite.message` is `null` and for which the caller finds a
  message, setting `cite.message` and `updated`; it does nothing when the
  root does not exist.
- **Control** (S4). `Store::update_control` changes the switches (rows
  since phase 6, `control.json` before) in its write's transaction and writes only when
  the change reports one, so `/scribe on`
  twice, `off` for a session that is not on, and a `Stop` or `SessionEnd`
  for a session never switched on write nothing. An off session can hold a switch with a pending
  `continues` link after restoring a handoff.
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
  `p-<max+1>` in its write's transaction, skipping any whose pair or stale entry already
  has a proposal in any state or whose entries are no longer all `active`,
  redacts them, and replaces `proposals.json` only when one opened.
  `Store::settle` applies one `#scribe approve|reject|retire|forget` in
  one transaction with the claim, each entry it changed and the command's
  audit line (phase 7), so an approved claim never leaves its older entry
  `active`; `Store::reapply_approved` (the scan's first step) still repairs
  a store a format-26 build left that way. A forget blanks the entry's `said` and body and, in
  `proposals.json`, the `text` of its quotes and the `reason` of every
  proposal about it, and clears `instruction.quote` and
  `removal_instruction.quote` to `null` on the entry and `instruction.quote`
  on every proposal about it while keeping each instruction's `session`,
  `message` and `at`. Nothing is written when nothing changed.
- **Retired-tag cleanup** (format 11, 2_design.md §11.3). In a write's transaction
  in the candidate write path (`open_pass_candidates` in `write_pass_window`,
  before counting the 20-open cap) and in `consistency scan`
  (`close_retired_candidates`), any open `candidate` proposal whose tag is
  absent from the effective tag table (`config.toml`, shipped tags, and
  `tags.json`) is closed automatically without calling a model or writing
  an entry: `state` becomes `rejected`, `settled_at` the current time,
  `settled_by` `null`, `settled_author` `{kind: scribe, name: tag-retired}`,
  and `closed_reason` `tag_retired`. Each close writes one audit line naming
  its proposal id; open candidates under tags in the table (including
  `preferences` and `standard`) and non-candidate proposals are untouched. The
  closed candidate keeps its reserved `older` entry id and stays in the
  suppression set like any rejection.
- **Modes and owners.** Every load refuses the root, a subdirectory, a
  record file or a document owned by another user (`store_foreign_owner`) or
  open to group or others, any group or other mode bit read included
  (`store_permissions`, naming the `chmod 700` or `chmod 600` that fixes it).
  Nothing is repaired automatically. Finder metadata files named `.DS_Store`
  or `._*` are ignored by store walks, including during the legacy import;
  they remain alongside the archived legacy files. On non-unix platforms
  these checks are no-ops.
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
- **Format.** One `FORMAT` (22) covers every document. A shape change bumps
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
  an upgraded document now persists it at the current format; a build older than S8
  refuses format 4 (`store_too_new`).
- **Format 8**. The format-7-to-8 step adds `quote: null` and
  `window: null` to each entry; `pickups.json` is new and has no older
  form. Other document shapes are unchanged. An older build refuses format
  8 (`store_too_new`).
- **Format 9**. The format-8-to-9 step adds `candidate: null` to each
  proposal. Other documents are unchanged.
- **Format 10**. The format-9-to-10 step adds `evidence: null` and
  `as_of: null` to each entry, and `role` (`user`) and `evidence: null` to
  each proposal candidate. Other documents are unchanged.
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
  windows to a checkpoint (a `rebuild_checkpoints` row since phase 6,
  `.<id>.rebuild.md` beside the record before) and replaces the record
  under the lock only if the record's `updated` is unchanged since the
  rebuild began. Checkpoints use the same format-4 session shape; their
  `updated` pins that visible revision rather than the checkpoint write time.
  No new persisted field or store version is needed. Each rebuilt segment
  has `origin: rebuild`. A per-session `locks/rebuild-<id>.lock` serializes
  rebuilders without holding a write transaction over model calls. The lock file
  remains after release; ordinary reads skip it. A changed visible revision
  causes rebuild to read the transcript again and fold in newer messages
  before retrying replacement. A successful replacement removes its
  checkpoint in the transaction that writes the record. It records `rebuilt_by` (format 6) with the `--author` that ran it. Identity, creation time, links and activity/catalog fields
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
- **Format 11** (capture rules, 2_design.md §11.2). Append-only step
  10→11 (`capture_admission`) adds null `closed_reason` to every proposal
  and null `admitted` to every candidate object; nothing else changes, and
  a format-10 file is checked against its frozen shape first. Config
  gains `[tags.<tag>.capture]` (`from`, `route`, `triggers`, `criteria`,
  `not`), each overriding the shipped rule of the same name alone;
  `triggers` replaces the shipped list whole (at most 32, each at most 120
  characters), `criteria` and `not` take at most 300 characters, and
  `from = "assistant"` is refused `config_invalid` (phase 2 accepts
  `assistant` and `both`, adds `max_open`, 1–20, and warns on the removed
  `route = "agent"`, which then catches nothing). A tag scribe ships no
  rules for must give `from`, `route` and `triggers`. A pass claim is
  written only under the lock, after its turn is checked again to fire a
  trigger of its tag under the config then on disk; the recomputed
  `admitted` is what is stored.
- **Format 12** (ORB-26360). Append-only step 11→12
  (`entry_inactive_since`) adds `inactive_since` to every entry: `null`
  for an active one, its `updated` for one already retired, superseded or
  forgotten (the best time an older record holds). Other documents are
  unchanged; a format-11 or format-12 `proposals.json` is checked against its
  frozen format-11 shape (`ProposalsV11`). At format 12 the key is required, `null` exactly while the entry
  is `active`, and set when retirement (`#scribe retire`, `entry retire`,
  an approved stale proposal), supersession or forgetting takes the entry
  out of `active`; forgetting a retired or superseded entry keeps its
  time. Metadata writes (`entry update`, cite filling) move `updated`,
  never `inactive_since`.
- **Format 13** (instructed settlement, 2_design.md §12.7). Append-only step
  12→13 (`instructed_settlement`) adds null `approved_by` and `instruction` to
  every entry and null `instruction` to every proposal, and lets
  `closed_reason` take `"instructed"`; nothing else changes. Format 10–12
  entries and unchanged sessions (format 6+) decode through typed YAML so
  local YAML tags stay intact (`EntryV12`; `inactive_since` is derived for a
  format-10 or 11 entry as the 11→12 step does, and refused in a file older than 12), and a
  format-11 or format-12 `proposals.json` is checked against its frozen shape
  (`ProposalsV11`, `CandidateV11`, `ClosedReasonV11`) first, independent of
  `EntryCandidate`.
- **Format 14** (statements, 2_design.md §11.1). Append-only step 13→14
  (`claim_statement`) adds null `stated_by` to every entry and null
  `statement` and `subject` to every proposal candidate; nothing else
  changes. A format-13 entry decodes through its frozen shape (`EntryV13`)
  and a format-13 `proposals.json` through `ProposalsV13`, so a stray
  format-14 key in an older file is refused.
- **Format 15** (instructed approval, 2_design.md §12.7). Append-only step
  14→15 (`instructed_approval`) changes no value; a format-14 entry decodes
  through `EntryV14` and a format-14 `proposals.json` through the current
  shape, refusing the instructed `said` and `"covered"`, which only format
  15 admits.
- **Format 16** (idea outcome, 2_design.md §11.6). Append-only step 15→16
  (`idea_outcome`) adds null `outcome` to every entry; nothing else
  changes. A format-14 or format-15 entry decodes through its frozen shape
  (`EntryV14`) and a format-15 `proposals.json` through `ProposalsDocument`,
  so a stray `outcome` key in an older entry is refused.
- **Format 17** (issues, 2_design.md §11.4). Append-only step 16→17
  (`issue_kind`) changes no value; it adds the `issues/` directory, whose
  files start at format 17. A format-16 build never reads `issues/`; it
  refuses every other document this build writes at format 17 as
  `store_too_new`. A
  format-16 `proposals.json` decodes through `ProposalsDocument`, since no
  proposal changed. Issue writes take the lock, redact and write through
  `write_atomic`; a repeated write that changes nothing writes nothing.
- **Format 18** (removal instruction, 2_design.md §12.7). Append-only step
  17→18 (`removal_instruction`) adds `removal_instruction` to every entry,
  moving `instruction` there when the entry has no `approved_by`; other
  documents are unchanged. A format-16 or format-17 entry decodes through its
  frozen shape (`EntryV16`) in typed YAML, so local YAML tags stay intact and a
  stray format-18 key is refused; formats 13 to 15 split the same way in
  `EntryV13` and `EntryV14`, and a format-16 or format-17 `proposals.json` has the
  current shape.
- **Format 19** (memory, 2_design.md §13). Append-only step 18→19
  (`memory_items`) changes no existing document; it admits `memory/`,
  whose files carry their own `format` and are read only at 19 or later.
- **Format 20** (memory consolidation, 2_design.md §13.4–13.5).
  Append-only step 19→20 (`memory_consolidation`) changes no entry,
  session, `control.json` or `proposals.json`: a format-18 or -19 entry
  and a format-15 to -19 `proposals.json` decode at the current shape. It
  adds the memory item keys `promoted_from`, `promotion`, `merged_into`,
  `superseded_by` and `unrelated_to` and the statuses `merged`,
  `superseded` and `promoted`; a format-19 item reads with them empty and
  is refused if it carries them.
- **Format 21** (a recurring `working` item proposed as a preference,
  2_design.md §13.7). Append-only step 20→21
  (`memory_preference_proposal`) changes no entry, session,
  `control.json` or `proposals.json`: a format-18 to -20 entry and a
  format-15 to -20 `proposals.json` decode at the current shape. It adds
  the memory item key `preference_proposal`; a format-20 item reads with
  it `null` and is refused if it carries it.
- **Format 22** (annotations, 2_design.md §14.8). Append-only step 21→22
  (`annotations`) changes no stored value: a format-18 to -21 entry and a
  format-15 to -21 `proposals.json` decode at the current shape. It admits
  `marks.json`, read only at 22 or later, and the annotation `stated_by`
  on an entry, refused below 22.
- **Format 23** (the user author channel, 2_design.md §15.8). Append-only
  step 22→23 (`user_author`) changes no stored value; every `Author` read
  maps `{kind: daniel, name: daniel}` to `{kind: user, name: user}`.
- **Format 24** (claims with context, 2_design.md §15.3). Append-only step
  23→24 (`claim_context`) adds null `context` to every proposal candidate;
  a format-14 to -23 `proposals.json` is checked at the frozen format-23
  shape first, so one carrying `context` is refused. From 24 an approved
  assistant entry may carry `stated_by` the session pass and the
  assistant's words in `quote`, refused below 24.
- **Format 26** (sessions in scribe.db, 2_design.md §15.7, §15.9 phase 6).
  Append-only step 25→26 (`sessions_sqlite`) changes no file's shape: a
  format-25 record or `control.json` decodes at the current shape. The
  first session write moves the sessions into schema version 3 and stamps
  `control.json` at 26, so a format-25 build refuses the store rather than
  reading an empty `sessions/`.
- **Format 25** (memory files, 2_design.md §15.5, §15.11). Append-only step
  24→25 (`memory_files`) changes no stored document: a format-24
  `proposals.json` decodes at the current shape. It admits
  `memory/profile.md`, `memory/now/` and `memory/milestones/`; the first
  memory write migrates the `m-<n>` items into them and stamps
  `control.json` at 25.
- **Agent writes** (S12). Each write verb takes the lock, checks the
  author guards (a reserved `--author` name — `user`, `daniel`, `scribe`,
  `consistency`, `session-pass`, `tag-retired` — is refused `reserved_author`; a Daniel entry's content,
  retirement without his checked instruction, the settling of a candidate
  without it, and the settling of a proposal
  whose `older` is a Daniel entry (with or without one) are refused `user_only` (`daniel_only` before format 23); author names between agents are
  attribution only, so any agent may update or retire an active agent
  entry, update an open agent proposal or settle one; a changed `--reason` when settling a
  proposal Daniel or scribe opened is refused `proposal_not_agent`),
  redacts, and writes the entry, `proposals.json` or `tags.json` through
  `write_atomic`. `entry add` numbers like capture. An agent settle uses the
  same two-write order as `#scribe approve`; both and recovery call the
  same entry-effect function. Recovery preserves an agent settler in
  `updated_by`. Entry content updates retain the original author and set
  `updated_by`; metadata-only updates of Daniel entries do likewise.
  Direct retirement stores its redacted reason; retirement on his checked
  instruction (`entry retire --session --instruction`, 2_design.md §12.1)
  also accepts his entry and stores `removal_instruction` with its redacted
  quote, keeping the text and any approval's `instruction`. `Store::forget_entry` (`entry forget`, only with a
  checked instruction and `--confirm`, §12.4) applies the same forget as
  `#scribe forget` (`consistency::forget`) under the lock, writing
  `proposals.json` first and then the entry, sets `updated_by` to the agent
  and `removal_instruction` to the instruction's `session`, `message` and
  `at` with `quote: null`; no database entry row is removed (while its
  moved-aside entry file under `pre-sqlite-<date>/` is deleted, matching
  proposals in `proposals.json*` are rewritten or dropped, and audit lines
  carrying its text are dropped), and an entry already forgotten writes
  nothing. `proposal settle --session --instruction` (2_design.md §12.3) settles
  an open candidate under the lock (every instructed write first refuses
  `instruction_not_found`, before writing anything, an instruction whose
  turn has no message id), refusing `instruction_stale` when the
  instruction was typed before the candidate's `opened_at`: it records the agent in
  `settled_author`, the instruction turn's cite in `settled_by` and
  `instruction` with its redacted quote; a rejection sets `closed_reason:
  "instructed"` and writes no entry, and an approval writes
  `proposals.json` and then the reserved entry through the same entry
  effect as `#scribe approve`, with `approved_by` and `instruction` set and
  the canonical command as `said`, or closes the claim `"covered"` when an
  active entry already says it. The same instruction again writes nothing;
  any other settlement of a settled claim is `proposal_settled`. Proposal reasons accept
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
  `/scribe link`, in one write's transaction: onto the child's record when it has
  one (no other field, `updated` included, changes), else onto its switch's
  `pending_links`. Every pass write merges the record's links on disk and
  the pending ones into the record it writes, so a link added while the
  model ran is never lost.
- **Writes** (S2). Every file except `audit.jsonl` is replaced whole by
  `fsio::write_atomic`: a temp file in the same directory, `fsync`, rename,
  `fsync` of the directory, in its write's transaction. Lock acquisition gives up after
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
  `entry.window.text`; (format 11) also
  `proposal.candidate.admitted.matched` and
  `proposal.candidate.admitted.criteria`; (format 13) also
  `entry.instruction.quote` and `proposal.instruction.quote`; (format 14) also
  `proposal.candidate.statement` and `proposal.candidate.subject`; (format
  24) also `proposal.candidate.context`; (format
  16) also `entry.outcome.reason` and `entry.outcome.task`; (format 17) also
  `issue.title`, `issue.signature`, `issue.cause`, `issue.fix`,
  `issue.occurrences.summary`, `issue.occurrences.request`,
  `issue.occurrences.error_lines`, `issue.occurrences.cause` and
  `issue.occurrences.fix`. Group metadata, closing summaries and stored message
  text are also masked at their database writes. A masked segment
  heading is cut back to 80 characters. Values shorter than 8 characters are not masked;
  identifiers, timestamps and paths are not free text. A `[redact] env`
  name with no value in the writer's environment refuses the write
  (`redact_env_unset`) before anything is stored; already-stored records
  are not rewritten.
- **Audit.** Lines are appended in one write in its write's transaction and `fsync`ed;
  never rewritten. A line is `{"format": 5, "at", "command", "status",
  "target", "error_code", "author", "instruction_session",
  "instruction_message", "issue", "duration_ms"}`: ids and codes, no argument
  values or error text. `target` is the resolved id of the record a write names,
  on refusal as well as success (`null` when it does not resolve). `author` is
  the `--author` name of a command that takes one, on every line it writes
  (STD-05 R2: a self-reported identity, kept in its own field); `null`
  otherwise. `instruction_session` is the `--session` of a write run on
  Daniel's instruction (`proposal settle`, `entry retire`, `entry forget`)
  and `instruction_message` the uuid of his turn once the check found it,
  else `null`; never the instruction's words. `issue` is `null` except on
  an issue write (`issue add|occur|update|move`) or a `#scribe forget` of an
  issue or occurrence, or a fire of the `issue` trigger in `hook
  user-prompt-submit`, where it is `{verb, id, occurrence, session,
  request_message, moved_to, trigger, request_sha256}`: the verb (`add`,
  `occur`, `update`, `move`, `forget` or `fire`), the issue (a move's
  source), the occurrence written, moved or forgotten (`null` for a whole
  issue, an update or a fire), the request's session (`--related
  session:<id>`, or the session the forget line or the firing turn was
  typed in), the uuid of Daniel's turn holding the request once the quote
  check found it (`null` for a forget and a fire: at `UserPromptSubmit` the
  turn has no uuid yet), a move's `issue-<n>#<m>` target, and (format 5)
  the fire's trigger phrase as configured and `request_sha256`, the sha256
  of the request turn: the fire's prompt, or, on an `add` or `occur`, the
  whole text and each text block of the turn the quote check found (empty
  otherwise). `issue stats` matches a fire to a write by session and a
  shared hash. One line per write, forget or fire, ids, hashes and the
  configured phrase only. A format-1 line, with no `author`, a format-2
  line, with neither instruction id, a format-3 line, with no `issue`, and
  a format-4 line, with no `trigger` or `request_sha256`, still read. A line
  that cannot be written is a warning and never changes the command's
  result. `issue stats` read the log without the lock and skipped a line
  that did not parse (a torn last line), counting it; a line of a newer
  format refuses the read (`store_too_new`). Phase 2 removes `issue stats`
  and the `fire`, and writes `issue: null` on every line; earlier lines
  with an `issue` object still read, as history nothing counts.

## Known gaps

- `session list` lists session records. A session switched on before its
  first pass has no record yet and is not listed;
  `/scribe status` in that session reports it as on with no record yet.
- `entry list --tag` matches a tag's name, not its aliases.

## Codex ingestion state (format 7)

Before the sessions moved, `control.json` has `codex_cursors: {"<session-id>":
"<host-message-id>"}`. Since phase 6 the cursors are rows of the
`codex_cursors` table in `scribe.db`, and `control.json` holds an empty
`codex_cursors` object.
Each session switch also stores `provider: claude-code|codex`. Provider stamping
and switch mutation share one write transaction. The Codex ingestion
lock `locks/codex-<session-id>.lock` (`.codex-<session-id>.lock` before phase 6) serializes lifecycle events independently of
the write transactions the records and cursors are written in.
The cursor advances only after effects, so an interruption can replay an
idempotent capture/settlement/control; it cannot silently lose a pending write.
Only ids are queued, never raw prompt text. A missing cursor in a rewritten
transcript refuses replay. Bootstrap starts in the current verified hook turn,
not at the beginning of a historical transcript. An earlier typed opt-in alone
does not activate a newly installed hook in a different turn.

Config adds `[codex.models]` (all uses default `gpt-6-luna`, transport `codex`).
Both model tables accept an explicit `provider` and per-use ids. Existing
`[models]` defaults and persisted entry/session shapes remain compatible.
