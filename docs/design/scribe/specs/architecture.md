---
type: design
summary: "Spec: architecture — four crates (scribe, scribe-core, scribe-store, scribe-tools), apps with one exec each, runtime and composition in core, repositories per record, record shapes as store contracts, pure tools, and the migration from the domain/store/core/CLI split"
last_validated: 2026-11-24
---

# Spec: Architecture

Scribe is a CLI over one store. Every command and hook parses its input, calls
one `exec` in `scribe-core`, and renders what comes back. Everything about one
subject (`group`, `entry`, `session`) is in one app folder in the core and one
repository in the store. Scribe starts no model and no agent: work that needs
one is an Orbit job that reads and writes through scribe's commands.

Written by 0-chief for Daniel on 2026-11-24 against scribe at `763052c`
(after ORB-27911 removed `ask` and the tmux popup), and approved by him. It
replaces the layer model in `ARCHITECTURE.md` and supersedes the four-crate
decision of ORB-27707; ORB-27921 carries the migration. Rationale and
Daniel's words are in
[4_decisions.md](../4_decisions.md#scribe-is-laid-out-by-app-in-four-crates-scribe-core-store-and-tools).

## Why This Exists

Four failures in the code at `763052c`:

- **One subject is spread over four places.** A group change touches
  `scribe-domain/src/group.rs` and `checks/group.rs`, eleven `group*` files in
  `scribe-core/src/cases/`, `scribe-store/src/db/groups.rs`, and
  `crates/scribe/src/commands/group.rs`.
- **`cases/` is flat.** 50 files side by side (`entry`, `entry_cmd`,
  `entry_verify`; `proposal`, `proposal_cmd`, `proposal_command`). The
  refactor that was meant to remove a flat structure rebuilt one.
- **The binary is not just a CLI.** It holds composition (`app.rs`), the
  model adapters and their process supervision (`models.rs`, `models/`), the
  port implementations (`commands/ports.rs`) and 25 KB of hook payload
  parsing.
- **The domain's rings follow the imports, not a design.** `ARCHITECTURE.md`
  lists five modules moved to another ring and six more the plan never named,
  each because an import forced it. A layer whose membership is decided by
  imports gives no guidance on where new code goes.

## Crates

```
scribe  ->  scribe-core  ->  scribe-store
                 |                ^
                 +-> scribe-tools-+   (contracts only)
```

| Crate | Owns | Internal dependencies | Banned |
|---|---|---|---|
| `scribe` (binary) | Argument declarations, the mapping from arguments to an app command, rendering, exit codes. The only stdout, stderr and terminal access. | `scribe-core` | `rusqlite`, `scribe-store`, `scribe-tools` |
| `scribe-core` | The apps, the runtime they share, and composition. | `scribe-store`, `scribe-tools` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite` |
| `scribe-store` | The data root and `scribe.db`, one repository per record, and the record shapes (`contracts`). The only crate that touches the data root. | none | `clap`, `tracing-subscriber`, any terminal crate |
| `scribe-tools` | Pure functions over bytes and text: transcript parsing, window building, marker and payload parsing. | `scribe-store` (its `contracts` module only) | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile`, `rustix`, `std::fs`, `std::net`, `std::process`, `std::env` |

`scribe-domain` is deleted. Its contents go to three places by kind
([Where each current file goes](#where-each-current-file-goes)).

The binary names only `scribe-core`. The core re-exports the record shapes it
returns as `scribe_core::contracts`, so rendering needs no second dependency.

## `scribe` (binary)

```
main.rs              capture the environment, parse, dispatch, map the result to an exit code
cli.rs               clap declarations
commands/<noun>.rs   arguments -> apps::<app>::Command; outcome -> payload
output/              payloads, rendering, the sink
error.rs             app errors -> one-line message and exit code
help.rs
```

A command handler does three things and nothing else:

1. turn parsed arguments into the app's `Command`;
2. call `scribe_core::apps::<app>::exec`;
3. turn the `Outcome` into an output payload.

Invariants:

- No handler opens the store, resolves a path, reads a transcript, reads an
  environment variable or decides anything about a record. If a handler needs
  an `if` about a record's state, that branch belongs in the app.
- `app.rs`, `models.rs`, `models/`, `commands/ports.rs` and
  `audit_middleware.rs` leave the binary (see [Composition](#composition) and
  [Runtime](#runtime)).
- Hook payload parsing (`commands/hook/payload.rs`) moves to `scribe-tools`;
  project resolution for a hook (`commands/hook/project.rs`) moves to the
  `hook` app.
- Every top-level command maps to exactly one app:

| Command | App |
|---|---|
| `entry` | `entry` |
| `group` | `group` |
| `session`, `transcript` | `session` |
| `mark` | `mark` |
| `proposal` | `proposal` |
| `memory` | `memory` |
| `tag` | `tag` |
| `search` | `search` |
| `brief` | `brief` |
| `consistency` | `consistency` |
| `hook` | `hook` |
| `init`, `store`, `export`, `plugin` | `admin` |

A `plugin` routine tool (`session-pass`, `consistency-scan`,
`memory-refresh`) is dispatched by `admin` to the app that owns the work; it
has no flow of its own.

## `scribe-core`

```
lib.rs               pub mod apps; pub use composition::Env; pub use store contracts
apps/
  <app>/
    mod.rs           Command, Outcome, Error, exec: dispatch only
    <flow>.rs        one file per command or hook event
    rules.rs         this app's pure rules (rules/ when it outgrows one file)
runtime/
  mod.rs             Runtime, Ctx
  host.rs            clock, transcript discovery and reads: the real ports
  resolve.rs         id and session resolution
  errors.rs          Error, read and write error mapping
  audit.rs           the audit guard around a mutating command
composition.rs       Env, roots, config, store, redactor
config.rs            config.toml shapes and parsing
providers/           interim: the Models port, its shapes and adapters
```

### Apps

- An app is one subject a user names on the command line. Its public surface
  is exactly `Command`, `Outcome`, `Error` and

  ```rust
  pub fn exec(env: &Env, command: Command) -> Result<Outcome, Error>
  ```

  `exec` starts the runtime, runs the command inside the audit guard when it
  mutates, and returns. It contains no flow.
- Under it, each app has `pub(crate) fn run(ctx: &Ctx<'_>, command: Command)`,
  which is `exec` without the runtime start. Tests and other apps call `run`.
  The crate's `test-support` feature exposes `run` and `Ctx` to the binary's
  integration tests, as the existing feature does for the store's test
  helpers.
- A flow has three steps, in this order:
  1. **load** what it needs from one `store.read()` snapshot into a plain
     input value;
  2. **plan** with pure functions from the app's `rules` and `scribe-tools`;
  3. **commit** in one `store.write(|tx| …)`, re-running inside the closure
     any check that must be atomic with the write.

  A flow never interleaves reads and writes outside one write closure, and
  never holds a snapshot across a write.
- The `*Verbs` extension traits on `Store` (`EntryVerbs`, `GroupVerbs`,
  `SessionVerbs`, …) stop being exported. Each becomes private to its app or
  is folded into the app's flows. Nothing outside `apps/<app>/` calls them.
- **An app may call another app only through that app's `run` or a function
  its `mod.rs` re-exports.** It never imports another app's `rules` or a flow
  file. App imports form a directed acyclic order, held in the guard script.
  Starting order, lowest first; the implementer confirms it against the
  imports and records the final order in `ARCHITECTURE.md`:

  ```
  tag, mark  <  entry, proposal, memory  <  session, group  <  search, brief, consistency  <  hook, admin
  ```
- There is no `apps/common`, `apps/shared` or `apps/util`. Code two apps need
  goes to the lower of the two in the order above, to `runtime` if it is about
  the context (ids, errors, audit), or to `scribe-tools` if it is pure parsing.

### Runtime

`runtime` is what every app runs against. It is `pub(crate)`: the binary never
sees it.

- `Runtime::start(env)` calls `composition` and owns the result: the store,
  the config, the redactor, and the real clock and transcript ports.
- `Ctx` is the borrowed view a flow receives: `store`, `config`, `clock`,
  `transcripts`, `redaction`. The port traits (`Clock`, `Transcripts`,
  `Redaction`, `AuditNotes`) stay as they are in `cases/ctx.rs`, so the
  existing fakes keep working.
- `runtime/host.rs` holds the real implementations that the binary supplies
  today from `commands/ports.rs`.
- `runtime` holds four subjects and no others: the context, id resolution,
  error mapping and the audit guard. A function belongs here only if it is
  about one of them. "Two apps use it" is not a reason.

### Composition

`composition.rs` is the one place that resolves anything about the host.

- `Env::from_process()` captures the environment variables scribe reads
  (`SCRIBE_ROOT`, `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `CLAUDE_CODE_ENTRYPOINT`, the redaction variables). `main.rs` calls it once
  and passes the value down. Tests build an `Env` by hand, as
  `tests/app.rs` does with `AppEnv` today.
- Composition picks the data directory, reads `config.toml`, opens the
  `Store`, and builds the redactor.
- Outside `composition.rs`, nothing in the core reads an environment variable
  or discovers a path. Outside `composition.rs` and `runtime/host.rs`,
  nothing in the core touches a file that is not reached through the store.

### Providers (interim)

Scribe's own model calls are being removed (ORB-27911). Five callers cannot
lose their model step until an Orbit job replaces it: session pass, session
catalog, session rebuild, memory refresh and group close. What replaces them
is not decided (open-item-19) and this spec does not decide it.

Until then:

- `core/providers/` holds everything about calling a model: the `Models`
  port, the request and answer shapes now in `scribe-domain/src/models.rs`,
  and the adapters now in `crates/scribe/src/models.rs` and `models/codex.rs`
  with their process supervision.
- Only `providers/` spawns a process. Only the five callers' flows import
  `providers`; the guard script lists them by path.
- When a caller's job exists, its import goes. When the last one goes, the
  folder is deleted with the `[models]` config section. The credential names
  in `[models] credentials` also select what redaction masks: they move to
  `[redact] env` in the same change.

## `scribe-store`

```
lib.rs               Store::open, read() -> Snapshot, write(|tx| …)
db/                  the connection, schema and migrations, row access per table;
                     fsio, locks, atomic writes, the codec, the archive import
repository/          one per record: entries, sessions, messages, proposals, tags,
                     groups, memory, marks, pickups, audit
contracts/           record shapes
redact.rs            the Redactor applied at the repository boundary
```

Invariants:

- **One repository per record, never per app.** Apps and records do not line
  up: the session app writes sessions, messages, proposals, entries and
  memory. An app composes repositories inside one snapshot or one write.
- **The store holds no rule and no use case.** No function named after a
  command (`settle_pass`, `close_group`), no validation beyond what a
  contract type enforces on itself, and no type named after a use case.
  `CloseGroupOutcome`, `PassWrite` and `PassWritten` in today's `lib.rs` move
  to the app whose flow produces them, or become a repository's plain result.
- **`contracts/` holds shapes only.** Allowed: type definitions, a
  constructor or parser that enforces the type's own invariant (`EntryId`
  accepts `e-<n>`, `TagName` its character set), serde and `Display`
  implementations, and conversions between two contract types. Not allowed: a
  function that takes two records and decides something, a function named
  `check_*`, a limit or constant that belongs to a flow (window sizes, retry
  counts), or anything that reads the clock.
- `contracts` imports nothing from `db/` or `repository/`. Only `db/` uses
  `rusqlite`. `db/fsio` imports nothing else from the store.
- `Store` stays one concrete type. A trait is added when a second backend
  exists.
- Project scaffolding (`init.rs`) is not persistence. It moves to the `admin`
  app, which writes through a repository or `db/fsio` like any other flow.

## `scribe-tools`

Pure functions. Bytes, text or a contract value in; a value out.

```
transcript/claude.rs, codex.rs   transcript bytes -> messages
window.rs                        the turns a pass or a capture looks at
markers.rs                       `#scribe` markers, settling lines, `/scribe` commands
payload.rs                       hook payloads -> typed events
text.rs                          tokens, normalization, verbatim comparison
```

Invariants:

- No I/O, no clock, no environment, no randomness: the same input gives the
  same output. A function that needs the time takes it as an argument.
- It imports `scribe_store::contracts` and nothing else from the store. It
  never receives a `Store`, a `Snapshot` or a `Tx`.
- It decides nothing about a record's lifecycle and returns no refusal about
  one. "Is this line a marker" is a tool; "may this entry be approved" is an
  app rule.
- It exists below the core because the Stop and capture hooks parse
  transcripts, so parsing must be reachable from an app. Whatever an Orbit
  activity calls that reads or writes records is an ordinary command mapped
  to an app, and that app uses these functions.
- When a model step leaves scribe, the pure halves of that flow (building
  the input a job hands its agent, checking the shape of what the agent
  returns) move here. Their names and shapes wait for open-item-19.

## Where each current file goes

Placement rule, in order. The tables apply it to the tree at `763052c`; where
an import shows a different home, place the file there and say so in the pull
request.

1. A type that is stored or returned as a record is a contract.
2. A function of bytes or text that decides nothing about a record is a tool.
3. A rule belongs to the app whose command it serves. Used by two apps, it
   goes to the lower one in the app order.
4. Anything about the context goes to `runtime`; anything about the host to
   `composition`.

### `scribe-domain`

| Module | Shapes to `store/contracts/` | Functions to |
|---|---|---|
| `record`, `origin`, `provider` | all | — |
| `entry`, `entry/records`, `action`, `handoff` | `Entry`, ids, statuses, patches, filters, `Mode` | `apps/entry/rules` |
| `session`, `activity`, `control` | `SessionRecord`, ids, `ControlState`, activity shapes | `apps/session/rules`; `control::parse` to `tools/markers` |
| `proposal`, `pass/candidates` | `Proposal`, ids, states, `EntryCandidate` | `apps/proposal/rules` |
| `tag` | `TagName` and tag shapes | `apps/tag/rules` |
| `group` | group and seat shapes | `apps/group/rules` |
| `memory/mod`, `memory/profile` | memory items, profile shapes | `apps/memory/rules` |
| `message` | `Message` rows | `apps/session/rules` |
| `transcript`, `transcript/codex` | the parsed message shape | `tools/transcript/` |
| `text`, `verbatim` | — | `tools/text` |
| `capture` | — | marker scanning to `tools/markers`; settling rules to `apps/hook/rules` |
| `annotation` | mark shapes | `apps/mark/rules` |
| `pass`, `pass/repair`, `claim`, `trigger`, `instruction` | — | `apps/session/rules/` (window building to `tools/window`) |
| `voice` | — | `tools/markers` if it only classifies text, else `apps/hook/rules` |
| `brief` | — | `apps/brief/rules` |
| `consistency` | — | `apps/consistency/rules` |
| `search` | — | `apps/search/rules`; the tokenizer to `tools/text` |
| `checks/{entry,records}` | — | `apps/entry/rules` |
| `checks/{group,memory,tag,admin}` | — | the app of the same name |
| `checks/candidates` | — | `apps/proposal/rules` |
| `models` | — | `core/providers/` |
| `config` | — | `core/config.rs` |
| `error` | — | `core/runtime/errors.rs` |
| `redact` | — | `store/redact.rs` |
| `fixtures` | — | `store`'s `test-support` feature |

### `scribe-core/src/cases/`

| Files | To |
|---|---|
| `entry`, `entry_cmd`, `entry_verify`, `pickup` | `apps/entry/` |
| `group`, `group/capture`, `group/close`, `group_add`, `group_backfill`, `group_close`, `group_cmd`, `group_join`, `group_leave`, `group_open`, `group_turns` | `apps/group/` |
| `session`, `session_activity`, `catalog`, `rebuild`, `pass`, `pass/run`, `continuation`, `transcript`, `codex`, `scribe_command` | `apps/session/` |
| `mark`, `mark_retry`, `annotation` | `apps/mark/` |
| `proposal`, `proposal_cmd`, `proposal_command` | `apps/proposal/` |
| `memory`, `memory_block`, `memory_cmd` | `apps/memory/` |
| `tag`, `tag_cmd` | `apps/tag/` |
| `search` | `apps/search/` |
| `brief` | `apps/brief/` |
| `consistency_scan` | `apps/consistency/` |
| `hook`, `session_start`, `session_end`, `stop`, `user_prompt_submit`, `instruction_check` | `apps/hook/` |
| `ctx`, `resolve`, `read_error`, `write_error`, `audit` | `runtime/` |

Inside an app, a `<noun>_cmd` or `<noun>_command` file is the dispatcher and
becomes `mod.rs`; the rest become flow files named for their command
(`apps/group/{open,join,leave,add,close,backfill,turns}.rs`).

### `scribe-store`

| Files | To |
|---|---|
| `db.rs`, `db/*`, `fsio`, `write`, `codec`, `archive`, `rebuild` | `db/` |
| `repo.rs` | `repository/mod.rs` (`Snapshot`, `Tx`) and one file per record |
| `documents`, `sessions`, `memory`, `tags`, `marks`, `pickups`, `candidates`, `ids` | the repository of that record; id resolution helpers stay beside it |
| `audit` | `repository/audit.rs` |
| `lib.rs` list and view types | `contracts/` when a command returns them, else the repository |
| `init` | `scribe-core/src/apps/admin/` |
| `seed`, `testing` | unchanged, behind `test-support` |

### `scribe` (binary)

| Files | To |
|---|---|
| `app.rs` | `scribe-core/src/composition.rs` |
| `models.rs`, `models/codex.rs` | `scribe-core/src/providers/` |
| `commands/ports.rs` | `scribe-core/src/runtime/host.rs` |
| `audit_middleware.rs` | `scribe-core/src/runtime/audit.rs` |
| `commands/hook/payload.rs` | `scribe-tools/src/payload.rs` |
| `commands/hook/project.rs`, `group_add.rs`, `group_turns*`, `codex.rs`, `stop.rs`, `session_*.rs`, `user_prompt_submit.rs` | whatever is not argument mapping goes to `apps/hook/` |
| `commands/session/{pass,catalog,rebuild,activity}.rs` | whatever is not argument mapping goes to `apps/session/` |

## Guards

`scripts/check-dependency-direction.sh` enforces what a crate edge cannot.
The ring map for `scribe-domain` is removed with the crate. The script then
holds:

- the banned crates per crate, from the table above (normal dependencies
  only, as today);
- the app order: an app imports only apps below it, and only through
  `apps::<app>::` paths that `mod.rs` exports;
- no `apps/common`, `apps/shared` or `apps/util`;
- only `composition.rs` names `std::env`; only `composition.rs`,
  `runtime/host.rs` and `providers/` name `std::fs`; only `providers/` names
  `std::process`;
- only the five listed flows import `providers`;
- the binary names no `scribe_store` and no `scribe_tools` path;
- `scribe-tools` names no store path but `scribe_store::contracts`;
- in the store: `contracts/` imports no `db` or `repository`; only `db/` uses
  `rusqlite`; no `fn check_` under `contracts/`.

## Migration

No step changes behaviour: no CLI, output, help, exit-code, format-version or
schema change. ORB-27911 carries the deletions, and its goldens change there,
not here. Each step compiles and passes CI alone. Existing tests and goldens
move with their code, unmodified in content. No new test cases without
Daniel's approval (his rule of 2026-11-23); guards go in the script.

1. **Land this spec** as `docs/design/scribe/specs/architecture.md`, with the
   decision in `4_decisions.md` and the four-crate decision
   marked superseded.
2. **Apps and runtime.** `cases/` becomes `apps/<app>/` and `runtime/`. Each
   app gets `Command`, `Outcome`, `Error`, `run` and `exec`. Handlers in the
   binary call `exec`. The app order guard arrives.
3. **Composition out of the binary.** `app.rs`, the port implementations,
   the audit guard and the model adapters move into the core. After this
   step the binary depends on nothing but `scribe-core`, and its tiers table
   in `ARCHITECTURE.md` is one row.
4. **Store layout.** `db/`, `repository/` and an empty `contracts/`; use-case
   types leave `lib.rs`; `init.rs` moves to `admin`.
5. **Dissolve `scribe-domain`.** Shapes to `contracts/`, pure parsing to the
   new `scribe-tools` crate, rules to their apps, `config` and `error` to the
   core. One record family per pull request (entry, session, proposal, …) so
   each is reviewable; the crate is deleted in the last one.
6. **Docs and guards.** `ARCHITECTURE.md` rewritten from this spec, the
   script updated, `AGENTS.md` pointers and the paths in the other specs
   corrected.

Steps 2 and 3 can run before ORB-27911 finishes; they touch the same files as
its step 2 (the annotation and consistency model judgements), so the captain
sequences them. Step 5 is easier after ORB-27911, since less code moves.

## Not decided here

- What replaces the five model callers, the Orbit jobs' shape and the tools
  an agent reads and writes through (open-item-19).
- Whether group close takes a `--summary` flag, and what the consistency scan
  keeps (open-item-19).

A departure from the crates, from "one repository per record", from
"contracts hold shapes only" or from "tools are pure" goes back to Daniel
through the captain.

