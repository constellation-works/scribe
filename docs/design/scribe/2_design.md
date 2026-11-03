---
title: scribe — Design
owner: claude
last_updated: 2026-11-02
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: design
type: design
summary: How the scribe plugin is bounded (one binary; hooks write, agents only ask), how #scribe entries, /scribe on session records, ask and the consistency routine work, where everything is stored, which model does what, and the slice plan that builds it.
tags: [scribe, plugin, architecture]
paths: ["crates/**", "plugin/**", "scripts/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-13117, ORB-13134]
---

# scribe — Design

The design of the scribe plugin: its boundary, the four parts named in
[1_overview.md §2](./1_overview.md#2-shape), the store, the model choices,
and the slices that build it. Nothing here is implemented yet; the code is
the template's note example until slice S1. The reasons behind each choice
are in [4_decisions.md](./4_decisions.md); open directions are in
[3_vision.md](./3_vision.md). Where this doc and the template's engineering
rules meet (atomic writes, the store lock, the audit guard, output modes,
gates), the template's mechanisms carry over unchanged and are not restated:
see [ARCHITECTURE.md](../../../ARCHITECTURE.md) and the
[store-module decision](./4_decisions.md#persistence-is-one-module-inside-the-core-crate).

## 1. Boundary and surfaces

One Rust binary, `scribe`, has three entry points. Each is a noun under the
STD-01 grammar:

| Entry point | Called by | Does |
|---|---|---|
| `scribe hook <event>` | Claude Code hooks (the `plugin/claude-code/` package) | Reads the hook's JSON on stdin; captures `#scribe` instructions, switches cataloging on and off, marks session passes due. The only path that writes entries. |
| `scribe plugin call` | Orbit's exec backend (`plugin/.orbit-plugin/`) | Reads the Orbit tool request on stdin, writes the `{"ok", …}` envelope on stdout. Serves `scribe.ask` and the routines' tools. |
| `scribe <noun> <verb>` | Daniel at a terminal, scripts | Read-only `entry list/show`, `session list/show`, `proposal list`, `tag list`, `question ask`, plus `consistency scan` and `session pass` for the routines. |

**The Orbit plugin tree** is `plugin/.orbit-plugin/`: `plugin.yaml`
(`schemaVersion: 2`, `kind: Plugin`, `metadata.name: scribe`), `bin/scribe`
(a launcher that execs the installed binary as `scribe plugin call`), `schemas/`
and `definitions/` (the two routines and their jobs). The backend is
`type: exec`, `timeout_ms: 180000`. Tools:

| Tool | `execution_kind` | `agent_callable` | `mcp_scope` |
|---|---|---|---|
| `scribe.ask` | `read_only` | — | `global` |
| `scribe.session-pass` | `mutating` | `false` | `none` |
| `scribe.consistency-scan` | `mutating` | `false` | `none` |

Orbit defaults `mcp_scope` to `workspace`, so every tool except `ask` sets
`none` explicitly, and a conformance test in `plugin/.orbit-plugin/tests/`
asserts that `ask` is the only advertised tool. Main agents therefore see
exactly `mcp__orbit__scribe_ask`. `ask` is `global` because the store is
Daniel's, not a workspace's. The two mutating tools run only from the
plugin's routines (Runner capability). Permissions: `fs.read` and `fs.write`
on the store root, `network: any` for the model, `env_pass` for the model
credential names ([§8](#8-models)), and `orbit_tools: [orbit.search,
orbit.task.show]` (read-only) for `ask`'s task evidence.

**The Claude Code package** is `plugin/claude-code/`
(`.claude-plugin/plugin.json`, `hooks/hooks.json`), installed with
`claude plugin install`. It registers `UserPromptSubmit`, `Stop`,
`SessionStart` and `SessionEnd`, each running `scribe hook <event>` with a
10-second timeout (30 seconds for `UserPromptSubmit`, which may call Haiku,
[§4.3](#43-resolving-this-and-it)). It ships no MCP server and no slash
command file: `/scribe` is intercepted by the `UserPromptSubmit` hook
([§5.1](#51-switching-on-and-off)). Orbit's plugin manifest has no hook or
command key, which is why this is a separate package
([4_decisions.md](./4_decisions.md#hooks-call-the-scribe-binary-directly-not-orbit)).

Writes never go through Orbit's tool surface: the hooks call the binary, so
no mutating tool needs `agent_callable` or an operator override, and the MCP
list stays at one tool.

## 2. Layering

The template's two crates and tiers stay
([4_decisions.md](./4_decisions.md#a-domain-library-crate-and-a-cli-crate)).
The note domain is deleted in S1 and replaced by:

| Module (`scribe-core`) | Owns | I/O |
|---|---|---|
| `entry` | `Entry`, `EntryId`, `TagName`, `Cite`, entry status; the next-id rule | none |
| `tag` | the tag table and alias resolution | none |
| `capture` | parsing a typed turn into `#scribe` instructions; tag resolution; the referent request and its validation | none |
| `session` | `SessionRecord`, the control state, the due rule | none |
| `transcript` | parsing a Claude Code transcript (JSONL) into typed messages, dropping tool payloads and synthetic turns | none (takes bytes) |
| `verbatim` | the one quote check ([§6.4](#64-the-verbatim-check)) | none |
| `ask` | tokenizing, ranking, evidence packing, answer validation | none |
| `consistency` | candidate pairing, staleness, proposal validation | none |
| `models` | the `Models` port (a trait) and the typed requests/responses for each use | none |
| `store` | the `Store` facade over `format`, `audit`, `fsio`; the only disk access | disk |

The CLI crate adds `commands/{entry,session,proposal,tag,question,consistency,plugin}.rs`
and `commands/hook/` (one file per event), plus two
adapters composed in `app.rs`: `ClaudeCliModels` (the `Models` port,
[§8](#8-models)) and `OrbitTasks` (task evidence for `ask`). Adapters that
spawn processes live in the CLI crate so the core stays free of
`std::process`, as the dependency-direction script already requires. The
adapters are a new module family; S1 adds them to `ARCHITECTURE.md` and the
script.

## 3. Storage

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`, as in the template. The
hooks, the CLI and the Orbit backend resolve the same root, so one store
serves every workspace on the host. The plugin's config key `root` (default
`~/.scribe`) is passed to the backend as `--root`.

```
~/.scribe/                       0700, owner-checked on every load
├── config.toml                  tags, models, cadence (Daniel edits; read-only to scribe)
├── tags/<tag>.json              {"format":1,"tag","entries":[…]}
├── sessions/<session-id>.json   {"format":1,"session":{…}}  one per opted-in session
├── control.json                 {"format":1,"sessions":{"<id>":{"on","since","due"}}}
├── proposals.json               {"format":1,"proposals":[…]}
├── audit.jsonl                  one line per mutating command (template shape)
└── .lock
```

Every file except `audit.jsonl` is replaced whole through
`fsio::write_atomic` under `.lock`; nothing is appended or edited in place.
Reads take no lock and write nothing (STD-01 R31), so `ask` works on a
read-only root. Each document carries `format: 1`; the template's
`notes.json` format is not carried over because no scribe store exists yet
(S1 starts `UPGRADES` empty for the new documents).

**Entry** (`tags/<tag>.json`):

```json
{
  "id": "decisions-12",
  "tag": "decisions",
  "said": "#scribe add this to decision",
  "content": "squash-merge agent PRs; keep merge commits for releases",
  "content_from": {"kind": "message", "role": "assistant", "message": "<uuid>", "at": "2026-11-02T19:04:11Z"},
  "cite": {"provider": "claude-code", "host": "dk-server-1", "session": "<session-id>",
           "message": "<uuid>", "at": "2026-11-02T19:05:40Z", "prompt_sha256": "…", "line": 3, "cwd": "…"},
  "recorded_at": "2026-11-02T19:05:41Z",
  "status": "active",
  "superseded_by": null,
  "review_after": "2027-05-01"
}
```

- `said` is the full `#scribe` line exactly as Daniel typed it.
- `content` is what the entry records. `content_from.kind` is
  `instruction` when it is a verbatim substring of `said`, or `message` when
  it is a verbatim span of an earlier message the instruction pointed at
  ("this", "it"); then `role` says whose words it is, and `ask` must never
  present an assistant's words as Daniel's.
- `status` is `active`, `superseded`, `retired` or `forgotten`; `forgotten`
  keeps the id and `cite` but blanks `said` and `content`
  ([4_decisions.md](./4_decisions.md#superseded-and-stale-entries-are-hidden-not-removed)).
- `review_after` comes from the tag's `stale_after_days` (none by default
  for `decisions`, 180 days for `preferences`).
- Ids are `<tag>-<n>`, `n` being the tag's max + 1 under the lock: short,
  self-describing in an echo, and never reused.

**Session record** (`sessions/<id>.json`): `session`, `provider`, `host`,
`cwd`, `title`, `started`, `updated`, `on_since`, `scribed_through` (the last
transcript message folded in), `summary`, `goals`, `outcome`, `next`
(character budgets 600/400/1200/600), `passes` (count) and `final` (true
after the session-end pass). The four text fields are model-written
summaries, labelled as such in `ask` evidence, never quoted as Daniel's words.

**Proposal** (`proposals.json`): `id` (`p-<n>`), `kind` (`supersedes` or
`stale`), `older`, `newer` (entry ids), `reason` (model text, at most 300
characters), `quotes` (verbatim spans from both entries), `state`
(`open`, `approved`, `rejected`), `opened_at`, `settled_at`, `settled_by`
(a `cite` of Daniel's approving turn).

**Config** (`config.toml`): `[tags.<name>]` with `aliases`, `description`
and `stale_after_days`; `[models]` with `ask`, `session`, `consistency`,
`referent`; `[catalog]` with `every_minutes` (20) and `every_turns` (10).
The file is created by `scribe hook session-start` on first run with the
defaults ([4_decisions.md](./4_decisions.md#only-decisions-and-preferences-ship-by-default))
if absent; scribe never rewrites it after that.

**Redaction.** Before any write of `said`, `content`, a session record's
text fields or a proposal's `reason`, the store masks the live values of the
credential variables named in the config and those used for model calls
(STD-05 R13, R14;
[4_decisions.md](./4_decisions.md#persisted-free-text-passes-one-redactor)).

## 4. `#scribe`

### 4.1 Detection

`scribe hook user-prompt-submit` receives `session_id`, `transcript_path`,
`cwd` and `prompt`. `capture::instructions` finds every line of `prompt`
that starts with `#scribe ` (after optional leading whitespace), skipping
lines inside code fences, blockquotes and `<pasted_content>` blocks (the
orbit-office `markers.rs` rule). Each such line is one instruction; a turn
with two lines writes two entries. A bare `#decision`, `#preferences` or any
other hashtag is not an instruction and captures nothing, with no reply.

Provenance check (an accident guard, not a security boundary — STD-05 R5):
before writing, the hook confirms `transcript_path` lies under
`~/.claude/projects/`, belongs to the user, and that its session id matches
`session_id`. The prompt itself is not yet in the transcript when the hook
runs, so the message uuid in `cite` is filled by the next `Stop` hook, which
finds the user message whose text hashes to `prompt_sha256`. A process that
can run `scribe hook` with forged stdin can forge an entry; that is
documented in [§9](#9-concerns--honest-limitations).

### 4.2 Parsing an instruction

Model-free grammar, tried in order:

1. `#scribe <tag>: <content>` — explicit tag, content after the colon.
2. `#scribe [add|record|note] <content> (to|in|under) [my|the] <tag>` — the
   tag is the last word group, matched against names and aliases.
3. `#scribe [add|record] (this|that|it|these) (to|in|under) [my|the] <tag>`
   — a referent instruction ([§4.3](#43-resolving-this-and-it)).
4. `#scribe approve <p-id>` · `#scribe reject <p-id>` ·
   `#scribe retire <entry-id>` · `#scribe forget <entry-id>` — settle a
   proposal or hide/blank an entry ([§7](#7-consistency-routine)).

Tag names and aliases are compared case-insensitively, singular and plural
both declared in config (`decision`, `decisions`; `preference`,
`preferences`, `pref`). An instruction whose tag matches nothing is refused:
no entry, and the reply lists the configured tags. An instruction that
matches no form is refused with the forms. A refusal is still audited
(`status: failure`, `error_code: unknown_tag` or `unparsed_instruction`).

Exactly one entry per instruction, also under retries: the idempotency key
is `(session, prompt_sha256, line)`. A repeat finds the existing entry and
echoes it instead of writing again.

### 4.3 Resolving "this" and "it"

Only form 3 uses a model. The hook parses the transcript, takes the last 12
messages before the prompt (user and assistant text, tool payloads dropped),
and asks the `referent` model (Haiku) for `{message, span}`: which message
the instruction points at and the exact passage to record. The answer is
accepted only if `message` is one of the 12 and `span` passes the verbatim
check against that message; then `content` is the span and `content_from`
cites it. On a timeout (20 s), a failed check or a model error, the entry is
still written, once, with `content` set to the previous assistant message's
first 600 characters, `content_from.kind = "message"`, and the echo says the
referent was guessed so Daniel can `#scribe retire` it. One instruction never
produces zero or two entries.

### 4.4 The reply

The hook returns `systemMessage` (shown to Daniel) and
`hookSpecificOutput.additionalContext` (shown to the main agent) with the
same text, and never blocks the turn, so the rest of the prompt still
reaches the agent:

```
scribe: added decisions-12 to decisions — "squash-merge agent PRs; keep merge commits for releases"
scribe: refused "#scribe add it to rituals" — no tag "rituals"; tags: decisions, preferences
```

The agent-facing text adds "recorded by scribe; no action needed", so the
main agent does not try to record it again.

## 5. `/scribe on`

### 5.1 Switching on and off

A prompt whose first line is `/scribe on`, `/scribe off` or `/scribe status`
is intercepted by the `UserPromptSubmit` hook, which updates `control.json`
and returns `decision: "block"` with the status as `reason`, so no model
turn is spent. Every session starts off. `on` records `on_since` and marks a
pass due; `off` stops further passes and keeps the record. `/scribe on` in a
session that is already on is a no-op that says so.

### 5.2 Cadence

`scribe hook stop` runs after each assistant reply. It does no model work:
it reads `control.json`, and when the session is on and either
`every_minutes` have passed since the last pass or `every_turns` user turns
have arrived, it sets `due` under the lock. `scribe hook session-end` sets
`due` with `final: true`. The plugin's `scribe-session-pass` routine
(`*/5 * * * *`, seeded disabled; Daniel enables it in one workspace) runs the
`scribe.session-pass` job, which processes every due session: at most five
per run, each with a 120-second model budget. No process is detached from a
hook ([4_decisions.md](./4_decisions.md#session-passes-run-from-an-orbit-routine-not-a-detached-hook-child)).

### 5.3 A pass

1. Parse the transcript from its first message (the whole opted-in
   session, not only messages since `on_since`), dropping tool payloads,
   `<task-notification>` and other synthetic turns, and attributing pasted
   blocks as pasted.
2. Send the previous record plus the messages after `scribed_through`
   (windowed to 150k characters, oldest dropped first with a note) to the
   `session` model (Haiku) for `{title, summary, goals, outcome, next}`.
3. Validate the budgets and the JSON shape; on failure, keep the previous
   record and leave `due` set for the next run (three failures in a row
   clear `due` and log a `warn`).
4. Write the record atomically, advance `scribed_through`, clear `due`.

A pass writes no tag entries, even if the transcript contains `#decision`
or an unprocessed `#scribe` line. There is no backfill: sessions that were
never switched on are never read.

## 6. `ask`

### 6.1 Contract

Input: `{"question": string, "scope"?: {"tags"?: [..], "since"?: date},
"include_hidden"?: bool}`. Output:

```json
{
  "status": "answered | no_record | conflict",
  "answer": "…",
  "claims": [{"text": "…", "quote": "…", "cite": "decisions-12"}],
  "conflicts": [{"cites": ["preferences-3", "preferences-9"], "note": "…"}],
  "evidence_considered": 14,
  "dropped_quotes": 0
}
```

`cite` is an entry id, `session:<id>` or `task:<id>`. A lookup
([§6.5](#65-lookups-records-by-recency-or-id)) also sets `"status":
"record"` and returns `records`, the stored records themselves. `scribe question ask
"<question>"` is the same operation for Daniel at a terminal (`--json`
prints this payload).

### 6.2 Retrieval (model-free)

1. Load every `active` entry (plus `superseded`/`retired` when
   `include_hidden`), every session record, and up to 20 Orbit tasks from
   `orbit.search` with the question's terms (title, status, and the first
   1,000 characters of the description).
2. Tokenize: lowercase, split on non-alphanumerics, drop stopwords and
   tokens under three characters, light suffix stripping (`-s`, `-es`,
   `-ing`, `-ed`). Tag names and aliases in the question boost their tag.
3. Rank with BM25 over `content` + `said` + tag for entries, the four text
   fields for sessions, title + description for tasks; entries get a 1.5×
   weight because they are Daniel's explicit words.
4. Keep the top 20 (entries first on ties), packed newest-first within a
   48k-character evidence budget.

If nothing scores above zero, `ask` returns `no_record` without calling a
model.

### 6.3 Answering

The `ask` model (the decision model, Sonnet by default) receives the
question and the numbered evidence, each item labelled with its kind, date,
whose words it is (`Daniel`, `assistant (recorded by Daniel)`, `session
summary`, `Orbit task`) and its status. The system prompt requires: answer
only from the evidence; every claim carries a verbatim quote and a cite; say
`no_record` when the evidence does not answer; when two items disagree,
return `conflict` with both cites rather than choose; never present a
summary or an assistant's words as Daniel's. The response is a JSON schema
(`claims`, `conflicts`, `status`, `answer`).

### 6.4 The verbatim check

`verbatim::contains(source, quote)`: both strings are NFC-normalized, `\r\n`
becomes `\n`, and runs of whitespace collapse to one space; the quote must be
non-empty, at least 8 characters, and a substring of the source. No case
folding, no punctuation stripping. The same function checks referent spans
([§4.3](#43-resolving-this-and-it)), proposal quotes ([§7](#7-consistency-routine))
and answers. In `ask`, a claim whose quote fails is dropped and counted in
`dropped_quotes`; if every claim drops, the status becomes `no_record`. A
claim whose cite is not in the evidence is dropped the same way. When the
evidence contains a hidden entry that matched (and `include_hidden` is
false), the answer notes that a superseded record exists, without quoting it.

### 6.5 Lookups: records by recency or id

`ask` takes any question, and orchestrators use it for handoffs: "can you
fetch me last session summary?" must return the latest session record, not
a ranked guess. Word-overlap ranking cannot answer that ("last" matches
nothing), so before §6.2 a model-free classifier recognises lookups:

| Asks for | Returns |
|---|---|
| the latest/last/previous session (optionally for a project or scope) | the newest session record in that scope by `updated_at` |
| a named session (`session:<id>`, or a title match) | that session record |
| open next steps / what is pending | the `next` items of the newest records in scope, newest first |
| the entries of a tag in a period ("decisions this week") | those entries, newest first |

A lookup returns `"status": "record"` and `records` with each record's
stored fields verbatim (summary, goals, outcome, next, timestamps, cite),
without a model call and without paraphrase. A question that is not a
lookup, or a lookup that matches nothing, falls through to §6.2. When the
classifier is unsure it falls through too, so a lookup never hides an
answer.

## 7. Consistency routine

The `scribe-consistency` routine (`0 6 * * 1`, weekly, seeded disabled)
runs `scribe.consistency-scan`:

1. **Candidates, model-free.** Within each tag, pair active entries whose
   token sets overlap (Jaccard ≥ 0.25 over content tokens) or share an
   explicit subject term; at most 40 pairs per run, newest pairs first.
   Entries past `review_after` are stale candidates. Pairs already settled
   (rejected or approved) are never proposed again.
2. **Judgement.** The `consistency` model (Haiku) labels each pair
   `contradicts`, `refines` or `unrelated`, with a reason and a verbatim
   quote from each side. A label survives only if both quotes pass the
   verbatim check.
3. **Proposals.** `contradicts` and `refines` become `supersedes`
   proposals (the newer entry supersedes the older by default; the model
   may not reverse the order); stale candidates become `stale` proposals.
   Nothing changes on an entry yet.

Daniel sees open proposals at session start (the `SessionStart` hook returns
`scribe: 3 proposals to review — scribe proposal list`) and settles each in a
typed turn: `#scribe approve p-4` sets the older entry `superseded` (or
`retired` for a stale proposal) and records `superseded_by`; `#scribe reject
p-4` closes it for good. `#scribe retire <entry-id>` hides an entry directly;
`#scribe forget <entry-id>` blanks its text for good. Agents cannot settle
proposals: settling is a `#scribe` instruction, which only a user-typed turn
carries. The orbit-office contradiction detector
(`app/packages/server/src/features/scribe/contradictions.ts`) contributes the
shape — newer side by date, held items, a resolve step — but not its
inputs, which were Orbit tasks fed by a ledger extractor that no longer
exists here.

## 8. Models

| Use | Config key | Default | Called from | Budget |
|---|---|---|---|---|
| Referent of "this"/"it" | `models.referent` | `claude-haiku-5-5` | `UserPromptSubmit` hook | 20 s |
| Session pass | `models.session` | `claude-haiku-5-5` | `scribe.session-pass` job | 120 s |
| Consistency judgement | `models.consistency` | `claude-haiku-5-5` | `scribe.consistency-scan` job | 120 s per batch of 10 pairs |
| `ask` | `models.ask` | `claude-sonnet-5-5` | `scribe.ask`, `scribe question ask` | 90 s |

All four go through one `Models` port with one adapter, `ClaudeCliModels`:
it runs `claude -p --model <id> --output-format json --json-schema <file>`
with the prompt on stdin, in its own process group, from a cleared
environment holding only `PATH`, `HOME`, `LANG` and the credential names
Daniel configures (`CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`;
STD-05 R10), killed and reaped at the budget (STD-03 R11–R13). The model
never receives tools. Why the CLI rather than an HTTP client:
[4_decisions.md](./4_decisions.md#models-are-called-through-the-claude-cli).
Tests use a fake `Models` implementation; no test calls a model.

Mutating commands — every `hook` event that writes, `session pass`,
`consistency scan` — are audited through the template's `AuditGuard`. Audit
lines hold ids and error codes only; `said`, `content` and model text never
enter the audit log.

## 9. Concerns & Honest Limitations

- **Provenance is an accident guard.** Any process running as Daniel can
  pipe forged JSON into `scribe hook user-prompt-submit` or edit `~/.scribe`.
  The transcript checks stop a confused agent, not a hostile one (STD-05 R5).
  Agents have shell access, so "main agents see only `ask`" holds for the MCP
  surface, not for the shell.
- **Claude Code hook details are unverified.** The exact `UserPromptSubmit`
  output fields (`systemMessage`, `additionalContext`, `decision: "block"`),
  and whether `/scribe` reaches the hook without a command file, are
  confirmed by S2/S4 against the installed Claude Code; if `/scribe` needs a
  command file, S4 adds one whose only effect is the same hook call.
- **One host, one store.** Sessions on the Mac and on dk-server-1 write
  separate `~/.scribe` stores; `ask` on one host does not see the other.
  Sync is open ([3_vision.md](./3_vision.md)).
- **Lexical retrieval misses paraphrase.** "merge strategy" will not find an
  entry that only says "squash". BM25 over a few thousand short entries is
  the start; [3_vision.md](./3_vision.md) records when to revisit.
- **The model sees Daniel's records.** Entries and session summaries go to
  Anthropic through the Claude CLI, as the sessions themselves already do.
  Nothing else leaves the host.
- **Cadence depends on Orbit.** With the session-pass routine disabled,
  sessions are marked due but never summarised; `scribe session list` shows
  `due` so the gap is visible.
- **A guessed referent is still an entry.** When Haiku fails, the fallback
  records the previous assistant message; the echo says so, but an
  unattended guess stays until Daniel retires it.
- **Whole-file rewrites.** Each tag is one file rewritten per entry; fine
  for thousands of entries per tag.
- **Stale is a date, not a judgement.** `review_after` flags age; whether a
  preference still holds is Daniel's call through the proposal.

## 10. Slice plan

Each slice is one Orbit task on `agent-main`, in order; each must pass
`make ci-fast`, `make ci-lint`, `make goldens` and the tests it adds, and
update these docs and [specs/](./specs/) for what it builds.

**S1 — Domain and store.** Replace the note example with entries, tags,
session records, control state and proposals, and the read-only CLI.

- `note.rs`, `query.rs`, the `note` noun, `notes.json` and their tests and
  goldens are gone; `entry`, `tag`, `session`, `verbatim` and `store` exist
  as in [§2](#2-layering)–[§3](#3-storage) and [§6.4](#64-the-verbatim-check).
- `scribe entry list [--tag <tag>] [--status <status>]`, `entry show <id>`,
  `session list`, `session show <id>`, `proposal list`, `tag list` work in
  table, plain and `--json` modes against a fixture store, and against a
  read-only root.
- A store with a newer `format`, a group-writable root or a linked file is
  refused with the template's errors; golden tests cover every help page.
- The redactor masks a configured credential value in every free-text
  field it covers, and a unit test pins that inventory.
- `specs/store.md` and `specs/output-contract.md` describe the new
  documents and payloads; `ARCHITECTURE.md`, the dependency script and the
  code section of `AGENTS.md` (the `note.rs`/`query.rs` and `notes`
  envelope wording) and `README.md` describe the new modules and envelopes.

**S2 — `#scribe` capture (no model).** The `UserPromptSubmit` hook and the
Claude Code package.

- `scribe hook user-prompt-submit` with a fixture hook payload writes exactly
  one entry per `#scribe` line for forms 1 and 2, with `said`, `content`,
  `cite` and an echo carrying tag, text and id.
- Lines in fences, blockquotes and `<pasted_content>`, and bare `#decision`
  markers, write nothing; an unknown tag is refused with the configured
  list; replaying the same payload writes nothing new.
- Every write and refusal is audited with ids and error codes only.
- `plugin/claude-code/` installs locally and a manual check in a live
  Claude Code session confirms the reply is shown to Daniel and the agent;
  the result is recorded in this doc ([§9](#9-concerns--honest-limitations)).
- `scribe hook stop` fills the cite's message uuid from the transcript.

**S3 — Referents and the models port.** Form 3 and `ClaudeCliModels`.

- With a fake `Models`, `#scribe add this to decisions` records the chosen
  span with `content_from.role`; a span failing the verbatim check, a
  timeout and a model error each still write exactly one entry, the
  fallback, with an echo that says it was guessed.
- `ClaudeCliModels` launches from a cleared allowlisted environment in its
  own process group and is killed and reaped at its budget (tested with a
  fake `claude` script on `PATH` under a temp `HOME`).

**S4 — `/scribe on` and session records.**

- `/scribe on|off|status` updates `control.json`, blocks the prompt and
  reports status; `on` twice is a no-op that says so.
- `scribe hook stop` marks due by minutes or turns; `session-end` marks
  final; neither calls a model.
- `scribe session pass` with a fake `Models` writes one record per due
  session within budgets, advances `scribed_through`, writes no entries,
  and keeps the previous record when the model output is invalid.
- A session never switched on is never read (a test transcript for it is
  untouched and absent from the store).

**S5 — The Orbit plugin tree and `ask`.**

- `plugin/.orbit-plugin/` passes `orbit plugin validate --render` and
  `orbit plugin test`; a conformance test asserts `scribe.ask` is the only
  tool with an `mcp_scope` other than `none`.
- With a fake `Models` and fixture tasks, `ask` returns `no_record` without
  a model call when nothing matches, drops claims whose quote or cite fails
  and counts them, returns `conflict` with both cites for two disagreeing
  entries, and excludes hidden entries unless `include_hidden`.
- `scribe question ask "<question>" --json` and the `scribe.ask` tool
  return the same payload for the same store.
- "can you fetch me last session summary?" returns `"status": "record"`
  with the newest session record in scope, its fields byte-identical to the
  stored record, and makes no model call; a non-lookup question still goes
  through retrieval and answering.

**S6 — Consistency routine.**

- `scribe consistency scan` with a fake `Models` turns a contradicting pair
  into one `supersedes` proposal, a stale entry into one `stale` proposal,
  and never re-proposes a settled pair.
- `#scribe approve p-<n>` marks the older entry `superseded` with
  `superseded_by`; `reject` closes the proposal; `retire` and `forget`
  behave as [§3](#3-storage) says; none is reachable from any Orbit tool.
- The `SessionStart` hook reports the open-proposal count; the two routines
  ship seeded disabled.

**Outside this repository.** Retiring orbit-office's scribe (its hooks,
`office.scribe-*` tools and `/scribe` command) is orbit-office work, filed
there once S4 is in use, so two scribes never intercept the same
`/scribe` prompt.

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-13117] — built the Rust CLI template this project was created from.
- [ORB-13134] — brought the template to STD-01..03 v2 and STD-04/05 v1.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
