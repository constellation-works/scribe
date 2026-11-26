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
| `scribe` (binary) | Argument declarations, the mapping from arguments to an app command, rendering, exit codes. The only stdout, stderr and terminal access. | `scribe-core` | `rusqlite`, `cw-sqlite`, `scribe-store`, `scribe-tools` |
| `scribe-core` | The apps, the runtime they share, and composition. | `scribe-store`, `scribe-tools` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `cw-sqlite` |
| `scribe-store` | The data root and `scribe.db`, one repository per record, and the record shapes (`contracts`). The only crate that touches the data root. | none | `clap`, `tracing-subscriber`, any terminal crate, direct `rusqlite` |
| `scribe-tools` | The functions that hooks and activities need, each subject whole in one place: finding, reading and parsing transcripts, window building, marker and payload parsing, project lookup. It has its own `fs` module and uses `scribe-store` as needed. | `scribe-store` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `cw-sqlite`, `tempfile`, `std::net`, `std::process`, `std::env` |

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
output/              payloads, rendering; the sink is the kit's cw-cli
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
| `session` | `session` |
| `transcript` | `transcript` |
| `mark` | `mark` |
| `proposal` | `proposal` |
| `memory` | `memory` |
| `tag` | `tag` |
| `search` | `search` |
| `brief` | `brief` |
| `consistency` | `consistency` |
| `hook` | `hook` |
| `init` | `init` |
| `store`, `export` | `store` (both act on the one store resource; see the amendment below) |

Every top-level command except `plugin` maps to exactly one app.

A `plugin` routine or workflow tool (`session-pass-prepare`, `session-pass-apply`,
`session-pass-fail`, `consistency-scan`,
`memory-refresh-prepare`, `memory-refresh-apply`, `group-close-prepare`,
`group-close-apply`) has no flow of its own. The binary's `plugin` handler
resolves the envelope's root (`Context::at`) and calls the app's `exec` (or
command handler that owns the work). `plugin` is the Orbit backend's transport,
not a subject, so it has no app and no row in the table; each of its tools calls
the app that owns the operation.

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
  resolve.rs         id and session resolution
  errors.rs          CommandError, read and write error mapping
  audit.rs           the audit guard around a mutating command, InstructionNote and AuditHost
composition.rs       Env, Context: roots, config, store, redactor
composition/host/    the runtime port traits implemented on Context: the clock, transcripts, the redactor
config.rs            config.toml shapes and parsing
config/tags.rs       tag and capture config, including the shipped-tag merge
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
  tag < mark < memory < entry < proposal < session < transcript < group < search < brief < consistency < hook < init < store
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
- `composition/host/` implements `runtime`'s ports on `Context`: the clock,
  the provider transcripts on this host and the redactor, so `runtime` never
  imports `composition` (STD-02 R42). Transcript discovery and reads
  are `scribe-tools/src/transcript/`, beside the parsing they feed.
- `runtime` holds four subjects and no others: the context, id resolution,
  error mapping and the audit guard. A function belongs here only if it is
  about one of them. "Two apps use it" is not a reason.

### Composition

`composition.rs` is the one place that resolves anything about the host.

- `Env::from_process()` captures the environment variables scribe reads
  (`SCRIBE_ROOT`, `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
  `CLAUDE_CODE_ENTRYPOINT`, `SCRIBE_LOG`, the redaction variables), and the
  process snapshot (`cw_app::Env`) that the binary's terminal decision reads
  (color, width, format). `main.rs` calls it once and passes the values down;
  the core never consults the snapshot. Tests build an `Env` by hand.
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
db/                  the connection, schema and migrations (through cw-sqlite), row access per table;
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
- **The app owns product rules and use cases.** `CloseGroupOutcome`,
  `PassWrite` and `PassWritten` are in the apps whose flows produce them.
  Each app runs its atomic checks inside the store transaction. Store
  APIs are named for the records they persist. The store owns format and
  persistence checks such as these:
  - `forgotten_refusal` (`error.rs`): a forgotten session is a tombstone,
    and every writer that touches a session refuses it under the write
    lock.
  - `refuse_file_store` (`db/documents.rs`): a file store whose records
    never moved into `scribe.db` is refused as a format.
  - `check_log` (`db/log.rs`): the codec runs it on every session record it
    reads, and the session writer before a write. A malformed log is
    `store_corrupt`.
  - `validate_entry` (`db/codec/entry.rs`): entry rows must agree with their
    ids and encode consistent provenance on read and write. The proposal app
    checks approval before materializing the entry in its transaction.
  - `Writer::scrub_journal` (`db/memory.rs`): remove old copies of file
    operations superseded by a replacement or identified by the caller's
    erasure predicate.
  - `export_target_exists` (`repository/sessions/export.rs::prepare_target`):
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
  `cw-sqlite` and its re-exported `rusqlite` (ORB-28614). `db/files` imports
  nothing else from the store; the file layer itself is the kit's `cw-fs`
  (ORB-28612).
- `Store` stays one concrete type. A trait is added when a second backend
  exists.
- Project scaffolding is not persistence. It is the `init` app's
  (`apps/init/install.rs`).

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

`clippy.toml` (`clippy::disallowed_types` and `clippy::disallowed_methods`,
denied workspace-wide in `Cargo.toml`) and
`scripts/check-dependency-direction.sh` enforce what a crate edge cannot
(STD-02 R6). The ring map for `scribe-domain` was removed with the crate.

`clippy.toml` holds the path boundaries (resolved regardless of import
spelling), with module-level `#![expect(..., reason = "...")]` only on the
boundary owners:

- `std::fs` I/O types (`File`, `OpenOptions`, `ReadDir`, `DirEntry`,
  `DirBuilder`) and filesystem functions (`read`, `read_to_string`, `write`,
  `remove_*`, `create_dir*`, `rename`, `copy`, `hard_link`, `soft_link`,
  `metadata`, `symlink_metadata`, `set_permissions`, `read_dir`, `read_link`,
  `canonicalize`, `exists`) — only `scribe-store/src/db/files.rs` beside
  `cw-fs`, `scribe-tools/src/fs.rs` for host files, and
  `scribe-core/src/composition.rs` for `PATH` metadata;
- `std::env` reads and directory discovery (`var`, `var_os`, `vars`,
  `vars_os`, `current_dir`, `home_dir`, `temp_dir`) and the process snapshot
  `cw_app::env::Env::capture` (only `scribe-core/src/composition.rs`; the
  binary has no exemption);
- `std::process::Command::{output,status,spawn}` (only `scribe-core/src/providers/cli.rs`);
- `std::process::exit` (banned with no production exemption so commands exit
  through normal error handling and `ExitCode`);
- `std::sync::mpsc::channel` (banned with no production exemption; the
  provider drain carries its single result on capacity-one
  `std::sync::mpsc::sync_channel`);
- `rusqlite::Connection::open` and its `open_with_flags` forms (banned with no
  production exemption since `scribe-store` opens `scribe.db` through
  `cw-sqlite`).

`scripts/check-dependency-direction.sh` holds the crate, socket and module
rules:

- the banned crates per crate, from the table above (normal dependencies
  only, as today);
- unit import cycles and declared in-layer orders (STD-02 R6, R39): the
  import graph is acyclic between crates, between the top-level modules of
  every crate, and between the sibling modules of every layer and module
  directory; every layer with more than about ten units (`apps/`, `db/`,
  `contracts/`, `repository/`, `scribe-tools`, `commands/`) obeys its
  declared order in `ARCHITECTURE.md`;
- the app order: an app imports only apps below it, and only through
  `apps::<app>::` paths that `mod.rs` exports; a relative `super::` path
  counts as the `crate::` path it names;
- the core's root modules are `apps`, `composition`, `config`, `providers`
  and `runtime`: `lib.rs` declares a file module only for one of them;
- no `apps/common`, `apps/shared` or `apps/util`;
- no `std::net` in `scribe-core` or `scribe-tools`, only `providers/` names
  `rustix` and `tempfile`, and `composition.rs` imports no app;
- only the listed model callers import `providers`, with their apps' `mod.rs`
  (whose `Command` carries the adapter) and `runtime/errors.rs` (whose
  `CommandError` carries the model failure); `providers/` imports no app
  and no runtime;
- the binary names no `scribe_store`, `scribe_tools` or
  `scribe_core::runtime` path;
- in `scribe-tools`, nothing names `scribe_core`, calls `Store::write`,
  reads the clock or draws randomness;
- in the store: `contracts/` imports no `db` or `repository`, reads no
  clock and has no `fn check_`; only `db/` uses `cw-sqlite` and its
  re-exported `rusqlite`, and no scribe source sets `journal_mode`,
  `synchronous`, `busy_timeout`, `user_version`, `secure_delete`,
  `PersistWal` or `wal_checkpoint` itself, so scribe.db is opened,
  configured, checkpointed and migrated only through `cw-sqlite`;
  `db/files.rs` names nothing of the store but its error;
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
- **Storage mechanisms stay in the store**, including the four refusals
  from #440, #444, #468 and #470 and the
  [entry provenance and journal checks](../4_decisions.md#entry-provenance-and-memory-journal-scrubbing-are-storage-mechanisms).
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
- **The `admin` app is split by subject** (ORB-28770; [decision](../4_decisions.md#application-units-are-sliced-by-subject-std-02-r45),
  STD-02@8 §R45). `init` is its own app (`apps/init/`, with the scaffold in
  `install.rs`). `store` holds `store migrate` and `export`, which both act on
  the one store resource. `transcript` is its own app (`apps/transcript/`) and
  reaches `session` and `tag` only through their public exports. The app order
  in the layer rules is the current one; the total order quoted above is
  ORB-27921's, with `admin` last. `plugin` is no app, so it has no row in the
  command table. The migration tables and steps above describe ORB-27921's
  moves and are unchanged.

## Not decided here

- The remaining model callers and their Orbit jobs and tools (open-item-19).
- What the consistency scan
  keeps (open-item-19).

A departure from the crates, from "one repository per record", from
"contracts hold shapes only" or from the tools' limits (a tool that
writes takes the caller's `Tx`; tools never name `scribe-core`) goes back to Daniel
through the captain.
