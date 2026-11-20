---
title: scribe — Decisions
owner: claude
last_updated: 2026-11-15
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: decisions
type: design
summary: Why superseded entries are hidden not removed, why one JSON file per tag and per session with no persisted index, why no migration from orbit-office and only two default tags, why hooks call the binary and agents see only ask, why models run through the claude CLI and which model does what, why session passes run from an opt-in routine or by hand with bounded windows and retries, why frontmatter is parsed by yaml_serde, why a session record is a rewritable header over an append-only log of segments, why sessions carry an active flag deactivated mainly when their feature is removed and why only Daniel-authored entries count as removal evidence, why agents read and write through the CLI under one skill with authors recorded and Daniel's entries left to him, why tags agents create live in the store, why a tmux popup rather than an app, and why backfill catalogs only sessions Daniel names, why action tags are config-marked with a capped window, a cancel window, pickups, a stuck line, a CLI quote check and SSH reads from the owner host, plus the engineering decisions carried over from the Rust CLI template.
tags: [scribe, decisions]
paths: ["Cargo.toml", "crates/**", "plugin/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21753, ORB-21764, ORB-21778, ORB-21784, ORB-21787, ORB-21795, ORB-23829, ORB-13117, ORB-13134, ORB-23849, ORB-23892, ORB-24005, ORB-24006, ORB-25080, ORB-25112, ORB-25977]
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
`stale_after_days = 180`). Further tags are added by Daniel editing the
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

## `session-start` reads only

**Recorded:** 2026-11 · [ORB-21795]
**Code anchors:** `crates/scribe/src/commands/hook/session_start.rs`

### Context

[§3](./2_design.md#3-storage) has `scribe hook session-start` create
`config.toml` with the defaults on first run. Every command already treats a
missing file as the defaults, and the hook runs before every session.

### Decision

`session-start` only reads `proposals.json` and prints the open count; it
writes nothing, so it is not audited and cannot fail a session start on a
read-only or missing store (a read error is logged and counts as none).

### Consequences

- Daniel creates `config.toml` when he wants to change a default; nothing
  writes it for him.
- Cost: no file on disk to show the defaults; `scribe tag list` shows them.

## Session links are typed, stored on the child, and manual

**Recorded:** 2026-11 · [ORB-23849]
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
- Links are manual: `/scribe on --from <id>` and `/scribe link <id>
  [--type …]`, typed by Daniel in the child. An unknown target, a
  self-link, a cycle (through links of either type), a second parent and a
  `link` in an uncataloged session are refused with stable codes and change
  nothing. Neither the hook inputs nor the transcripts carry a parent
  session id (checked across 4,970 transcripts on dk-server-1,
  [2_design.md §5.4](./2_design.md#54-session-links)), so nothing is
  linked automatically.
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
- Cost: links exist only where Daniel types them; a forked or side session
  he forgets to link stands alone, as before.
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
