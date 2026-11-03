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
and the slices that build it. Slices S1 (the domain, the store and the
read-only CLI) and S2 (`#scribe` capture without a model, and the Claude Code
package) are implemented; the rest is not yet. The reasons behind each choice
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
`type: exec`, `timeout_ms: 720000`: the routine tools may need 600 s (five
sessions at 120 s) and 480 s (four consistency batches at 120 s), and `ask`
is bounded by its own 90 s budget. Tools:

| Tool | `execution_kind` | `agent_callable` | `mcp_scope` |
|---|---|---|---|
| `scribe.ask` | `read_only` | — | `global` |
| `scribe.session-pass` | `mutating` | `false` | `none` |
| `scribe.consistency-scan` | `mutating` | `false` | `none` |

Orbit defaults `mcp_scope` to `workspace`, so every tool except `ask` sets
`none` explicitly, and a conformance test in `plugin/.orbit-plugin/tests/`
asserts that `ask` is the only advertised tool. Main agents therefore see
exactly `mcp__orbit__scribe_ask` once orbit-office's scribe is retired (see
*Outside this repository* in [§10](#10-slice-plan)); until then its
`office.scribe-*` tools are on the list too. `ask` is `global` because the
store is Daniel's, not a workspace's. The two mutating tools run only from the
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
As built in S2, it registers `UserPromptSubmit` (30 s) and `Stop` (10 s)
only; `SessionStart` and `SessionEnd` join with the slices that give them
something to do (S4, S6), since a registered hook whose command does not
exist fails on every session. `plugin/.claude-plugin/marketplace.json` is a
one-plugin local marketplace, so the package installs from a checkout with
`claude plugin install scribe --marketplace ./plugin`.

Entry writes never go through Orbit's tool surface: the hooks call the
binary. The two routine tools write session records and proposals, but they
are `mutating` and not agent-callable, so no tool needs `agent_callable` or an
operator override, and the MCP list stays at one tool.

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
| `proposal` | `Proposal`, its id, kind, state and quotes | none |
| `record` | the metadata every record shares: `Project`, `Topic`, `Related`; list selection (`Listing`) | none |
| `config` | parsing `config.toml` text into tags, models, cadence and redaction settings | none (takes text) |
| `transcript` | parsing a Claude Code transcript (JSONL) into typed messages, dropping tool payloads and synthetic turns | none (takes bytes) |
| `verbatim` | the one quote check ([§6.4](#64-the-verbatim-check)) | none |
| `ask` | tokenizing, ranking, evidence packing, answer validation | none |
| `consistency` | candidate pairing, staleness, proposal validation | none |
| `models` | the `Models` port (a trait) and the typed requests/responses for each use | none |
| `store` | the `Store` facade over `format`, `audit`, `fsio`, `redact`; the only disk access | disk |

The CLI crate adds `commands/{entry,session,proposal,tag,question,consistency,plugin}.rs`
and `commands/hook/` (one file per event), plus two
adapters composed in `app.rs`: `ClaudeCliModels` (the `Models` port,
[§8](#8-models)) and `OrbitTasks` (task evidence for `ask`). Adapters that
spawn processes live in the CLI crate so the core stays free of
`std::process`, as the dependency-direction script already requires. The
adapters are a new module family; the slice that adds the first one adds
them to `ARCHITECTURE.md` and the script. S1 built `entry`, `tag`,
`session`, `proposal`, `record`, `config`, `verbatim` and `store` and the
`entry`, `session`, `proposal` and `tag` commands. S2 built `capture`,
`transcript`, the store's first writes (`Store::capture`,
`Store::fill_cite_messages`) and `commands/hook/` (`user_prompt_submit.rs`,
`stop.rs`, plus `payload.rs` for the stdin shapes and the provenance check
and `project.rs` for the project lookup).

## 3. Storage

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`, as in the template. The
hooks, the CLI and the Orbit backend resolve the same root, so one store
serves every workspace on the host. The plugin's config key `root` (default
`~/.scribe`) is passed to the backend as `--root`.

```
~/.scribe/                       0700, owner-checked on every load
├── config.toml                  tags, models, cadence (Daniel edits; read-only to scribe)
├── entries/<tag>/<id>.md        one file per entry: YAML frontmatter + verbatim body
├── sessions/<yyyy>/<mm>/<session-id>.md   one per opted-in session: frontmatter + sections
├── discussions/<yy-mm>/<slug>.md   one per distilled discussion (§4.5), Daniel may edit
├── control.json                 {"format":1,"sessions":{"<id>":{"on","since","due","final"}}}
├── proposals.json               {"format":1,"proposals":[…]}
├── audit.jsonl                  one line per mutating command (template shape)
└── .lock
```

Every file except `audit.jsonl` is replaced whole through
`fsio::write_atomic` under `.lock`; nothing is appended or edited in place.
A `#scribe` creates one new entry file; a session pass replaces one session
file.
Reads take no lock and write nothing (STD-01 R31), so `ask` works on a
read-only root. Each document carries `format: 1`; the template's
`notes.json` format is not carried over because no scribe store exists yet
(S1 starts `UPGRADES` empty for the new documents).

**Metadata.** Entries and session records are Markdown files whose YAML
frontmatter is the metadata `ask` and the CLI filter on, so a lookup narrows
by fields before any ranking. Every record carries:

| Field | Meaning |
|---|---|
| `format`, `kind` | `1`; `entry` or `session` |
| `id` | `decisions-12`, or the session id |
| `status` | entry: `active`, `superseded`, `retired`, `forgotten`; session: `open`, `final` |
| `project` | the repository slug from the cwd's git remote, else the cwd's basename; `global` when Daniel says so |
| `topics` | 0–5 lowercase kebab-case labels (`ci`, `crew-routing`, `release`) |
| `related` | references the record mentions: `task:ORB-123`, `pr:<owner>/<repo>#7`, `repo:<slug>`, `session:<id>` |
| `terms` | sessions only: phrases Daniel coins or defines, each `{term, said, message}` where `said` is a verbatim span of Daniel's own turn that defines or first uses it |
| `provider`, `host` | where it was recorded |
| `created`, `updated` | RFC 3339 UTC |

`topics` are filter labels and are distinct from a tag, which names where an
entry goes (`decisions`, `preferences`). An entry's topics are the extra
hashtags on its `#scribe` line (`#scribe add this to decision #ci`) and
nothing else, since capture calls no model. A session's topics come from
the `session` model at each pass, which is given the existing topic list and
told to reuse it before coining one. The consistency routine proposes
merging near-duplicate topics (`ci` and `continuous-integration`) as it does
superseding entries; nothing is renamed without Daniel's approval.

**Entry** (`entries/<tag>/<id>.md`):

```markdown
---
format: 1
kind: entry
id: decisions-12
tag: decisions
status: active
project: orbit
topics: [merge-policy]
related: [task:ORB-123]
provider: claude-code
host: dk-server-1
created: 2026-11-02T19:05:41Z
updated: 2026-11-02T19:05:41Z
said: "#scribe add this to decision #merge-policy"
content_from: {kind: message, role: assistant, message: <uuid>, at: 2026-11-02T19:04:11Z}
cite: {session: <session-id>, message: <uuid>, at: 2026-11-02T19:05:40Z, prompt_sha256: "…", line: 3, cwd: "…"}
superseded_by: null
review_after: 2027-05-01
---
squash-merge agent PRs; keep merge commits for releases
```

The body is `content`, verbatim.

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

**Session record** (`sessions/<yyyy>/<mm>/<id>.md`): the common
frontmatter plus `title`, `cwd`, `started`, `on_since`, `scribed_through`
(the last transcript message folded in), `passes` (count) and `next_open`
(the number of `next` items); `status: final` after the session-end pass.
The body has four sections, `## Summary`, `## Goals`, `## Outcome` and
`## Next` (character budgets 600/400/1200/600). The four text fields are model-written
summaries, labelled as such in `ask` evidence, never quoted as Daniel's words.

**Discussion** (`discussions/<yy-mm>/<slug>.md`): the distilled record of
a conversation that concluded something, in the shape of Daniel's almanac
discussions (`almanac/15-discussions/`). Frontmatter: the common fields with
`kind: discussion`, plus `title`, `status` (`seed`, `growing`, `settled`),
`sources` (the `session:<id>` and `message` ids each point came from) and
`terms`. Body, in order: a thesis paragraph; `## The arc`, numbered
`### <n>. <claim>` points, each opening with one blockquoted verbatim quote
of Daniel (`> "…" — Daniel`) followed by one interpreting paragraph, a
change of mind headed `Correction #<n> —`; `## Through-lines`;
`## The one-line takeaway`; `## References` (sessions, tasks, related
discussions as `[[slug]]`). The quotes are Daniel's words; everything else
is the model's interpretation and is labelled so in `ask` evidence.

**Proposal** (`proposals.json`): `id` (`p-<n>`), `kind` (`supersedes` or
`stale`), `older`, `newer` (entry ids), `reason` (model text, at most 300
characters), `quotes` (verbatim spans from both entries), `state`
(`open`, `approved`, `rejected`), `opened_at`, `settled_at`, `settled_by`
(a `cite` of Daniel's approving turn).

**Config** (`config.toml`): `[tags.<name>]` with `aliases`, `description`
and `stale_after_days`; `[sources]` with `discussions_readonly`, a list of
directories `ask` reads as extra discussion evidence and never writes
(default: the constellation almanac's `15-discussions`, when present); `[models]` with `ask`, `distill`, `session`, `consistency`,
`referent` and `credentials` (the variables model calls are given, default
`CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`); `[catalog]` with
`every_minutes` (20) and `every_turns` (10); `[redact]` with `env`, further
variables whose values are masked. A missing file or table is the defaults;
an unknown key or bad value is refused, naming the file and line
([specs/store.md](./specs/store.md#config)).
The file is created by `scribe hook session-start` on first run with the
defaults ([4_decisions.md](./4_decisions.md#only-decisions-and-preferences-ship-by-default))
if absent; scribe never rewrites it after that.

**Redaction.** Before any write of `said`, `content`, a session record's
title, text fields and terms, or a proposal's `reason` and quotes, the store masks
the live values of the variables in `[redact] env` and `[models] credentials`
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

As built (S2): the transcripts directory is `$CLAUDE_CONFIG_DIR/projects`,
else `$HOME/.claude/projects`. The path must be absolute, without `.` or `..`
components, inside that directory and named `<session_id>.jsonl`; when the
file exists it must be a regular file (not a link) owned by this uid. A
missing file passes, because a session's first prompt can reach the hook
before Claude Code has written the transcript. A failed check records
nothing; each `#scribe` line gets a `could not record` reply and an audit
line with `error_code: provenance_failed`. The `Stop` hook first checks,
reading only, whether any entry of the session still has `message: null`;
when none does (the usual case) it returns without opening the transcript,
locking or writing. Otherwise it parses the transcript
([§2](#2-layering), `transcript`: user and assistant text only, tool
payloads, sidechain and meta lines dropped), hashes each user message's whole
text and each of its text blocks, and gives each waiting cite the uuid of the
matching message written closest to the cite's `at`; a cite whose prompt is
not in the transcript yet stays `null` for the next `Stop`.

### 4.2 Parsing an instruction

Model-free grammar, tried in order:

1. `#scribe <tag>: <content>` — explicit tag, content after the colon.
2. `#scribe [add|record|note] <content> (to|in|under) [my|the] <tag>` — the
   tag is the last word group, matched against names and aliases. `<content>`
   is never a bare `this`, `that`, `it` or `these`; those lines are form 3.
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

As built (S2): hashtags at the end of the line, after the tag, are the
entry's topics, lowercased and deduplicated (more than five is refused as
`unparsed_instruction`); they are not part of `content`. Forms 3 and 4, and
`#scribe distill …` ([§4.5](#45-distilling-a-discussion)), are recognised
and refused with `error_code: unsupported_instruction` until S3, S6 and S7
build them, so they are never misread as form 2; a form-3 line whose tag is
unknown is refused as `unknown_tag` first, as in the reply example below.
The tag word of form 2 may end in `.`, `!`, `,` or `;`.

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

As built (S2): the reply is one line of hook JSON,
`{"systemMessage": …, "hookSpecificOutput": {"hookEventName":
"UserPromptSubmit", "additionalContext": …}}`, with one echo line per
`#scribe` line in prompt order; a prompt without one prints nothing. A
replayed line echoes `scribe: already recorded <id> in <tag> — "…"`. When
the store or `config.toml` cannot be used, each line echoes `scribe: could
not record "…" — <error>` and the hook still exits 0, so the prompt goes
through. Quotes in an echo are flattened to one line and cut at 200
characters (`scribe entry show <id>` has the whole text). The `Stop` hook
prints nothing.

### 4.5 Distilling a discussion

Two more `#scribe` forms, explicit like the rest; nothing is distilled
unless Daniel asks.

- `#scribe distill this discussion [as "<title>"]` reads the current
  session's turns since the last distill point (or the whole session) and
  writes a new discussion with `status: growing`.
- `#scribe add this to <slug or title>` reads the turns since that
  discussion was last updated in this session and extends it.

The `distill` model (the decision model; distilling is judgment) returns
the thesis, arc points with quotes, through-lines, takeaway, topics and
terms as JSON. Every quote must pass the verbatim check against one of
Daniel's own turns; a point whose quote fails is dropped and counted in the
echo. Related discussions are proposed from topic and term overlap with the
store and the read-only sources.

Extending never rewrites what exists, because Daniel may have edited the
file: new arc points are appended after the last numbered point,
`sources`, `terms`, `topics` and `updated` are merged, and the thesis,
through-lines and takeaway are left as they are unless the extension adds a
`Correction`, in which case the echo says the thesis may need Daniel's edit.
The echo names the file path, the points added and any dropped quotes.

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
   `session` model (Haiku) for `{title, summary, goals, outcome, next,
   topics, related, terms}`, giving it the store's existing topics and terms
   to reuse. Each term's `said` must pass the verbatim check against one of
   Daniel's own turns (never an assistant turn); a term that fails is
   dropped, the rest of the record is kept.
3. Validate the budgets and the JSON shape; on failure, keep the previous
   record and leave `due` set for the next run (three failures in a row
   clear `due` and log a `warn`).
4. Write the record atomically, advance `scribed_through`, clear `due`.

A pass writes no tag entries, even if the transcript contains `#decision`
or an unprocessed `#scribe` line. There is no backfill: sessions that were
never switched on are never read.

## 6. `ask`

### 6.1 Contract

Input: `{"question": string, "scope"?: {"tags"?: [..], "topics"?: [..],
"project"?: string, "kind"?: "entry" | "session", "since"?: date},
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

0. Filter by frontmatter first. `scope` fields (`project`, `topics`,
   `tags`, `since`, `kind`) and any project name, topic or date the
   question names ("decisions on ci this month") restrict the candidate
   records before ranking.
1. Load every `active` entry (plus `superseded`/`retired` when
   `include_hidden`), every session record, every discussion (the store's
   and the read-only sources'), and up to 20 Orbit tasks from
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
becomes `\n`, and runs of whitespace collapse to one space; the quote's own
leading and trailing space is dropped, so a span cut at a line break still
matches. The quote must then be at least 8 characters and a substring of
the source. No case
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
| the latest/last/previous session (optionally for a project or topic) | the newest session record matching that frontmatter, by `updated` |
| a named session (`session:<id>`, or a title match) | that session record |
| open next steps / what is pending | the `next` items of the newest records in scope, newest first |
| the entries of a tag in a period ("decisions this week") | those entries, newest first |
| what was concluded about a subject ("what did we conclude about level 4?") | the discussions whose title, topics or terms match, newest `updated` first, with thesis and takeaway verbatim; the store's before the read-only sources' |
| a term Daniel uses ("graduate to level 4", "what did I mean by resident orchestrator") | the session records whose `terms` contain it (case and whitespace normalised), newest first, each with the defining `said` span, plus entries whose body contains it |
| the last orchestrator or on-call session for a workspace | the newest `orchestration`-tagged assignment task in `ws_constellation` whose title names that workspace, with its execution summary verbatim, cited `task:<id>` |

Terms exist because Daniel's main conversational agent is a manager he
discusses designs and ideas with, and his vocabulary carries meaning across
those sessions: when he says "graduate to level 4", any agent can `ask` for
the session where he defined it instead of guessing. A summary written by
Haiku may paraphrase a term away, so terms are kept in Daniel's exact words.

A lookup returns `"status": "record"` and `records` with each record's
stored fields verbatim (summary, goals, outcome, next, timestamps, cite),
without a model call and without paraphrase. A question that is not a
lookup, or a lookup that matches nothing, falls through to §6.2. When the
classifier is unsure it falls through too, so a lookup never hides an
answer.

## 7. Consistency routine

The `scribe-consistency` routine (`0 6 * * 1`, weekly, seeded disabled)
runs `scribe.consistency-scan`. First it re-applies each approved proposal
whose older entry is still `active`: approval writes two files, and this
repairs a crash between them
([4_decisions.md](./4_decisions.md#one-markdown-file-per-record-metadata-in-yaml-frontmatter)). Then:

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
| Distilling a discussion | `models.distill` | `claude-sonnet-5-5` | `UserPromptSubmit` hook, run as a detached job like a session pass | 180 s |

All five go through one `Models` port with one adapter, `ClaudeCliModels`:
it runs `claude -p --safe-mode --tools "" --model <id> --output-format json --json-schema <schema-json>`
with the prompt on stdin, in its own process group, from a cleared
environment holding only `PATH`, `HOME`, `LANG` and the credential names
Daniel configures (`CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`;
STD-05 R10), killed and reaped at the budget (STD-03 R11–R13).
`--safe-mode` turns off the hooks and plugins of the user's Claude Code setup,
so a model call never fires `scribe hook`; `--tools ""` removes the built-in
tools, so the model never receives tools. `--bare` is not used: it reads only
`ANTHROPIC_API_KEY`, and Daniel's login is OAuth. S3 checks both flags against
a real `claude -p` run. Why the CLI rather than an HTTP client:
[4_decisions.md](./4_decisions.md#models-are-called-through-the-claude-cli).
Tests use a fake `Models` implementation; no test calls a model.

Mutating commands — every `hook` event that writes, `session pass`,
`consistency scan` — are audited through the template's `AuditGuard`. Audit
lines hold ids and error codes only; `said`, `content` and model text never
enter the audit log. A hook run writes one line per entry written
(`target` the entry id), per refusal and per failed line (`target`
`session:<id>`, with its `error_code`) and per cite filled; a run that
changed nothing (no `#scribe` line, a replay, a `Stop` with nothing waiting)
writes none.

## 9. Concerns & Honest Limitations

- **Provenance is an accident guard.** Any process running as Daniel can
  pipe forged JSON into `scribe hook user-prompt-submit` or edit `~/.scribe`.
  The transcript checks stop a confused agent, not a hostile one (STD-05 R5).
  Agents have shell access, so "main agents see only `ask`" holds for the MCP
  surface, not for the shell.
- **Claude Code hook details are partly verified.** S2 confirmed against
  Claude Code 2.1.317 on dk-server-1 that `plugin/claude-code/` validates
  (`claude plugin validate`, also `--strict`), installs from the local
  marketplace into a throwaway `HOME`, and is listed with two hooks,
  `UserPromptSubmit` and `Stop`. Whether the reply's `systemMessage` reaches
  Daniel and its `additionalContext` the agent needs a live session and is
  the S2 manual check below; `decision: "block"` and whether `/scribe`
  reaches the hook without a command file are S4's to confirm; if `/scribe`
  needs a command file, S4 adds one whose only effect is the same hook call.
- **S2 manual check in a live session: pending (operator, ORB-21760).** The
  leaf task that built S2 runs in a sandbox without an interactive Claude
  Code session, so the check is the operator's. Steps: `make install`;
  `claude plugin install scribe --marketplace ./plugin` from this checkout;
  start a new `claude` session in any repository; type a prompt with the
  line `#scribe decisions: S2 live check` plus a question on the next line;
  confirm the `scribe: added decisions-<n> to decisions — "S2 live check"`
  line is shown in the terminal (Daniel) and that the agent's answer shows
  it saw the same text (ask it to repeat what scribe said); after the reply,
  `scribe entry show decisions-<n> --json | jq .cite.message` is a uuid, not
  `null`; then `#scribe add it to rituals` shows the refusal listing the
  configured tags. Record the result, the Claude Code version and the date
  here, replacing this item.
- **The prompt hash must match the transcript's text.** The `Stop` hook
  finds the prompt by hashing the transcript's user text. If Claude Code
  records a prompt differently from what it passed the hook (expanded
  pasted blocks, attachments), the cite stays `message: null`; the entry
  is still written and cited by session, line and hash.
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
- Entries and session records are Markdown files with the frontmatter in
  [§3](#3-storage); a record with missing or unknown frontmatter keys is
  refused, naming the file.
- `scribe entry list [--tag <tag>] [--status <status>] [--project <p>]
  [--topic <t>]`, `entry show <id>`, `session list [--project <p>]
  [--topic <t>]`, `session show <id>`, `proposal list`, `tag list` work in
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
- The argv carries `--safe-mode` and `--tools ""` (asserted on a fake `claude`
  that records its argv), and a manual `claude -p` run with those flags is
  recorded in [§9](#9-concerns--honest-limitations).

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
- A pass keeps a term only when its `said` is a verbatim span of one of
  Daniel's turns; a term quoting an assistant turn is dropped.

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
- "graduate to level 4" returns the session records whose `terms` hold that
  phrase with their defining spans, and "last on-call session for orbit"
  returns the newest matching assignment task's execution summary verbatim,
  both without a model call.

**S6 — Consistency routine.**

- `scribe consistency scan` with a fake `Models` turns a contradicting pair
  into one `supersedes` proposal, a stale entry into one `stale` proposal,
  and never re-proposes a settled pair; and a scan re-applies an approved
  proposal whose older entry is still `active`.
- `#scribe approve p-<n>` marks the older entry `superseded` with
  `superseded_by`; `reject` closes the proposal; `retire` and `forget`
  behave as [§3](#3-storage) says; none is reachable from any Orbit tool.
- The `SessionStart` hook reports the open-proposal count; the two routines
  ship seeded disabled.

**S7 — Discussions.**

- `#scribe distill this discussion as "<title>"` with a fake `Models` writes
  one discussion in the [§3](#3-storage) shape whose every quote passes the
  verbatim check against Daniel's turns; a point quoting an assistant turn
  is dropped and counted in the echo.
- `#scribe add this to <slug>` on a discussion Daniel edited by hand
  appends new points after the last one and leaves every existing line
  byte-identical.
- `ask` answers "what did we conclude about <subject>" from a store
  discussion and from a fixture `discussions_readonly` directory, and scribe
  never writes into that directory.

**Outside this repository.** Retiring orbit-office's scribe (its hooks,
`office.scribe-*` tools and `/scribe` command) is orbit-office work, filed
there once S4 is in use, so two scribes never intercept the same
`/scribe` prompt.

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-13117] — built the Rust CLI template this project was created from.
- [ORB-13134] — brought the template to STD-01..03 v2 and STD-04/05 v1.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
