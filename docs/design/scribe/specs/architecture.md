---
type: design
summary: "Spec: architecture — four crates (scribe, scribe-core, scribe-store, scribe-tools), apps with one exec each, runtime and composition in core, repositories per record, record shapes as store contracts, tools that keep each subject whole (transcript finding, reading and parsing), and the migration from the domain/store/core/CLI split"
last_validated: 2026-11-25
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

Amended on 2026-11-25 (ORB-27921 step 6) to describe the tree as built. Each
change is a departure the captain accepted during the migration, and each
is listed under [Amendments](#amendments). The four invariants this spec
sends back to Daniel are unchanged: the crates, one repository per record,
contracts hold shapes only, and the tools' limits. The placement tables
under [Where each current file goes](#where-each-current-file-goes) describe
the tree at `763052c`. They are kept as the migration's plan, with the
amended rows marked.

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
                 +-> scribe-tools-+
```

| Crate | Owns | Internal dependencies | Banned |
|---|---|---|---|
| `scribe` (binary) | Argument declarations, the mapping from arguments to an app command, rendering, exit codes. The only stdout, stderr and terminal access. | `scribe-core` | `rusqlite`, `scribe-store`, `scribe-tools` |
| `scribe-core` | The apps, the runtime they share, and composition. | `scribe-store`, `scribe-tools` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite` |
| `scribe-store` | The data root and `scribe.db`, one repository per record, and the record shapes (`contracts`). The only crate that touches the data root. | none | `clap`, `tracing-subscriber`, any terminal crate |
| `scribe-tools` | The functions that hooks and activities need, each subject whole in one place: finding, reading and parsing transcripts, window building, marker and payload parsing, project lookup. It has its own `fs` module and uses `scribe-store` as needed. | `scribe-store` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile`, `std::net`, `std::process`, `std::env` |

`scribe-domain` is deleted. Its contents went to three places by kind
([Where each current file goes](#where-each-current-file-goes)).

The binary names only `scribe-core`. The core re-exports the record shapes
it returns at its root (`scribe_core::Entry`, `scribe_core::SessionId`, …),
and the moved modules at the paths they had in it (`scribe_core::handoff`,
`scribe_core::capture`, …). Rendering needs no second dependency.

## `scribe` (binary)

```
main.rs              capture the environment, parse, dispatch, map the result to an exit code
cli.rs               clap declarations
commands/<noun>.rs   arguments -> apps::<app>::Command; outcome -> payload
commands/entry/      entry noun dispatch, per-verb arguments and shared rendering
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
- Composition, the model adapters, the host ports and the audit guard are
  in the core (see [Composition](#composition), [Providers](#providers-interim)
  and [Runtime](#runtime)).
- Hook payload parsing is `scribe-tools/src/payload.rs`. Project resolution
  is `scribe-tools/src/project.rs`: it reads git metadata from disk, so it
  is a tool and not part of the `hook` app.
- The binary declares no audit. Each app declares its own (see
  [Apps](#apps)).
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

A `plugin` routine or workflow tool (`session-pass-prepare`, `session-pass-apply`,
`session-pass-fail`, `consistency-scan`,
`memory-refresh-prepare`, `memory-refresh-apply`, `group-close-prepare`,
`group-close-apply`) has no flow of its own. The binary's `plugin` handler
resolves the envelope's root (`Context::at`) and calls the app's `exec` (or
command handler that owns the work). `admin` holds no plugin command.

## `scribe-core`

```
lib.rs               pub mod apps, composition, providers; the store's shapes and the moved modules re-exported
apps/
  <app>/
    mod.rs           Command (with audit and subject), Outcome, Error, exec, run: dispatch only
    <flow>.rs        one file per command or hook event
    rules.rs         this app's pure rules (rules/ when it outgrows one file)
    verbs.rs         the app's private store verbs
runtime/
  mod.rs             Ctx, the port traits, RunLimits
  host/              the ports implemented on Context: the clock, transcripts, the redactor
  resolve.rs         id and session resolution
  errors.rs          CommandError, read and write error mapping
  audit.rs           the audit guard around a mutating command
composition.rs       Env, Context: roots, config, store, redactor
config.rs            config.toml shapes and parsing
config/voice.rs      the prompt half of voice
providers/           interim: the Models port, its shapes (models.rs) and adapters (cli.rs, cli/)
```

### Apps

- An app is one subject a user names on the command line. Its public surface
  is `Command`, `Outcome`, `Error` and

  ```rust
  pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError>
  ```

  `Context` is the composed host (see [Composition](#composition)). The
  binary builds one per process and passes it to every `exec`.
  `CommandError` is the one error every app's `exec` returns. `exec` runs the
  command inside the audit guard when it mutates, and returns. It contains
  no flow.
- Each app declares its audit beside its `Command`:
  `Command::audit(&self) -> Option<AuditMeta>` gives the command path and
  whether it commits stepwise, or `None` for a read.
  `Command::subject(&self, &Context) -> AuditSubject` gives the record and
  author, resolved before the command runs. Both are exhaustive. `exec`
  builds the `Audit` from them; the binary declares none. The hook app
  declares on its `Event`.
- Under it, each app has `pub(crate) fn run(ctx: &Ctx<'_, E>, command: Command)`,
  which is `exec` without the audit guard. Other apps call `run`. Tests
  reach a flow through `exec`, or through the entry points the crate's
  `test-support` feature exposes (`exec_unaudited`, `hook::group_add`, the
  `*Verbs` traits), as the store's `test-support` does for its helpers.
- A flow has three steps, in this order:
  1. **load** what it needs from one `store.read()` snapshot into a plain
     input value;
  2. **plan** with the app's pure `rules` and with `scribe-tools`;
  3. **commit** in one `store.write(|tx| …)`, re-running inside the closure
     any check that must be atomic with the write.

  A flow never interleaves reads and writes outside one write closure, and
  never holds a snapshot across a write.
- The `*Verbs` extension traits on `Store` (`EntryVerbs`, `GroupVerbs`,
  `SessionVerbs`, …) are private to their app (`apps/<app>/verbs.rs`). They
  are exported only under `test-support`, for tests. No production code
  outside `apps/<app>/` calls them.
- **An app may call another app only through that app's `run` or a function
  its `mod.rs` re-exports.** It never imports another app's `rules` or a flow
  file. App imports form a directed acyclic order, held in the guard script.
  The order the imports confirmed, lowest first, as `ARCHITECTURE.md` and
  the guard record it:

  ```
  tag < mark < memory < entry < proposal < session < group < search < brief < consistency < hook < admin
  ```
- There is no `apps/common`, `apps/shared` or `apps/util`. Code two apps need
  goes to the lower of the two in the order above, to `runtime` if it is about
  the context (ids, errors, audit), or to `scribe-tools` if it is about
  transcripts, windows, markers or payloads.

### Runtime

`runtime` is what every app runs against. It is `pub(crate)`: the binary never
sees it.

- Composition's `Context` owns the store, the config, the redactor and the
  host roots. An app's `exec` lends a flow the `Ctx` built from it.
- `Ctx` is the borrowed view a flow receives: `store`, `config`, `clock`,
  `transcripts`, `redaction`. The port traits (`Clock`, `Transcripts`,
  `Redaction`, `AuditNotes`) are in `runtime/mod.rs`, so test fakes
  implement them.
- `runtime/host/` implements the ports on `Context`: the clock, the provider
  transcripts on this host and the redactor. Transcript discovery and reads
  are `scribe-tools/src/transcript/`, beside the parsing they feed.
- `runtime` holds four subjects and no others: the context, id resolution,
  error mapping and the audit guard. A function belongs here only if it is
  about one of them. "Two apps use it" is not a reason.

### Composition

`composition.rs` is the one place that resolves anything about the host.

- `Env::from_process()` captures the environment variables scribe reads
  (`SCRIBE_ROOT`, `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `CLAUDE_CODE_ENTRYPOINT`, the redaction variables). `main.rs` calls it once
  and passes the value down. Tests build an `Env` by hand.
- Composition picks the data directory, reads `config.toml`, opens the
  `Store`, and builds the redactor, into one `Context` (`Context::resolve`,
  or `Context::at` for a root the caller names, as the plugin's envelope
  does).
- Outside `composition.rs`, nothing in the core reads an environment variable
  or discovers a path. Outside `composition.rs` and `providers/`, nothing in
  the core touches a file: it reaches the data root through the store and
  every other file through `scribe-tools`.

### Providers (interim)

Scribe's own model calls are being removed (ORB-27911). Session pass,
session catalog, session rebuild, group close and memory refresh use
deterministic prepare/apply flows and Orbit jobs, as approved in §19 of the
design; the provider adapters remain until their deletion (ORB-28144).

Until then:

- `core/providers/` holds everything about calling a model: the `Models`
  port (`providers/mod.rs`), the remaining adapter inputs
  (`providers/models.rs`), and the CLI adapters with their process
  supervision (`providers/cli.rs`, `providers/cli/`).
- Only `providers/` spawns a process. No session command reaches the
  provider adapter any more. The guard script lists the allowed imports by
  path.
- When a caller's job exists, its import goes. When the last one goes, the
  folder is deleted with the `[models]` config section. The credential names
  in `[models] credentials` also select what redaction masks: they move to
  `[redact] env` in the same change.

## `scribe-store`

```
lib.rs               Store::open, read() -> Snapshot, write(|tx| …); the public contract modules
db/                  the connection, schema and migrations, row access per table;
                     files (the store's use of cw-fs), locks, the codec, the archive import, the log check
repository/          one per record: entries, sessions, messages, proposals, tags,
                     groups, memory, marks, pickups, audit
contracts/           record shapes
error.rs             the store's Error and refusal codes
digest.rs            the content hashes records carry
redact.rs            the Redactor applied at the repository boundary
fixtures.rs          shared test records, behind test-support
seed.rs, testing.rs  test support, behind test-support
```

Invariants:

- **One repository per record, never per app.** Apps and records do not line
  up: the session app writes sessions, messages, proposals, entries and
  memory. An app composes repositories inside one snapshot or one write.
- **A snapshot only reads.** `read()` hands out `ReadOnly` repositories,
  on which no record write is callable; only a `write(|tx| …)`'s
  `ReadWrite` repositories write, inside that transaction. The audit
  guard's own line outside a write goes through `Store::record_audit`.
- **The store holds no rule and no use case.** No function named after a
  command (`settle_pass`, `close_group`), no validation beyond what a
  contract type enforces on itself, and no type named after a use case.
  `CloseGroupOutcome`, `PassWrite` and `PassWritten` are in the apps whose
  flows produce them. Four refusals stay in the store, each one a check
  that every writer, including the store's own, must make at the point of
  the write:
  - `forgotten_refusal` (`error.rs`): a forgotten session is a tombstone,
    and every writer that touches a session refuses it under the write
    lock.
  - `refuse_file_store` (`db/documents.rs`): a file store whose records
    never moved into `scribe.db` is refused as a format.
  - `check_log` (`db/log.rs`): the codec runs it on every session record it
    reads, and the session writer before a write. A malformed log is
    `store_corrupt`.
  - `export_target_exists` (`repository/sessions.rs::prepare_target`):
    `export` refuses a target that is not an empty directory. The check is
    the precondition of the store's own filesystem write, and only the
    store touches the filesystem.
- **`contracts/` holds shapes only.** Allowed: type definitions, a
  constructor or parser that enforces the type's own invariant (`EntryId`
  accepts `e-<n>`, `TagName` its character set), serde and `Display`
  implementations, and conversions between two contract types. Not allowed: a
  function that takes two records and decides something, a function named
  `check_*`, a limit or constant that belongs to a flow (window sizes, retry
  counts), or anything that reads the clock.
- `contracts` imports nothing from `db/` or `repository/`. Only `db/` uses
  `rusqlite`. `db/files` imports nothing else from the store; the file layer
  itself is the kit's `cw-fs` (ORB-28612).
- `Store` stays one concrete type. A trait is added when a second backend
  exists.
- Project scaffolding is not persistence. It is the `admin` app's
  (`apps/admin/init.rs`).

## `scribe-tools`

The functions hooks and activities are built from. One subject is whole in one
place: everything about a transcript (finding the file, reading it, parsing
it, cutting a window from it) is here, not split between a reader in the core
and a parser elsewhere. Daniel, 2026-11-24: "allow tools to have its own fs
module or import from scribe-store as needed. instead of creating logics
spread across multiple places".

```
fs.rs                            the crate's only file access
host.rs                          host-file reads the transcript and project tools share
transcript/                      discovery, reads, claude.rs and codex.rs parsing
window.rs                        the turns a pass or a capture looks at
markers.rs, markers/control.rs   `#scribe` markers, settling lines, `/scribe` commands
origin.rs                        whether a prompt was typed by the user
payload.rs                       hook payloads -> typed events
project.rs                       the project a working directory belongs to
text/                            tokens, normalization, verbatim comparison, claims, triggers
```

Invariants:

- File access goes through `fs.rs`; no other module in the crate names
  `std::fs`. It reads no environment variable and discovers no root: the
  caller passes resolved paths, which composition produced.
- It may use `scribe-store`: the contracts, and a repository where the
  function's job needs one (a transcript cursor, for example). A tool that
  writes takes the caller's `Tx`; it never calls `store.write` itself, so a
  flow still commits in one transaction.
- No clock and no randomness: a function that needs the time takes it as an
  argument.
- A tool is a function an app calls. Which command runs, in what order, and
  what is refused stay in the app: "is this line a marker" and "read this
  transcript from the cursor" are tools; "may this entry be approved" is an
  app rule.
- It never names `scribe-core`. It sits below the core because the Stop and
  capture hooks read transcripts, so that code must be reachable from an
  app. Whatever an Orbit activity calls is an ordinary command mapped to an
  app, and that app uses these functions.
- `session_distill` holds shared window and header requests and answers,
  budgeting, citation validation and repair. The session app builds them
  from the store, applies capture rules and commits the checked windows.

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
| `record`, `provider` | all | — |
| `origin` | — | amended: `tools/origin.rs` (it classifies a prompt's text) |
| `entry`, `entry/records`, `action`, `handoff` | `Entry`, ids, statuses, patches, filters, `Mode` | `apps/entry/rules` |
| `session`, `activity`, `control` | `SessionRecord`, ids, `ControlState`, activity shapes | `apps/session/rules`; `control::parse` to `tools/markers`; amended: `check_log` to `store/db/log.rs` |
| `proposal`, `pass/candidates` | `Proposal`, ids, states, `EntryCandidate` | `apps/proposal/rules` |
| `tag` | `TagName` and tag shapes | `apps/tag/rules` |
| `group` | group and seat shapes | `apps/group/rules` |
| `memory/mod`, `memory/profile` | memory items, profile shapes | `apps/memory/rules` |
| `message` | `Message` rows | `apps/session/rules` |
| `transcript`, `transcript/codex` | the parsed message shape | `tools/transcript/` |
| `text`, `verbatim`, `claim`, `trigger` | — | `tools/text` (the hashes to the store's `digest`) |
| `capture` | — | marker scanning to `tools/markers`; settling rules to `apps/hook/rules` |
| `annotation` | mark shapes | `apps/mark/rules` |
| `pass`, `pass/repair`, `instruction` | — | `apps/session/rules/` (window building to `tools/window`) |
| `voice` | — | amended: the prompt half to `core/config/voice.rs`, the entry half to `apps/entry/rules/voice.rs` |
| `brief` | — | `apps/brief/rules` |
| `consistency` | — | `apps/consistency/rules` |
| `search` | — | `apps/search/rules`; the tokenizer to `tools/text` |
| `checks/{entry,records}` | — | `apps/entry/rules` |
| `checks/{group,memory,tag,admin}` | — | the app of the same name |
| `checks/candidates` | — | `apps/proposal/rules` |
| `models` | — | `core/providers/` |
| `config` | — | `core/config.rs` |
| `error` | — | amended: `store/error.rs` (the store's `Error`, which every store call returns); the apps' `CommandError` is `core/runtime/errors.rs` |
| `redact` | — | `store/redact.rs` |
| `fixtures` | — | `store/fixtures.rs`, behind `test-support` |

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
| `commands/ports.rs` | the clock to `scribe-core/src/runtime/host.rs`; transcript discovery and reads to `scribe-tools/src/transcript/` |
| `audit_middleware.rs` | `scribe-core/src/runtime/audit.rs` |
| `commands/hook/payload.rs` | `scribe-tools/src/payload.rs` |
| `commands/hook/project.rs`, `group_add.rs`, `group_turns*`, `codex.rs`, `stop.rs`, `session_*.rs`, `user_prompt_submit.rs` | whatever is not argument mapping goes to `apps/hook/` |
| `commands/session/{pass,catalog,rebuild,activity}.rs` | whatever is not argument mapping goes to `apps/session/` |

## Guards

`scripts/check-dependency-direction.sh` enforces what a crate edge cannot.
The ring map for `scribe-domain` was removed with the crate. The script
holds:

- the banned crates per crate, from the table above (normal dependencies
  only, as today);
- the app order: an app imports only apps below it, and only through
  `apps::<app>::` paths that `mod.rs` exports; a relative `super::` path
  counts as the `crate::` path it names;
- the core's root modules are `apps`, `composition`, `config`, `providers`
  and `runtime`: `lib.rs` declares a file module only for one of them;
- no `apps/common`, `apps/shared` or `apps/util`;
- in the core, only `composition.rs` names `std::env`, only `composition.rs`
  and `providers/` name `std::fs`, and only `providers/` names
  `std::process`;
- only the five listed flows import `providers`, with their apps' `mod.rs`
  (whose `Command` carries the adapter) and `runtime/errors.rs` (whose
  `CommandError` carries the model failure); `providers/` imports no app
  and no runtime;
- the binary names no `scribe_store` and no `scribe_tools` path;
- in `scribe-tools`, only `fs.rs` names `std::fs`, nothing names
  `scribe_core`, and nothing calls `Store::write`;
- in `scribe-tools`, nothing reads the clock or draws randomness;
- in the store: `contracts/` imports no `db` or `repository`, reads no
  clock and has no `fn check_`; only `db/` uses `rusqlite`; `db/files.rs`
  names nothing of the store but its error; the store reads no environment
  variable and discovers no path;
- the deleted paths (`scribe-domain`, `cases/`, the binary's moved files)
  stay deleted.

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
5. **Dissolve `scribe-domain`.** Shapes to `contracts/`; transcript, window,
   marker and payload code to the new `scribe-tools` crate; rules to their
   apps; `config` and `error` to the core. One record family per pull request (entry, session, proposal, …) so
   each is reviewable; the crate is deleted in the last one.
6. **Docs and guards.** `ARCHITECTURE.md` rewritten from this spec, the
   script updated, `AGENTS.md` pointers and the paths in the other specs
   corrected.

Steps 2 and 3 can run before ORB-27911 finishes; they touch the same files as
its step 2 (the annotation and consistency model judgements), so the captain
sequences them. Step 5 is easier after ORB-27911, since less code moves.

## Amendments

Recorded on 2026-11-25 at the end of ORB-27921. Each is a departure the
captain accepted on the pull request named, and none touches the four
invariants.

- **The app contract is `exec(&Context, Command) -> Result<Outcome,
  CommandError>`**, not `exec(&Env, …) -> Result<…, Error>` (#434, #435).
  The binary holds one composed `Context` per process, and every app
  returns the core's `CommandError`. There is no `Runtime`: the `Context`
  owns what `Runtime::start` was to own.
- **Each app declares its audit beside its `Command`** (`Command::audit`,
  `Command::subject`), and `exec` takes no `Audit` (#470, on Daniel's
  direction). The plugin's routine `memory-refresh-apply` keeps its empty
  subject through `Command::Apply { routine: true, .. }`.
- **`run` stays `pub(crate)`.** Tests use `exec`, `exec_unaudited` and the
  `test-support` entry points.
- **The app order is a total order** (`tag < mark < memory < entry <
  proposal < session < group < search < brief < consistency < hook <
  admin`), as the imports set it (#425–#431).
- **The store's error is `store/error.rs`**, not `core/runtime/errors.rs`
  (#449). Every store call returns it, and the store sits below the core.
- **`voice` is split** (#454): the prompt half is `core/config/voice.rs`,
  the entry half `apps/entry/rules/voice.rs`.
- **Four refusals stay in the store** (`forgotten_refusal`,
  `refuse_file_store`, `check_log`, `export_target_exists`; #440, #444, #468,
  #470), as listed under [`scribe-store`](#scribe-store).
- **`origin` and project lookup are tools** (`tools/origin.rs`, #451;
  `tools/project.rs`, #431, #433). The core re-exports `origin`.
- **The core re-exports the store's shapes at its root and in modules at
  their old paths**, not as `scribe_core::contracts` (#439–#469). Test
  bodies moved unchanged against those paths.
- **The plugin's routine tools are dispatched by the binary's handler**, not
  by `admin` (#430, #434). It resolves the envelope's root with
  `Context::at` and calls the owning command's handler.
- **The `*Verbs` traits are exported under `test-support`** for tests, not
  removed (#425–#431).
- **`scribe-tools` bans `tempfile`** as well, and it, like the contracts,
  reads no clock.

## Not decided here

- The remaining model callers and their Orbit jobs and tools (open-item-19).
- What the consistency scan
  keeps (open-item-19).

A departure from the crates, from "one repository per record", from
"contracts hold shapes only" or from the tools' limits (a tool that
writes takes the caller's `Tx`; tools never name `scribe-core`) goes back to Daniel
through the captain.
