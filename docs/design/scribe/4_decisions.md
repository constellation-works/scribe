---
title: scribe — Decisions
owner: claude
last_updated: 2026-11-22
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: decisions
type: design
summary: Why superseded entries are hidden not removed, why one JSON file per tag and per session with no persisted index, why no migration from orbit-office and only two default tags, why hooks call the binary and agents see only ask, why models run through the claude CLI and which model does what, why session passes run from an opt-in routine or by hand with bounded windows and retries, why frontmatter is parsed by yaml_serde, why a session record is a rewritable header over an append-only log of segments, why sessions carry an active flag deactivated mainly when their feature is removed and why only Daniel-authored entries count as removal evidence, why agents read and write through the CLI under one skill with authors recorded and Daniel's entries left to him, why tags agents create live in the store, why a tmux popup rather than an app, and why backfill catalogs only sessions Daniel names, why action tags are config-marked with a capped window, a cancel window, pickups, a stuck line, a CLI quote check and SSH reads from the owner host, why scribe keeps a descriptive memory the session pass writes without approval, why that memory has a personal base fed by every typed session (with people, recurring dates and light catalog entries) and a work layer fed only by /scribe on sessions, why scribe reads Claude Code and Codex sessions only (no hosted server, app connector or ChatGPT import), why the personal base, people, recurring dates and light catalog entries are parked, why the session pass writes Daniel's ideas itself with their outcome, why #scribe becomes an annotation anywhere in a typed turn cataloged at the end of the turn with a statement and his verbatim quote (proposed), why tags become configurable catchers, memory a fixed three-part synthesis and the rest one SQLite database, with approved entries in SQLite, `issue` a catcher tag rather than a record kind, only cited assistant turns stored, and the meaning landing before the storage (proposed), plus the engineering decisions carried over from the Rust CLI template.
tags: [scribe, decisions]
paths: ["Cargo.toml", "crates/**", "plugin/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21753, ORB-21764, ORB-21778, ORB-21784, ORB-21787, ORB-21795, ORB-23829, ORB-13117, ORB-13134, ORB-23849, ORB-23892, ORB-24005, ORB-24006, ORB-25080, ORB-25112, ORB-25977, ORB-25980, ORB-26214, ORB-26219, ORB-26240, ORB-26256, ORB-26349, ORB-26360, ORB-26405, ORB-26265, ORB-26510, ORB-26529, ORB-26640, ORB-26650, ORB-26709]
---

# scribe — Decisions

Record non-obvious decisions here by title. Task references carry provenance; superseded decisions remain in place so their original reasoning stays legible. See [CONVENTIONS.md §3](../CONVENTIONS.md#3-what-earns-a-decision-entry) for the admission rule and required `Cost:` line.

The plugin decisions come first. Each one marked **Overridable by Daniel**
was a choice among real alternatives in his directions
([1_overview.md §1](./1_overview.md#1-directions-daniel-2026-11-02)); he
can reverse it by saying so, and the change is recorded here as a new entry
that supersedes the old one. The template's engineering decisions follow,
restated for the plugin. Decisions that only concerned the template's note
example (an empty list printing `[]`, `note add` naming its file on stderr)
were dropped with [ORB-21747]; they are in git history at `c9807fa`.

## Superseded and stale entries are hidden, not removed

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

Daniel's directions ask for "a routine that checks for inconsistencies,
outdated record removal". Deleting an entry when a newer one supersedes it
loses the history of how a preference changed, and a wrong supersession
(the routine's model misjudging a pair) could not be undone. Keeping
everything visible makes `ask` quote positions Daniel has abandoned.

### Decision

Nothing is deleted automatically. An approved `supersedes` proposal marks
the older entry `superseded` with `superseded_by`; an approved `stale`
proposal or `#scribe retire <id>` marks it `retired`. Both stay on disk and
are excluded from `ask` evidence unless the caller passes `include_hidden`;
an answer that matched a hidden entry says a superseded record exists. Real
removal happens only on Daniel's explicit `#scribe forget <id>`, which
blanks `said` and `content` and keeps the id, cite and status `forgotten`,
so citations elsewhere do not dangle. "Removal" in the directions is read as
removal from answers.

### Consequences

- A wrong approval is reversible by hand, and `ask --include-hidden` can
  answer "what did Daniel used to prefer".
- Cost: tag files keep growing with entries nobody will be shown; and a
  reader expecting the routine to delete must learn that only `forget`
  does.

## One Markdown file per record, metadata in YAML frontmatter

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/store/format.rs::split_frontmatter`, `::decode_entry`, `::decode_session`

### Context

The store needs entries, session records, the on/off state and proposals,
written by hooks in concurrent sessions and read by `ask` from any
workspace. The first version of this design kept one JSON file per tag and
per session. Daniel, 2026-11-02: "for chat session summaries, we are
probably better off using a good metadata system in the frontmatter, e.g.
tags to filter by", and "same for decisions and preferences I suppose".
orbit-office kept Markdown in a git-backed Library, which brought a commit
per pass; SQLite would add a second persistence discipline.

### Decision

A single root, `~/.scribe` by default, shared by every workspace on the
host. Each entry is `entries/<tag>/<id>.md` and each session record is
`sessions/<yyyy>/<mm>/<id>.md`: YAML frontmatter holding the metadata
(`project`, `topics`, `related`, `status`, dates, cite) and a Markdown body
(the verbatim entry, or the four session sections)
([2_design.md §3](./2_design.md#3-storage)). `control.json`,
`proposals.json`, `config.toml`, `audit.jsonl` and `.lock` stay as they
were. Every file is replaced whole through `write_atomic` under the one
store lock. There is no git repository: history is the audit log.

### Consequences

- `ask` and the CLI filter by frontmatter before ranking; a record is
  readable and greppable as a file, and Daniel can open one in an editor.
- A `#scribe` writes one new file and never rewrites other entries; ids are
  allocated by scanning `entries/<tag>/` under the lock.
- Cost: a YAML parser dependency
  ([chosen in S1](#frontmatter-is-parsed-by-yaml_serde-and-configtoml-by-toml));
  every load parses one file per record, which is
  fine for thousands of records and is revisited if a load exceeds a second.
- A cross-file operation (approving a proposal changes `proposals.json` and
  an entry file) is two atomic writes under one lock, not one transaction;
  a crash between them leaves an approved proposal whose entry is still
  active, which the next scan repairs by re-applying approved proposals.
- `topics` is a new metadata field distinct from tags; near-duplicate topics
  are merged only through an approved proposal.

## Discussions live in the scribe store; the almanac is read-only

**Recorded:** 2026-11 · [ORB-21747] · **Daniel's choice**
**Code anchors:** `crates/scribe-core/src/store/` (S7)

### Context

Daniel keeps selective, distilled discussions in his almanac
(`almanac/15-discussions/`, 15 of them): "I used to keep selective
discussions where we discussed something meaningful. and I can reference it
later in chats. I want scribe to fill that gap." The constellation rule is
that agents never write the almanac.

### Decision

Daniel chose, 2026-11-02: discussions scribe distills live in the scribe
store (`discussions/<yy-mm>/<slug>.md`) in the almanac's format. `ask`
reads the almanac discussions as a read-only source and never writes them.
The existing 15 are not moved.

### Consequences

- The almanac stays Daniel's alone; both sets answer the same questions.
- A discussion Daniel wants in the almanac he copies there himself.

## ask ranks in memory with no persisted index

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/ask/` (from S5)

### Context

`ask` needs model-free retrieval. A persisted index (SQLite FTS5, as
orbit-office uses for its Library, or embeddings) must be built and kept in
step with the records; building it inside `ask` would make a read write,
which STD-01@2 §R31 forbids, and keeping it current from every writer adds
a second store that can drift. Embeddings would also need a model call per
write. The expected corpus is small: hundreds of entries and session
records, growing by a few a day.

### Decision

No persisted index. `ask` loads the active records and ranks them with BM25
in memory on every call, plus up to 20 Orbit tasks fetched through
`orbit.search` at query time ([2_design.md §6.2](./2_design.md#62-retrieval-model-free)).
Revisit when a cold `ask` spends over 200 ms in retrieval or the store
passes 20,000 records.

### Consequences

- `ask` is a pure read, works on a read-only root, and can never answer
  from a stale index.
- Cost: no paraphrase or synonym matching ("merge strategy" misses
  "squash"); every call reparses every file.

## No migration of orbit-office scribe records

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

orbit-office's scribe left v1 SCR files (one line per extracted entry),
v2 SCR files (session sections plus an append-only tag list), a decision
ledger and preference pages, all in its Library on the Mac. Daniel's
directions reject the way most of them were made: "I don't think
auto-backfilling is a good idea … too noisy", and only explicitly marked
things should become entries. orbit-office itself declined a v1→v2
backfill because it would have produced about 184 preference proposals.

### Decision

scribe imports nothing. The orbit-office records stay where they are,
readable in orbit-office's Library, and `ask` does not search them. Entries
start empty; the first entries are Daniel's first `#scribe` lines.

### Consequences

- Every entry in the store is one Daniel asked for under the new rule.
- Cost: decisions Daniel already made are invisible to `ask` until he
  restates them with `#scribe`. A later opt-in importer, one proposal per
  human-decided ledger line with a verbatim quote, is a direction in
  [3_vision.md](./3_vision.md), not a commitment.

## Only decisions and preferences ship by default

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

Daniel named `#preferences` and `#decision` and said "other tags" may be
added. Shipping more defaults (say `rules`, `people`, `projects`) guesses
at categories he did not ask for and makes the refusal list longer; shipping
none makes the first `#scribe` fail.

### Decision

`config.toml` is created with two tags: `decisions` (aliases `decision`,
`dec`; never stale) and `preferences` (aliases `preference`, `pref`;
`stale_after_days = 180`) (amended 2026-11-20 by "Decisions are recorded on
their Orbit task, not in a shipped tag": `decisions` no longer ships).
Further tags are added by Daniel editing the
config; scribe never creates a tag on its own, and an instruction naming an
unknown tag is refused with the list.

### Consequences

- The refusal message is short and the tag list is Daniel's.
- Cost: a new tag needs a file edit rather than a `#scribe` instruction.
- Amended 2026-11-10 ([ORB-23892]): agents may now create tags with
  `scribe tag add`; they live in the store, not the config, and config tags
  win. See
  [agent-created tags live in the store](#agent-created-tags-live-in-the-store-and-config-tags-win).
  The two defaults and "scribe never creates a tag on its own" stand:
  scribe's own model work still creates none.

## Hooks call the scribe binary directly, not Orbit

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `plugin/claude-code/hooks/hooks.json`; `crates/scribe/src/commands/hook/` (from S2)

### Context

Writes must come only from provider hooks. Orbit's plugin manifest has no
hook or slash-command key, so hooks ship as a Claude Code package either
way. If they wrote through Orbit (`orbit tool run scribe.append`), the
write tool would need either `agent_callable: true` with an `agent_tools`
grant — which makes it callable by any agent — or an operator override in
the hook's environment (STD-05@1 §R4). orbit-office took the first route and
its write tools ended up on the MCP list.

### Decision

The Claude Code package runs `scribe hook <event>` on `PATH`. Orbit hosts
only `scribe.ask` and the two routine tools. No tool in Orbit's registry
writes an entry: the routine tools write session records and proposals and
are not agent-callable (see the `ask` decision below), so there is no entry
write for an agent to be granted.

### Consequences

- The MCP list is one tool, and no Orbit grant can widen the write path.
- Cost: the binary must be on `PATH`. The plugin's `bin/scribe` launcher
  execs that same binary, so hooks, routines and `ask` share one version;
  the hooks work without Orbit, the session passes and `ask` do not.
- Amended 2026-11-10 ([ORB-23892]): "Writes must come only from provider
  hooks" no longer holds. Daniel's entries still come only from his typed
  `#scribe` lines, and still never through Orbit; from S12 agents also
  write, through the CLI, with `author.kind: agent`
  ([agents read and write through the CLI](#agents-read-and-write-through-the-cli-taught-by-one-skill)).
  No Orbit tool gains a write.

## ask is the only tool advertised to agents

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `plugin/.orbit-plugin/plugin.yaml` `spec.tools`; `plugin/.orbit-plugin/tests/conformance/` (from S5)

### Context

Daniel: "I would rather keep mcp surface small for my main agents - they
can just have \"ask\" mcp." Orbit advertises a plugin tool at `workspace`
scope unless told otherwise, and hiding a tool is not a permission check
(STD-05@1 §R3).

### Decision

`scribe.ask` is `read_only` with `mcp_scope: global`. `scribe.session-pass`
and `scribe.consistency-scan` are `mutating`, not `agent_callable`, and
`mcp_scope: none`, so Orbit's capability floor refuses them to agents
whether or not they are listed. A conformance test fails if any other tool
is advertised.

### Consequences

- Main agents see exactly `mcp__orbit__scribe_ask`.
- Cost: an agent cannot list tags or entries except by asking; the
  read-only CLI (`scribe entry list`) remains reachable from a shell, which
  is acceptable because it only reads.
- Superseded in part, 2026-11-10 ([ORB-23892]): the MCP surface is
  unchanged (`scribe.ask` is still the only advertised tool, and the
  conformance test stays), but agents now read and write through the CLI,
  which one skill teaches. "Only reads" no longer describes the shell
  surface. See
  [agents read and write through the CLI](#agents-read-and-write-through-the-cli-taught-by-one-skill).

## Models are called through the claude CLI

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `crates/scribe/src/models.rs::ClaudeCliModels` (from S3)

### Context

Four uses need a model: referents in the hook, session passes and
consistency scans in routines, and `ask`. Calling the Anthropic API
directly needs an HTTP client and TLS stack (new dependencies under
`cargo deny`) and an API key Daniel does not otherwise keep; his Claude Code
login already authorizes `claude -p`, which orbit-office's scribe pass also
used.

### Decision

One `Models` port in the core; one adapter that runs
`claude -p --safe-mode --tools "" --model <id> --output-format json` with the
prompt on stdin, from a cleared,
allowlisted environment, in its own process group, with a hard budget
([2_design.md §8](./2_design.md#8-models)). `--safe-mode` keeps the user's
hooks and plugins, including `scribe hook`, out of the model call, and
`--tools ""` gives the model no tools; `--bare` is not used because it reads
no OAuth login. A model
call is an operator-configured network request (STD-05@1 §R20): the model
ids and credential names come from `config.toml`.

### Consequences

- No HTTP dependency, no new secret, and tests swap the port for a fake.
- Cost: a process launch per call (about a second of overhead), a hard
  dependency on the `claude` binary being on `PATH` wherever scribe runs
  a model, and output parsing tied to the CLI's JSON format.

## Haiku for capture-side work, a decision model for ask

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

Daniel: "\"asking\" probably better with a decision model; and a routine
that checks for inconsistencies, outdated record removal, and small model
like haiku for \"scribe on\"." The referent and consistency uses were not
named.

### Decision

`models.session`, `models.referent` and `models.consistency` default to
`claude-haiku-5-5`; `models.ask` defaults to `claude-sonnet-5-5`, read as
the "decision model": a stronger model that weighs conflicting evidence and
declines to answer. Each is one config key. Haiku's output in every use is
checked by code (verbatim spans, budgets, JSON schema) and a consistency
finding only becomes a proposal Daniel approves, which is why a small model
is enough there.

### Consequences

- The frequent calls (every session pass, every referent) are cheap; the
  call agents depend on gets the strongest default.
- Cost: `ask` is the slowest and most expensive call and runs on every
  agent question; a model id that is retired breaks its use until the
  config changes.

## Session passes run from an Orbit routine, not a detached hook child

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `plugin/.orbit-plugin/definitions/routines/scribe-session-pass.yaml` (from S4)

### Context

A pass takes up to two minutes of model time, longer than a hook may block.
orbit-office's Stop hook launched `claude -p` and did not wait; a detached
process needs a durable owner record and a resource-bounded container
(STD-03@2 §R16), which a hook cannot portably give on both Linux and macOS.

### Decision

Hooks only mark a session due. A plugin routine (`*/5 * * * *`, seeded
disabled) runs `scribe.session-pass`, which Orbit supervises with its own
timeout and limits (STD-03@2 §R22) and which processes at most five due
sessions per run.

### Consequences

- No scribe process outlives its spawner.
- Cost: a summary lags the due mark by up to five minutes, and nothing is
  summarised until Daniel enables the routine in one workspace.
- Superseded in its cadence by
  [a failed pass stays due, and Daniel runs the pass by hand](#a-failed-pass-stays-due-and-daniel-runs-the-pass-by-hand):
  that manual-only cadence was itself reversed on 2026-11-10; see
  [consecutive windows and bounded retries](#consecutive-windows-and-bounded-retries-keep-session-passes-cheap).

## A session record covers the whole opted-in session

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

`/scribe on` may be typed mid-session. Summarising only what follows it
gives a record whose goals and outcome miss the session's start; reading
the earlier part could look like the backfill Daniel rejected.

### Decision

Turning a session on catalogs that whole session, from its first message,
in consecutive bounded windows ([2_design.md §5.3](./2_design.md#53-a-pass)).
The 2026-11-10 windowing decision below supersedes the original 150k
newest-message cutoff; a longer session no longer loses its older messages.
"No backfill" means no other session is ever read: scribe never scans
transcripts it was not switched on for.

### Consequences

- A record states the session's real goal even when `on` came late; long
  deltas are processed oldest first across multiple windows.
- Cost: messages typed before `/scribe on` are summarised, which Daniel may
  not expect; `/scribe off` stops future passes but does not shrink the
  record.
- Amended 2026-11-10 ([ORB-23892]): "no other session is ever read" now
  has one exception, a past session Daniel names with `scribe session
  catalog <id>`; and `scribe transcript list` reads transcripts, writing
  nothing, to help him choose. scribe still never scans or catalogs on its
  own. See [backfill is by name](#backfill-is-by-name-a-session-daniel-picks).
  The whole-session rule itself stands, and from format 4 a session's
  record grows a log instead of being rewritten in one budget
  ([a rewritable header over an append-only log](#a-session-record-is-a-rewritable-header-over-an-append-only-log)).

## The #scribe grammar is model-free

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `crates/scribe-core/src/capture.rs` (from S2)

### Context

`#scribe add this to decision` mixes routing (which tag) with content.
A model could parse any phrasing, but a model in the routing path could
send an entry to the wrong tag or invent one, and would put a model call on
every `#scribe`.

### Decision

Tag and form are resolved by a fixed grammar over configured names and
aliases ([2_design.md §4.2](./2_design.md#42-parsing-an-instruction)). A
model is used only to find the passage "this"/"it" points at, and its answer
must pass the verbatim check. If that model fails, the entry is still
written once with a marked fallback, so one instruction always yields
exactly one entry.

### Consequences

- An unknown or unparseable instruction is refused deterministically, with
  the list of tags or forms.
- Cost: phrasings outside the grammar ("#scribe remember for decisions
  that…") are refused rather than understood.

## Proposals are settled only by #scribe in a typed turn

**Recorded:** 2026-11 · [ORB-21747]

### Context

"Daniel approves" needs a channel only Daniel uses. A CLI verb or an Orbit
tool is equally reachable by an agent with a shell or a grant.

### Decision

`#scribe approve|reject <p-id>`, `#scribe retire|forget <entry-id>` are
instructions like any other `#scribe` line, captured by the
`UserPromptSubmit` hook. There is no CLI or Orbit verb that settles a
proposal or changes an entry's status.

### Consequences

- Every status change cites the turn Daniel typed.
- Cost: approving needs a Claude Code session, and the guard is the same
  accident guard as capture (STD-05@1 §R5): a process running as Daniel can
  still forge a hook call.
- Superseded 2026-11-10 ([ORB-23892]) by
  [entries Daniel wrote are settled only by Daniel](#entries-daniel-wrote-are-settled-only-by-daniel):
  agents may now open proposals, and settle those whose older entry an
  agent wrote, through `scribe proposal settle`. A proposal about an entry
  Daniel wrote is still settled only by his `#scribe approve|reject`, and
  `#scribe forget` stays his alone.

## Persisted free text passes one redactor

**Recorded:** 2026-11 · [ORB-21747]
**Code anchors:** `crates/scribe-core/src/store/redact.rs::Redactor`, `::REDACTED_FIELDS`

### Context

Entries quote Daniel and session records summarise transcripts, which can
contain a pasted token. STD-05@1 §R13 wants every writer of free text to
pass one redactor at the durability boundary, and §R14 wants it to match
secret values, not words.

### Decision

The store masks, before every write of `said`, `content`, session text and
proposal `reason`, the live values of the credential variables named in
`config.toml`: `[redact] env`, plus `[models] credentials`, the variables
the model calls are given. It also masks high-confidence credential
shapes a configured value cannot cover — a pasted or echoed credential
scribe was never told about (STD-05@1 §R14): an `Authorization: Bearer …`
token, `ghp_`/`github_pat_`, `sk-ant-`/`sk-`, `AKIA…` and `xox[bp]-`
tokens, and PEM private-key blocks, each replaced by a labelled marker
such as `[redacted:github-token]`. Shapes require a token boundary and a
minimum tail length, so ordinary hyphenated words (`task-pilot`) and
short look-alikes survive. A unit test holds the inventory of covered fields
(`REDACTED_FIELDS`, which also covers a session's `title` and `terms` and
each proposal quote). A value shorter than 8 characters is not masked. Audit lines stay
identifiers only. S1 builds the redactor; its first caller is the first
writer, in S2. Already-stored records are not rewritten.

A `[redact] env` name with no (non-empty) value in the writing process's
environment fails closed: `scribe session pass`, `scribe consistency
scan` and `#scribe` capture refuse (`redact_env_unset`, naming the
variables) before any write, rather than store unmasked text silently.
This matters in the Orbit plugin routines, whose `env_pass` carries only
the two model credentials: an extra `[redact] env` name must be added to
`env_pass` (or removed from the config) before the routines can write.
The model credentials themselves are not checked this way: they are
alternatives, so one may legitimately be unset.

### Consequences

- A configured secret never reaches `~/.scribe` in clear, and neither
  does a credential in one of the known shapes.
- Cost: a quote that contained a masked value no longer passes the verbatim
  check against its source, and a secret in no known shape that scribe was
  not told about is not masked.

## A domain library crate and a CLI crate

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `Cargo.toml` `[workspace] members`; `scripts/check-dependency-direction.sh::policy`

### Context

STD-02 asks for an enforceable boundary between the domain and the surface.
The domain must not know about argument parsing, terminals or log
subscribers, and scribe has three surfaces (hooks, the Orbit backend, the
CLI) over one domain.

### Decision

Two crates: `scribe-core` (library; business logic and persistence) and
`scribe` (binary; composition, adapters and surfaces). Cargo makes clap
unreachable from the core rather than merely banned. Further crates still
need an STD-02@4 §R7 reason; the default for new code is a module in one of
these two.

### Consequences

- The domain/surface boundary is enforced by the compiler plus
  `check-dependency-direction.sh`, not by review.
- Cost: two manifests, a `[workspace.dependencies]` table, and `pub` on the
  core's API where one crate would use `pub(crate)`.

## Persistence is one module inside the core crate

**Recorded:** 2026-09-26
**Code anchors:** `crates/scribe-core/src/store/mod.rs`; `scripts/check-dependency-direction.sh` (the `store | tests` loop)

### Context

Rules tested through a filesystem are slow and fragile, and a second backend
would mean untangling them first. Four crates (types, domain, storage, CLI)
is heavy without an STD-02@4 §R7 reason.

### Decision

Business logic (`entry`, `capture`, `session`, `ask`, `consistency`,
`verbatim`) does no I/O and never imports `crate::store`. Everything that
touches disk is the `store` module: the `Store` facade over private
submodules `format`, `audit`, `fsio` and `redact`. The dependency-direction
script
bans file and process access, and `crate::store` imports, in every other
core module. When persistence earns its own crate, `store/` moves out whole.

### Consequences

- The capture grammar, ranking and verbatim rules are unit-tested without a
  filesystem.
- Cost: `Store` is a concrete type, not a trait, and the module boundary is
  enforced by visibility and grep, which are weaker than a crate edge.

## Commands are one module per noun

**Recorded:** 2026-09-26
**Code anchors:** `crates/scribe/src/commands/mod.rs`

### Context

One dispatch file with every flag in `cli.rs` reads badly past a few verbs.

### Decision

`commands/mod.rs` holds the `Command` enum of nouns, the audited-command
table and `run`, the one chokepoint. Each noun is a module with its
subcommands, flags, help and handlers; a noun that outgrows one file becomes
a directory split along its verbs (`commands/hook/` has one file per event).
Every surface follows `<tool> <noun> <verb>` (STD-01@2 §R1): the CLI form of
`ask` is `scribe question ask`, and the Orbit backend entry is
`scribe plugin call`.

### Consequences

- Adding a noun is one module plus one variant and one `run` arm.
- Cost: the command tree is spread across modules, so the help goldens are
  its review surface; and `question ask` is wordier than a bare `ask`.

## Mutating commands are audited through one guard

**Recorded:** 2026-09-26
**Code anchors:** `crates/scribe/src/audit_middleware.rs::AuditGuard`; `crates/scribe/src/commands/mod.rs::Command::audit`, `::run`

### Context

A store changed by hooks in several sessions and by routines needs an answer
to "what changed it, and did that work", without making reads write
(STD-01@2 §R31).

### Decision

An RAII `AuditGuard` at the dispatch chokepoint (STD-02@4 §R24), starting
in the failure state, told the outcome by `mark_result`, writing one JSON
line to `<root>/audit.jsonl` on drop. `Command::audit` declares per command,
exhaustively, whether it is audited; reads return `None`. Lines hold
identifiers only (command, entry or session id, status, stable error code,
timing), never `said`, `content` or model text (STD-05@1 §R13). The log
fails open with a warning (STD-02@4 §R31).

### Consequences

- A panic or early return in a hook is logged as a failure, and a refused
  `#scribe` leaves an audit line with its error code.
- Cost: every audited command pays a second lock and an `fsync`, and the
  log grows without rotation.

## --json is the one alias for --format json

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `crates/scribe/src/cli.rs::GlobalArgs`, `crates/scribe/src/cli.rs::Cli::parse_args`

### Context

STD-01 R7 requires `--json` on every data command; R3 wants one spelling
per concept, and `--format` must exist for `table` and `auto`.

### Decision

`--json` is a global boolean, shorthand for `--format json`, and the only
alias. A conflict with `--format` is checked after parsing and reported as a
usage error (exit 2).

### Consequences

- Scripts can rely on `--json` everywhere, humans on `--format`.
- Cost: two spellings of one mode and a hand-written conflict check.

## A list's JSON is an envelope with total and truncated

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `crates/scribe/src/output/render.rs::Envelope`

### Context

A list capped by `--limit` must say so in its machine payload (STD-01@2
§R34), and turning a shipped bare array into an envelope later is a
breaking change (§R10).

### Decision

Every list's JSON is `{"<records>": [...], "total": N, "truncated": bool,
"warnings": [...]}`, where `<records>` names the noun (`entries`,
`sessions`, `proposals`, `tags`) and `warnings` lists the record files left
out because they did not parse
([below](#one-malformed-record-makes-its-file-corrupt)); an empty result is
the same envelope with empty arrays. Filters
apply before the limit (§R33), and a cut list says so on stderr in every
mode.

This departs from STD-01@2 §R10 ("field names MUST be stable") and the
workspace guide's list-envelope contract for one key: S1 renames the
template's `notes` key to `entries`. The note domain is removed in S1, and no
scribe release exists (no tag; the CHANGELOG has only `Unreleased`), so the
rename is the one break recorded here. From the first release the names are
stable.

### Consequences

- A capped page cannot pass for a complete one.
- Cost: `jq '.entries[]'` instead of `jq '.[]'`, and every list verb
  inherits the envelope.

## The store lock uses std file locks, not a crate

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `crates/scribe-core/src/store/fsio.rs::LockGuard::acquire`

### Context

STD-03 R6 needs a kernel advisory lock released on holder death; Rust 1.89
stabilized `File::lock`/`try_lock`.

### Decision

`std::fs::File::try_lock` with an in-process polling deadline;
`rust-version` is 1.89. Prefer std over a crate whenever std covers the
need.

### Consequences

- One fewer dependency to audit.
- Cost: no blocking-with-timeout primitive, so the wait is a 20 ms poll;
  concurrent `#scribe` lines in several sessions queue on it.

## clap without color or terminal-width features

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `Cargo.toml` `[workspace.dependencies] clap`

### Context

STD-01 R17 requires the color/TTY decision in one place; clap's `color` and
`wrap_help` features query the terminal themselves and would make help
goldens machine-dependent.

### Decision

clap is built with `default-features = false`, without `color` or
`wrap_help`.

### Consequences

- Help renders byte-identically everywhere, so goldens are stable.
- Cost: no colored or wrapped help.

## Usage errors keep clap's rendering

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `crates/scribe/src/output/mod.rs::finish_parse_error`

### Context

clap's usage errors carry a tip, the usage line and a pointer to `--help`,
the actionable detail STD-01 R21 asks for.

### Decision

In human mode usage errors keep clap's multi-line rendering (first line
`error: …`, exit 2); in JSON mode they are one `{"error","code":
"usage_error"}` object. STD-01@2 §R19 allows this rendering
([ORB-13134]), so it is not a deviation.

### Consequences

- Wrappers branch on exit code 2 and the stable `code`.
- Cost: human-mode stderr has several lines for a usage error and one for a
  command failure.

## standards-check warns locally and fails in CI

**Recorded:** 2026-09 · [ORB-13117]
**Code anchors:** `Makefile` `standards-check`; `.github/workflows/ci.yml` `STANDARDS_STRICT`

### Context

Standards are vendored into `docs/standards/` by the constellation's sync,
which may run after the repository is created.

### Decision

Without `docs/standards/check.sh`, `make standards-check` warns and exits
0 unless `STANDARDS_STRICT=1`, which CI sets.

### Consequences

- A just-created copy passes `make ci` locally.
- Cost: a local `make ci` can pass while CI fails, until the standards are
  vendored.

## Store format upgrades are an append-only registry

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `crates/scribe-core/src/store/format.rs::UPGRADES`, `::FORMAT`

### Context

An older build that loads a document with a field it does not know and
rewrites it drops that field (STD-02@4 §R16, STD-03@2 §R10); STD-03@2 §R23
wants every layout change shipped as an appended migration.

### Decision

Each persisted document (entry file, session record, control, proposals)
carries `format`, one number for the whole store. A shape change bumps it and
appends one step to `UPGRADES`; the step is given which document it is
upgrading, so a change to one type passes the others through. Shipped steps
are never edited. A newer format is refused
for reads and writes; reads upgrade in memory only. The plugin's documents
start at format 1 with empty registries: the template's `notes.json` is not
carried over because no scribe store exists yet.

### Consequences

- Older builds refuse newer stores instead of damaging them.
- Cost: even an additive field locks older builds out until they upgrade,
  which matters because an older scribe binary left on `PATH` refuses the
  store until it is replaced.

## Store state is owner-only and checked on every load

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `crates/scribe-core/src/store/fsio.rs::create_private_dir`, `::inspect_dir`, `::inspect_subdir`, `::inspect_file`, `::LockGuard::acquire`

### Context

Entries and session records are Daniel's personal data. STD-05@1 §R8 wants
state created owner-only; §R9 wants it refused when foreign-owned, writable
by others or reached through a link. A repair is a write, which reads may
not do.

### Decision

The root and subdirectories are created `0700`, files `0600`. Every load,
reads included, refuses a directory or file owned by another uid or
writable by group or others, naming the `chmod` that fixes it, and refuses a
record file, document, subdirectory or lock file that is a link (the root
itself may be one). Nothing is repaired automatically. On
non-unix platforms the checks are no-ops.

### Consequences

- Another local user can neither read the store nor plant one.
- Cost: a store copied in under a loose umask is refused until Daniel runs
  the named `chmod`.
- Superseded in part, 2026-11-10 ([ORB-24006]): the check refused only group
  and other write bits, so a group or other read bit was accepted. See
  [store state refuses any group or other mode bit](#store-state-refuses-any-group-or-other-mode-bit).

## Store state refuses any group or other mode bit

**Recorded:** 2026-11-10 · [ORB-24006]
**Code anchors:** `crates/scribe-core/src/store/fsio.rs::check_owner_and_mode`, `crates/scribe-core/src/error.rs::Error::Permissions`

### Context

The security and QA review of 2026-11-10 found that the load check refused
only group and other write bits. A `0755` root and a `0644` `config.toml` were
read without a refusal, so another local user could read Daniel's entries and
config. That contradicts the consequence recorded above, that another local
user can neither read the store. STD-05@1 §R9 asks only that state not be
writable by others; §R8 asks for owner-only modes.

### Decision

Every load refuses a root, subdirectory, record file, document or `config.toml`
that any group or other mode bit reaches (`mode & 0o077 != 0`), read and
execute bits included. A write refuses the same way for the lock file, since
loads take no lock. It names the `chmod 700` or `chmod 600` that fixes it. Nothing is repaired automatically, and reads still never write.
This supersedes the write-only check recorded above.

### Consequences

- Another local user can neither read nor traverse the store, as the
  decision above claimed.
- Cost: a `config.toml` made under a `022` umask (`0644`, the usual mode of a
  hand-written file) is refused until `chmod 600`, and a `0755` root until
  `chmod 700`. The refusal names the fix.
- STD-05@1 §R9 requires less than this (no group or other write). Owner-only
  is stricter than the standard, so it is a tightening, not a deviation.

## One malformed record makes its file corrupt

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `crates/scribe-core/src/store/mod.rs::Store::load_entries`, `::Skipped`, `crates/scribe-core/src/store/format.rs::decode_frontmatter`

### Context

STD-02@4 §R32 wants one bad item isolated while the rest proceed. A file
that is rewritten whole, if read past a bad record, would be rewritten
without it: data loss.

### Decision

Deviation from STD-02@4 §R32 within one file: an entry file, session
record, control or proposals document that fails to parse (bad YAML or JSON,
an unknown or missing key, an id that does not match its path) is refused as
`store_corrupt`, naming the file, and nothing rewrites it. Across files the
rule holds: `ask` and the entry and session lists skip a corrupt record
file, report it on stderr and in the payload's `warnings`, and answer from
the rest; `show` of it fails, and a write to it refuses. Only parse failures
are skipped: a link, loose modes or a newer format refuses the whole command,
since they say the store, not one record, cannot be trusted.

### Consequences

- A damaged file is never partially rewritten, and one bad tag does not
  silence the others.
- Cost: `control.json` and `proposals.json` are one file each, so one bad
  proposal hides them all (and one bad switch, every session's `on`) until
  it is fixed by hand.

## Missing gate tools warn locally and fail in CI

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `Makefile` `missing_tool`, `test`, `goldens`, `deny`, `ci-fast`

### Context

STD-04@2 §R10 says a gate whose tool is missing fails; `cargo-nextest` and
`cargo-deny` are not installed everywhere this repository is built.

### Decision

Deviation from STD-04@2 §R10 for local runs only: without `cargo-deny` the
supply-chain check is skipped and without `cargo-nextest` tests run under
`cargo test`, each with a warning. Under `CI=true` either missing tool
fails.

### Consequences

- A fresh clone passes `make ci` with only cargo installed.
- Cost: a local green `make ci` without those tools is weaker than CI's.

## Environment settings fall back rather than refuse

**Recorded:** 2026-09 · [ORB-13134]
**Code anchors:** `crates/scribe/src/output/sink.rs::resolve_mode`; `crates/scribe/src/output/mod.rs::init_logging`

### Context

STD-02@4 §R28 refuses a bad configuration value at load; STD-01@2 §R8
requires an unrecognized output-format variable to fall back to `auto`.

### Decision

Deviation from STD-02@4 §R28 for the environment: an unrecognized
`SCRIBE_FORMAT` means `auto`, and an unparseable `SCRIBE_LOG` is reported
once and replaced by `warn`. `config.toml` is not environment: a bad key or
value there is refused at load, naming the key, and a hook that cannot load
it replies with that error instead of capturing.

### Consequences

- A stray variable never stops a command.
- Cost: an unrecognized `SCRIBE_FORMAT` gives no warning.

## Standard rules the plugin brings into force

**Recorded:** 2026-11 · [ORB-21747]

### Context

The template recorded which rules of STD-01@2, STD-02@3, STD-03@2,
STD-04@1 and STD-05@1 it met only because it did not do what they govern.
The plugin does several of those things.

### Decision

These rules bind from the slice that starts doing what they govern:

- Spawned model processes (S3): STD-03@2 §R11–§R15 (process group, kill
  and reap, drained output) and STD-05@1 §R10–§R12 (cleared, allowlisted
  environment; no secret in argv).
- Provenance and authority (S2): STD-05@1 §R1, §R2 (the hook payload is a
  claim; host and transcript checks stamp the cite), §R5 (documented as an
  accident guard).
- Plugin surface (S5): STD-05@1 §R3 (capability, not listing, keeps agents
  off the mutating tools), §R20 (model calls are operator-configured).
- Free text persisted (S1): STD-05@1 §R13, §R14 (one redactor).
- Installed defaults (S4): STD-03@2 §R25 for the seeded `config.toml` and
  routines, which are created once and never overwritten. S2 seeds
  nothing: an absent `config.toml` means the built-in defaults.
- Unattended work (S4, S6): STD-03@2 §R22 through Orbit's routine limits
  plus each model budget.

Still not exercised: destructive CLI verbs (STD-01@2 §R5; `forget` is a
`#scribe` instruction, not a command), async code and nested locks
(STD-03@2 §R1–§R3), listeners (STD-05@1 §R16–§R19), git automation
(STD-03@2 §R32).

### Consequences

- A reviewer checks each slice against its rows here.
- Cost: the list goes stale unless the slice that starts doing one of these
  things also edits it.

## Frontmatter is parsed by yaml_serde, and config.toml by toml

**Recorded:** 2026-11 · [ORB-21753]
**Code anchors:** `Cargo.toml` (`yaml_serde`, `toml`, `unicode-normalization`), `crates/scribe-core/src/store/format.rs::decode_frontmatter`, `crates/scribe-core/src/config.rs::Config::parse`

### Context

[Records are Markdown with YAML frontmatter](#one-markdown-file-per-record-metadata-in-yaml-frontmatter),
which needs a YAML parser behind serde. `serde_yaml` is archived and
unmaintained; `serde_yml`, its best-known fork, carries a soundness
advisory; `saphyr` parses YAML but leaves serde to a separate, younger
crate. Frontmatter is
refused on an unknown or missing key, so the parser must hand serde a
complete mapping and refuse a repeated key. `config.toml` is TOML by the
design, and the verbatim check needs Unicode NFC.

### Decision

`yaml_serde` (MIT; the YAML organization's maintained continuation of
`serde_yaml`, on the pure-Rust `libyaml-rs`) parses frontmatter into a
mapping, which rejects repeated keys; the format probe and the `kind` check
run on the mapping, then `deny_unknown_fields` structs take the rest.
Nullable fields that must be present (`superseded_by`, `review_after`,
`message`, `scribed_through`) use a deserializer that errors when the key is
absent rather than defaulting it. `toml` 1 (parse and serde only, no writer)
reads `config.toml`, and `unicode-normalization` gives the NFC for
`verbatim::contains`. All three pass `cargo deny`.

### Consequences

- Records stay plain YAML that Daniel can edit; a typo is refused, naming
  the file and key, rather than silently dropped.
- Cost: three new dependency trees to track. YAML's typing still bites a
  hand edit: the parser follows YAML 1.2, so `title: on` is text, but
  `title: true`, `title: 12` or `title: null` is refused as corrupt until it
  is quoted. The writer (S2) serializes through `yaml_serde`, which quotes
  such values.

## S2 refuses forms 3 and 4 rather than guessing

**Code anchors:** `crates/scribe-core/src/capture.rs`

### Context

Form 3 (`#scribe add it to <tag>`) needs a model to resolve the referent
(S3), form 4 (`#scribe accept|reject|forget …`) needs proposals (S6), and
`#scribe distill` needs S7. S2 has none of them, and a bare form-2 reading
of `add it to decisions` would record the word "it".

### Decision

S2 recognises those shapes and refuses them with `unsupported_instruction`,
echoing the refusal. A form-3 line whose tag is not configured is refused as
`unknown_tag` first, so the reply already lists the tags.

### Consequences

- Nothing is ever recorded from a misread instruction; Daniel sees why.
- Cost: until S3 a `#scribe add that to decisions` does nothing but echo.
- Superseded for form 3 by S3 ([ORB-21778]), which resolves referents
  ([2_design.md §4.3](./2_design.md#43-resolving-this-and-it)); form 4 and
  `distill` are still refused this way.

## A referent with nothing before it is refused

**Recorded:** 2026-11 · [ORB-21778]
**Code anchors:** `crates/scribe-core/src/capture.rs` (`window`, `resolve`,
`Refusal::NoReferent`); `crates/scribe/src/commands/hook/user_prompt_submit.rs`

### Context

[2_design.md §4.3](./2_design.md#43-resolving-this-and-it) says a form-3
line always writes one entry, falling back to the previous assistant
message. A session's first prompt, or one that reaches the hook before
Claude Code has written the transcript, has no earlier message: there is
nothing to ask the model about and nothing to fall back to. A session where
only Daniel has spoken has no assistant message.

### Decision

With no earlier message the line is refused as `no_referent` (audited like
any refusal) and no model is called. With earlier messages but no assistant
one, the fallback is the latest message, cited with its role. Every other
case writes exactly one entry, as the design says. A replayed line is found
by its cite before the transcript is read, so it never calls the model
again.

### Consequences

- An entry never holds empty or invented content, and the echo tells Daniel
  to write the content itself.
- Cost: "one instruction never produces zero entries" has one exception,
  and Daniel must retype a referent typed as a session's first prompt.

## One audit line per hook outcome, none for a no-op

**Code anchors:** `crates/scribe/src/audit_middleware.rs`;
`crates/scribe/src/output/payload.rs` (`HookOutcome`)

### Context

One prompt can hold several `#scribe` lines, some written and some refused,
and nearly every prompt and every `Stop` changes nothing. One line per
command would either lose which line failed or log every keystroke.

### Decision

A hook result carries its outcomes: an entry written or a cite filled
(`target` the entry id) or a line refused or failed (`target`
`session:<id>`, `error_code`). The guard writes one audit line per outcome
and none when there are none; a hook that fails before a result (an invalid
payload) writes one failure line.

### Consequences

- The audit log shows every write and refusal with ids and codes only.
- A replay writes no audit line; it changed nothing.

## The provenance check allows a missing transcript

**Code anchors:** `crates/scribe/src/commands/hook/payload.rs`

### Context

STD-05@1 §R1 treats the hook payload as a claim. Claude Code may run a
session's first `UserPromptSubmit` before it writes the transcript file.

### Decision

The path must be absolute, inside the transcripts directory, named
`<session_id>.jsonl` and, when it exists, a regular file owned by this uid;
a missing file passes. A failure records nothing and replies
`provenance_failed` for each line.

### Consequences

- The first prompt of a session can be captured.
- Cost: a forged payload naming a not-yet-existing transcript path passes;
  like the rest of the check this is an accident guard (STD-05@1 §R5), not
  a defence against a same-user process ([2_design.md §9](./2_design.md#9-concerns--honest-limitations)).

## sha2 hashes the prompt

**Code anchors:** `Cargo.toml`; `crates/scribe-core/src/capture.rs` (`prompt_sha256`)

### Context

The idempotency key and the `Stop` hook's transcript match both need the
prompt's SHA-256, which `cite.prompt_sha256` already names.

### Decision

The RustCrypto `sha2` 0.10 crate (MIT or Apache-2.0, pure Rust), added as a
workspace dependency of `scribe-core`; it passes `cargo deny`.

### Consequences

- Cost: one more small dependency tree to track.

## The Claude Code package installs from a local marketplace

**Code anchors:** `plugin/.claude-plugin/marketplace.json`;
`plugin/claude-code/`

### Context

Claude Code installs plugins from a marketplace. scribe is not published to
one, and the package must install from a checkout.

### Decision

`plugin/` is a one-plugin marketplace, `scribe-local`, whose plugin source
is `./claude-code`; `claude plugin install scribe --marketplace ./plugin`
installs it. The hooks run `scribe` from `PATH` (`make install`).

### Consequences

- Installing needs no network and no publication.
- Cost: the package and the binary are installed separately and can drift;
  a missing binary makes each hook fail visibly in Claude Code.

## A session pass counts its failures from the audit log

**Code anchors:** `crates/scribe-core/src/store/mod.rs` (`failures_in_a_row`);
`crates/scribe/src/commands/session/pass.rs`

### Context

[2_design.md §5.3](./2_design.md#53-a-pass) clears `due` after three failed
passes in a row. Keeping a counter in `control.json` or the record changes
a persisted shape, which bumps the store format for one integer.

### Decision

Every session pass already writes one audit line, `command` `session pass`,
`target` `session:<id>`, with its status and code. The count is the run of
non-`ok` lines for that target since the last `ok` one, plus the current
failure. No persisted shape changes.

### Consequences

- The count survives restarts and needs no new store state.
- Cost: an audit log that cannot be read, or lines lost to an append
  failure, under-count, so a session may cost one or two more model calls
  before `due` is cleared.
- Superseded by
  [a failed pass stays due, and Daniel runs the pass by hand](#a-failed-pass-stays-due-and-daniel-runs-the-pass-by-hand):
  no failure clears `due`, and `failures_in_a_row` is gone. The later
  bounded-retry decision counts failures in control state for scheduling,
  without reintroducing a give-up threshold.

## A failed pass stays due, and Daniel runs the pass by hand

**Recorded:** 2026-11 · [ORB-23829] · **Overridable by Daniel**
**Code anchors:** `crates/scribe/src/commands/session/pass.rs` (`failed`,
`PassArgs`); `crates/scribe-core/src/control.rs` (`chosen`);
`crates/scribe/src/output/mod.rs` (`finish`); `crates/scribe/src/commands/plugin.rs`
(`failed_passes`)

### Context

On the Mac the Orbit clock's `PATH` holds no `claude`, so from 2026-11-08
20:00Z every routine pass failed with `model_unavailable`. The third
failure in a row cleared `due` whatever the code, so five sessions were
left on, not due and without a record, and the routine run still reported
success. Daniel, 2026-11-10: "I don't think we need automatic scribe
updates. I can just run "scribe session pass" - simpler and less noisy."
The `scribe-session-pass` routine was switched off that day.

### Decision

- Daniel can run `scribe session pass` by hand where `claude` is on
  `PATH`. Later on 2026-11-10 he reversed the manual-only choice: "perhaps
  routine was not a bad idea then? let's add that back". Once a host enables
  the routine, it runs every 5 minutes; see the windowing decision below.
  No setting names another `claude`. The plugin keeps
  shipping the routine and its job, seeded disabled.
- A failed pass, of any code, keeps the previous record and leaves `due`
  set; there is no give-up bound. The later windowing decision preserves
  successful checkpoints, adds bounded automatic backoff, and lets Daniel
  retry immediately with `--session`. `gave_up` stays in the `--json`
  report, always `false`.
- A run with any failed pass prints its report and then exits 1 with
  `session_pass_failed`, naming each failed session and its code;
  `scribe.session-pass` answers the same code and message, not retryable.
- `scribe session pass --session <id>` (repeatable) passes the named
  sessions whether or not they are due, each once and without the
  five-per-run cap, and refuses a session that is not on
  (`session_not_on`). It is the supported way to pass a session whose `due`
  an earlier give-up cleared.

### Consequences

- A failure is seen when it happens, on Daniel's terminal, and nothing is
  dropped: automatic runs retry after backoff, and explicit sessions retry
  immediately.
- The five sessions stranded by the old rule are passed with `--session`;
  no one edits `control.json` by hand.
- Cost: a session that keeps failing remains due indefinitely. The later
  backoff caps repeated model cost at one attempt per hour, while the
  enabled routine checks every 5 minutes and the manual command remains
  available.

## Consecutive windows and bounded retries keep session passes cheap

**Recorded:** 2026-11-10 · [ORB-23853] · **Overridable by Daniel**
**Code anchors:** `scribe-core::pass::window_with_limits`,
`scribe-core::control::fail`, `Store::checkpoint_pass`,
`store/format.rs::UPGRADES`, `SessionRequest::prompt`,
`commands/session/pass.rs`

### Context

A first pass sent 53,852 characters to Haiku. XML message framing and tags
inside the transcript were followed by malformed structured output: later
fields landed inside the `goals` string as XML. Daniel: "The point of
scribe was to use a cheap model". At 03:37Z on 2026-11-10 he also reversed
manual-only operation: "perhaps routine was not a bad idea then? let's add
that back". The Mac enabled the 5-minute routine that day. Daniel then
settled the shipped default: plugin-seeded routines start disabled for the
operator to review, following Orbit's convention.

### Decision

- Keep `claude-haiku-5-5`. Send consecutive oldest-first windows, default
  40 messages or 15,000 Unicode characters of message content, configurable
  with positive `[catalog] window_messages` and `window_chars`. Include
  pasted content in that budget. A single oversized message is cut with a
  marker inside the limit; headers and the previous record are additional.
- Each window receives the persisted record so far (none initially) and
  produces the whole updated record. Use plain headers for messages,
  pasted blocks and the previous record; retain ids for citations and the
  rule that messages are data. Keep one fresh validation retry per window,
  every field limit and topic/term reuse. Terms must name a user message
  already processed and quote its own words verbatim.
- Checkpoint every valid window through the store lock, redaction and
  `write_atomic`, advancing `scribed_through` and the record-update count
  `passes`. Keep the session due until all windows finish; only then
  finalize an ended session. A failed later window resumes from the last
  checkpoint. JSON adds per-session `windows`, successful checkpoints in
  this invocation, including those before failure.
- Keep consecutive failure count and `retry_after` in `control.json`,
  bumping the global store format to 2 with an appended format-1 upgrade.
  Retry automatically after 5, 10, 20, 40, then at most 60 minutes; success
  or switching off resets backoff. Keep the session due, never give up.
  `scribe session pass --session <id>` ignores the delay.
- The routine continues to ship seeded disabled, unchanged. Each host
  enables it with `orbit routine enable`; then it runs every 5 minutes.
  Daniel can also use the manual command. The CLI finishes all windows;
  the routine starts no further window after its existing 50-second
  start-by deadline and counts the unfinished session in `remaining`.

### Consequences

A large delta costs several cheap calls instead of one large prompt, and
an interruption or failure preserves earlier work. Old stores remain
readable without writes; the next mutation writes format 2. This does not
fix the Mac clock's missing `claude` PATH entry: the host must make the CLI
reachable before enabling the routine. Live Haiku validation on Daniel's
Mac remains an operator check after installation.

## A pass finds its transcript by session id

**Code anchors:** `crates/scribe/src/commands/session/pass.rs` (`find`)

### Context

`control.json` keeps `{on, since, due, final}` per session, not the
transcript path the hooks saw, and adding it is a format bump.

### Decision

A pass looks for `<transcripts>/<project-dir>/<session-id>.jsonl` across
Claude Code's project directories (by name, first match), then applies the
hooks' provenance check (this user's regular file, inside the transcripts
directory, named for the session). The record's `cwd` and `started` come
from the transcript's first message.

### Consequences

- No persisted-shape change, and the pass never trusts a path from a hook
  payload it did not see.
- Cost: one directory listing per due session; a transcript moved out of
  the projects directory leaves the pass `waiting`.

## `/scribe off` clears a pending pass

**Code anchors:** `crates/scribe-core/src/control.rs` (`switch`)

### Context

[2_design.md §5.1](./2_design.md#51-switching-on-and-off) says `off` stops
further passes and keeps the record. A pass due at the moment of `off`
could still run.

### Decision

`off` clears `due` and `final` as well as `on`, and a pass only runs for a
session that is on and due. Messages after the last pass are not recorded.

### Consequences

- `off` means no model reads the session again until `on`.
- Cost: turns between the last pass and `off` stay out of the record;
  `/scribe on` again marks a pass due that picks them up.

## `/scribe` ships a command file that expands to itself

**Code anchors:** `plugin/claude-code/commands/scribe.md`;
`crates/scribe-core/src/control.rs` (`parse`)

### Context

Without a command file Claude Code may answer `/scribe` with "unknown
command" before `UserPromptSubmit` runs; with one, the prompt the hook sees
may be the typed line or the expanded body, and a plugin command may be
named `/scribe:scribe`.

### Decision

`commands/scribe.md` (`disable-model-invocation: true`) has the body
`/scribe $ARGUMENTS`, so either form the hook receives starts with
`/scribe <arg>`; `parse` accepts `/scribe` and `/scribe:scribe`. The hook
blocks the prompt in every case, so the command file has no effect of its
own.

### Consequences

- `/scribe` works whichever way Claude Code delivers it.
- Cost: unverified live until the operator's check
  ([2_design.md §9](./2_design.md#9-concerns--honest-limitations)).

## The plugin backend serves `ask` alone until S6

### Context

[2_design.md §1](./2_design.md#1-boundary-and-surfaces) asks for
`timeout_ms: 720000`, `fs.read` and `fs.write` on the store root, a
`bin/scribe` launcher and `definitions/` holding the two routines, while
[§10](./2_design.md#10-slice-plan) puts the routines in S6. Orbit 0.44.0 caps
`timeout_ms` at 300000, renders `{{config.<key>}}` from a literal default
(no `~`, no `$HOME`; a relative path joins the plugin root), and refuses a
write root that does not exist.

### Decision

S5 ships `ask` live and the routine tools declared but answering
`not_implemented`, with no `definitions/`. `timeout_ms` is 150000 (30 s of
task reads, 90 s of model, margin). The store is `[plugins.scribe] root`, an
absolute path the operator sets, granted `fs.read` only; `fs.write` is
`{{plugin_state}}`, which always exists. Until `root` is set, `ask` answers
`not_configured` naming the setting. The launcher is `bin/scribe-plugin`, so
the tree never holds a file named like the binary it runs.

### Consequences

- The manifest validates and its goldens pass with the defaults; the store
  is unreadable to the plugin until the operator names it, and a CLI-only
  user is unaffected.
- S6 revisits the timeout (five sessions at 120 s exceed 300000; the job
  takes fewer per call or several calls) and adds the store write grant with
  the routines. Superseded in those parts by
  [the routine tools' deadline](#the-routine-tools-start-no-unit-after-a-deadline-and-audit-inside-the-call).

## `ask` reads Orbit tasks through `orbit tool run`

### Context

Task evidence and the orchestrator lookup need `orbit.search` and
`orbit.task.show` ([§6.2](./2_design.md#62-retrieval-model-free),
[§6.5](./2_design.md#65-lookups-records-by-recency-or-id)). Inside the
plugin those calls must go through Orbit's callback channel, which `orbit
tool run` uses when run from the plugin's environment (`ORBIT_BIN`, the
`ORBIT_PLUGIN_CALLBACK*` variables, descriptor 3). Federated `workspaces`
search is refused inside managed runs.

### Decision

The core defines a `Tasks` port; the CLI's `OrbitTasks` runs `$ORBIT_BIN`
(else `orbit`) `tool run` through the model adapter's supervisor, from a
cleared environment of `PATH`, `HOME`, `LANG` and `ORBIT_*`. It searches the
bound workspace, or the plugin's `task_workspaces`. A failure is logged and
`ask` answers without task evidence.

### Consequences

- One supervisor covers both spawning modules; `tasks.rs` is the second
  module allowed to spawn, and `check-dependency-direction.sh` requires it
  to go through `models::supervise`.
- From a terminal outside an Orbit workspace, `question ask` logs one
  warning and answers from the store alone.

## The routine tools start no unit after a deadline and audit inside the call

**Recorded:** 2026-11 · [ORB-21795]
**Code anchors:** `crates/scribe/src/commands/plugin.rs::audited`, `crates/scribe/src/commands/mod.rs::RunLimits`

### Context

[2_design.md §1](./2_design.md#1-boundary-and-surfaces) sizes the backend at
`timeout_ms: 720000` (five sessions at 120 s, four batches at 120 s) and
puts the routines in `definitions/`. Orbit 0.44.0 caps `timeout_ms` at
300000, lists a plugin's jobs and routines under `spec.definitions` (any
path), seeds every routine disabled, resolves an empty or relative
`{{config.<key>}}` against the install tree and refuses a write grant there,
under its global root, or on a directory that does not exist. `plugin call`
learns which store it serves only from the envelope, after `Command::audit`
has run.

### Decision

- `timeout_ms` is 300000. The tools run the CLI's code under
  `commands::RunLimits`: `scribe.session-pass` starts no session after the
  first once 50 s have passed (one session is at most two 121 s asks), and
  `scribe.consistency-scan` no batch after the first once 170 s have (one
  batch is at most 121 s); what is left is `remaining` or `pairs_left` for
  the next run. The CLI keeps five sessions and four batches with no time
  limit. Each job step is 300 s, each routine `timeout_minutes: 6` with
  `overlap: forbid`.
- The routines and jobs are `routines/*.yaml` and `jobs/*.yaml`, listed
  under `spec.definitions`.
- The store root is granted `fs.read` and `fs.write`. Its default is
  `/var/tmp`, a directory every host has, outside any Orbit tree, which
  scribe treats as unset (`not_configured`), so the manifest validates and
  its goldens run before the operator sets it. A new `transcripts` setting
  (absolute; default empty, which is a harmless read of the install tree) is
  granted `fs.read`; `scribe.session-pass` answers `not_configured` without
  it.
- The routine tools are audited by an `AuditGuard` started inside the call,
  on the store the envelope names, under the CLI verbs' command names
  (`session pass`, `consistency scan`), with the same `mark_result` lines.
  `PluginCommand::audit` stays `None`, so `scribe.ask` writes nothing. This
  departs from `STD-02@4 §R24`'s single chokepoint in where the guard
  starts, not in what it records: one `AuditGuard` implementation serves both.

### Consequences

- A busy store catches up over several routine runs; the CLI still does a
  full run by hand.
- A failure counter that reads the audit log (`session pass`) sees the
  plugin's passes and the CLI's alike.
- Until `root` is set, the plugin holds a write grant on `/var/tmp` it never
  uses.
- Cost: a sentinel default the code must recognise, and two places that
  start an audit guard instead of one.

## Approving needs the older entry active

**Recorded:** 2026-11 · [ORB-21795]
**Code anchors:** `crates/scribe-core/src/consistency.rs::settle`

### Context

A proposal stays `open` while Daniel retires, forgets or supersedes one of
its entries by another proposal. Applying it then would overwrite that
newer decision, for example turning a `forgotten` entry back into a
`superseded` one.

### Decision

`#scribe approve <p-id>` is refused `entry_not_active` unless the older entry
is `active`; the proposal stays open, and `#scribe reject` closes it.
`reapply` likewise touches only an older entry that is still `active`.

### Consequences

- An approval never changes an entry Daniel settled another way.
- Cost: a stale open proposal needs an explicit reject to leave
  `proposal list`.

## Forgetting an entry blanks the proposals' copies of it

**Recorded:** 2026-11 · [ORB-21795]
**Code anchors:** `crates/scribe-core/src/consistency.rs::settle`

### Context

[§3](./2_design.md#3-storage) says `forgotten` blanks `said` and `content`.
Proposals quote both of their entries and their reasons paraphrase them, so
the words of a forgotten entry would survive in `proposals.json`.

### Decision

`#scribe forget <entry-id>` also blanks the `text` of every proposal quote of
that entry and the `reason` of every proposal whose `older` or `newer` it
is, in the same locked write. The proposals' ids, kinds, states and cites
stay.

### Consequences

- A forgotten entry's words leave the whole store at once.
- Cost: a proposal about a forgotten entry no longer says why it was
  opened, or what the other entry's quote was contrasted with.

## `session-start` is read-only except for handoff restoration and seat inheritance

**Recorded:** 2026-11 · [ORB-21795]
**Refined by session refresh and session groups:** Outside the §16.7
continue-handoff path and §17 group seat inheritance across a clear,
`session-start` remains read-only. Restoring a continue handoff writes the
child's `continues` link and inherited switch, and inheriting a group seat
appends the child to the predecessor's open seat; both audit their writes.
**Code anchors:** `crates/scribe/src/commands/hook/session_start.rs`,
`crates/scribe/src/commands/hook/continuation.rs` (the restoration)

### Context

[§3](./2_design.md#3-storage) has `scribe hook session-start` create
`config.toml` with the defaults on first run. Every command already treats a
missing file as the defaults, and the hook runs before every session.

### Decision

Outside the continue-handoff restoration in [design §16.7](./2_design.md#167-continuing-after-a-clear)
and an inherited open group seat in [design §17](./2_design.md#17-session-groups-and-seats),
`session-start` is read-only: it reads the store and writes nothing. It
replies with a message and additional context:

- The message is the open-proposal count (`scribe: N proposals to review`),
  the stuck-action line for action entries with no pickup after an hour, and
  at most one continuation line. On a start that is not `clear`, that line is
  the same-project reminder (or the list of possible predecessors when
  several qualify). In an interactive Claude Code session whose transcript
  names a cleared predecessor, the restoration gives the line instead: an
  ambiguity note when several are named, or a failure note when the restore
  fails.
- The context is a restored continue handoff's body, any group-seat status,
  then, in an interactive session, the brief for this project with its memory
  page. Seat inheritance and ancestry lookup run only for interactive Claude
  Code starts whose source is `startup`, `clear`, or omitted. With no open
  groups, an unseated session gets no seat context. When open groups exist but
  no predecessor line exists, multiple predecessors exist, or a single
  predecessor holds no open seat, the context recommends `scribe group join`.
  A session replaced on its seat is told it was replaced without a join
  recommendation. A seat-lookup error is reported in `systemMessage`, and
  `additionalContext` also gives the recommendation.

The proposals come from the store: its database, or `proposals.json` before
migration. A read that fails is logged and that part is left out; the rest
still prints. When it restores a continue handoff, it writes the `continues`
link and inherited switch under `Store::write`; when it inherits an open
group seat, it appends the child to the predecessor's seat chain. Both writes
are audited. These are the hook's write paths.

### Consequences

- Daniel creates `config.toml` when he wants to change a default; nothing
  writes it for him.
- Cost: no file on disk to show the defaults; `scribe tag list` shows them.

## Session links are typed, stored on the child, and usually manual

**Recorded:** 2026-11 · [ORB-23849]
**Refined by session refresh:** A desktop clear names its predecessor in the
new transcript. A continue handoff authorizes the hook to record that link
and carry the recording switch ([design §16.7](./2_design.md#167-continuing-after-a-clear)).

**Code anchors:** `crates/scribe-core/src/session.rs` (`SessionLink`, `check_link`, `children`), `crates/scribe-core/src/control.rs` (`link`), `crates/scribe-core/src/store/mod.rs` (`link_session`), `crates/scribe/src/commands/hook/scribe_command.rs`

### Context

Daniel, 2026-11-10: "we need a way to indicate relationships between
sessions, e.g. I have nested scribed sessions between this session and one
of the branched agent sessions." Every session record stood alone, so an
orchestration session and the side session it started each retold the
shared story. The existing `related` field lists `session:<id>` references
the model extracted, which says what a session mentions, not how sessions
are structured.

### Decision

- A new `links` field, `[{type, session}]` with `type` `spawned_from` or
  `continues`, mirrors Orbit task relations. `related` keeps its type and
  meaning; no free-form `related_to` until a real case needs one.
- A link is stored once, on the child. Children are derived at read time;
  a parent record is never rewritten because a child appeared.
- Links are ordinarily manual: `/scribe on --from <id>` and
  `/scribe link <id> [--type …]`, typed by Daniel in the child. A desktop
  clear is the automatic exception: when its checked transcript ancestry
  names one predecessor, `session-start` considers that predecessor's
  newest active handoff. It records a `continues` link and carries over the
  recording switch only when that newest handoff has mode `continue`. A
  newer active handoff with mode `new` or no mode takes precedence over an
  older active `continue` handoff, leaving the ordinary brief and recording
  switch alone ([design §16.7](./2_design.md#167-continuing-after-a-clear)).
  It does not pair sessions by checkout or time. For a manual link, an
  unknown target, a self-link, a cycle (through links of either type), a
  second parent and a `link` in an uncataloged session are refused with
  stable codes and change nothing.
- A link typed before the session has a record waits on its switch
  (`pending_links` in `control.json`) and moves onto the record with the
  first pass write. Store format 3 adds `links` and `pending_links`; the
  2-to-3 upgrade gives older documents empty lists in memory.
- A child's pass is given each linked session's latest title and summary as
  data and told to summarize only its own slice.

### Consequences

- Records written before format 3 load unchanged but for an empty `links`,
  and the next write persists format 3; an older build refuses format 3
  instead of dropping links (STD-03 R10).
- A link write rewrites only the child's record, keeping its `updated`, so
  it does not count as a pass for the `Stop` hook's cadence.
- Cost: a fork or side session stands alone unless Daniel links it or it
  qualifies for the continue-handoff exception.
- Cost: `ask` does not follow links yet; a linked record is evidence only
  on its own merits.

## A session record is a rewritable header over an append-only log

**Recorded:** 2026-11-10 · [ORB-23892] · **Daniel's choice**
**Code anchors:** `crates/scribe-core/src/session.rs`, `crates/scribe-core/src/pass.rs`, `store/format.rs` (from S8, S9)

### Context

A session record had four sections under fixed budgets, the whole record
rewritten by every window. Daniel, 2026-11-10: "limiting each session to
2500 is insufficient for long sessions - e.g. after writing first version
of the session using all 2500 characters, the next update must remove a
part of the original. So over multiple re-writes, the beginning session's
context will get lost, defeating the purpose." Raising the budgets only
moves the point where history is cut. Starting a new record each time the
budget fills was ruled out: it pollutes the store with partial records of
one session.

### Decision

One record per session, in two parts
([2_design.md §3](./2_design.md#3-storage),
[§5.5](./2_design.md#55-the-log-segments-and-sealing)):

- A **header** every pass rewrites: the whole session's title, goals, next,
  and a short summary written as a table of contents.
- An append-only **log** in the same file: `## Log`, one segment per
  stretch of the session, each with its number, message range, dates, a
  heading, about 800 characters of text, and its own topics and `related`.
  Only the last segment is open; the others are sealed and never sent back
  to the model to rewrite. The store refuses a write that changes a sealed
  segment.
- `session show` prints the header and the log's table of contents;
  `--log` adds the text. `ask` and `record search` rank segments and cite
  `session:<id>#<n>`.

### Consequences

- A long session keeps its beginning: the cost of length is more segments,
  not lost text. One file still answers "what was this session about".
- Retrieval gets finer: a segment is a unit with its own dates and topics,
  so a hit points at the part of a session that matters.
- Cost: a persisted-shape change (format 4) with an upgrade step, a body
  parser that must keep frontmatter and headings in step, and a model
  prompt that must give segment boundaries the code can check. A segment
  sealed with a poor summary stays poor until `session rebuild`.

## Outcome folds into the log

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**

### Context

The header could keep `## Outcome`. But an outcome rewritten every pass is
the same 2,500-character budget that lost the start of long sessions, and
"what happened" is exactly what the log records in order.

### Decision

The format-4 header is title (120), summary (1200, a table of contents),
goals (600) and next (1000). There is no outcome section: each segment's
text says what was reached in it. The JSON payload keeps `outcome`, always
`""` for a format-4 record, so no reader breaks. The upgrade step moves an
old record's outcome into segment 1, after its summary.

### Consequences

- The header stays small (under 3,000 characters), so every pass's
  rewrite is cheap and nothing in it needs history.
- Cost: no one field answers "how did it end"; a reader takes the last
  segments or `next`. Scripts that read `.outcome` get `""` and must move
  to `.log`.

## A segment seals when it is full, when the thread changes, or after three windows

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**

### Context

If the model alone decided when to start a segment, a cheap model could
keep one segment open for a whole session and rewrite it down to 800
characters, which is the loss the log exists to prevent. If scribe alone
decided, by a fixed size, segments would cut threads in the middle.

### Decision

- The model opens a new segment (sealing the open one) when the thread
  changes, by naming the message it starts at.
- scribe enforces the budget: an open segment's text over 900 characters
  is refused with "seal it and start a new segment". After the one model
  retry, a remaining text overrun is cut deterministically and, if no new
  segment follows, sealed at the window's end. Header budget and misplaced
  start repairs follow [§5.5](./2_design.md#55-the-log-segments-and-sealing),
  with a warning per repair; other shape defects still refuse the window.
- scribe seals the open segment itself after `[catalog] segment_windows`
  (3) windows, and at the end of the final pass.
- At most three new segments per window.

### Consequences

- One segment summarizes at most about 120 messages or 45,000 characters,
  bounding what any rewrite can compress away.
- Cost: a long single-topic stretch is split into several segments whose
  headings repeat; a configurable bound is one more setting.

## Store format 4 carries the log, active and author in one step

**Recorded:** 2026-11-10 · [ORB-23892]
**Code anchors:** `store/format.rs::UPGRADES` (from S8)

### Context

The log, the `active` flag, entry and proposal authors, the catalog marker
and the tags store all change persisted shapes. Each could bump the format
on its own, in the slice that needs it.

### Decision

One bump, format 4, in S8, with one appended upgrade step covering every
new field; the slices after it fill fields that already exist. The step
converts in memory: a session's summary and outcome become sealed segment
1 (`origin: upgrade`), sessions are active, entries are Daniel's (every
entry so far came from a typed `#scribe` line), proposals are the
consistency routine's. A session whose transcript remains can be rebuilt
into real segments with `scribe session rebuild <id>`; one whose
transcript is gone keeps the converted segment.

### Consequences

- One migration for Daniel's store, one refusal point for older builds.
- Cost: S8 is larger than the slices after it, and fields such as
  `cataloged_by` exist (always `null`) before the slice that sets them.

## Sessions carry an active flag, separate from status

**Recorded:** 2026-11-10 · [ORB-23892] · **Daniel's choice**
**Code anchors:** `crates/scribe-core/src/activity.rs` (from S8)

### Context

Daniel, 2026-11-10: "we should add \"isActive\" field to sessions so that
scribe or based on configurable duration limit can periodically deactivate
stale sessions", and "A feature that no longer exists (if it exists in
session store) should be deactivated." `status` already says whether a
session has ended, which is a different question from whether its content
still describes something current.

### Decision

`active` (default `true`) in a session's frontmatter, beside `status`,
with `keep_active` (an exemption) and `active_change` (`{at, by, route,
reason, cite}` of the last change). An inactive session is hidden, not
removed, like a superseded entry: left out of `session list`, `record
search` and `ask` evidence by default, shown with `--include-inactive`,
`--include-hidden` or `include_hidden`, and always by `session show`.
Every change is audited and reversible with `session activate`.

### Consequences

- Records about removed features stop answering questions without losing
  their history.
- Cost: a second state axis readers must learn (`final` and active, `open`
  and inactive are both possible), and hidden sessions can surprise a
  reader who forgets the flag.

## Removal of the feature is the main deactivation route; age is a generous backstop

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/activity.rs`, `crates/scribe-core/src/consistency.rs` (from S10)

### Context

Daniel asked for both a configurable age limit and deactivation of
sessions about features that no longer exist. The two pull apart: the goal
is context for features that still exist, and such a session can be old,
so an age rule alone would hide the sessions that explain long-lived
features.

### Decision

- Three audited routes, each with a reason: `removed` (the consistency
  routine, when a later record says the session's feature was removed or
  replaced; model judgement whose quote of the later record is verbatim-
  checked, citing it); `age` (the same routine, model-free, when the last
  message is older than `[sessions] inactive_after_days`, default 120);
  `manual` (`scribe session deactivate <id> --reason …` and `scribe session
  activate <id>`, by an agent or Daniel).
- `keep_active: true`, set with `session activate <id> --pin`, exempts a
  session from both automatic routes.
- Age counts from the last message, not `updated`, because rebuilds,
  links and catalogs rewrite `updated`.
- The routine deactivates directly instead of opening a proposal: the
  change hides, never alters, and is one command to reverse.
- scribe never reads a repository to decide whether a feature exists. That
  check belongs to the agent or routine that calls `deactivate`.

### Consequences

- An old session about a living feature stays active for four months
  without anyone touching it, and indefinitely once pinned.
- Cost: a wrong `removed` judgement hides a useful session until someone
  looks with `--include-inactive`; the age limit still hides an unpinned
  session about a long-lived feature after 120 days.

## Removal evidence is Daniel-authored entries only

**Recorded:** 2026-11-10 · [ORB-24005] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/consistency.rs` (`later_records`, `judge_removals`)

### Context

Security review of the 2026-11-10 shift: the `removed` route paired a
session with any later record — a session header, a segment, or an
entry — and kept a `removed` label whenever the model's quote appeared
verbatim in that record. Session text is model-written: a pass
summarizes a cataloged transcript, so text injected into a transcript
can surface in a segment, and an agent can write `entry add "X was
removed"` directly. Either gives the model a quote it can always
satisfy, and the scan deactivates the related sessions, hiding them
from `ask` (reversible and audited, but silent until looked for).

### Decision

- Only active entries Daniel wrote (`author.kind` is `daniel`) are
  removal evidence. Session headers and segments — open or sealed —
  and agent-written entries never pair and never deactivate.
- Sealed segments stay excluded: sealing proves a segment never
  changed after it was written, not that Daniel wrote it; its words
  are still a model's summary of a transcript.
- `judge_removals` re-checks the cite as defence in depth: a `removed`
  label whose later cite is not an entry id is dropped, so a pair
  built from model-written or agent-written text deactivates nothing
  even if it reaches the judge.

### Consequences

- Injected or agent-supplied words cannot hide a session; only
  Daniel's own marked words can, through the one channel only he uses.
- Cost: a removal Daniel mentions only in conversation (never in a
  `#scribe` entry) no longer deactivates anything; the session waits
  for the age route, a pin, or a manual `session deactivate`.

## Agents read and write through the CLI, taught by one skill

**Recorded:** 2026-11-10 · [ORB-23892] · **Daniel's choice**
**Code anchors:** `plugin/claude-code/skills/scribe/SKILL.md`, `crates/scribe/src/commands/record.rs` (from S11–S13)

### Context

Daniel, 2026-11-10: "I don't want to keep having to manually search for
specific session id's to hand to agents; and I am not sure about adding
another set of MCP tools. so I think a single scribe skill that teaches
terminal commands will suffice." Agents "should able to filter sessions by
topics, tags, bm25 search, date range, add/update proposals, tags and
entries directly", with `scribe.ask` kept for intensive and vague
searches. He confirmed the division of work the same day: scribe's own job
is summarizing and cataloging sessions; agents do the direct writes.

### Decision

- The MCP surface stays `scribe.ask` alone. Agents use the CLI: `record
  search` (BM25 without a model), list filters (`--since`, `--until`,
  `--status`, repeated `--topic` and `--related`), unique id prefixes, and
  write verbs for entries, tags and proposals, each audited and recording
  its author ([2_design.md §1.1](./2_design.md#11-the-cli-agent-surface)).
- One `scribe` skill in the Claude Code package teaches the commands and
  when to `ask` instead; a route under the constellation umbrella's
  `.agents/skills/scribe/` serves other providers.
- Names follow STD-01 R1–R2: the search is `record search`, not a bare
  `scribe search`, since one tool must not mix verb-first and noun-first
  orders; backfill candidates are `transcript list`.
- scribe's model work stays the session pass, `ask` and the consistency
  routine; no model writes an entry, tag or proposal for an agent.

### Consequences

- An agent finds a session by topic, date or word without Daniel handing
  it an id, and records what it learns without a new MCP tool.
- The store is on the Mac, so the skill serves Mac sessions; box workers
  still cannot read scribe.
- Cost: the write path is no longer agent-free, which was this plugin's
  distinctive claim ([3_vision.md](./3_vision.md)); the author field and the
  guard on Daniel's entries are what keep his words distinguishable.

## Every entry and proposal records its author

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/record.rs` (`Author`, from S8)

### Context

Once agents write, an entry no longer implies "Daniel said this". `ask`
quotes entries as Daniel's words and weights them 1.5×. Scribe cannot
verify who ran a CLI command: any process running as Daniel can.

### Decision

- Every entry and proposal records `author: {kind, name}`. `kind` is
  `daniel` for a typed `#scribe` turn (the hooks write it), `agent` for any
  CLI write verb, and `scribe` for scribe's own routine (proposals only).
- Every CLI write verb requires `--author <name>` (by convention the
  agent's crew or model) and records `kind: agent` whatever the name;
  `kind` means the channel, not a claimed identity, so `--author daniel`
  still writes `kind: agent`.
- An agent entry has no typed line to cite: `said` is `""` and `cite` is
  `null`.
- Existing entries upgrade to `daniel`; existing proposals to `scribe`.

### Consequences

- `ask` can label and weight by channel, and Daniel's words stay marked by
  the one channel only he uses.
- Cost: Daniel at a terminal writing through the CLI is recorded as an
  agent; `--author` is an unchecked label; `cite` can now be `null` in an
  entry's JSON, which readers of agent entries must handle.

## Agent-created tags live in the store, and config tags win

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/tag.rs`, `store/` (`tags.json`, from S12)

### Context

Agents may now add tags. Tags are defined in `config.toml`, which is
Daniel's and which scribe never rewrites. Writing agent tags into the config
would break that rule, lose Daniel's comments and formatting on a TOML
rewrite, and put agent writes outside the store lock and audit.

### Decision

Agent tags live in `tags.json` in the store (format 4): written under the
lock, audited, redacted, with an author. The tag table is the config's tags
merged with the store's. Config tags win: a store tag may not take a name
or alias a config tag claims (`tag_conflict`), a later config edit that
claims one drops it from the store tag with a warning, and `tag update`
refuses a config tag (`tag_configured`). Daniel promotes a store tag by
adding it to his config.

### Consequences

- `config.toml` stays Daniel's alone, and his edits always prevail.
- Cost: tags live in two places; `tag list` shows a `source` column so a
  reader can tell which.

## Entries Daniel wrote are settled only by Daniel

**Recorded:** 2026-11-10 · [ORB-23892] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/consistency.rs` (`settle`, from S12)

### Context

Agents may now open, update and settle proposals. Settling a proposal
supersedes or retires an entry; if an agent could do that to an entry
Daniel wrote, an agent could hide his words.

### Decision

- Agents may open a proposal against any entry, update the reason of an
  open proposal an agent opened, and settle a proposal whose older entry
  is agent-authored.
- A proposal whose older entry Daniel wrote is settled only by his
  `#scribe approve|reject`; `proposal settle` refuses it (`daniel_only`).
- Agents may change only the topics and related references of a Daniel
  entry, never its content, and may not retire it. `#scribe forget` stays
  Daniel's alone.
- `ask` never presents an agent entry as Daniel's words and gives it no
  1.5× weight.

### Consequences

- An agent can tidy its own notes without waiting for Daniel, and Daniel's
  words leave answers only by his hand.
- Cost: agent proposals against Daniel's entries wait in the session-start
  count until he settles them; the refusal is an accident guard, not a
  permission (STD-05@1 §R5).

## Humans browse scribe in a tmux popup over fzf

**Recorded:** 2026-11-10 · [ORB-23892] · **Daniel's choice**
**Code anchors:** `crates/scribe/src/commands/tmux.rs`, `crates/scribe/src/popup.rs`, `crates/scribe/src/popup.sh` (from S13)

### Context

Daniel: "we need a very simple desktop UI or tmux UI for humans. perhaps
tmux will be better - having another desktop app will be cumbersome." A
Rust TUI (ratatui) would add dependencies under `cargo deny` and code to
maintain for what is mostly a list and a preview.

### Decision

A POSIX shell script of about 150 lines over `fzf`, bound to
a tmux key with `display-popup`: sessions, entries, proposals and backfill
candidates in one list, `scribe … show` as the preview, Enter opens the
record in the pager, a key copies the id. No new Rust dependency. It first
shipped as a standalone file, `scribe-tmux`, that nothing installed; it is
now the read-only `scribe tmux` subcommand, the script embedded in the
binary (`include_str!`, run with `sh -c`) so there is nothing to install
and its child commands run the started executable, and its views ask for
`--format table` because `show` through a pipe prints only its list line. A real
TUI is built only if this proves too small.

### Consequences

- Daniel browses in the terminal he already uses, and the script reuses
  the CLI's plain output, so it changes when the CLI does.
- Cost: `fzf` must be installed; the script parses tab-separated columns,
  so a column change in plain output is a change to the script too.
- Deviation from `STD-03@2 §R11` (own process group) and the supervisor
  path of `§R12–R13`: the popup child is interactive. fzf and the pager
  read the tty, which a background process group cannot do (SIGTTIN), and
  a person ends the session, so there is no budget to enforce. `popup.rs`
  runs it in the caller's foreground group, waits for it, and reports its
  exit status (`§R13`'s reaping holds). It is not an untrusted child, so
  it inherits the environment (`STD-05@1 §R10` covers children that are
  not fully trusted); its commands are the started executable's own
  list and show verbs, and it drops `SCRIBE_FORMAT`.

## Backfill is by name: a session Daniel picks

**Recorded:** 2026-11-10 · [ORB-23892] · **Daniel's choice**
**Code anchors:** `crates/scribe/src/commands/session/catalog.rs`, `crates/scribe/src/commands/transcript.rs` (from S14)

### Context

Daniel: "we need to backfill some of important chat sessions on orbit and
scribe projects. not everything but certain long ones", then "we don't need
to go all the way back to aug. those will just add noise. the primary goal
is to understand the context of existing features, to provide continuity
... month of oct will be a good starting point." `session pass --session`
refuses a session never switched on (`session_not_on`), and the original
design read no other session. Most sessions start from the constellation
root (463 of 947 transcripts on the Mac), so the cwd does not say which
project a session was about.

### Decision

- `scribe session catalog <id>` switches a past session on by name,
  recording `cataloged_by`, runs its passes to the end with no routine
  deadline, and marks it final when its transcript is a day old. It writes
  a session record, labelled as a summary, and no entries.
- `scribe transcript list --since <date>` lists uncataloged transcripts,
  model-free and writing nothing, with project, dates, size and Daniel's
  turn count, at least 20 turns by default. A transcript's project is the
  configured project (`[projects.<slug>] paths`) whose paths it mentions
  most, else its cwd's.
- Daniel picks and runs `catalog` by hand, a few at a time; scribe never
  scans or catalogs on its own. A session only about a removed feature is
  skipped, or deactivated if cataloged; the few that matter most are also
  distilled into discussions (S7).

### Consequences

- October's long orbit, scribe and constellation-site sessions can give
  agents the context of existing features without cataloging everything.
- Path counting keeps scribe generic: it knows no project names, only the
  paths Daniel configures.
- Cost: the attribution is a heuristic that a session touching many repos
  can fool; each catalog is a full model run over a long transcript.

## Audit lines name the agent and the resolved target; rebuilds persist their author

**Recorded:** 2026-11-10 · **Implementation decision** (ORB-24003, against
STD-05@1 §R2)

A `session rebuild` replaces a record's header and log, so its audit line,
which named only `session:<id>`, could not say who replaced them. A refused
write named neither its record nor its `--author`, so a `daniel_only`
refusal was not attributable. Store format 6 adds the nullable `rebuilt_by`
header field, `{kind, name}`: the rebuild that writes the record sets it, a
pass carries it forward, and the upgrade step writes `null`. Audit format 2
adds `author`, the `--author` name of a command that takes one, on success
and on refusal. It has its own field because STD-05 R2 keeps a self-reported
identity apart from the arguments, which the line still never carries.
The target of an entry, proposal or session write is resolved before the
command runs, so a refusal names the id it aimed at. An id that does not
resolve leaves `target` `null` rather than echoing what was typed. A format-1
line still reads, with no author.

The author stays a self-report. The audit line is an accident guard, not an
OS security boundary (STD-05@1 §R5, as the S12 decision records). `session
show` prints `rebuilt_by` only in `--json`.

## Task References

- [ORB-21747] — designed the standalone scribe plugin and recorded its decisions.
- [ORB-13117] — built the Rust CLI template this project was created from.
- [ORB-13134] — brought the template to STD-01..03 v2 and STD-04/05 v1.
- [ORB-21753] — S1: built the domain, the frontmatter store and the read-only CLI.
- [ORB-21764] — S2: built `#scribe` capture (no model) and the Claude Code package.
- [ORB-21760] — the operator's live check of the S2 package.
- [ORB-21778] — S3: referents (form 3), the `Models` port and `ClaudeCliModels`.
- [ORB-21784] — S4: `/scribe on|off|status`, session-pass cadence and session records.
- [ORB-21787] — S5: the Orbit plugin tree and `ask`.
- [ORB-21795] — S6: the consistency routine, the settling verbs and the routines.
- [ORB-23849] — session links (`spawned_from`, `continues`) between session records.
- [ORB-23892] — session records with a log, the `active` flag, the CLI agent
  surface and skill, the tmux popup and backfill by name (designed).
- [ORB-24006] — store state refuses any group or other mode bit; the
  redaction script takes its key on stdin.
- [ORB-24003] — `rebuilt_by` on session records (format 6) and the author and
  resolved target on audit lines, including refusals (audit format 2).
- [ORB-24030] — host-controlled identity: Daniel's writes only through his
  typed prompt, agent author names as attribution only.
- [ORB-25080] — `#scribe/global` records a typed line with `project: global`.
- [ORB-26204] — an agent Daniel instructs settles claims and deletes
  records through the CLI (proposed).
- [ORB-26240] — the personal memory base and the per-session work layer,
  `people`, recurring dates and light catalog entries (proposed).
- [ORB-26349] — extended the duplicate check's negation markers, and made
  Stop recovery decide coverage as of the turn.
- [ORB-26360] — entry `inactive_since` (format 12), whole-second Stop
  coverage, and the coverage check for resolved referents.
- [ORB-26640] — proposed tags as catchers, the fixed three-part memory and
  one SQLite store, with the four decisions of the accepted proposal.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.


## S12 writes format 5 and preserves write provenance

**Recorded:** 2026-11-10 · **Implementation decision**

The S12 sketch called the new tag document and agent entry shape format 4.
The shipped format-4 reader requires an entry cite and has no `agent`
content provenance variant. The workspace guide, STD-02@4 §R16 and
STD-03@2 §R23 require a new version rather than changing a shipped format
in place. S12 appends step 4→5, keeping prior steps unchanged. Old entries
and proposals remain readable without writes or locks; the next write
uses format 5. The step adds nullable `retirement_reason` to entries and
`updated_by` to proposals. The optional tags document carries creator and
last updater. These additive fields retain required reasons and writers
without destroying the original author. Input descriptions and reasons
are bounded at 300 characters before credential redaction.

Agent notes have `said: ""`, `content_from: {kind: agent}`, `cite: null`;
Daniel entries retain typed provenance. The decoder rejects inconsistent
author/provenance combinations. CLI writers construct agent authors even
when the supplied name is `daniel`. This is an accident guard, not an OS
security boundary (STD-05@1 §R5).

Prompt labels alone cannot guarantee attribution. When an ask request
contains agent evidence, the checked response renders the verified quotes
with each evidence source label, replaces model-written claim attribution,
and uses a fixed conflict notice with the checked cites. This gives up
some synthesized prose in mixed-source answers to enforce that an agent's
words never become Daniel's. Agent evidence has base weight 1.0×;
the ordinary named-tag boost is independent of author.

## Agent writes use their own name and edit only their own work

**Recorded:** 2026-11-10 · [ORB-24002] · **Overridable by Daniel**
**Superseded in part:** by "Daniel writes only through his typed prompt;
agent names are attribution" (2026-11-15): the `session catalog` Daniel
channel and the between-agents ownership rule (`not_author`, the
self-settle guard) are gone. The reserved names, `entry_not_active`, the
Daniel-entry rules and the settle `--reason` rule still hold.
**Code anchors:** `crates/scribe-core/src/store/agent.rs` (`check_agent_author`, the agent write guards), `crates/scribe/src/commands/session/catalog.rs`

### Context

"Every entry and proposal records its author" recorded `kind: agent`
whatever `--author` name a CLI write passed, so `entry add --author
daniel` mixed with Daniel's entries under `entry list --author daniel`,
and `session catalog --author daniel` from any agent's shell produced a
`cataloged_by` identical to Daniel's popup catalog. The agent write verbs
also let any agent rewrite another agent's note, update a retired note,
replace the reason on a proposal Daniel or the consistency routine
opened while settling it, and settle its own proposal — looser than the
skill's "an open proposal you opened" and than "agents never change,
retire or settle against Daniel's entries" (STD-05@1 §R1, §R2, §R5).

### Decision

- `daniel`, `scribe` and `consistency` are reserved author names. Every
  agent-channel write (entry, tag and proposal writes, `session
  activate|deactivate`, `session rebuild`, `session catalog`) using one
  is refused `reserved_author`; reads still accept them as filters.
- *(Superseded: no CLI path records Daniel; he types `/scribe catalog
  <id>`.)* `session catalog` keeps a Daniel channel: `--author daniel` records
  `cataloged_by: {kind: daniel}` only when stdin is a terminal — Daniel
  typing or pasting the tmux popup's command in a shell — and is refused
  through a pipe. An agent catalogs under its own name, recorded
  `{kind: agent}`, so the two are distinguishable in the record and the
  switch.
- *(Superseded: names are attribution only; `not_author` and the
  self-settle guard are removed. `entry_not_active` stays.)* Between
  agents, an agent edits only its own work. `entry update` and
  `entry retire` of an agent entry and `proposal update` require the
  record's author name (`not_author`); a non-active agent entry refuses
  updates (`entry_not_active`). `proposal settle` is the mirror image:
  the proposal's author cannot settle it (`not_author`), so a second
  agent settles a proposal about an agent entry. Daniel entries keep the
  metadata-only rule for every agent, and store tags stay shared
  vocabulary any agent may update, recorded in `updated_by`.
- Settling with `--reason` replaces the proposal's reason, so it is
  allowed only on an agent-opened proposal. On a proposal Daniel or
  scribe opened, a changed reason is refused `proposal_not_agent`;
  settling without `--reason` keeps the original reason, with the settler
  recorded in `settled_author`.

### Consequences

- Daniel's words and catalogs are marked by channels an agent cannot
  claim from its usual pipe, and one agent cannot silently rewrite
  another's note, a routine proposal's reason, or its own proposal's
  verdict.
- Cost: `--author` is still an unchecked label — a process running as
  Daniel can choose another agent's name or edit the store directly, and
  a script Daniel runs non-interactively cannot catalog as `daniel`
  (it can under an agent name). These refusals are accident guards, not
  permissions (STD-05@1 §R5). Cross-agent tidying of a note now goes
  through its author or a new note.

## Daniel writes only through his typed prompt; agent names are attribution

**Recorded:** 2026-11-15 · [ORB-24030] · **Daniel's decision**
**Supersedes:** the `session catalog` Daniel channel and the between-agents
rule of "Agent writes use their own name and edit only their own work"
**Code anchors:** `crates/scribe/src/commands/hook/scribe_command.rs` (`run_catalog`), `crates/scribe/src/commands/session/catalog.rs` (`begin`), `crates/scribe/src/commands/session/pass.rs` (a catalog switch passed as a catalog), `crates/scribe-core/src/store/agent.rs`

### Context

After ORB-24002, `--author` is still a name the caller claims. Any process
can pass `--author opus` and edit an opus note. `script -q /dev/null scribe
session catalog <id> --author daniel` gets past the terminal check with a
fake pty. Agents and Daniel run as the same Unix user, so a terminal is no
proof that Daniel is the one writing (STD-05@1 §R5).

### Decision

Daniel chose on 2026-11-15, between the candidate shapes in ORB-24030:

- **Daniel's writes go only through a channel agents cannot drive: the
  `#scribe` and `/scribe` lines in his typed prompt, read by the hook.**
  Cataloging a past session by name becomes the prompt command `/scribe
  catalog <id>`. The CLI refuses `--author daniel` on every write,
  including `session catalog`, whether or not stdin is a terminal. No CLI
  path records `{kind: daniel}`. Rejected alternatives: a secret that agent
  sandboxes cannot read (interactive sessions on the Mac run outside the
  sandbox as Daniel) and an OS confirmation prompt (a dialog on every
  catalog).
- **Between agents, author names are attribution only.** scribe records the
  name an agent gives, but no longer enforces ownership between agents.
  This removes `not_author` on `entry update|retire` and `proposal update`,
  and the self-settle guard. Per-crew credentials from Orbit were rejected
  as more work than this is worth. Daniel's entries keep the metadata-only
  rule for every agent, and the reserved names (`daniel`, `scribe`,
  `consistency`) are still refused.

### Consequences

- Only Daniel's typed prompt can create a Daniel-attributed record. A faked
  pty or a claimed name no longer reaches it.
- Any agent may tidy another agent's note or settle any agent proposal. The
  record keeps who wrote and who changed it (`updated_by`,
  `settled_author`), and the design no longer claims that one agent's work
  is protected from another.

### As built

- `/scribe catalog <id>` (an id or a unique prefix; also `$scribe catalog
  <id>` in Codex) makes `session catalog`'s checks and writes its switch
  with `cataloged_by: {kind: daniel}`, but runs no window in the hook: a
  catalog is many model calls. `session pass` now runs any switch with
  `cataloged_by` as a catalog (`origin: catalog`, attribution kept, final
  and off once the transcript is a day old), so the next pass finishes it;
  `session catalog <id> --author <name>` resumes it now, keeping Daniel's
  attribution. The tmux popup's Enter on a backfill candidate leaves
  `/scribe catalog <id>` in the paste buffer.
- The `not_author` code is gone from the CLI's codes; scripts matching it
  see the write succeed instead.
- Outside this repository, the interim limits that wait on ORB-24030 are
  to be removed by a constellation change: the "Until ORB-24030 lands"
  sentence in the constellation `AGENTS.md` (scribe paragraph), the same
  sentence and the "once ORB-24030 lands" clause in
  `.claude/skills/orchestration-session/SKILL.md`, and the learnings entry
  "Scribe's `--author` is a claim, not an identity" in
  `.claude/skills/orchestration-session/references/learnings.md`.

## Codex defaults to Luna and shares the domain

Daniel requested complete Codex capture/catalog support, Luna as its default
scribe, a generic distributable skill and an initializer. Keep the existing
Claude contract and Models port. A provider-tagged control upgrade is preferable
to guessing provenance or transport from model names. Codex defaults to the
configured Luna id `gpt-6-luna`; explicit provider/model overrides are honored,
with no silent Claude fallback. Global jobs preserve their existing model table
unless the caller explicitly selects Codex.

Host transcript content provenance, not an invoked skill or a hook's own claim,
is required for human writes (STD-05 §R1). Metadata absence refuses recording;
unknown transcript versions never block ordinary coding. Checks remain accident
guards against confused same-user processes (STD-05 §R5), and do not replace the
pending host-controlled author identity design or relax agent author limits.
Current-turn bootstrap avoids replaying historical settlement commands. Effects
precede the cursor and existing idempotent actions make crash replay safe.

Codex exec uses the existing bounded process-group supervisor (STD-03 §R11–R15).
Private isolated cwd/schema/instructions and disabled hooks/plugins/tool features
prevent recursive writes; an installed-CLI synthetic local provider probe
confirmed no tools were advertised. Optional session fields are made required
and nullable on the Codex strict wire, without changing the Claude schema.

The generic skill is a first-class source and embedded binary asset. `scribe init`
provides useful offline distribution after a binary install; project hooks use
the resolved absolute binary. A local Codex marketplace is an alternate install,
not a public release. Digest-owned receipts and conflict refusal preserve edits;
trust remains Codex's digest-bound host action (STD-05 §R21).

## Narrow provider startup writes use a generated Orbit variant

An actual sandboxed Luna pass exposed two startup writes outside Codex's
ephemeral log/sqlite directories: its `tmp` subtree and existing
`installation_id`. A read-only Codex home was insufficient before inference.
A native sandbox probe succeeded with only those two writes; sibling auth and
configuration files remained unwritable. Do not copy authentication or grant
the whole provider home write access (STD-05 §R1, §R4).

Orbit's generic `fs.write_files` capability expresses the installation file
without granting its parent. Directory `fs.write` keeps its existing semantics.
The Codex-enabled package therefore requires three explicit provider paths and
the `codex` executable. It is generated from the canonical Orbit package, with
byte/mode parity checked in `ci-fast`, retaining the same tools, schemas, jobs
and disabled schedules except for the required configuration and narrow grants.
The default package stays usable without Codex or extra filesystem permissions.
This avoids conditional template syntax, broad fallback paths and manually
maintained provider manifests. The runtime smoke must exercise the actual
Orbit sandbox with synthetic transcript/store data; direct Luna success alone
does not establish plugin compatibility.

Codex can report nonterminal `error` notifications and completed error items
while retrying WebSockets or falling back to HTTPS, then return a valid model
answer and complete successfully. An actual sandboxed session pass exposed
this sequence. Recognize those typed diagnostic events with string messages;
do not classify success by matching English retry text. Accept only after
exit zero, exactly one schema-valid answer and a completed turn, with no later
events. Failed turns, tool activity, unknown events and malformed diagnostics
still refuse. The [Codex JSONL contract](https://learn.chatgpt.com/docs/non-interactive-mode#make-output-machine-readable)
documents `error` separately from `turn.failed`; the native runtime probe
establishes the observed recovery behavior.

## The skill briefs the last session and suggests store upkeep

**Code anchors:** `skills/scribe/SKILL.md` (and its packaged copies)

Daniel, 2026-11-15: "When starting a new session on a project, the agent
should look up most recent scribe session note on that project. If there are
any pending, unresolved, or parked items from last session, they should brief
the user. in fact, they should just brief the user on the last session. The
agent should proactively suggest potential entries, new tags for better
cataloging, and deactivate stale sessions."

- The skill, not a `SessionStart` hook, carries the brief. The project's own
  last session is the one that matters, the agent can label the summary as
  of its writing and check an item before acting on it, and the umbrella's
  global-last-session hook was removed the day before for being stale and
  not project-specific.
- Entries stay the human's typed `#scribe` lines; the agent offers the line
  to type. Tags are created under the agent's own name once the human
  agrees.
- Deactivation is the one write outside `add`, and only on the human's
  explicit yes for that session. `session deactivate` refuses reserved
  names, so the human cannot run it as themselves; the agent runs it under
  its own name and names the approval in the reason. Age alone is not
  staleness (§5.7): the agent proposes only a session whose feature was
  removed, replaced or superseded, checked against the repository or task
  state.

## A typed line is global only when it opens with #scribe/global

**Recorded:** 2026-11-15 · [ORB-25080]
**Code anchors:** `crates/scribe-core/src/capture.rs` (`GLOBAL_OPENER`, `FORMS`)

### Context

Every entry has a `project`, a repository slug or `global`, and
`scribe entry list --project global` already filters on it. The design
said `global` applies when Daniel says so, but a typed `#scribe` line
always stored the cwd's project. The only way to write `project: global`
was `scribe entry add --project global`, which records an agent author,
not Daniel's own words.

### Decision

The global marker is the opener `#scribe/global` in place of `#scribe`.
The rest of the line is unchanged, so it works on all three capture forms
(`#scribe/global <tag>: <content>`, `#scribe/global add <content> to <tag>`,
`#scribe/global add this to <tag>`). The entry is stored with
`project: global` and `author.kind: daniel`. A line that still opens with
`#scribe` keeps the cwd's project.

The opener is a different token from a topic hashtag and from a tag word.
`#global` at the end of a line stays a topic (`global` is ordinary
kebab-case, and the topic scan would have consumed a `#global` marker).
A tag or alias named `global` stays addressable as `#scribe global: …` or
`#scribe add … to global`. A trailing word inside the content would have
been ambiguous, so the marker is only the first word.

`#scribe/` followed by anything other than `global` is refused, and so is
`#scribe/global` on a line that does not record an entry (`approve`,
`reject`, `retire`, `forget`, `distill`): nothing is written and nothing
is settled. The refusal's forms text includes `#scribe/global`.

### Consequences

- Daniel can mark one line in a prompt as global and leave the next line
  on the cwd's project. Moving an existing entry between projects is
  unchanged and still out of scope.
- Cost: a line that used to be ignored because it began `#scribe/<word>`
  is now an instruction and is refused. `#scribe/global` is reserved, so
  it cannot be the start of content.

## Action tags are marked in config, and three ship

**Recorded:** 2026-11-15 · [ORB-25112] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/tag.rs` (`SHIPPED_ACTION_TAGS`, `ActionTag`), `crates/scribe-core/src/config.rs` (`RawTag`)

> **Superseded in part, 2026-11-20 ([ORB-25977]):** the shipped set. `task`
> and `friction` no longer ship; `standard`, `remind` and `idea` do
> ([see below](#task-and-friction-no-longer-ship-as-action-tags)). The
> mechanism (config marks `action = true`, the switches, the window, the
> consequences for entries) stands as written; the sentences below that name
> `task` and `friction` as shipped describe the original choice.
>
> **Superseded in part, 2026-11-21 ([ORB-26405]):** "unless it claims their
> names". A config tag of the same name still overrides only the fields it
> sets. A different config entry that claims a shipped name or alias does not
> make the file `config_invalid`: Daniel's config wins, the shipped tag loses
> only that word, and `tag list` warns
> ([see below](#config-tags-merge-with-the-shipped-set)).

### Context

Daniel wants `#scribe standard:`, `#scribe task:` and `#scribe friction:`
to capture work for consumers outside scribe (a standards intake, Orbit
task and friction intakes), and agents to write such entries themselves,
while decisions and preferences stay Daniel's memory. A new kind of work item
should not need a code change.

### Decision

A tag is an action tag when config says `action = true`; `require_quote`
and `ask_first` refine it and are refused on any other tag. `standard`
(`require_quote`), `task` and `friction` ship with no aliases and survive a
config that declares its own `[tags]`, unless it claims their names; a
config tag of the same name starts from the shipped settings, so
`[tags.task] ask_first = true` changes only the switch. An action entry is
an ordinary entry under its tag: same id scheme, same settling verbs. What
is added around it (window, quote, pickups) is null or absent for memory
tags. `ask` and the session brief leave action entries out unless asked
for them.

### Consequences

- `tag list`, the unknown-tag refusal and config without tags now name
  five tags, not two; a store that already has a `task` directory under a
  memory tag of that name keeps it only if config declares it.
- Cost: three tags exist in every store whether or not a consumer reads
  them; an entry nobody picks up shows in the stuck line instead of
  disappearing.

## An action entry's window is capped at 40 turns and 12,000 characters

**Recorded:** 2026-11-15 · [ORB-25112]
**Code anchors:** `crates/scribe-core/src/capture.rs` (`action_window`, `WINDOW_TURNS`, `WINDOW_CHARS`, `WINDOW_TURN_CHARS`)

The window keeps the turns since the previous `#scribe` line, so a
consumer sees the discussion that produced the entry without a model call.
A long session between two `#scribe` lines could be megabytes; the cap
keeps an entry file readable and a `--window` listing bounded. 40 turns
and 12,000 characters (with any single turn cut to 4,000) hold a typical
correction thread and match the order of the catalog window
(`window_chars = 15000`). Oldest turns go first, because the turns nearest
the line explain it; the prompt itself is always kept, and `truncated` and
`omitted` say what was dropped. Cost: a correction whose context began
more than 40 turns back loses its start; the consumer can still read the
session transcript through the `session:<id>` cite.

## Agent entries have a five-minute cancel window

**Recorded:** 2026-11-15 · [ORB-25112] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/action.rs` (`CANCEL_WINDOW`), `crates/scribe-core/src/store/pickups.rs`

Agents write action entries unasked and announce them in one line; the
human cancels with `#scribe retire <id>`. That only works if no consumer
has acted before the human reads the line. For five minutes after an agent wrote
it, the read surface reports the entry as inside its cancel window and
`entry pickup` refuses it; the human's own entries have none. Five minutes
covers reading the reply and typing the retire line in the same session;
longer would delay every intake run. Cost: an intake polling more often
sees fresh agent entries it must skip and revisit.

## Pickups live in their own document

**Recorded:** 2026-11-15 · [ORB-25112]
**Code anchors:** `crates/scribe-core/src/store/pickups.rs`, `crates/scribe-core/src/store/format.rs` (`PickupsDocument`)

A consumer must record that it took an entry, and later that the human
rejected its result, without rewriting the entry (entries are the human's
words or an agent's attributed note, and their files are never rewritten
by a third party). `pickups.json` holds `{entry, author, at, result,
rejected}` notes, written by `entry pickup` under the lock and audited;
the same note twice changes nothing. The stuck count, `tag list`'s rejected
count and each entry's `action.pickups` are derived from it. Cost: a
second document to keep consistent; a pickup naming an entry that is
later forgotten stays as a dangling note.

## An action entry is stuck after one hour without a pickup

**Recorded:** 2026-11-15 · [ORB-25112] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/action.rs` (`STUCK_AFTER`), `crates/scribe/src/commands/hook/session_start.rs`

The `SessionStart` hook prints one line counting active action entries
older than one hour with no pickup, per tag, and nothing otherwise. An
intake that runs at least hourly has picked up anything older, so the
line means a consumer is down or the entry is under a tag nobody reads.
Cost: with no intake running at all, every session start repeats the
line until the entries are picked up or retired.

## The quote check runs in the CLI against this host's transcript

**Recorded:** 2026-11-15 · [ORB-25112]
**Code anchors:** `crates/scribe/src/commands/entry.rs` (`verify_quote`), `crates/scribe-core/src/action.rs` (`quote_found`)

A `standard` entry changes rules for every agent, so an agent may write
one only from the human's own correction: `--quote` must be a verbatim
span of one of the human's typed turns in the session named by `--related
session:<id>`. The transcript lives on the host the session ran on, and
reading it is I/O, so the CLI locates it as the hooks do (with the same
provenance checks) and refuses `quote_not_found` when the words are not
there; the store only enforces that a `require_quote` entry has one. Cost:
an agent on another host than its session's transcript cannot write a
`standard` entry; it files a friction instead.

## The owner host reads action entries over SSH

**Recorded:** 2026-11-15 · [ORB-25112] · **Overridable by Daniel**
**Code anchors:** `crates/scribe/src/commands/entry.rs` (`list --after --window`, `pickup`)

### Context

Entries are captured where Daniel types, mostly the Mac. Their consumers
(the standards, task and friction intakes) run on the owner host,
dk-server-2. The store is one host, one store: nothing in scribe
replicates or merges stores, and a second writer elsewhere would break the
lock and the id scheme.

### Decision

The owner runs the read surface on the Mac over SSH, with no copy:
`ssh <mac> scribe entry list --tag <tag> --after <cursor> --window --json`
for new entries (the cursor is the newest `created` it has seen), and
`ssh <mac> scribe entry pickup <id> --author <intake> [--result <ref>]
[--rejected]` to note what it did. Both are ordinary CLI calls, audited on
the Mac, and the store stays single-writer on its own host. scribe adds no
network code and never calls Orbit.

### Consequences

- Precondition, unverified when this was recorded: a non-interactive SSH
  login from dk-server-2 to the Mac with `scribe` on its `PATH`. The
  documented route today is the reverse (Mac to the box). Until it exists,
  the intake cannot read Mac entries; the stuck line on the Mac makes that
  visible.
- An entry captured on dk-server-1 or dk-server-2 is read the same way
  from that host's store.
- Cost: the intake depends on the Mac being awake and reachable; entries
  wait, and are counted as stuck, while it is not.


## Session passes propose unmarked statements; Daniel alone creates their entries

**Recorded:** 2026-11-15 · ORB-25163 · background-agents candidate use 6
**Code anchors:** `pass/candidates.rs`, `store/candidates.rs`, `proposal.rs`,
`commands/session/pass.rs`, `skills/scribe/SKILL.md`

Daniel approved the background-agent uses on 2026-11-15. The opted-in pass
already reads these turns, so it returns up to three candidate preferences
or decisions beside each window's record, sharing three slots across the
session's invocation. Twenty open candidates bound the store-wide queue.
The added output is at most 1,680 value characters per window plus JSON
syntax (three quotes of 400, message ids of 128 and tags of 32), without an
extra model call. Relevant duplicate context adds at most 6,000 value
characters of input. These bounds implement STD-08 §R1 and §R3; full details
are in design §5.3.

The pass checks an exact span from the named user's own current-window turn
and derives its provenance from the original transcript. Pasted and quoted
material, assistant words, marked instructions and action tags are excluded.
Model judgement handles semantic novelty; deterministic normalized and
lexical checks under the store lock suppress existing and rejected statements.
This may conservatively drop closely related new wording. A candidate whose
quote would be redacted is omitted to retain exact quotation on approval.

Store format 9 adds a nullable candidate payload and a new proposal kind,
with an append-only 8→9 upgrade. Existing proposal fields keep their types;
for this new kind, `older` is the reserved eventual entry id. Reserving it
without a file preserves both the machine contract and idempotent approval
recovery. Typed and agent entry allocators count every reservation.
The approved proposal is the durable decision before writing its entry,
matching consistency approvals' recovery journal (STD-03 §R8). A replay or
consistency scan completes the exact reserved entry once, preserving any
later retirement or forgetting. The original turn is the entry's cite;
`settled_by` remains the separate approving turn. Rejection keeps its payload
for suppression and creates nothing. Agent proposal mutation refuses this kind.

This refines the earlier skill decision to offer ready-to-type memory lines:
in an on session, the skill stops doing that and briefs open proposals instead.
Off or unknown sessions keep the manual suggestion flow. Action tags remain
agent-written under their existing rules. Rebuilds create no proposals.

## Findings are scribe entries with no worker

**Recorded:** 2026-11-16 · ORB-25177 · background-agents candidate use 9
**Code anchors:** `tag.rs`, `entry.rs`, `store/agent.rs`, `store/format.rs`,
`pass/candidates.rs`, `ask.rs`, `search.rs`, `skills/scribe/SKILL.md`

Daniel (2026-11-16) asked scribe to catalogue what agents state across
sessions instead of letting it be forgotten or re-derived, and approved a
`finding` tag ("have the doc session add the finding tag"). Pending work,
check-backs and future considerations already had homes as the action
tags `task`, `remind` and `idea`; findings were the gap. scribe does not
become a second task tracker: a finding has no consumer, no transcript
window and no pickup, and neither the session-start line nor the session
brief ever lists findings, so neither grows with history.

A finding is an agent's measured or verified result with a pointer to
its evidence and the date it was true. `entry add --tag finding` refuses
an entry with no evidence pointer (`evidence_required`) and refuses the
fourth finding of a writing session (`finding_cap`, cap 3), both under
the store lock; a write that names no session counts under its author
and the write's day. `ask` and `record search` return a finding as that
agent's claim as of its date, with its pointer, never as Daniel's words.
Store format 10 adds `evidence`/`as_of` to entries (`null` on upgrade)
and `role`/`evidence` to a candidate proposal, with an append-only 9→10
upgrade.

The session pass is the safety net for what agents did not record: it
also reads an assistant turn's own recap and may propose, under the same
3-per-pass and 20-open caps, a measured result as a `finding` candidate
or something not done or not verified under `task`, `remind` or `idea`.
Daniel's typed `#scribe approve <p-id>` on such a candidate writes
exactly one agent-authored entry quoting and citing the assistant turn —
never one attributed to him — and `#scribe reject` closes it for good.
The added output per window stays bounded (design §5.3: at most 3,216
value characters including an optional 512-character evidence pointer,
STD-08 §R1, §R3), and no candidate is ever opened from a tool payload.

## `task` and `friction` no longer ship as action tags

**Recorded:** 2026-11-20 · [ORB-25977] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/tag.rs` (`SHIPPED_ACTION_TAGS`), `crates/scribe-core/src/models.rs` (session-pass candidate prompt)

### Context

The decision "Action tags are marked in config, and three ship"
([ORB-25112]) shipped five action tags, among them `task` and `friction`,
so that an intake outside scribe could turn an entry into an Orbit task or
friction record. Daniel (to 0-aide, 2026-11-20T06:12Z): "we don't really
need friction and tasks - they can be created via orbit. what do you
think". The chief agreed and proposed this change; Daniel (chief's chat,
2026-11-20): "Yes, file it". Measured on the Mac store at 2026-11-20T06:13Z:
no `task` entry and no `friction` entry since they shipped; `standard` 2,
`remind` 1. Every session that can write a scribe entry can call Orbit
directly, and a direct filing carries complexity, context files and
acceptance criteria that a relayed entry cannot.

### Decision

This supersedes the shipped set of [ORB-25112], and only that. `standard`,
`remind` and `idea` ship; `task` and `friction` do not. Everything else of
the older decision stands: a config that marks `[tags.task] action = true`
(or `friction`) still makes it an action tag with the window, cancel window,
pickups and stuck line, and starts from no shipped settings, because none
exist for the name. The session pass no longer proposes open work under
`task`: work that is not done is filed in Orbit by the agent
(`orbit.task.add`, `orbit.friction`, in every session), and a check-back
stays `remind`. No store-format change: an entry already stored under `task`
or `friction` is an entry under a tag the config no longer defines, so it
is still listed, shown and settled by id.

### Consequences

- `tag list` and the unknown-tag refusal on a store with no `[tags]` config
  name `decisions`, `finding`, `idea`, `preferences`, `remind` and
  `standard`; `#scribe task:` and `#scribe friction:` are refused as unknown
  tags unless config declares them.
- The consumers in the constellation repository (the scribe relay's tag
  list, the standards plugin's work-intake) change separately; either order
  works, since `scribe entry list --tag <unknown> --json` answers an empty
  list with exit 0.
- Cost: a store that holds `task` or `friction` entries keeps them readable
  and settleable, but they no longer show in `tag list` or count in the
  session-start stuck line unless config declares the tag again.

## Decisions are recorded on their Orbit task, not in a shipped tag

**Recorded:** 2026-11-20 · [ORB-25980] · **Overridable by Daniel**

### Context

scribe shipped two memory tags, `decisions` and `preferences` ("Only
decisions and preferences ship by default"). Daniel (chief's chat,
2026-11-20): "I think decisions are also weak. we log decisions on the orbit
task itself mostly, quoting me verbatim. what do you think". The chief agreed
and proposed this change; Daniel: "Yes, file it". Measured on the Mac store
at 2026-11-20T06:15Z: 0 entries under `decisions`; the whole store held 4
entries (1 `preferences`, 1 `remind`, 2 `standard`) against 26 cataloged
sessions and 20 open proposals. A choice about a piece of work is written on
its Orbit task with Daniel's words quoted, and a design choice in the
repository's `4_decisions.md`. `ask` already reads Orbit task records as
evidence, so those are found and cited. A standing choice with no task to
sit on is a preference.

### Decision

This supersedes the shipped set of "Only decisions and preferences ship by
default", and only that. `preferences` and `finding` ship as the memory
tags; `decisions` and its aliases `decision` and `dec` do not. A config that
declares `[tags.decisions]` (with or without those aliases) makes it work
like any config tag. The session pass proposes unmarked preferences only: a
one-time choice about a piece of work is not proposed, because it belongs on
its task, and a standing choice is proposed as a preference. The consistency
scan works over whatever memory tags the store has. The skill no longer reads
`decisions` or offers a decision line; it says a decision is recorded on its
Orbit task with the human's words quoted, or in the feature's
`4_decisions.md` when it is a design decision. No store-format change: an
entry already stored under `decisions` is an entry under a tag the config no
longer defines, so it is still listed, shown, searched, cited by `ask` and
settled by id.

### Consequences

- `tag list` and the unknown-tag refusal on a store with no `[tags]` config
  name `finding`, `idea`, `preferences`, `remind` and `standard`;
  `#scribe decisions:`, `#scribe decision:` and `#scribe dec:` are refused as
  an unknown tag unless config declares them. `decision` is free as a name,
  for example for the action tag the standards plugin's work-intake reads,
  which is config-declared in the constellation repository and unchanged.
- A store that registered `decisions` in `tags.json` keeps the registration;
  one that declared it in `config.toml` keeps it working.
- Cost: a standing decision with no task to sit on is recorded as a
  preference, and a `decisions` entry stops showing in `tag list` unless
  config declares the tag again.

## Claims carry a statement, tags carry capture rules, and issues are their own record

**Recorded:** 2026-11-21 · [ORB-26214] · **Overridable by Daniel** · proposed, not built
**Paths:** `crates/scribe-core/src/tag.rs` (`SHIPPED_ACTION_TAGS`, `FINDING_TAG`), `crates/scribe-core/src/pass/candidates.rs`, `crates/scribe-core/src/store/candidates.rs`, `crates/scribe/src/commands/hook/user_prompt_submit.rs` — design §11

### Context

Daniel, chief's chat, 2026-11-21: "few issues with current cataloging. 1.
they are out of context - I propose having the scribe summarize the context
and quote verbatim where appropriate 2. decisions and tasks are just noise
- good thing we got rid of them 3. to reduce noise, we need a set of
trigger words and criteria for each tags - start with sensible defaults and
able to be modified 4. let's also get rid of remind 5. let's introduce
"issues" - we will catalog issues we've faced, e.g. github api 429 request
issue we faced yesterday, cpu resource exhaustion incidents. and upon
similar incidents, the scribe will append the new incident under
"occurrences" section with date and summary, fixes."

Then: "suggested triggers () * idea: "have * an idea", "what do you think
about* idea" - anything along these lines. * issues: "can you investigate
....", "rca", "find root cause" - the agent should persist this and answers
that follow * finding - let's delete this for now." On who writes an
issue: "agent should write it - summarize context and add verbatim
quotes." On `standard`: "ok let's keep standard, and route it to a larger
model." Earlier the same day: "Keep idea". He answered "Yes, file it" to
filing the revised design.

Measured on the Mac store, 2026-11-21: 20 open candidate proposals, the
store-wide cap, so the pass opens none; 14 under `decisions` or `task`,
tags that no longer ship, and 2 under `finding`; several unreadable alone
(p-8: "ok that makes sense. then let's keep it like this.").

### Decision

Six choices, specified in design §11. Each supersedes the earlier rule it
names, and only that.

1. **A claim's content is a statement; the quote stays attached.** The pass
   writes a standalone statement (300 characters) and a subject (80) beside
   the verbatim quote; an empty subject opens no claim. On approval the
   statement is the entry's content, the quote is its `quote`, and
   `stated_by: {kind: scribe, name: session-pass}` says whose words the
   content is. This supersedes "approved content is the exact quote"
   (§5.3, from "Session passes propose unmarked statements; Daniel alone
   creates their entries"). Older candidates keep that rule.
   Alternatives: keep the quote as content and show a context field beside
   it (cost: `ask` still answers from the fragment and lists stay
   unreadable); let the model rewrite the quote (cost: no verifiable words
   remain); quote a wider span (cost: p-8's referent is often several turns
   back, so the span grows past the 400-character quote bound and is still
   not a sentence that stands alone).
2. **Two gates per tag: a model-free trigger, then the criteria.** Config
   `[tags.<name>.capture]` with `from`, `route`, `triggers`, `criteria` and
   `not`, shipped defaults overridable field by field, the admitting
   trigger and criteria kept on each claim, and `scribe tag show`.
   Alternatives: criteria only, given to the model (cost: today's noise,
   and a model call on every window); triggers only (cost: "never" fires on
   ordinary sentences); a model-chosen trigger list (cost: not checkable
   without a model and not editable by Daniel).
3. **`remind` and `finding` stop shipping, and the pass stops reading
   assistant turns.** This supersedes the shipped sets of "Action tags are
   marked in config, and three ship", "`task` and `friction` no longer ship
   as action tags" and "Decisions are recorded on their Orbit task, not in
   a shipped tag", and the pass's safety net in "Findings are scribe
   entries with no worker". The `evidence`/`as_of` fields and the
   `require_evidence` guards stay. Alternatives: keep the tags shipped but
   never proposed (cost: unused tags stay in `tag list` and the skill);
   delete the finding code too (cost: a persisted-contract change and the
   existing findings' labels, for no observed gain).
4. **An open candidate under a tag no longer in the tag table closes
   itself** as `rejected` with `closed_reason: tag_retired`, before the cap
   is counted. Alternatives: Daniel rejects them by hand (cost: 16 typed
   lines today, and again at every tag change); delete them (cost: they
   leave the suppression set and the pass may propose them again); raise
   the cap (cost: the noise the cap bounds).
5. **Issues are a record kind the agent writes, with no approval.** A
   header (title, signature, status, cause, fix) over occurrences newest
   first; the prompt hook flags his investigation request without a model;
   the answering agent searches, then writes an issue or an occurrence; his
   request is quote-checked, the agent's error lines are labelled its
   evidence. Alternatives: an entry under an `issue` tag (cost: an entry is
   one statement, so each occurrence would rewrite it); an Orbit friction
   (cost: it is scoped to one product and closes, so the next incident
   elsewhere starts from nothing); the pass writes issues from the
   transcript (cost: the parsed conversation has no tool output, so the
   error lines and the fix would be lost); a candidate Daniel approves
   (cost: against his "agent should write it", and the queue the cap
   bounds grows).
6. **Idea capture is built on the candidate route, not the span route.**
   ORB-25983's span route was archived with it; under `route = "write"` the
   pass writes the quote-checked claim as an `idea` entry instead of
   opening a candidate. Alternative: build the span route for ideas alone
   (cost: a second capture mechanism for one tag, with its own checks).

Differences from the chief's proposal on ORB-26214: the `route` and
`subject` fields are added; `from` accepts only `daniel` until a need is
seen; `*` is defined as zero to four words; the approved entry records
`stated_by`; the cleanup closes as `rejected` with a reason, in the
candidate write path and the consistency scan; the issue hook runs in
every interactive session, on or off; `issue` is a reserved tag name and
an occurrence is cancelled with `#scribe forget`; the `require_evidence`
guards stay; ORB-25987's content becomes the statement and its point 4
(deferred assistant ideas) is not built until seen.

### Consequences

- What Daniel approves shrinks to `preferences` claims and `standard`
  entries; ideas and issues are cancelled, not approved.
- `ask` and the attribution check must read Daniel's words from `quote`,
  not from content, for every entry with `stated_by` set.
- Three store formats, one per step: 11 (capture provenance and
  `closed_reason`), 12 (statement, subject, `stated_by`), 13 (issues).
- ORB-25987 needs its content and point-4 criteria revised before it is
  built; ORB-26204's implementation follows step (a).
- Cost: an approved preference's content is scribe's sentence, not
  Daniel's, so a wrong statement he approved misstates him until retired;
  a preference phrased with no trigger is never proposed; an issue exists
  only when the agent follows the hook line.

## An agent settles claims and deletes records on Daniel's checked instruction

**Recorded:** 2026-11-21 · [ORB-26204] · **Overridable by Daniel** · proposed, not built
**Refines:** "Daniel writes only through his typed prompt; agent names are attribution" (no CLI path records `{kind: daniel}`), "Entries Daniel wrote are settled only by Daniel", and §5.3's "Agents cannot settle or edit candidate proposals"
**Paths:** `crates/scribe-core/src/store/agent.rs` (`change_agent_proposal`, the `daniel_only` refusal of a candidate), `crates/scribe/src/commands/entry.rs` (`verify_quote`, `RetireArgs`), `crates/scribe/src/commands/proposal.rs` — design §12

### Context

Daniel, chief's chat, 2026-11-21: "I will be interacting with the scribe
primarily to manage scribe artifacts, e.g. instruct scribe to approve
certain claims, delete certain artifacts, etc. the scribe will be using cli
tool to perform these operations." Asked "File a scribe design task so the
aide can approve claims and delete on your instruction?", he answered "Yes,
file it". "The scribe" is the formation's aide, an agent session he types
to.

Today the CLI refuses every candidate (`daniel_only`), refuses retiring or
rewriting his entries, and has no forget or delete verb; his only route is
one typed `#scribe approve|reject|retire|forget <id>` line per record.
Measured on the Mac store, 2026-11-21: 20 open proposals, all candidates.

### Decision

1. **His instruction is a checked span of a turn he typed.** The agent
   passes `--session <id> --instruction '<his words>'`; the CLI reads that
   transcript on this host with §4.1's provenance checks and requires the
   words to be a verbatim span of a plainly typed user turn, one of the
   last three and at most 30 minutes old, naming the target id (in his
   turn or in the reply he answered) and the act (model-free word lists).
2. **An approved claim from his turn is his entry; the agent is recorded
   as its settler.** `author: {kind: daniel}` as a typed approval writes
   it, with `approved_by: {kind: agent, name}` on the entry and
   `settled_author`, `settled_by` and `instruction` on the proposal; `ask`,
   `record search` and `entry show` say an agent approved it on his
   instruction. Assistant-turn claims still become agent entries.
3. **"Delete" never removes a file.** A claim is rejected
   (`closed_reason: instructed`), an entry retired, or forgotten only when
   his words say forget, erase or for good (new `entry forget`, with
   `--confirm` per STD-01@2 §R5), a session deactivated. The audit keeps
   ids and codes; after `forget` the instruction's words are dropped too.
4. **Only an interactive session he is in.** The instruction turn must carry
   Claude Code's interactive `entrypoint: "cli"`, not `"sdk-cli"` (every
   `claude -p` run), and be fresh and typed by him as in 1.

Alternatives considered:

- **Keep `#scribe`-only settlement** (the aide replies with the lines for
  him to type). Cost: one typed line per record with its id (20 lines for
  today's queue), against his stated way of working, where the aide runs
  the CLI.
- **`#scribe` with several ids per line** (`#scribe approve p-4 p-9`), the
  aide composing it for him. Cost: he still types or pastes ids, a
  pasted line can land in a `<pasted_content>` block, which §4.1 skips,
  and it does not serve deletes he states in words.
- **Lift `daniel_only` with no evidence** (any agent's `--author` may
  settle a candidate). Cost: any agent, including headless Orbit runs, can
  turn its own reading of a chat into his preferences; ORB-24030 removed
  exactly this kind of claimed identity.
- **A one-time code the hook shows him** (a `systemMessage` the model
  does not see, which he retypes to the aide). Cost: a code to type on
  every instruction, and a hook write on every prompt; it is still
  forgeable by a process running as him, so it buys no boundary the
  transcript check lacks.
- **The hook records each typed prompt as a ticket the CLI redeems.**
  Cost: a store write on every typed turn of every session, for the same
  evidence the transcript already holds.
- **Write the approved entry as the agent's.** Cost: his own checked words
  lose their attribution and 1.5× weight in `ask`, which is what a claim
  exists to capture; the approver is recorded instead.
- **Delete by removing files.** Cost: a removed claim leaves the
  suppression set and the pass proposes it again; a removed entry breaks
  other records' cites and `superseded_by` links.

### Consequences

- The aide can work through his queue in words, and the store can still
  tell an instructed settle (instruction recorded, checked) from an agent
  acting alone (refused `daniel_only` as today).
- One store format step adds `instruction` on proposals and
  `approved_by`/`instruction` on entries; `closed_reason` gains
  `instructed`.
- Not built until seen: consistency proposals about his entries or
  content changes on instruction, session removal or `session forget`,
  forgetting a rejected claim's words, multi-id or description-based
  settling, and instructions from non-interactive or remote sessions.
- Cost: a claim he approved through the aide counts as his words; the
  checks are word lists, so a misdirected instruction can pass and only the
  aide's echo and his later retire catch it (a negated or excepted one is
  refused, [§12.2](./2_design.md#122-question-1--how-his-instruction-is-evidenced-and-what-the-store-verifies)); an agent that
  can write a transcript file as him can forge an instruction (an
  accident guard, STD-05@1 §R5, not a boundary); the aide must run on the
  Mac, where the store and its transcript are.

## scribe keeps a descriptive memory the pass writes without approval

**Recorded:** 2026-11-21 · [ORB-26219] · **Overridable by Daniel** · steps (a) built (ORB-26258, store format 19), (b) built (ORB-26259, store format 20) and (c) built (ORB-26260, store format 21)
**Reverses:** [3_vision.md §4](./3_vision.md#4-deliberately-out-of-scope)'s "no inference of preferences from behaviour", for descriptive memory only
**Paths:** `crates/scribe-core/src/pass/` (the pass's model output), `crates/scribe-core/src/consistency.rs`, `crates/scribe/src/commands/hook/` (`session-start`) — design §13

### Context

Daniel, chief's chat, 2026-11-21 (ws_constellation ORB-26194, relayed by
the chief): "I think we should expand scribe's role to manage memory
system. we already keep track of sessions, preferences, and issues (these
are the main defaults - rest are my additions). What we are lacking is a
holistic memory, e.g. user profile, interests, projects, working
preferences, etc. we used to do this back in the days, checkout
knowledgebase/almanac/about-me. can you propose a design". On the chief's
four sections: "let's just include profile, projects, and working."

Then, of a regular user: "the scribe design was been focused too much on
my personal use. I think now thinking from the perspective of a regular
user like my wife. She is bounded to chatGPT and resistant to change
because of memory." "memory --- provides a holistic overview of the user
-- akin to how a human builds a \"sense\" of another person; preferences,
projects, recent events/sessions (list last n title and session_id?)".
"Say, my wife had a sad event yesterday and is in solemn mood; humans will
adjust their interaction based on that … Or if there is a big exciting
event coming up, like a wedding, asking about how that's going … would
feel more personalized." On the `now` lifecycle: "I would say don't fade
it out until the due date. … It feels nice when other people remember
details about you (not me really but for my wife this is true) even after
the event (not reference it but at least be able to recall)." "I would say
session is a dump of history. and we add \"events\" or something, for big
life events like getting married, having a child, graduating, promotion,
moving to a new city, etc. and we can promote some of \"now\"." "yes
milestones sounds good." On sending older milestones through `ask`:
"should we still restrict to \"ask\" for older sessions? I don't see the
added benefit." To filing each step: "Yes, file it", "Yes, add it".

What the old `about-me` got wrong, read by the chief on 2026-11-21: it was
run by hand (last 2026-07-08) and went stale; it was whole-file prose, so
no claim had its own source or date and each pass rewrote everything;
stable facts and current priorities shared a file; nothing injected it at
session start. Measured: 60 Claude auto-memory notes on this host
(2026-11-20) against 2 scribe `preferences` entries.

### Decision

1. **One record kind, `memory`, with five fixed sections**: `profile`,
   `projects`, `working`, `now`, `milestones`. An item is a statement
   with occurrences, each a verbatim quote of the user with its date and
   session, the shape of §11.4's issues.
2. **Writing is implicit and scribe's.** The session pass writes at most
   three items per pass from the user's typed turns of an on session, each
   quote checked verbatim; the author is `{kind: scribe, name:
   session-pass}`, never Daniel; there is no approval step.
3. **Memory describes; preferences bind.** A `working` item is never a
   rule; one that recurs in three sessions is offered as a `preferences`
   candidate he approves.
4. **Time is derived, not written.** Decay (90 days, `projects` and
   `working`; `profile` never decays) and the `now` windows (until its date
   and 14 days after; 7 days after the latest mention when undated) are
   computed when read, so the read-only `SessionStart` hook needs no
   routine.
5. **`now` stores what was said, never a mood**; the `acknowledged` mark
   limits unprompted mentions and never removes an item from the page.
6. **`milestones` are permanent, all on the page**, promoted from `now` by
   three editable defaults (a profile fact changes; recurs in three
   sessions; the user says so).
7. **The page goes to the agent of an interactive session only**, about
   2,400 characters, with the last 5 sessions as the index.
8. **Sensitive content** is refused (credentials, finance, health detail,
   third parties' identifying details) except a stated personal event in
   `now`, kept with the short window, promoted only by recurrence or his
   word, and removable with `#scribe forget`.
9. **Every section is switchable alone, and personal life is a setting
   off by default.** `[memory.sections]` switches each of the five; with
   `[memory] personal = false`, the shipped default and Daniel's work
   store, no personal event or household fact is written in any section.
   Daniel approved `work` and `personal` presets on 2026-11-21 ("Yes, add
   presets"), designed in [ORB-26240] after this decision; this one only
   guarantees the settings they set.

   > **Superseded in part, 2026-11-21 ([ORB-26240]):** point 2's source
   > (on sessions only) and point 9's default and its presets. `personal`
   > ships on in every store, off interactive sessions feed the personal
   > base, and the separation is per session: see
   > [Memory has a personal base from every typed session and a work layer from on sessions](#memory-has-a-personal-base-from-every-typed-session-and-a-work-layer-from-on-sessions).
   > The section switches stand.

   > **ORB-26240 parked 2026-11-21 ([ORB-26256]):** point 2 stands again
   > (an on session is the only source). Point 9's default stays as
   > ORB-26240 set it, `personal` on, and means the pass of an on session
   > may keep personal elements; see
   > [The personal base, people, recurring dates and light catalog entries are parked](#the-personal-base-people-recurring-dates-and-light-catalog-entries-are-parked).

Alternatives considered:

- **Keep Claude auto-memory only.** Cost: it is Claude's alone (Codex and
  other providers read none of the 60 notes), each note is prose with no
  quote, source session or date, nothing consolidates or decays them, and
  a regular user of another provider has nothing.
- **Revive the hand-run `about-me`.** Cost: it went four months stale
  because a person had to run the distiller; whole-file prose cannot go
  stale one claim at a time, every run rewrites the files, and nothing
  reads it at session start.
- **Approve each item, as preferences are.** Cost: a queue the 20-open cap
  already chokes (§11), for statements that bind nothing; against his
  principle of implicit value (1_overview.md, 2026-11-20).
- **Memory as entries under a `memory` tag.** Cost: an entry is one
  statement, so a repeat would rewrite it or duplicate it, and recurrence,
  the basis of promotion and of the preference proposal, could not be
  counted (the reason issues are their own kind).
- **One record kind per section.** Cost: five shapes, five sets of verbs
  and checks for items that differ only in their lifetime.
- **Store the inferred mood** ("is in a solemn mood"). Cost: a diagnosis
  scribe cannot check against his words, which outlives the day it was
  true and is the most sensitive thing it could keep.
- **Fade a `now` item a few days after it is said**, or remove it once
  raised (the chief's first proposal). Cost: Daniel rejected both: "don't
  fade it out until the due date".
- **Send milestones older than a year through `ask`.** Cost: Daniel saw
  no benefit; the list is a handful a year, so the page holds it.
- **Inject the page into every session**, headless runs included. Cost:
  personal events in every Orbit drain's context, for agents that never
  converse with him.
- **Keep personal events whenever they are typed**, with no `personal`
  setting. Cost: Daniel's work store, the shipped default, would fold in
  his family's events, which the presets he approved keep apart.
- **Decay `profile` like the others** (the chief's proposal applied 90
  days to every section). Cost: stable facts are rarely restated, so the
  page would lose who he is while keeping what he did lately.

Differences from the chief's proposal on ORB-26219, each named: the page
budget is 2,400 characters, not 1,500, because `now`, `milestones` and the
sessions list were added after the estimate; the pass merges a repeat at
once (`same_as` and a lexical guard) as well as in the weekly scan, since
`now` items live for days and recurrence must count in time; `profile`
does not decay; decay and windows are derived rather than written; a
milestone may be written directly when rule (a) or (c) holds on the turn;
sensitive events are flagged by a model-free word list as well as the
model, are kept only in `now` or `milestones` and are never promoted by
rule (a); a `now` statement with a mood word its quote lacks is refused;
the acknowledged mark is set by the agent's `memory ack` or by the user
raising the item, and a dated item is "not yet raised" again once its
date passes, which gives the follow-up; statements name third parties by
relation, not name; the page is injected only into interactive sessions;
an agent may retire or forget an item without his instruction check,
since items are scribe-authored; personal life is kept only with
`[memory] personal` on, off by default (the captain's steering of
2026-11-21, relaying the approved presets).

### Consequences

- Every interactive session starts with about 600 tokens of memory the
  agent reads; Daniel sees what is kept with `scribe memory show`.
- Store format step 19 (`memory_items`) adds the `memory/` directory and
  kind; step 20 (`memory_consolidation`) adds the merge, supersede and
  promotion keys and statuses, which a format-19 build refuses; step 21
  (`memory_preference_proposal`) adds a `working` item's
  `preference_proposal`, which a format-20 build refuses. A merge and
  a promotion move the occurrences rather than copy them, so the user's
  words are stored once and `forget` follows the link to every item that
  held them; a
  `working` promotion opens a `preferences` candidate within the 20-open
  cap.
- `ask` and the attribution check read the user's words only from
  occurrence quotes, never from statements.
- Not built until seen: importing `about-me` or Claude auto-memory,
  export to files, an `interests` section, hand-written items, a
  milestone cap, an edit verb, and always-on capture of a session the user
  did not switch on (designed in the entry below, parked 2026-11-21).
  A ChatGPT import and an app connector are not planned at all
  ([scribe reads Claude Code and Codex sessions only](#scribe-reads-claude-code-and-codex-sessions-only)).
- Cost: what the page says about him is scribe's sentence, chosen and
  sectioned by a model without his review, so a wrong statement shapes
  agents' behaviour until he retires it; the health and third-party gates
  beyond the word lists are model judgement; a personal event he typed in
  an on session is kept, unless forgotten, as long as the store.

## Memory has a personal base from every typed session and a work layer from on sessions

**Recorded:** 2026-11-21 · [ORB-26240] · **Overridable by Daniel** · proposed, not built · **points 1–7 parked 2026-11-21** ([ORB-26256], below)
**Supersedes in part:** [scribe keeps a descriptive memory the pass writes without approval](#scribe-keeps-a-descriptive-memory-the-pass-writes-without-approval), points 2 (on sessions as the only source) and 9 (`personal = false` by default, and the presets)
**Reverses:** [3_vision.md §4](./3_vision.md#4-deliberately-out-of-scope)'s "Scanning or cataloging sessions on scribe's own initiative", for the personal base and light catalog entries of interactive sessions only
**Paths:** `crates/scribe-core/src/pass/`, `crates/scribe-core/src/control.rs`, `crates/scribe/src/commands/hook/` (`stop`, `session-start`) — design §13.12–§13.16

> **Parked 2026-11-21 ([ORB-26256]).** Daniel: "park 2-5, keep 1, 6, 7".
> Points 1–7 below (the base pass, `/scribe private`, light catalog
> entries, `people`, recurring dates, and the settings they need) are
> parked, and nothing here is scheduled; the text stays for when he unparks
> it. What survives: `[memory] personal` ships on, and means the pass of
> an `/scribe on` session may keep personal elements. See
> [The personal base, people, recurring dates and light catalog entries are parked](#the-personal-base-people-recurring-dates-and-light-catalog-entries-are-parked).

### Context

Daniel, chief's chat, 2026-11-21 (ws_constellation ORB-26194, relayed by
the chief), on recurring dates and people: "Also, I forgot about recurring
dates, e.g. birthdays of the user and family members, wedding anniversary
date, etc. it would be nice if agent says happy birthday, or remind the
user on upcoming family birthdays. any suggestions for these? now that I
see, it may be appropriate to build a brief profiles of those close to the
user, e.g. husband's profession and birthday, if they are brought up. and
pets as well." To "Add a \"people\" section and recurring dates to
ORB-26219 before it starts?": "Yes, add both".

On presets, after "Yes, add presets": "just to be clear, let's say a user
inits with personal preset. the user is still able to do \"/scribe on\" for
work right? I think that is the right separation. work related sessions
should be selectively chosen. also, I am fine with using personal as the
base - I don't ever share anything personal anyways so there is nothing
there to fill".

To "Should a session you do not switch on still leave a light catalog
entry (title and summary)?": "Yes, light entry".

ORB-26219 had shipped `[memory] personal = false` as the default and read
only sessions he switched on. Need: Daniel's example of a regular user, not
yet observed in use; for him the base is expected to stay empty.

### Decision

1. **The split is per session, not per store.** Every store ships the same
   default. A **personal base** (`profile`, `now`, `milestones`, `people`,
   recurring dates; memory items only, no tag entries, no candidates, no
   approvals) is fed by every interactive session a human types in, on or
   off. A **work layer** (the session record with its log, claims, issues,
   `projects`, `working` and work facts in the base sections) is fed only by
   a session switched on with `/scribe on`. `sdk-cli` and unmarked sessions
   feed neither.
2. **A base pass** reads an off interactive session after 30 idle minutes
   or at session end, one `session` model call per window under §13.3's
   bounds, keeping personal-layer elements only.
3. **`/scribe private`** keeps one session out of the base and the catalog.
4. **A session that is off leaves a light catalog entry** (title, date,
   summary of at most 300 characters), as Daniel decided. It is the base
   pass's behaviour, not a setting: it stops only with `[memory] base =
   false` or `/scribe private`. The page's recent-sessions list prefers
   sessions switched on in the last 14 days.
5. **`people`** is a sixth section: a person or pet record made on a stated
   relation or a second mention in another session, at most 5 facts of 120
   characters and 4 dates, each with his verbatim quote. A third party's
   health, finances, beliefs, contact details, address and age are never
   kept; a record is shown in `scribe memory show` and removed with
   `#scribe forget p-<n>`, which suppresses it.
6. **Recurring dates are a yearly field** on `profile`, `people` dates and
   `milestones` (a wedding milestone yields the anniversary), computed into
   the page's `now` by the read-only `SessionStart` hook with no model
   call: 14 days ahead for others' dates and anniversaries, on the day for
   the user's own; `acknowledged` per year; no push.
7. **No preset.** Each difference is one setting (`personal`, `base`,
   `base_idle_minutes`, `[memory.sections] people`, `[memory.recurring]`).

Alternatives considered:

- **Presets chosen once per store** (`work` default, `personal`; the first
  proposal, approved and then revised by Daniel). Cost: one person has both
  kinds of conversation, so a store-wide choice either drops a regular
  user's work sessions or Daniel's family events; Daniel's own words chose
  the per-session cut.
- **Read only on sessions, as ORB-26219 did.** Cost: a regular user would
  have to type `/scribe on` in every chat for scribe to know her at all,
  the friction Daniel's example (a user "resistant to change because of
  memory") rules out.
- **(b) An off session leaves nothing but memory items**, with
  `[sessions] on_by_default` for a regular user. Cost: her catalog is empty
  unless every session is fully on, which brings the work layer's claims
  and approvals into personal chats; Daniel decided (a).
- **A `[sessions] light_catalog` setting.** Cost: one more state in the
  pass and its tests, for a need not seen; `/scribe private` and `base =
  false` already stop it.
- **Light entries as a separate record kind.** Cost: a second shape for
  sessions, second list and show verbs, and a migration when a light
  session is switched on; a `depth` field on the header costs one store
  format step.
- **The recent-sessions list by recency alone.** Cost: Daniel's ~60 light
  entries a week push his chosen sessions off the page.
- **A `people` record on every name** (no second-mention rule). Cost: a
  name said once in passing becomes a profile of someone who did not opt
  in.
- **Keep a third party's health or similar facts** when the user states
  them. Cost: the most sensitive facts about someone with no say, kept for
  the store's lifetime; the user's own `now` item already carries the
  event for its short window.
- **Store reminders in `now`** when a date nears (a routine writing
  items). Cost: a routine that must run on time, writes to clean up after
  each year, and a `SessionStart` that is no longer read-only.
- **Push a reminder** (notice, mail). Cost: a new surface and its consent,
  for a need not seen; the reminder is said at the next session.

Differences from the chief's proposal, each named: the base pass reads off
sessions after 30 idle minutes or at session end, which the proposal left
open; `/scribe private` is added, so the always-on base has a per-session
off switch (the reverse of `/scribe on` that 3_vision.md question 9 named);
the `#scribe` hook stays in every session, so the base is "no tags" for the
pass only; work relations (colleague, manager) make no `people` record; a
name without a relation is held only as a salted hash until its second
session; "brief" is 5 facts, 120 characters each, 4 dates, and a sixth
fact supersedes the stalest; a person's year of birth is never kept, the
user's own may be; sensitive items never recur; recurring milestones are
found by an editable word list; the user's own wedding anniversary uses the
14-day window, since he plans for it; the page gains a `people` part and
2,700 characters; the light entry is a behaviour, not a setting; the
recent-sessions list prefers sessions switched on in the last 14 days.

### Consequences

- About 60 base passes and 60 light entries a week on Daniel's Mac, to
  keep a base expected to stay empty; for a regular user, every chat is
  recallable and feeds her memory.
- Store format steps: `layer`, `person`, `recurs` on items (in ORB-26219's
  step (a)); the `people/` kind; `depth` on session headers. An older
  build refuses a store holding them.
- The issue hint of §11.4 runs only in on sessions.
- Not built until seen: presets, push notifications, non-yearly dates and
  user-set reminders, anniversaries of a loss, a cap on people, contact or
  calendar import, un-suppressing a person, forgetting a light entry's
  words, a light-entry setting, a base for non-interactive sessions.
- Cost: scribe now reads what the user types in sessions he did not
  choose, unless he says `/scribe private`; facts about people who did not
  opt in are kept on his word alone, with the third-party gates beyond the
  word lists left to the model; a light summary is a model's sentence
  about a conversation he never chose to record.

## scribe reads Claude Code and Codex sessions only

**Recorded:** 2026-11-21 · [ORB-26256] · **Overridable by Daniel**
**Supersedes in part:** [3_vision.md §1](./3_vision.md#1-open-questions) question 9's three routes (a ChatGPT export import, an app connector, always-on capture), and the "importing a ChatGPT export, an app connector" lines of [scribe keeps a descriptive memory the pass writes without approval](#scribe-keeps-a-descriptive-memory-the-pass-writes-without-approval) and of design §13.11
**Paths:** none (a scope decision; no code or store shape changes) — design §13.11, 3_vision.md §1

### Context

Daniel, chief's chat, 2026-11-21 (ws_constellation ORB-26194, relayed by
the chief), after the design had named routes for "a user who lives
elsewhere", his wife, who "is bounded to chatGPT and resistant to change
because of memory" (§13's opening): "no hosted server - I am not managing
other's memory. claude code and codex support only then. so be it".

Until then 3_vision.md question 9 listed three undesigned routes to a
provider scribe has no hook in: importing a ChatGPT export, an app
connector that reads such conversations as they happen, and always-on
capture with an off-the-record switch. A connector or a hosted import is a
server that holds other people's conversations and memory, run by Daniel.

### Decision

1. **scribe reads Claude Code and Codex sessions, and nothing else.** Its
   memory, catalog and `ask` are built from those two providers'
   transcripts, on the host that runs them. A user whose conversations are
   elsewhere is not a user scribe serves.
2. **No hosted server, app connector or ChatGPT-export import**, as future
   work or as "not built until seen". They are removed from 3_vision.md
   question 9, §13.11 and the memory decision's consequences, not deferred.
3. Daniel's reason, in his words: "I am not managing other's memory". A
   server that keeps a stranger's memory is a service he operates for
   them, with its consent, retention and breach questions; scribe is his
   own tool.

Alternatives considered:

- **A hosted scribe** that other users reach over the network. Cost:
  Daniel would run a service holding other people's words, which he
  declined; it would also need accounts, a store per user and a
  provenance story for turns not typed through a scribe hook.
- **An app connector or ChatGPT export import**, kept as "not built until
  seen". Cost: a standing promise to a provider scribe cannot read
  verbatim, so no `ask` quote could be checked against what the user typed
  (the rule every answer keeps); Daniel closed it ("so be it").

Differences from the chief's proposal: none; this records his words.

### Consequences

- The regular-user example of §13 (a user in ChatGPT) is no longer a
  target: nothing in the plan serves her.
- 3_vision.md question 9 is answered; questions 1 and 8 (other providers,
  box workers) are unchanged, and Codex is a reader scribe already
  supports ([2_design.md, Codex integration](./2_design.md#codex-integration-and-portable-skill)).
- Not built, and not planned: a hosted server, an app connector, a
  ChatGPT export import.
- Cost: a user who lives in another provider gets no scribe memory at
  all, and Daniel's wife, the example that shaped §13, is not served.

## The personal base, people, recurring dates and light catalog entries are parked

**Recorded:** 2026-11-21 · [ORB-26256] · **Overridable by Daniel** · design only; nothing parked was built
**Parks:** points 1–7 of [Memory has a personal base from every typed session and a work layer from on sessions](#memory-has-a-personal-base-from-every-typed-session-and-a-work-layer-from-on-sessions), and the settings they need; their text is kept for when he unparks them
**Paths:** none (a sequencing decision) — design §13.10, §13.12–§13.16

### Context

Daniel, chief's chat, 2026-11-21, on the consolidated task list of §13
(the chief's numbering): "park 2-5, keep 1, 6, 7". The captain reads that
as: parked are the personal base pass for off sessions with `/scribe
private` (§13.12), people records (§13.13), recurring dates (§13.14) and
light catalog entries for off sessions (§13.15); kept are slice (a), the
`memory` kind with the session-start page, (b) consolidation and promotion,
and (c) `working` → `preferences` candidates (§13.10).

Taken the same day as [scribe reads Claude Code and Codex sessions
only](#scribe-reads-claude-code-and-codex-sessions-only), which removes
the user the parked parts were for.

### Decision

1. **Parked: §13.12, §13.13, §13.14 and §13.15** (and the limits and order
   of §13.16 that belong to them). Their text stays in the design, marked
   "parked 2026-11-21" with his words, for when he unparks them. Nothing in
   them is built or scheduled.
2. **Kept, unchanged: §13.10 (a), (b) and (c).**
3. **Reason.** Claude Code and Codex only, so there is no surface whose
   sessions the base would serve beyond what `/scribe on` already covers.
   (The design adds no better one: the base's own cost, about 60 base
   passes and 60 light entries a week on Daniel's Mac to keep a base he
   expects to stay empty, §13.16, was set against a regular user who is
   now out of scope.)
4. **While the base is parked, memory items are written only from sessions
   switched on with `/scribe on`**, including the personal sections
   (`profile`, `now`, `milestones`). A session that is off writes nothing,
   as today.
5. **Slice (a) does not add `layer`, `person` or `recurs`.** Only parked
   parts read them; they come, as a store format step, with the part that
   needs them. The settings that exist only for parked parts (`[memory]
   base`, `base_idle_minutes`, `[memory.sections] people`,
   `[memory.recurring]`) are not shipped with (a).
6. **`[memory] personal = true`** ships, as under ORB-26240, and means: the
   pass of an on session may keep the personal elements (household in
   `profile`, personal events in `now`, personal milestones); `false`
   refuses them as ORB-26219 did (`memory_personal_off`). It no longer says
   which sessions feed memory.

Alternatives considered:

- **Build the base anyway** for Daniel's own sessions. Cost: 60 base
  passes a week for a base he expects to stay empty; he parked it.
- **Delete the parked sections.** Cost: the design work (the third-party
  gates, the salted-hash ledger, the derived recurring dates) is lost if he
  unparks them; he asked that the text be kept.
- **Ship `layer`, `person` and `recurs` now as nullable fields**, as
  §13.16 had planned. Cost: three persisted fields nothing reads, which
  the persisted-shape rule makes permanent; adding a field later is a store
  format step like any other.

### Consequences

- Slice (a)'s store format step is the `memory/` directory and kind, as
  ORB-26219 had it, without the addendum's fields.
- The issue hint of §11.4 stays as built: it runs in every interactive
  session.
- The session-start page keeps ORB-26219's layout (2,400 characters; no
  `people` part; no derived recurring lines).
- Cost: a regular user who never types `/scribe on` has no memory and no
  catalog; the gap between "every session feeds memory" and "only chosen
  sessions" is the work Daniel parked.

## Stop recovery keeps a line covered when it was typed

**Recorded:** 2026-11 · [ORB-26349]
**Code anchors:** `crates/scribe-core/src/pass/candidates.rs::covers_turn`, `crates/scribe-core/src/store/mod.rs::covered_instruction_lines`

### Context

Stop recovery asks which `#scribe` lines an entry already covers, so it
does not write them and does not repeat the prompt hook's echo. That
question used the entries active when Stop ran. Retiring the covering
entry later in the same session made the next Stop treat the earlier line
as new and write it, even though the line was covered when Daniel typed it.
The prompt hook may have been missed: the first Stop is often the first
observer, and by then the entry may already be retired.

### Decision

Coverage for Stop recovery is as of the turn. An entry covers that turn
when it is the same tag and project, `covers_statement` matches, and the
entry was active at the turn's timestamp. It already existed
(`created <= at`). A retired or superseded entry had not yet left active
when `updated >= at`, because retirement and supersession set `updated`
and leave the content in place. An entry that is still active also covers
through `covering_entry`, including one recorded after the turn, so a
later entry still suppresses a duplicate. A new `#scribe` line typed after
retirement is still written: live capture ignores retired, superseded and
forgotten entries.

The alternative was a durable log of covered turns, written when the
prompt hook or Stop first observed the cover. That would survive `forget`,
which blanks the words, and a later edit of `updated`. It would also be a
new store document and a format step, and it would miss a turn whose
prompt hook was missed and whose covering entry was retired before the
first Stop. The timestamps already on the entry answer "active at this
turn" for retire and supersede without that document.

A same-second tie counts as still active. Entry timestamps are whole
seconds, so the order inside that second is not recorded, and recapturing
a line that may have been covered is the worse miss.

### Consequences

- A line covered when typed is not captured by a later Stop after the
  covering entry is retired or superseded.
- A forgotten entry's content is blank, so Stop cannot see that it covered
  the turn. A missed line can be written again. That follows from blanking
  the words.
- A metadata write that moves `updated` on an already-retired entry to
  after a turn can make Stop treat that turn as covered. Cite filling runs
  after recovery in the same Stop, so the first Stop after the turn still
  sees the retirement time.
- Cost: there is no separate retirement timestamp, so the rule uses
  `updated`, which other writes also set. A forgotten line is not
  protected, and a same-second line typed after retirement is not captured.

## Stop coverage ends when the entry left active, in whole seconds

**Recorded:** 2026-11 · [ORB-26360]
**Code anchors:** `crates/scribe-core/src/pass/candidates.rs::covers_turn`, `crates/scribe-core/src/store/mod.rs::covered_draft`, `crates/scribe-core/src/store/format.rs::entry_inactive_since`

### Context

[Stop recovery keeps a line covered when it was typed](#stop-recovery-keeps-a-line-covered-when-it-was-typed)
read `updated` as the end of a retired or superseded entry's active
window. Review ORB-26357 found three gaps: `updated` is whole seconds while
transcript turns carry fractions, so a turn at .250 before a retirement at
.800 of the same second was treated as uncovered; form-3 referent lines
were never checked, so a resolved referent was written once its covering
entry was retired; and a metadata write after retirement (`entry update`,
cite filling) moves `updated`, so a later missed line could be dropped as
covered.

### Decision

Entries gain `inactive_since` (store format 12): the time a retirement,
supersession or forgetting took the entry out of `active`, `null` while it
is active, never moved by metadata writes. `covers_turn` ends the active
window there, and compares it and `created` with the turn in whole
seconds, so a tie inside a second still counts as active (the earlier
decision's rule). Stop recovery applies the same check to a form-3
referent after it resolves, through `Store::covered_draft`, and skips a
covered one quietly as it skips a covered form-2 line.

A new field was chosen over deriving the time from proposals or the audit
log: direct retirement has no proposal, and audit lines carry no
timestamps of state. The upgrade fills `inactive_since` from `updated` for
entries already inactive, since no better time exists in an older record.

### Consequences

- The three findings are closed: a sub-second turn inside the retirement's
  second is covered, a covered referent is not captured after retirement,
  and a metadata edit no longer changes which turns an entry covered.
- An entry retired before format 12 and edited afterwards keeps the edit
  time as its `inactive_since`; such a store may treat a few more turns as
  covered, the old behaviour.
- A line typed later in the retirement's own second is still treated as
  covered; the order inside the second is not recorded.
- Cost: a format bump, so a build older than format 12 refuses the store
  (`store_too_new`).

## Config tags merge with the shipped set

**Recorded:** 2026-11-21 · [ORB-26405] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/config.rs` (`Config::parse`, `merge_with_shipped`), `crates/scribe-core/src/tag.rs` (`TagTable::shipped`, `TagTable::defaults`)

### Context

A `[tags]` table used to replace the shipped set, keeping only the action
tags whose names the file had not claimed. Declaring `[tags.remind] action =
true` dropped `preferences`, and the retired-tag cleanup then closed every
open preferences candidate. Daniel, 2026-11-21: a user should be able to
define and add new tags with their own criteria and trigger words. Adding a
tag must not remove a shipped one.

Review of the first merge (scribe#91) refused the file when a config alias
collided with a shipped name or alias (`pref`, `standard`). Those files were
valid before: the alias resolved to the config tag and the store stayed
readable. A config file must never make a store unreadable.

### Decision

This supersedes the "unless it claims their names" sentence of [Action tags
are marked in config, and three
ship](#action-tags-are-marked-in-config-and-three-ship), and only that
sentence. `[tags]` merges with the shipped set (`preferences`, `idea`,
`standard`). A declared tag that is not shipped is added. A declared shipped
tag overrides only the fields it sets and inherits the rest: aliases,
description, stale interval, action (including `require_quote` and
`ask_first`) and each capture field. `aliases = []` clears aliases. There is
no syntax that deletes a shipped tag.

A word the file sets — a tag's name, or an alias in an `aliases` list it
writes — belongs to that config entry. When the word is a shipped tag's
name, that shipped tag is left out. When it is only a shipped alias, the
shipped tag loses that alias and keeps everything else. `tag list` prints a
line naming both tags; `tag list`, `entry list` and `record search` still
succeed. Omitting `aliases` does not claim the inherited aliases, so another
config tag may take one. Two config entries that claim one word are still
`config_invalid`.

The alternative was to keep refusing every collision, including one with a
shipped default. That protects a reader from a quiet loss, and it also turns
a file Daniel already uses into a store nothing can read.

### Consequences

- `[tags.remind] action = true` alone still lists `idea`, `preferences`
  (180 days, aliases `preference` and `pref`, shipped capture) and
  `standard`, plus `remind` as an action tag. A pass and `consistency scan`
  do not close an open preferences candidate for `tag_retired`.
- `[tags.rituals] aliases = ["pref"]` keeps `preferences` without the alias
  `pref`, and `tag list` warns. `aliases = ["standard"]` leaves `standard`
  out and warns. `[tags.preferences]` with aliases `preference` and `pref`,
  plus `[tags.remind] action = true`, reads as that explicit preferences tag
  plus the shipped action tags and `remind`, with no warning.
- The Mac `~/.scribe/config.toml` repeats `[tags.preferences]` only to keep
  the tag under the old replacement rule. Once this ships, that block can
  be dropped; this change does not edit the file.
- Cost: a shipped alias or name can disappear because another tag claimed
  the word, visible only as a `tag list` line. A reader who misses the line
  can think the shipped tag was never there. Two config entries that both
  want the word are still refused, so the loss is only against a default he
  did not write.

## An approved claim closed because its entry is covered records `covered`

**Recorded:** 2026-11-21 · [ORB-26244] · **Overridable by Daniel** · built
**Refines:** §5.3's covered approval ("there is no new closed reason and no format bump") and "An agent settles claims and deletes records on Daniel's checked instruction"
**Paths:** `crates/scribe-core/src/store/candidates.rs` (`reapply_candidates`), `crates/scribe-core/src/store/mod.rs` (`Store::settle`), `crates/scribe-core/src/store/agent.rs` (`settle_candidate_on_instruction`), `crates/scribe-core/src/store/format.rs` (`instructed_approval`, step 14→15) — design §12.3, §12.7

### Context

An approved claim whose reserved entry file is missing (an interrupted
approval) is closed `rejected` by the scan's recovery or a repeated typed
approval when an active entry now says the same. Before agents could approve
on Daniel's instruction, that close kept the typed approver and no reason.
An instructed approval carries an `instruction`, and format 13 reads a
rejected proposal with an instruction as exactly an instructed rejection
(`closed_reason: instructed`). Review ORB-26354 (scribe#84) found that the
close therefore either left a store that no longer reads or, with
`instructed`, claimed a rejection Daniel never asked for. The same review
found that the recovery wrote a typed `#scribe approve` line with no
`approved_by`, claiming an approval he did not type.

### Decision

1. **A new closed reason, `covered`.** The close keeps the approval's
   `settled_at`, `settled_by`, `settled_author` and `instruction` and sets
   `closed_reason: covered`, for typed and instructed approvals alike. A
   first typed approval that finds its claim covered still closes with no
   reason, as before; an instructed one sets `covered`, since its
   instruction must be explained.
2. **The entry is rebuilt from the proposal.** `Proposal::candidate_entry`
   sets `approved_by`, `instruction` and the canonical `scribe proposal
   settle` command as `said` whenever the proposal carries an instruction,
   so recovery writes the same entry as the approval.
3. **Store format 15.** Neither the new `said` nor `covered` reads at
   format 14, so the step 14→15 (`instructed_approval`) changes no value but
   makes an older build call the store too new instead of corrupt.

The alternative was to leave a covered instructed approval `approved` with
no entry. Every scan would then find it again and report it, and the
reserved id would point at nothing.

### Consequences

- `proposal list` shows `covered` in the `closed_reason` column; `--json`
  gains the value, with no field renamed.
- A format-14 build refuses a store this build wrote once an agent has
  approved a claim on instruction (`store_too_new`).
- Cost: one more closed reason for readers to learn, and a format step that
  changes no data.

## The session pass writes Daniel's ideas itself, with their outcome

**Recorded:** 2026-11-21 · [ORB-26265] · **Overridable by Daniel**
**Code anchors:** `crates/scribe-core/src/store/candidates.rs` (`open_pass_candidates`, `pass_entry`, `written_from`), `crates/scribe-core/src/pass/candidates.rs` (`check`, `checked_outcome`, `write_exclusions`), `crates/scribe-core/src/entry.rs` (`Outcome`), `crates/scribe-core/src/tag.rs` (the shipped `idea` capture rules)

### Context

Daniel, chief's chat 2026-11-20: "same for idea - there are probably so
many forgotten ideas that never got resurfaced. probably mostly bad ideas
to begin with but I think it would be good to keep them captured
automatically." Ideas were captured only when he typed `#scribe idea:`, or
through a candidate he had to approve. On 2026-11-21 he approved step (b)
of [2_design.md §11.7](./2_design.md#117-order-and-sequencing), b1 and b2:
"approve both". On duplicates, the same day: "I hope scribe isn't adding
duplicate entries. I had aide record the idea explicitly and it also exists
in raw format that may later be added."

### Decision

Under a tag whose capture route is `write` (the shipped `idea`), the
session pass writes a checked claim from Daniel's own turn as an active
entry, with no proposal, in the locked step that would otherwise open a
candidate ([2_design.md §11.6](./2_design.md#116-idea-capture-after-finding-is-gone)).
The content is scribe's statement and the byte-for-byte span is its
`quote`; the pass is the author and `stated_by`, so the entry is never
Daniel's own and every reader labels it as captured. Each records an
outcome (`open`, or `parked` in his words); an idea that became a task in
the session is not written. At most 3 per session per run, apart from the
candidate slots and with no extra model call. Before each write the store
runs the shared lexical duplicate check against active entries of the tag
and project and against prior candidates, and the model is sent the
existing ideas as exclusions, since the lexical check cannot match a short
typed idea against the raw paragraph it summarizes.

The alternative not taken was to send every captured idea to nebula, the
curated corpus, so ideas resurface where the constellation's knowledge is
searched. nebula stays curated: nothing here writes to it.

### Consequences

- Ideas from an on session are recorded without Daniel settling anything;
  he cancels one with `#scribe retire|forget <id>`. They do not count on
  the `SessionStart` stuck line.
- Outcome `task` is reserved for a later settling route; the pass never
  writes it.
- Cost: with no approval, the pass's guards are the only check, and a
  paraphrase the model still returns is written as a second idea. A model
  that misreads an acted-on proposal as an idea writes an entry Daniel did
  not mean to keep. Sending ideas to nebula instead would have made every
  one of them curated knowledge, filling a corpus kept for vetted material
  with ideas Daniel himself expects to be mostly bad, and would have needed
  nebula's review for each.

## Audit lines name the instruction's session and message (audit format 3)

**Recorded:** 2026-11-21 · [ORB-26244], [ORB-26503] · **Overridable by Daniel** · built
**Refines:** "Every mutating command is audited" (audit lines carry ids and codes, never values)
**Paths:** `crates/scribe-core/src/store/audit.rs` (`AUDIT_FORMAT`), `crates/scribe/src/audit_middleware.rs`, `crates/scribe/src/commands/instruction.rs` (`check`), `crates/scribe-core/src/instruction.rs` (`located_message`) — design §12.4, specs/output-contract.md

### Context

A write an agent runs on Daniel's instruction (`proposal settle`, `entry
retire`, `entry forget` with `--session` and `--instruction`) changes his
records on his word. The audit log is where a later reader checks which
turn backed each change or refusal; before format 3 a line named the
command, target and author only. Review ORB-26500 found that task 3 noted
the turn's message id only once every check passed, so a refusal after the
turn was found (stale, mismatched) lost it.

### Decision

1. **Audit format 3** adds `instruction_session` (from `--session`) and
   `instruction_message` to every line; an older line reads with neither
   (STD-02 R16). Neither field carries the instruction's words.
2. **`instruction_message` is the turn the check located**, whether or not
   it then passes: the CLI notes `located_message` before the remaining
   checks. It is `null` only when no turn Daniel typed holds the
   instruction, or that turn has no message id (which is refused).

The alternative was to note the message only on success, keeping refusals
minimal. That leaves a refused stale or mismatched instruction untraceable
to the turn it quoted, which is exactly what an audit of a refused act on
his word needs.

### Consequences

- An audit reader can tie every instructed change or refusal to his turn.
- Readers must accept both shapes; the format-2 reader test covers it.

## An entry keeps its approval instruction apart from its removal

**Recorded:** 2026-11-21 · [ORB-26503] · **Overridable by Daniel** · built
**Refines:** "An agent settles claims and deletes records on Daniel's checked instruction" (one `instruction` per entry)
**Paths:** `crates/scribe-core/src/entry.rs` (`Entry::removal_instruction`), `crates/scribe-core/src/store/agent.rs` (`retire_agent_entry`, `forget_entry`), `crates/scribe-core/src/store/format.rs` (`removal_instruction`, step 17→18; `EntryV16`) — design §12.3, §12.4, §12.7

### Context

Format 13 gave an entry one `instruction`: the checked instruction that
approved, retired or forgot it. Review ORB-26500 found that retiring an
entry an agent approved on instruction replaced the approval's instruction
with the retirement's, so `entry show --json` paired `approved_by` and the
approval command with the retirement's words: a removal presented as the
evidence for an approval.

### Decision

1. **A second field, `removal_instruction`**, holds the instruction an
   entry was retired or forgotten on. `instruction` is from format 18 only
   the instruction `approved_by` approved on, set exactly when `approved_by`
   is, and nothing later replaces it. The decoder refuses an `instruction`
   without `approved_by` and a `removal_instruction` on an entry that is
   neither retired nor forgotten.
2. **Store format 18.** The step 17→18 moves an older entry's `instruction`
   to `removal_instruction` when it has no `approved_by`; an approved entry
   keeps it as the approval's. A format-15 to format-17 approved entry already retired on
   instruction lost its approval's instruction then and cannot be told
   apart; its proposal still holds the approval's `instruction`.
3. **Forgetting blanks both quotes** and keeps both instructions' ids.
4. Format-14 to format-17 entries decode through typed YAML (`EntryV14`,
   `EntryV16`), like formats 10–13, so a local YAML tag reads as before (the
   same review found that bumping to 15 had sent format-14 entries through
   a JSON conversion that read `!local scribe` as a map; format 16's
   `EntryV14` already restored typed decoding, and a fixture test now pins
   it).

The alternative was to keep `instruction` as the latest act and add an
`approval_instruction`. Then a reader pairing `approved_by` with
`instruction`, as format 13 to 15 documented, would still read a retirement
as the approval's evidence.

### Consequences

- `entry show --json` and `entry list --json` gain `removal_instruction`; a
  retired or forgotten entry of Daniel's reports its instruction there and
  `instruction: null`, a change of meaning for that one case.
- A format-17 build refuses a store this build wrote (`store_too_new`).
- Cost: one more field, and a decoder rule tying `instruction` to
  `approved_by`.

## `#scribe` is an annotation, cataloged at the end of the turn

**Recorded:** 2026-11-21 · [ORB-26510] · **Overridable by Daniel** · approved 2026-11-21 ("Approve as proposed"); step 1 built ([ORB-26529], store format 22), steps 2 and 3 not built
**Refines:** [`#scribe` capture in 2_design.md §4](./2_design.md#4-scribe), [A claim carries a statement (§11.1)](./2_design.md#111-a-claim-carries-a-statement)
**Paths:** `crates/scribe-core/src/capture/**`, `crates/scribe/src/hook/**`, `crates/scribe-core/src/store/**`, `docs/design/scribe/2_design.md` §14

### Context

Daniel, chief's chat, 2026-11-21: "current scribe's inline command only
work as a standalone comment, i.e. it doesn't work when mixed into other
sentences. I think a better design is this: * when I am writing something,
I should be able to annotate scribe messages along side it. And the scribe
agent will read it, and surrounding subsequent and previous messages if
required, and catalog it accordingly. having to write a separate scribe
message is redundant. * no more verbatim captures on "#scribe <tag>:
<msg>". all entries should have short context/summary and verbatim
messages for citation. if I want to capture verbatim notes, then I could do
so manually."

The need is observed. Twice that day a `#scribe` inside a sentence was
missed and he typed it again on its own line (`idea-1`, `remind-2`).
`standard-2` holds "add below" as its content, because form 1 records the
words after the colon and the standard was below them. Of his 8 typed
entries, 7 are form 1 and 1 is an approval; forms 2 and 3 never wrote one.
Two of his unquoted mid-sentence `#scribe` tokens were talk about scribe,
not annotations, so a model has to tell them apart.

### Decision

A `#scribe` token anywhere in a typed turn, outside code, quotes,
blockquotes and pasted content, marks the turn
([2_design.md §14](./2_design.md#14-scribe-as-an-annotation)).
The prompt hook writes nothing for it and echoes that the turn is marked.
The same turn's `Stop` hook sends a bounded window (the 8 messages before,
the marked turn and the assistant's reply) to one Haiku call with a 30 s
budget. The model returns, per marker, either an entry (tag, statement,
subject, his verbatim quote and the message it comes from) or `none` with
a reason. Checked entries are written with author `daniel`, content the
statement, `quote` his span and `stated_by: {kind: scribe, name:
annotation}`, with no proposal. A word right after the marker that names a
tag is a hint the model must follow. Forms 1 to 3 fold into the marker,
and §4.3's referent call and guessed fallback go. Settling (form 4) stays a
whole-line, model-free command. A `marks.json` store document keeps each
marker's outcome, so nothing found and refusals are not retried, and a
model failure is retried at most 3 times. Existing entries are not
rewritten.

**Differences from his sketch and the chief's proposal, each named:**

1. **When it is written.** The chief's proposal said the echo comes one
   turn later. Stop runs at the end of the reply to the marked turn, so the
   entry and its echo come at the end of the same turn, with the reply
   already in the window.
2. **"Subsequent messages".** His sketch has scribe read later messages
   "if required". The design reads the reply after the marked turn but not
   his next turn. In both missed annotations what he pointed at was in the
   same turn. Reading his next turn is not built until seen.
3. **"No more verbatim captures".** Kept as he said for content: the
   content is always the statement. His words are never lost, because the
   `quote` holds them verbatim and `entry show` prints them. Whether a
   separate verbatim form is still wanted is put to him ("manually" is a
   question, §14.11).
4. **Settling verbs stay a command.** His sketch covers annotating, not
   settling; the chief's proposal kept them whole-line. They stay whole-line
   and model-free because they change records. A verb after a marker in a
   sentence is refused with a reason, not read by the model.
5. **A hint binds.** The chief's proposal let the model derive the tag. A
   tag word right after the marker is his choice and the model must use
   it; only an unhinted marker is classified.
6. **No tag found.** The chief's proposal asked for a choice between a
   refusal and an open proposal. The design echoes `none` with a reason and
   writes nothing: the marker is his consent to one entry, so a proposal
   would ask him to approve what he already said, and he can type it again
   with a hint.
7. **A durable outcome per marker.** Not in either: added so a `none` or a
   refusal is not sent to the model again at each Stop, and retries are
   bounded.
8. **Existing entries.** Left verbatim, with no model rewrite, as the chief
   proposed; put to him as a question.

### Alternatives

- **Catalog at submit**, in the prompt hook, as form 3 does today. The echo
  arrives before the reply. **Cost:** the prompt waits for the model call
  (up to its budget) on every marked turn, and the model cannot read the
  reply, which is where "this" often points.
- **Keep the line grammar and only widen detection** to a marker inside a
  sentence, recording the words after it verbatim. **Cost:** a marker in a
  sentence has no clear end, so the content would be a fragment like "what
  follows is an idea" or "add below", the problem `standard-2` already
  shows, and Daniel asked for statements.
- **Every unquoted token is a marker** with no `none` outcome. **Cost:** the
  two observed mentions would each have written an entry he did not mean,
  which he would have to retire.
- **An open proposal when the model is unsure.** **Cost:** he approves what
  he already marked, and the open queue (20 candidates waiting today) grows.

### Consequences

- A marked turn costs one Haiku call, at most 30 s at the end of the turn,
  with at most 3 attempts. Stop handles at most 2 marked turns per run.
- An entry's content is now the model's statement, so a weak or wrong
  statement is caught only when he reads the echo. His exact words stay in
  `quote`, and only spans inside it are credited to him.
- `models.referent` and the form 1 to 3 grammar are removed in step 2 of
  §14.9; a config that sets `models.referent` loads with a warning for the
  30-day compatibility window. Built 2026-11-21 ([ORB-26530]).
- One store format step adds `marks.json` and the `annotation` statement
  author.
- Cost: an unquoted mention spends a call and may still write an entry he
  did not mean; the content he sees is no longer his own sentence; and the
  turn ends up to 30 s later when it holds a marker.

## Tags are catchers, memory is a fixed synthesis, and the rest lives in one SQLite database (proposed)

**Recorded:** 2026-11-22 · [ORB-26640] · **Daniel's choice** ("your proposal sounds good", 2026-11-22). He confirms it when he approves the design.
**Would supersede, once approved:** [One Markdown file per record, metadata in YAML frontmatter](#one-markdown-file-per-record-metadata-in-yaml-frontmatter), [ask ranks in memory with no persisted index](#ask-ranks-in-memory-with-no-persisted-index) for messages, and the five sections and switches of [scribe keeps a descriptive memory the pass writes without approval](#scribe-keeps-a-descriptive-memory-the-pass-writes-without-approval). The task that lands each phase adds the `**Superseded by:**` line.
**Code anchors:** `crates/scribe-core/src/tag.rs::CaptureRules`, `::SHIPPED_CAPTURE`, `crates/scribe-core/src/models.rs::SessionRequest::memory_prompt`, `crates/scribe-core/src/memory/mod.rs::Section`, `crates/scribe-core/src/store/format.rs::UPGRADES`, `crates/scribe-core/src/store/fsio.rs::LockGuard`
**Paths:** `docs/design/scribe/2_design.md` §15

### Context

Daniel, chief's chat, 2026-11-22, after the pass wrote three memory items
that were not memory (`m-3`, a one-off "Run the requeue", kept as
`working`):

- "we need to keep things generic, without personalizing around me";
- "We need to draw a clearer distinction between tag entries and memory
  entries";
- "memory: no on/off and no customization for now. everyone gets the same
  shape";
- "just memory as files, and the rest (tags & session & audit log) in
  sqlite?"

Other facts behind the decision:

- Shipped code names him 494 times.
- `store/format.rs` holds 21 upgrade steps in 2,654 lines.
- The audit log has 1,296 lines.

### Decision

The full design is in [2_design.md §15](./2_design.md#15-catchers-claims-with-context-one-memory-shape-and-a-sqlite-store-proposed).

**Three record kinds.**

- A tag entry is a remark caught from a session, citing one quote. It lives
  in SQLite.
- Memory is a synthesis written as prose: `memory/profile.md`, `now/` and
  `milestones/` Markdown files. It has one fixed shape with no switches.
  `profile.md` is rebuilt by a weekly `scribe memory refresh` on a stronger
  model.
- A session is what happened, with its user turns. It lives in SQLite.

**Catchers and claims.**

- A tag is a catcher in `config.toml`: triggers, criteria and `not`, `from
  = user | assistant | both`, `route = propose | write`, and `max_open`.
- `preference` and `standard` ship.
- What scribe infers under a `propose` catcher is a claim. A claim carries
  a context paragraph and the verbatim quote.
- `scribe brief` is the one session-start page: the memory block that
  `scribe memory show` prints (Profile, Now, Milestones), then recent
  sessions, then open claims, with fixed caps and no model call. The
  `SessionStart` hook injects it, and a manual instruction runs it. There
  is no `memory overview` verb (Daniel, 2026-11-22: "yes, put it on both
  tasks").

**Generic text.** Shipped text says "the user", with an optional `[user]
name`. The output contract stays fixed in code.

### Alternatives

- **Keep the Markdown store and add a messages index beside it.**
  - Cost: two persistence disciplines. That is the reason the store
    rejected SQLite in 2026-11, and it applies with more force once
    messages are stored. The lock, atomic-rewrite and upgrade code also
    stay.
- **Keep the five memory sections and fix only the `working` definition.**
  - Cost: `working` and `preferences` would still overlap.
  - The items would still be single quotes rather than a synthesis.
  - Daniel asked for no switches, and the switches would remain.
- **A new `approval` field on tags,** as the chief's proposal had.
  - Cost: it duplicates `route`, and allows combinations that mean nothing
    (2_design.md §15.10, item 1).

### Consequences

- The store stops being greppable and diffable, except for memory.
  - Cost: inspection goes through `scribe export` and the CLI.
- One new dependency, `rusqlite` with SQLite bundled.
  - Cost: a C build in CI, and a larger binary.
- A write and its audit row commit together, which closes the approval gap
  that `reapply_approved` repairs.
- Storing every typed turn of a switched-on session is new data.
  - Cost: it is redacted on write, owner-only, and deleted by forget, but it
    is still more of the user's words at rest than today.
- The profile job spends one stronger-model call a week.
  - Cost: about 120 s of `sonnet` per run, bounded by its input caps.

## Approved entries live in SQLite with a Markdown export (proposed)

**Recorded:** 2026-11-22 · [ORB-26640] · **Daniel's choice** (decision 1 of the accepted proposal)
**Code anchors:** `crates/scribe-core/src/store/mod.rs::load_entries`, `::entry_path`

### Context

Approved preferences are the records a user is most likely to want to read
and edit by hand. Today each one is a file, `entries/preferences/<id>.md`.

### Decision

**Option chosen:** entries live in the `entries` table, like every other
kind but memory. The read-only `scribe export --format markdown` writes
them in today's file shape. An entry is corrected through the CLI
(`entry update`, `retire`, `forget`), as today.

### Alternatives

- **Approved preferences stay as hand-editable Markdown files,** and only
  claims move to SQLite.
  - Cost: approval would then write a file and a row, which are two stores
    with no transaction across them. That is the gap that `reapply_approved`
    repairs today.
  - The lock and `write_atomic` code would stay alive for one kind.
  - A hand edit would bypass the audit trail.

### Consequences

- One transaction for approval, entry and audit.
- Cost: a user cannot open a preference in an editor. They read it with
  `entry show` or an export.

## `issue` is a catcher tag, not a record kind (proposed)

**Recorded:** 2026-11-22 · [ORB-26640] · **Daniel's choice**. It replaces decision 2 of the accepted proposal ("issue records and findings are agent-written kinds, not catchers: outside this redesign").
**Code anchors:** `crates/scribe-core/src/issue.rs`, `crates/scribe-core/src/store/issues.rs`, `crates/scribe-core/src/issue_trigger.rs`, `crates/scribe-core/src/tag.rs::TagTable::issue_capture`, `crates/scribe/src/commands/issue.rs`
**Would supersede, once approved:** [Claims carry a statement, tags carry capture rules, and issues are their own record](#claims-carry-a-statement-tags-carry-capture-rules-and-issues-are-their-own-record), for issues only

### Context

Store format 17 made issues a record kind of their own:

- `scribe issue add|occur|update|move|search|show|list|stats`;
- occurrences, and resolved and reopened states;
- a reserved `issue` tag whose route-`agent` triggers have the prompt hook
  ask the agent for a root cause.

Daniel, chief's chat, 2026-11-22, reading the `Records:` group of `scribe
--help`: "we are getting rid of "issue" correct? not sure why it shows up
as command. it should be a tag".

Measured on the Mac store the same day: no issue record exists, and the
audit log holds one issue trigger fire.

### Decision

**Option chosen:** `issue` is a tag.

- The record kind, its CLI, `issue stats`, the hook's root-cause line,
  `#scribe forget <issue-id>` and `record search --kind issue` are removed.
- An incident is caught by a catcher like any other remark.
- `issue` ships as an **example** catcher (`from = "both"`, `route =
  "write"`, [2_design.md §15.2](./2_design.md#152-tags-are-catchers)), not
  as a default. The defaults stay `preference` and `standard`. Daniel,
  asked "Should `issue` ship as a third default tag?", answered "No,
  example only" (2026-11-22).
- Any stored issue becomes one `issue` entry, model-free (2_design.md
  §15.11).
- It is a breaking CLI change, so it lands in phase 2 with the approved
  design, never before.
- Agent notes (`scribe entry add`) and stored `finding` entries are
  unchanged: `finding` is already a tag.

### Alternatives

- **Keep issue records outside the redesign,** as the accepted proposal
  had.
  - Cost: a second capture path (route `agent`), a reserved name, eight
    verbs and a record kind with its own file format and audit fields, all
    kept for zero records.
- **Ship `issue` as a third default catcher.**
  - Cost: it goes against the defaults Daniel named.
  - An assistant-reading collector fires on every debugging session of
    every user.

### Consequences

- The catcher schema has two routes, both written by scribe, and no
  reserved tag.
- Cost: the same failure seen twice is two entries, not one issue with two
  occurrences.
- Cost: an issue has no resolved or reopened state.
- Cost: there are no precision statistics for the trigger.
- Cost: a config that sets `[tags.issue.capture]` with route `agent` loads
  with a warning for the 30-day window, and is then refused.

## The store keeps only the assistant turns a claim cites (proposed)

**Recorded:** 2026-11-22 · [ORB-26640] · **Daniel's choice** (decision 3 of the accepted proposal)
**Code anchors:** `crates/scribe-core/src/pass/candidates.rs` (the `Speaker::User` filter), `crates/scribe-core/src/proposal.rs::EntryCandidate::role`

### Context

The `messages` table stores the user's typed turns of switched-on sessions.
A catcher with `from = "assistant"` cites an assistant turn, and that quote
must stay checkable after the provider transcript is gone.

### Decision

**Option chosen:** the only assistant turns stored are those a claim or a
collected entry cites (`role = assistant`). No other assistant message is
stored.

### Alternatives

- **Store every assistant message** of a switched-on session.
  - Cost: several times the volume of user text (assistant turns are
    longer), all of it model output, which is not the user's words.
  - More to redact and to forget.
  - Nothing yet reads it.

### Consequences

- `ask` and search cover the user's words and the cited assistant turns
  only.
- Cost: an answer that needs what the assistant said elsewhere in the
  session reads the segment summaries, or the transcript while it exists.

## The meaning lands before the storage, and the messages table opens the storage (proposed)

**Recorded:** 2026-11-22 · [ORB-26640] · **Daniel's choice** (decision 4 of the accepted proposal), refined by the design
**Paths:** `docs/design/scribe/2_design.md` §15.9

### Context

The accepted order put all of the meaning first (tags, claims, the brief,
the memory shape, de-personalising), then the storage, starting with the
new messages table. Three parts of the memory shape need tables:

- pending undated `now` candidates;
- removed `now` history;
- profile revisions, and the verbatim messages the profile job reads.

### Decision

**Option chosen:** the meaning that needs no storage goes first:

1. stop writing `working` memory;
2. generic prompts and catchers;
3. claims with context and the brief.

Then comes the SQLite foundation, with the messages table first. The memory
shape and the profile job come next, then sessions, then the remaining
kinds. Last, the file store is removed after the compatibility window.
This is the order of the accepted decision, with one move: the memory
shape goes after the SQLite foundation, because building its tables on the
file store first would mean building them twice.

### Alternatives

- **The memory shape before any SQLite,** as accepted.
  - Cost: pending candidates, history and revisions would need file
    documents and format steps, which phase 7 would then throw away.
  - The profile job would read verbatim turns from provider transcripts,
    which may have been removed.
- **Storage first.**
  - Cost: the observed failure (`m-3`) and the claims nobody can settle
    would wait behind the largest change.

### Consequences

- The `m-3` class of item stops in phase 1, a small change.
- Cost: memory keeps its current shape, minus `working` and `projects`,
  until phase 5.

## Search is a top-level verb

**Recorded:** 2026-11-22 · [ORB-26646] · **Daniel's choice** (2026-11-21)
**Paths:** `crates/scribe/src/commands/search.rs`, `crates/scribe/src/help.rs`

### Context

`scribe record search` was the only verb under the `record` noun. The noun
existed only to satisfy STD-01 §R1 (`<tool> <noun> <verb>`). The search
spans every kind of stored record (entries, sessions, issues; `--kind`
narrows it), so it never named one resource. Daniel: "I would say just use
`scribe search`, no need for alias." and "in orbit, we also have `orbit
update`, `orbit search` etc. can you adjust the standard?"

### Decision

- The command is `scribe search`, a top-level verb, with the same
  arguments, flags, ranking, output (`--json` included) and exit codes.
- The `record` noun is removed with no alias: `scribe record search` is an
  unknown command (exit 2). STD-01 §R35's keep-for-one-release rule names
  flags and config keys, not commands; the removal is recorded in the
  changelog as a breaking change.
- **Deviation from `STD-01@2 §R1`**, which says one tool must not mix
  `<tool> <verb>` with `<tool> <noun> <verb>`. Scope: `scribe search` only.
  Every other command keeps the noun-first order, including
  `scribe issue search` (one resource) and `scribe question ask`. The
  matching change to §R1 is scribe entry `standard-5`, in the standards
  intake; when the canon admits a cross-resource verb, this deviation ends.

### Alternatives

- **Keep `record search`.** Cost: a noun that names no resource, kept only
  for the grammar rule.
- **Add `scribe search` and keep `record search` as an alias.** Cost: two
  spellings of one command; Daniel said no alias.

### Consequences

- Callers outside this repository that run `scribe record search` fail with
  a usage error until they change; a separate task updates the umbrella's.

## Format 23 renames JSON fields and a code without aliases

**Recorded:** 2026-11-22 · [ORB-26709] · **Daniel's choice** (design §15.8,
part of the §15 proposal he accepted, [ORB-26640]; shipped in PR #133 under
[ORB-26650])
**Code anchors:** `crates/scribe/src/output/payload.rs::TranscriptPayload`,
`crates/scribe-core/src/error.rs::WriteRefusal::code`,
`crates/scribe-core/src/record.rs::AuthorKind`

### Context

Format 23 renamed machine output: the `transcript list` and `transcript show`
JSON keys `daniel_turns` and `daniel_first_turns`, the error code
`daniel_only`, and the author `kind` and `name` values `daniel`. STD-01@2
§R10 makes renaming a field a breaking change, and the workspace guide says
`--json` names are contracts. The change was recorded only in
`specs/output-contract.md`, not as a deviation from §R10 here; the review in
[ORB-26670] found the gap.

### Decision

A deviation from **STD-01@2 §R10** for format 23, with this scope:

- JSON keys `daniel_turns` → `user_turns` and `daniel_first_turns` →
  `user_first_turns` in `transcript list --json` and `transcript show --json`.
- Error code `daniel_only` → `user_only`, returned when an agent write
  touches the user's own entries.
- Author values `kind: daniel` → `kind: user` and `name: daniel` →
  `name: user`, in every `--json` record that carries an author.
- Human labels, outside the machine contract: the `USER_TURNS` table header
  and the `user_turns` and `user_first_turns` labels in `show`'s table form.
  Plain output has no header and prints one line per record, so R9's piped
  form carries none of them.
- `daniel_asked` → `user_asked`: `specs/output-contract.md` names this
  issue-occurrence label, but no `asked` label exists in the code at this
  HEAD, so neither spelling is printed. It is listed so the rename is on
  record.

No alias. The old keys, code and author values are not emitted beside the
new ones, for three reasons:

1. §15.8 says "Nothing shipped names a person." An alias would put
   `daniel_turns` back into every transcript payload.
2. No release has shipped: there is no tag, and the CHANGELOG has only
   `Unreleased`. The names become stable at the first release, the same rule
   the list envelope recorded under [ORB-13134] follows.
3. The only readers, the skill and the tmux popup, changed in the same
   package. A search of the repository at this HEAD finds no old key, code or
   author value in output code, the plugin or the skill; the only code hit is
   a test function's name.

A second deviation, from **STD-01@2 §R35**, covers one accepted value.
`--author-kind daniel` and the stored `daniel` author stay accepted, as a clap
value alias (`crates/scribe/src/commands/entry.rs`,
`crates/scribe/src/commands/proposal.rs`) and a serde alias
(`crates/scribe-core/src/record.rs`), and they print nothing. R35 requires a
stderr warning on use of a renamed flag or config key; an accepted value is
named in R35 only as a breaking change, so this silent alias is recorded here
rather than justified as a warning. The config key
`[tags.<tag>.capture] from = "daniel"` does warn, as deprecated, and is read
for a 30-day window (`crates/scribe-core/src/config.rs`).

### Alternatives

- **Emit the old names beside the new ones.** Keeps any script working, but
  puts the personal name back into the machine contract that §15.8 removes,
  and keeps the names until a release that has not happened. Rejected.
- **Record the deviation (chosen).** Costs silent breakage for any script
  written against the old names.

### Consequences

- Cost: a script that reads `.daniel_turns` gets `null`, and one that matches
  `daniel_only` finds nothing, with no warning. None is known to exist.
- From the first release these names are frozen; a later rename is a breaking
  change under §R10 and needs an alias path.
- The S14 rows in `2_design.md` keep the old keys as provenance, as §15.8
  allows. `specs/output-contract.md` is the current contract.
