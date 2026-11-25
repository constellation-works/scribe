# Architecture

The layer model that [`scripts/check-dependency-direction.sh`](scripts/check-dependency-direction.sh) enforces
([STD-02](docs/standards/STD-02-rust-architecture-and-errors/STD-02.md) R1, R6).
A lower layer never imports a higher one. A new crate, edge or domain module
changes this file and that script in the same commit.

Why four crates: [the four-crate decision](docs/design/scribe/4_decisions.md#four-crates-domain-store-use-cases-and-cli)
(ORB-27707). Other decisions are in [4_decisions.md](docs/design/scribe/4_decisions.md).

> **Being replaced.** Daniel approved a layout by app on 2026-11-24:
> [specs/architecture.md](docs/design/scribe/specs/architecture.md)
> ([decision](docs/design/scribe/4_decisions.md#scribe-is-laid-out-by-app-in-four-crates-scribe-core-store-and-tools)).
> ORB-27921 migrates the code step by step; this file describes the code as
> it is until its last step rewrites it from the spec.

## Crates

```
scribe-domain  <-  scribe-store  <-  scribe-tools  <-  scribe-core  <-  scribe
```

`scribe-tools` names `scribe-domain` directly until the domain is dissolved
(ORB-27921 step 5), and the binary names `scribe-tools` from ORB-27921 step 3
PR 2 until PR 4, when composition is in the core.

| Crate | Owns | Internal dependencies | Banned |
|-------|------|-----------------------|--------|
| `scribe-domain` | Records and the pure rules over them: entries, sessions, proposals, tags, the parsing of config, and the request and answer shapes of the `Models` port. Values only, no I/O. | — | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile`, `rustix` |
| `scribe-store` | The data root and `scribe.db`: the `Store` (`open`, `read() -> Snapshot`, `write(\|tx\| …)` and the repositories), locks, atomic writes, format stamps, the import of the file store's archives, the audit record, and the redactor every persisted free-text field passes (`redact.rs`). The only crate that touches the data root. | `scribe-domain` | `clap`, `tracing-subscriber` and any terminal crate |
| `scribe-tools` | Functions of host files, bytes and text that decide nothing about a record: provider transcripts found under their roots, read under the provenance checks and parsed into typed messages (`transcript/`, with `codex`), the origin guard (`origin.rs`), a working directory's project from git's own files (`project.rs`), the `#scribe` markers, settling lines and `/scribe` control lines of a typed turn (`markers.rs`, with `control`), the fields of a hook's stdin payload and their decoding (`payload.rs`), a session pass's window over a transcript's messages (`window.rs`), the text primitives every layer may use: ranking tokens, sensitive and finance words, claim, verbatim and trigger predicates (`text/`), the host's name (`host.rs`), and the crate's only `std::fs` access (`fs.rs`). | `scribe-domain`, `scribe-store` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile` |
| `scribe-core` | The apps (`apps/<app>/`, one per subject, each holding its use cases) and the `runtime/` they run against: the context and its ports, id resolution, the shared errors and the audit lines a use case owes. Each use case loads what it needs from a snapshot, plans with domain functions, calls the ports and commits in one store write. Its root re-exports `scribe-domain` and `scribe-store` at the paths they had in it. `composition.rs` captures the environment (`Env`) and resolves the `Context` the apps run against; `runtime/host/` implements the ports on it; `providers/` holds the `Models` port and the model adapters. | `scribe-domain`, `scribe-store`, `scribe-tools` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite` |
| `scribe` (binary) | Arguments, the grouped root help, dispatch, the surface error and rendering. The audit guard runs inside the core's `exec` (`runtime/audit.rs`). | `scribe-core`, `scribe-tools` (interim) | `rusqlite` |

The store holds no rule: a check that decides whether a write may happen
belongs to its app, which passes it into the write as a closure the store
runs at the same step and under the same lock. `forgotten_refusal` is the
deliberate exception. A forgotten session is a tombstone, and every writer
that touches a session must refuse it under the write lock, including the
store's own internal writers, so the refusal stays beside `Error` in the
store's `error.rs` rather than in one app.

The banned crates are normal dependencies only. Dev-dependencies are exempt:
the tests of `scribe-core` and `scribe` stage database states with
`rusqlite`, and `scribe-core`'s use `tempfile` for their roots. The script
reads the `[dependencies]`, `[build-dependencies]` and target-specific tables,
never `[dev-dependencies]`.

### Test support

Each library has a `test-support` feature that only dev-dependencies turn on,
so a shipped build never compiles it:

- `scribe-domain`: `fixtures`, the record fixtures the tests share.
- `scribe-store`: `Store::seed_records` (`seed.rs`), so the CLI's integration
  tests can stage records, and `testing`. `testing` holds `StoreTestReads`,
  the shared test helpers, and the store internals that the store tests in
  `scribe-core` reach: `StoreTestInternals`, whose methods each delegate to a
  private store method, and modules named after the store's own (`audit`,
  `codec`, `db`, `memory`, `write`). None of it is in a normal build's public
  API.
- `scribe-core`: turns on `scribe-store`'s and re-exports `testing`.

The store's tests that call the use cases (`*Verbs`) are in `scribe-core`
(`src/tests/store/`), since `scribe-store` cannot see the use cases and has
no dev-dependency on `scribe-core`. Its other tests are in `scribe-store`.

## Apps inside `scribe-core`

An app is one subject a user names on the command line
([the architecture spec](docs/design/scribe/specs/architecture.md#apps)).
Apps import each other in one order, lowest first, and only through what the
lower app's `mod.rs` exports, never one of its flow files. The script holds
the order; a directory under `apps/` that is not in it fails the check, and
so does `apps/common`, `apps/shared` or `apps/util`. Only `session`,
`memory` and `group` name the `Models` port: they hold the five model callers
(pass, catalog and rebuild; memory refresh; group close), and each carries the
adapter in its `Command`.

```
tag < mark < memory < entry < proposal < session < group < search < brief < consistency < hook < admin
```

This is the spec's starting order made total by the imports: `proposal`
calls `entry`, `session` calls `entry` and `proposal`, `group` calls
`session`, `brief` calls `memory`, `consistency` calls `proposal` and
`session`, and `hook` calls all of them. `admin` holds `init`, `store
migrate` and `export`. Migration step 3 moves the plugin's routine dispatch
into `admin`, which then runs the routines of `session`, `consistency` and
`memory`.

Each app's `mod.rs` holds its `Command`, `Outcome` and `Error`, a public
`exec` and a crate-private `run`. Its flows live in private files beside
it. A binary handler maps its arguments to a `Command`, calls `exec`, and
renders the `Outcome` with one total match. Until step 3, `exec` takes the
`Runtime` the binary builds from its parts
(`exec(&Runtime, Command)`). Step 3 changes it to take the `Env`, starts the
runtime inside, and moves the audit guard into `runtime/audit`, where
`HookOutcome`, the shape the guard reads for a hook's lines, already is.

The binary names no `scribe_core::cases`: `cases/` is gone, and the script
fails if either crate names it again.

## Tiers inside `scribe`

| Tier | Unit | Owns | May depend on |
|------|------|------|---------------|
| Surface | `scribe::cli`, `::help`, `::commands`, `::error`, `::output` | Argument declarations, the grouped root help built from them (`help`), dispatch, the surface error type every command ends with (`error`), rendering, and the only stdout, stderr and terminal access. `main` captures `scribe_core::composition::Env` once and resolves the `Context` (`scribe::Context`, composition's with the binary's error). | `scribe-core` |

## Rings inside `scribe-domain`

The domain's modules are flat files, each in one ring. A module imports only
modules of its own ring or an earlier one, by path, in a grouped import, or
through a name the crate root re-exports. The script holds the module→ring
map; a new module fails the check until it is given a ring.

| Ring | Modules |
|------|---------|
| 1. base | `record`, `provider` |
| 2. records | `session` |

`brief`, `search` and `consistency` left the domain in ORB-27921 step 5
(PR 5-3) for the core's `apps/{brief,search}/rules.rs`. `consistency` was
split by its lowest user: the stale step is in `apps/consistency`, settling
and approval in `apps/proposal`, and `forget` with the `Settled` outcome in
`apps/entry`. The core's root re-exports them as `crate::brief`,
`crate::search` and `crate::consistency`.

`action` followed in PR 5-4: `Pickup` to the store's
`contracts/action.rs`, the cancel window, state, stuck count and quote check
to `apps/entry/rules.rs`, and `tag_counts` to `apps/tag/rules.rs` (the tag
app sits below entry in the app order). The core's root re-exports them as
`crate::action`.

`checks` followed in PR 5-5, each file to the app it serves:
`checks/{entry,records}` to `apps/entry/rules.rs`, `checks/candidates` to
`apps/proposal/rules.rs`, and `checks/{group,memory,tag,admin}` to the app
of the same name. From `checks/mod.rs`, `refusal` and `forgotten_refusal`
went to the store's `error.rs`, beside `Error`. The agent-author check went
to `apps/tag/rules.rs`, and the free-text and reason checks to
`apps/entry/rules.rs`, the lowest app that uses them. The session-write
checks went to `apps/session/rules.rs`. `checks::group::number`, the
`g-<n>` parser, went to the store's `repository/ids.rs` beside
`resolve_prefix`, where the store keeps id-format parsing; contracts hold
shapes only. The store root re-exports it as `group_number`.

PR 5-6 moved `activity` and `pass` out and split `control`:

- `activity` went whole to `apps/session/rules/activity.rs`.
- `control`'s switch transitions went to `apps/session/rules/control.rs`.
  Its line parser (`parse`, `command_text`, `is_control`, `Switch`) stayed in
  the domain's `control` while the domain's `transcript` read it. It went to
  `tools/markers/control.rs` in PR 5-12.
- `pass` went to `apps/session/rules/pass.rs`, with `pass/repair` under it.
- The pass's window (`conversation`, `window`, `window_with_limits`) went
  to `scribe-tools/src/window.rs`.
- `pass/candidates` went to `apps/proposal/rules/candidates.rs`.

The core's root re-exports them as `crate::activity`, `crate::control` and
`crate::pass`.

PR 5-8 moved `annotation` and `group`:

- The shapes the store keeps go to the store's contracts:
  - `MarkRecord`, `MarkState` and their codes and attempt limit go to
    `contracts/mark.rs`;
  - `Group`, `Seat`, `NewGroup` and `SeatHold` go to `contracts/group.rs`.
- The marker refusal (`record_tag_refusal`, `CheckRefusal`) goes to
  `apps/mark/rules.rs`.
- The group app's requests and outcomes, the consent a quoted turn gives and
  the automatic seat name go to `apps/group/rules.rs`.
- `entry_belongs` goes to `apps/entry/rules.rs`, because only the entry app
  uses it.

The core's root re-exports them as `crate::annotation` and `crate::group`,
with the marker scanning from `scribe_tools::markers` (PR 5-9).

PR 5-9 moved `capture` and `instruction`:

- `capture`'s marker scanning goes to `scribe-tools/src/markers.rs`, and its
  settling-line parser goes with it, because `marks` skips a settling line
  by parsing it.
- A typed turn (`Turn`) and its prompt hashes go to `apps/hook/rules.rs`.
- `capture`'s re-exports of the origin guard and the text hashes are
  dropped. Their users name `origin` and `text`.
- `instruction` goes to `apps/entry/rules/instruction.rs`, because the entry
  app is its lowest user. `ClaudeSeen`, what a member's Claude capture has
  seen of a message id, goes to the store's `contracts/group.rs`.
- The group app now resolves a Claude batch against the seen ids, read
  through the store's `GroupRows::seen`. The store no longer runs that rule.

The core's root re-exports them as `crate::capture` and `crate::instruction`.
`verbatim` stayed in the domain's base ring because `claim` imported it,
until PR 5-14 moved it to `tools/text` with `claim` and `text`.

PR 5-10 moved `error` whole to the store's `error.rs`, beside `refusal` and
`forgotten_refusal`. Every store call returns that `Error`, and the store
cannot name the core. The store root exports `Error`, `Result` and
`WriteRefusal`. The core's `crate::error` names the store's module, so
`runtime/errors.rs` maps the same type to codes and the core's paths are
unchanged.

PR 5-11 moved `message` and `memory`, each split into store shapes and app
rules:

- `message`:
  - `NewMessage`, `MessageMatch` and `MessageSearch` go to the store's
    `contracts/message.rs`.
  - Which typed turns are kept (`typed_turn`, `keeps`, `typed_text`) goes
    to `apps/entry/rules/message.rs`. The entry app is the lowest user,
    through the instruction check's group capture.
  - A pass window's messages (`window`) go to
    `apps/session/rules/message.rs`.
- `memory`:
  - The files and rows the store reads and writes go to the store's
    `contracts/memory.rs`: `Stem`, `NowKind`, `NowItem`, `Milestone`, their
    front matter, `Raised`, `Pending`, `ProfileView`, the `Refusal` codes
    and the title, text and slug bounds.
  - The checks, drafts, plan, block and profile join `apps/memory/rules.rs`,
    with `profile` under it.

The core's root re-exports both as `crate::message` and `crate::memory`. With
`memory` gone, `transcript`, `codex`, `origin` and `control`'s parser can
leave in PR 5-12, and `config` follows in PR 5-13.

PR 5-12 moved the transcript family to the tools:

- `transcript` became `scribe-tools/src/transcript/parse.rs`. The tools'
  `transcript` module re-exports it, so its parsing sits beside the
  discovery and the provenance check.
- `transcript/codex` went to `scribe-tools/src/transcript/codex.rs`.
- `origin` went to `scribe-tools/src/origin.rs`. The store never reads it,
  and it holds the origin guard's functions, not shapes.
- `control`'s line parser went to `scribe-tools/src/markers/control.rs`.

The tools root names the domain's `entry`, `proposal`, `record`, `session`,
`SessionId` and `Speaker`, and its own `control` and `codex`, at the paths
the moved code uses. The core's root re-exports `crate::transcript`,
`crate::codex`, `crate::origin` and `crate::control` from the tools. The
domain has no transcript ring now.

`redact` went whole to the store's `redact.rs` in PR 5-7. The store root
re-exports `Redactor`, `REDACTED_FIELDS` and `MIN_SECRET_CHARS`.

PR 5-13 moved `config` to `scribe-core/src/config.rs`. The store returns
`config.toml`'s text (`Tags::config_text`) and the store tags (`Tags::all`).
The core parses the text (`config::load`), and the tag app merges in the
store tags (`apps/tag/config.rs`, `ConfigReads`: `config`, `configured` and
`definition`). `Error::Config` carries the parse error as a boxed source.
`Redactor::from_config` (now the core's `RedactorFromConfig` trait) and
`unresolved_redact_env` went with `config`, and build the `Redactor` through
`Redactor::new`. The window limits (`WindowLimits`, `WINDOW_MESSAGES`,
`WINDOW_CHARS`) went to `scribe-tools/src/window.rs`, which uses them.

PR 5-14 moved the domain's text and model modules:

- `text`, `verbatim`, `claim` and `trigger` went to `scribe-tools/src/text/`
  (`mod.rs`, `verbatim.rs`, `claim.rs`, `trigger.rs`). They match and compare
  text and decide nothing about a record. The core's root re-exports
  `crate::text`, `crate::verbatim`, `crate::trigger` and `crate::claim`.
- The SHA-256 hashes (`prompt_sha256`, `bytes_sha256`) went to the store's
  `digest.rs`, per ruling (h). The core's `crate::text` and `crate::capture`
  re-export them.
- `models` went to `scribe-core/src/providers/models.rs`. Its pass-window
  pieces (`PassMessage`, `CUT_MARK`, `PROMPT_MESSAGE_CHARS`) went to
  `scribe-tools/src/window.rs`, which uses them, and the memory context the
  store reads (`MemoryContext`, `PendingContext`) went to the store's
  `contracts/memory.rs`. `providers/models.rs` re-exports both, so
  `crate::models` names them as before.

PR 5-15 moved `proposal` and `voice`:

- The proposal shapes went to the store's `contracts/proposal.rs`: the id,
  kind, state, quote, closed reason, `Admitted`, `EntryCandidate` with its
  own-shape methods (`stated`, `is_older_claim`, `has_valid_provenance`),
  the context and subject bounds and `INSTRUCTED_SAID_PREFIX`, which the
  store's codec reads.
- The rules went to `apps/proposal/rules/proposals.rs`:
  `context_stands_alone`, `close_retired_tag_candidates`, the entry an
  approved candidate becomes (`CandidateEntry::candidate_entry`, a trait so
  the call keeps its text), `ProposalChange` and `ProposalFilter`. The core's
  `crate::proposal` re-exports both halves.
- `voice` was split by its lowest users. The prompt half (`UserName`,
  `ModelUse`, `Instructions`, `check_instruction`, `Voice` and the two
  bounds), which `config.rs` and `providers/models.rs` consume, went to
  `config/voice.rs`. The entry half (whose words an entry holds, and the
  lines and labels that name them) went to `apps/entry/rules/voice.rs`.
  The core's `crate::voice` re-exports both halves.

PR 5-16 moved `tag`:

- The shapes went to the store's `contracts/tag.rs`: `TagTable` with its
  constructor, the shipped tables and its reads, `TagDef`, the capture
  rules (`CaptureFrom`, `CaptureRoute`, `CaptureRules`, `Rule`,
  `RuleOrigin`) and their bounds, `ActionTag`, `FINDING_TAG`, `TagError`,
  and `StoreTag`, which `tags.json` holds.
- The rules went to `apps/tag/rules/tags.rs`: the merge of store tags under
  the configured table (`MergeStore::merge_store`, a trait so the call keeps
  its text), the `tag list` summary (`summarize`, `TagSummary`) and the
  `tag update` patch (`TagPatch`). `MAX_FINDINGS_PER_SESSION` went to
  `apps/entry/rules.rs`, its only user's rules. The core's `crate::tag`
  re-exports all of them.

PR 5-17 moved `entry` and `handoff`:

- The entry shapes went to the store's `contracts/entry.rs` and
  `contracts/entry/records.rs` whole: `Entry`, its ids, statuses, cites,
  outcomes, windows, patches, filters and the decision and open-item
  fields, with their own-shape methods. The store exposes them as the module
  `scribe_store::entry`. `is_decision_or_item` became the contract method
  `TagName::is_decision_or_item`.
- The handoff shape went to `contracts/handoff.rs` (`scribe_store::handoff`):
  `HANDOFF_TAG`, `Mode`, `HEADINGS` and `MAX_CHARS`, and
  `handoff::session_ids` became the contract method `Entry::session_ids`.
  The checks a handoff write must pass (`check`, `check_related`,
  `session_refs_changed`, `is_handoff`, `for_session` and their errors)
  went to `apps/entry/rules/handoff.rs`. The core's `crate::handoff`
  re-exports both halves.
- The `entry` fixture went to the store's `fixtures.rs` (behind
  `test-support`), which re-exports the domain's session fixtures until
  those records move.

These are the rings the code has, and they differ from the approved lists in
ORB-27707 where an import forces it:

- **Naming:** ids live with their records, so there is no `ids` module.

The model request and answer shapes (`models`), the `Models` port,
`ModelError` and the `claude` and `codex` adapters live in
`scribe-core/src/providers/` until ORB-27911 removes scribe's in-process
model calls. The other open targets are
listed in the four-crate decision.

## Module rules

Checked by grep over non-test sources in `scripts/check-dependency-direction.sh`
and `scripts/check-terminal-guard.sh`: what a crate edge cannot express. "Module"
includes any file added later.

- `scribe-domain` and `scribe-core` do no file, network or process I/O (`std::fs`, `std::net`, `std::process`): `scribe-core` reaches files through `scribe-store` and `scribe-tools`. The exceptions are in `scribe-core`: `composition.rs` and `providers/` name `std::fs`, and only `providers/`, which spawns the model processes, names `std::process`, `rustix` and `tempfile`.
- `scribe-domain`, `scribe-store` and `scribe-core` read no environment variables and discover no paths (`std::env::`, `home_dir`, `current_dir()`); the caller passes resolved roots. In `scribe-core` the one exception is `composition.rs`, which imports no app.
- The tools' `text::claim` module owns lexical claim and provenance predicates below the `pass` orchestrator; the tools' `window` owns pass window limits. Their callers (`config`, `consistency`, `memory`, `message`) do not import `pass`.
- A port imports none of the modules that call it: the model shapes do not import `memory` or `group`, and `providers/` imports no app and no runtime.
- Only five flows name the `Models` port (`providers`, `Models`, `ModelError`, `ModelsOnce`, `ModelsEach`): session pass, catalog and rebuild, memory refresh and group close, with the `mod.rs` of their apps.
- In `scribe-store`, `db/fsio.rs` imports nothing of the store but its error type, and `contracts/` imports nothing from `db/` or `repository/` and has no `check_*` function.
- In `scribe-store`, only `db/` uses `rusqlite` (and `testing.rs`, test support).
- The binary spawns no process: `scribe-core`'s `providers/` (the model transports) does.
- In `scribe-tools`, only `fs.rs` names `std::fs`; nothing names `scribe_core`, `std::env`, `std::net` or `std::process`, discovers a path, or calls `Store::write`.
- `output/` imports no `app`, `cli` or `commands`.
- `commands/` writes no file (`std::fs::write`, `create`, `rename`, `remove`, `copy`, `set_permissions`, `OpenOptions`); every write goes through the store.
- Only `output/` touches stdout or stderr, asks whether a stream is a terminal, or reads the color and format variables.

The crate edges replaced these grep rules:

- business modules name no store and no use case;
- the store names no use case;
- `tempfile` and `rustix` stay out of the domain (the core's `providers/` uses them for the model processes);
- `rusqlite` stays out of the domain, the use cases and the CLI.
