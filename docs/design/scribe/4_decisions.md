---
title: scribe — Decisions
owner: claude
last_updated: 2026-11-03
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: decisions
type: design
summary: Why superseded entries are hidden not removed, why one JSON file per tag and per session with no persisted index, why no migration from orbit-office and only two default tags, why hooks call the binary and agents see only ask, why models run through the claude CLI and which model does what, why session passes run from a routine, why frontmatter is parsed by yaml_serde, plus the engineering decisions carried over from the Rust CLI template.
tags: [scribe, decisions]
paths: ["Cargo.toml", "crates/**", "plugin/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21753, ORB-21764, ORB-21778, ORB-21784, ORB-21787, ORB-21795, ORB-13117, ORB-13134]
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

## A session record covers the whole opted-in session

**Recorded:** 2026-11 · [ORB-21747] · **Overridable by Daniel**

### Context

`/scribe on` may be typed mid-session. Summarising only what follows it
gives a record whose goals and outcome miss the session's start; reading
the earlier part could look like the backfill Daniel rejected.

### Decision

Turning a session on catalogs that whole session, from its first message,
within the 150k-character window of [2_design.md §5.3](./2_design.md#53-a-pass):
a longer session loses its oldest messages, with a note in the record.
"No backfill" means no other session is ever read: scribe never scans
transcripts it was not switched on for.

### Consequences

- A record states the session's real goal even when `on` came late, unless
  the session is longer than the window.
- Cost: messages typed before `/scribe on` are summarised, which Daniel may
  not expect; `/scribe off` stops future passes but does not shrink the
  record.

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

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
