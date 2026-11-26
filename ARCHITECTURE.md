# Architecture

Scribe is a CLI over one store. Every command and hook parses its input,
calls one `exec` in `scribe-core`, and renders what comes back. Everything
about one subject (`group`, `entry`, `session`) is in one app folder in the
core and one repository per record in the store.

This file is the layout as built. Its source is
[the architecture spec](docs/design/scribe/specs/architecture.md)
([decision](docs/design/scribe/4_decisions.md#scribe-is-laid-out-by-app-in-four-crates-scribe-core-store-and-tools)),
amended at the end of ORB-27921 to the tree that migration produced.
[`scripts/check-dependency-direction.sh`](scripts/check-dependency-direction.sh)
enforces it ([STD-02](docs/standards/STD-02-rust-architecture-and-errors/STD-02.md)
R1–R6, R39). Every crate and top-level module has its place here, and every path
written in code formatting exists
([`scripts/check-architecture-map.sh`](scripts/check-architecture-map.sh),
STD-02 R43). A new crate, edge, app or guard changes this file, the spec and
those scripts in the same commit. Other decisions are in
[4_decisions.md](docs/design/scribe/4_decisions.md).

## Crates

```
scribe  ->  scribe-core  ->  scribe-store
                 |                ^
                 +-> scribe-tools-+
```

| Crate | Owns | Internal dependencies | Banned |
|-------|------|-----------------------|--------|
| `scribe` (binary) | Argument declarations, the grouped root help, the mapping from arguments to an app's `Command`, rendering and exit codes. The only stdout, stderr and terminal access, through the kit's `cw-cli` sink. It reads no environment variable: composition captures the environment and passes it in (`cw-app` is named for the captured `Env` type only). It declares no audit and decides nothing about a record. | `scribe-core` only | `rusqlite`, `cw-sqlite` |
| `scribe-core` | The apps (`apps/<app>/`), the `runtime/` they run against, composition (`composition.rs`, which captures the environment once, its `cw-app` `Env` type included), the config (`config.rs`) and, for now, the model adapters (`providers/`). Only `apps/init/install.rs` names `cw-fs` (the project scaffold's atomic writes and lock). | `scribe-store`, `scribe-tools` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `cw-sqlite` |
| `scribe-store` | The data root and `scribe.db`, one repository per record, and the record shapes (`contracts/`). The only crate that touches the data root. Its file layer is the kit's `cw-fs` (durable writes, owner-only checks, capped reads, the advisory lock) and its SQLite open, configuration, migration and WAL-checkpoint layer is the kit's `cw-sqlite` (using `cw-sqlite`'s re-exported `rusqlite`); the store keeps no copy. | — | `clap`, `tracing-subscriber`, any terminal crate, direct `rusqlite` |
| `scribe-tools` | The functions hooks and activities are built from, each subject whole in one place: transcripts found, read and parsed, the pass window, `#scribe` markers and control lines, hook payloads, project lookup, the origin guard and the text primitives. Its own `fs.rs` is its only file access. | `scribe-store` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `cw-sqlite`, `tempfile` |

The banned crates are normal dependencies only. Dev-dependencies are exempt:
the tests of `scribe-core` and `scribe` stage database states with
`rusqlite`, and use `tempfile` for their roots. The script reads the
`[dependencies]`, `[build-dependencies]` and target-specific tables, never
`[dev-dependencies]`. In the core, `rustix` and `tempfile` are normal
dependencies for `providers/` only.

The binary's normal dependencies are `scribe-core`, the argument, output and
host crates (`clap`, `serde`, `serde_json`, `thiserror`, `time`, `tracing`,
`tracing-subscriber`, `unicode-width`) and, on unix, `rustix`. It is the only
crate that declares `clap` or `tracing-subscriber`.

The binary names only `scribe-core`. The core re-exports the store's record
shapes at its root (`scribe_core::Entry`, `scribe_core::SessionId`, …) and
the moved modules at the paths they had in it (`scribe_core::handoff`,
`scribe_core::capture`, `scribe_core::origin`, …), so rendering needs no
second dependency.

## `scribe` (binary)

```
main.rs              capture the environment once (composition), parse, decide the terminal once, resolve the Context, run, render, map the result to an exit code
cli.rs               clap declarations
help.rs              the grouped root help
commands/<noun>.rs   arguments -> apps::<app>::Command; Outcome -> payload
commands/entry/      entry noun dispatch, per-verb arguments and shared rendering
output/              payloads (including session and memory shapes), rendering (including record detail); the sink itself is the kit's `cw-cli`
error.rs             app errors -> one-line message and exit code
tests/               the binary's own tests (cfg(test))
```

A command handler turns parsed arguments into the app's `Command`, calls
`scribe_core::apps::<app>::exec`, and turns the `Outcome` into an output
payload. Nothing else. No handler opens the store, reads a transcript or an
environment variable, or branches on a record's state.

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
| `store`, `export` | `store` |

`plugin call` serves the Orbit plugin's tools. Its routine and workflow tools
(`session-pass-prepare`, `session-pass-apply`, `session-pass-fail`,
`consistency-scan`, `memory-refresh-prepare`,
`memory-refresh-apply`, `group-close-prepare`, `group-close-apply`) have no
flow of their own. The handler resolves the envelope's root with `Context::at`
and calls the app's `exec` (or command handler that owns the work). `plugin` is
the Orbit backend's transport, not a subject, so it has no app of its own.

The `commands/` layer (17 modules) imports only strictly earlier modules in
this order (STD-02 R39):

```
instruction < store < brief < consistency < entry < export < group < hook < init < mark < memory < proposal < search < session < tag < transcript < plugin
```

## `scribe-core`

```
lib.rs               pub mod apps, composition, providers; the store's shapes and the moved modules re-exported
apps/<app>/
  mod.rs             Command (with audit and subject), Outcome, Error, exec, run: dispatch only
  <flow>.rs          one file per command or hook event
  rules.rs, rules/   the app's pure rules
  verbs.rs           the app's private store verbs (*Verbs)
runtime/
  mod.rs             Ctx, the port traits (Clock, Transcripts, Redaction, AuditNotes), RunLimits
  resolve.rs         id and session resolution
  errors.rs          CommandError, read and write error mapping
  audit.rs           the audit guard around a mutating command, InstructionNote and AuditHost
composition.rs       Env, Context: the captured environment (the variables scribe reads, `SCRIBE_LOG`, and the process snapshot the terminal decision reads), and the resolved roots, store, provider and audit state
composition/host/    the runtime port traits implemented on Context
config.rs            config.toml shapes and parsing
config/tags.rs       tag and capture config, including the shipped-tag merge
config/voice.rs      the prompt half of voice
providers/           interim: the Models port, its shapes (models.rs) and the CLI adapters (cli.rs, cli/)
```

The core's root modules are its places: `apps`, `composition`, `config`,
`providers` and `runtime`. `lib.rs` declares a file module only for one of
them, and the guard fails a new one until it is given a place here and in
[the script](scripts/check-dependency-direction.sh).

### Apps

An app is one subject a user names on the command line. Its public surface is
`Command`, `Outcome`, `Error` and

```rust
pub fn exec(ctx: &Context, command: Command) -> Result<Outcome, CommandError>
```

- **The audit is declared beside the `Command`.** Each app's `mod.rs`
  implements `Command::audit()`, which gives the command path and whether it
  commits stepwise, or `None` for a read (STD-01 R31), and
  `Command::subject(&Context)`, which gives the record and author, resolved
  before the command runs so a refusal is attributable too. Both are
  exhaustive matches. `exec` calls `Audit::declared` on them and runs the
  command inside `runtime::audit::guarded` (`runtime/audit.rs`). The guard
  exists before the command runs and records when it is dropped, so a command
  that returns early or panics is still logged, as a failure. `exec_unaudited`
  (test-support only) runs the same command with no guard. The hook app
  declares on its `Event`, and `HookEvent` names the four events for Claude
  Code and Codex alike. The plugin's routine `memory-refresh-apply` keeps its empty
  subject through `Command::Apply { routine: true, .. }`.
- **The context is `composition::Context`.** The binary builds it once with
  `Context::resolve(root, &Env::from_process())` and `with_provider`. It holds
  the opened `Store`, the Claude and Codex transcript roots, the provider,
  whether the process is interactive, and the audit state of the running
  command: the instruction note it acts on, and whether a stepwise command
  already committed its line. `exec` lends the ports to `run` as a
  `Ctx<'_, CommandError>` (`ctx.core()`, `composition/host/`): the store, the
  clock, the transcripts and the redactor, all implemented on the `Context`.
  Only `composition.rs` reads the environment.
- **`run` is `exec` without the guard,** `pub(crate)`, on the `Ctx` that
  `exec` lends. Other apps call it.
- **A flow loads, plans, commits:** it reads what it needs from one
  `store.read()` snapshot, plans with its app's rules and the tools, and
  commits in one `store.write(|tx| …)`. Any check that must be atomic with
  the write runs again inside the closure.
- **The `*Verbs` traits** (`EntryVerbs`, `GroupVerbs`, `SessionVerbs`, …) are
  private to their app. The crate root exports them under `test-support`
  only.

Apps import each other in one order, lowest first, and only through what the
lower app's `mod.rs` exports, never one of its flow files:

```
tag < mark < memory < entry < proposal < session < transcript < group < search < brief < consistency < hook < init < store
```

`proposal` calls `entry`; `session` calls `entry` and `proposal`;
`transcript` calls `session` and `tag`; `group` calls `session`; `brief` calls
`memory`; `consistency` calls `proposal` and `session`; `hook` calls all of
them; `init` and `store` call no app. Code two apps need goes to the lower
one, to `runtime` if it is about the context (ids, errors, audit), or to
`scribe-tools` if it is about transcripts, windows, markers or payloads.
There is no common, shared or util app. A relative `super::`
import is read as the `crate::` path it names, and a grouped import is read
as the paths it names (`crate::{apps::entry::X}`, `crate::apps::{entry::X}`),
so the guard holds each to the same order.

### Runtime, composition and providers

- `runtime` is `pub(crate)`: the binary never names it. It holds four
  subjects: the context (`Ctx` and its ports), id resolution, error mapping
  (`CommandError`, the one error every `exec` returns) and the audit guard.
- `composition.rs` is the one place that resolves anything about the host.
  `Env::from_process()` captures the variables scribe reads (`SCRIBE_ROOT`,
  `HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `CLAUDE_CODE_ENTRYPOINT`, the
  redaction variables). `Context::resolve` picks the data directory, opens the
  `Store` and resolves the transcript roots and the provider. `config.rs`
  parses the config file; `Context::redactor` and `Context::models` build from a
  parsed `Config`. `composition/host/` implements `runtime`'s ports on the
  `Context` (STD-02 R42), so `runtime` never imports `composition`.
- `providers/` is interim while ORB-27911 removes Scribe's own model calls.
  Session pass, session catalog, session rebuild, group close and memory
  refresh use deterministic prepare/apply/begin/finish flows; Orbit jobs
  perform their model work. The provider adapters remain until their deletion
  (ORB-28144). Group close reads evidence, drafts outside the store, then
  applies the supplied summary atomically. The CLI compares the evidence
  digest only when `--if-evidence` is supplied; the plugin apply tool always
  requires it.

## `scribe-store`

```
lib.rs               Store::open, read() -> Snapshot, write(|tx| …); the public contract modules
db/                  the connection, schema and migrations (through `cw-sqlite`), row access per table (including messages and their FTS index);
                     files.rs (the store's use of the kit's cw-fs: read caps, the root link), locks,
                     the codec, the archive import, the log check (log.rs)
repository/          one per record: entries, sessions, messages, proposals, tags,
                     groups, memory, marks, pickups, audit, and id resolution (ids.rs)
contracts/           record shapes
error.rs             Error, Result, WriteRefusal and the refusal codes
digest.rs            the content hashes records carry
redact.rs            the Redactor every persisted free-text field passes
fixtures.rs          shared test records (test-support)
seed.rs              seed records for tests (test-support)
testing.rs           test support (test-support)
```

- **One repository per record, never per app.** An app composes repositories
  inside one snapshot or one write.
- **A snapshot only reads.** `read()` hands out `ReadOnly` repositories;
  only a `write(|tx| …)`'s `ReadWrite` repositories write, inside that
  transaction. The audit guard's line outside a write goes through
  `Store::record_audit`.
- **`contracts/` holds shapes only:** type definitions, a constructor or
  parser that enforces the type's own invariant, serde and `Display`, and
  own-shape methods. It imports nothing from `db/` or `repository/`, has no
  `check_*` function and reads no clock.
- **In-layer orders (STD-02 R39):** the three multi-module layers in
  `scribe-store` import only strictly earlier units in these orders:
  - `db/`:
    `archive < files < log < scope < codec < locks < memory < messages < groups < sessions < docs < import < write < documents`
  - `contracts/`:
    `audit < provider < record < session < entry < handoff < tag < action < group < mark < memory < message < proposal`
  - `repository/`:
    `ids < memory < audit < entries < groups < marks < messages < pickups < proposals < sessions < tags`
- **The app owns product rules.** The calling app evaluates a rule requiring
  atomicity inside the store's transaction. The store owns format and
  persistence checks such as these:
  - `forgotten_refusal` (defined in `error.rs`): a forgotten session is a
    tombstone, and every writer that touches a session refuses it under the
    write lock. The store's session writers (`db/`) and the core's session
    verbs (`crates/scribe-core/src/apps/session/verbs.rs`) make that check and call the helper.
  - `refuse_file_store` (`db/documents.rs`): a file store whose records never
    moved into `scribe.db` is refused as a format.
  - `check_log` (`db/log.rs`): the codec runs it when it decodes and encodes a
    session record (`encode_session` refuses a write that would not read
    back), and the session row reader runs it on each row; a malformed log is
    `store_corrupt`.
  - `validate_entry` (`db/codec/entry.rs`): entry rows must agree with their
    ids and encode consistent provenance on read and write. The proposal app
    checks approval before materializing the entry in its transaction.
  - `Writer::scrub_journal` (`db/memory.rs`): remove copies of superseded
    file operations and copies matching an erasure predicate supplied by
    the caller. See the
    [entry and journal decision](docs/design/scribe/4_decisions.md#entry-provenance-and-memory-journal-scrubbing-are-storage-mechanisms).
  - `export_target_exists` (`repository/sessions/export.rs::prepare_target`):
    `export` refuses a target that is not an empty directory. The check is
    the precondition of the store's own filesystem write, and only the store
    touches the filesystem.

### Public API

What the core and the tools import from `scribe_store`:

- **Contract modules:** `entry` (with `entry::records`), `handoff`, `record`,
  `provider`, and `session` (the shapes, with `db::log`'s `check_log` and
  `LogError`).
- **Root shapes:** `Provider`; `Pickup`; `ClaudeSeen`, `Group`, `LastGroup`,
  `NewGroup`, `Seat`, `SeatHold`; `MarkRecord`, `MarkState` and their codes;
  the memory, message, proposal and tag shapes.
- **The store:** `Store`, `Snapshot`, `Tx`, the repositories (`repository`),
  `SessionLock`, `ImportFacts`, `Imported`, `ClaudeCapturePosition`,
  `ClaudeGroupCapture`, `DB_FILE`, `digest`, `Redactor`,
  `REDACTED_FIELDS`, `MIN_SECRET_CHARS`.
- **Errors and refusals:** `error`, `Error`, `Result`, `WriteRefusal`,
  `refusal`, `forgotten_refusal`.
- **Audit and ids:** `AUDIT_FILE`, `AuditEvent`, `AuditStatus`, `IssueAudit`;
  `resolve_prefix`, `group_number`; `EntryList` and the session and memory
  read results.

## `scribe-tools`

```
fs.rs                            the crate's only file access
host.rs                          the host's name and the host-file reads the tools share
transcript/                      discovery, provenance-checked reads, provider parsing and project attribution
window.rs                        a pass's window over a transcript, and its limits
markers.rs, markers/control.rs   `#scribe` markers, settling lines, `/scribe` control lines
origin.rs                        whether a prompt was typed by the user
payload.rs                       hook payloads -> typed events
project.rs                       a working directory's project, from git's own files
group_close.rs                   pure group-close request and answer shapes
memory_refresh.rs                pure memory-refresh request, answer, basis and apply shapes
session_distill.rs               shared inputs, output contracts and prompting rules for session distillation
session_distill/                 the distillation checks (check.rs, check/repair.rs)
voice.rs                         the user noun and configurable instruction text for model requests
text/                            tokens, normalization, verbatim comparison, claims, triggers
```

A tool is a function an app calls. Which command runs, in what order, and
what is refused stay in the app. The tools may use the store's contracts and
a repository, but a tool that writes takes the caller's `Tx` and never calls
`Store::write`. They take every root and every time as an argument.

The top-level modules of `scribe-tools` (13 modules) import only strictly
earlier modules in this order (STD-02 R39):

```
fs < host < origin < text < voice < markers < project < payload < transcript < window < group_close < memory_refresh < session_distill
```

## Test support

Each library has a `test-support` feature that only dev-dependencies turn on,
so a shipped build never compiles it:

- `scribe-store`: `fixtures` (`entry`, `session`, `segment`, `T0`),
  `Store::seed_records` (`seed.rs`), and `testing`. `testing` holds
  `StoreTestReads`, the shared helpers, and the store internals the core's
  store tests reach: `StoreTestInternals` and modules named after the store's
  own (`audit`, `codec`, `db`, `memory`, `redact`, `write`).
- `scribe-core`: turns on the store's, re-exports `testing`, and exposes the
  `*Verbs` traits and the unaudited entry points tests drive
  (`apps::session::exec_unaudited`, `apps::hook::exec_unaudited`,
  `apps::hook::group_add`).

The store's tests that call the `*Verbs` are in `scribe-core`
(`src/tests/store/`), because the store cannot see the apps.

The binary's integration tests live in `crates/scribe/tests/`. Each target
loads its own `support/<subject>.rs`, where a caller-tracked `check` drives
the subject. The shared fixture supplies an isolated home and store.
Checks accept subject arguments, a full command via `Cli`, or an explicit
environment or launcher when the case exercises one. A case can keep the
fixture between commands to inspect changes to the same store. Expectations
check the exit code and can return the decoded JSON or UTF-8 stdout.

Large targets use `include!` for groups of related cases. This keeps their
existing libtest names available to exact filters, including the replay
script. The Makefile formats these files alongside Cargo's module tree.

## Guards

Checked across the workspace by `clippy.toml` (`clippy::disallowed_types` and
`clippy::disallowed_methods`, denied workspace-wide in `Cargo.toml`, with
module-level `#![expect(..., reason = "...")]` on the boundary owners),
`scripts/check-dependency-direction.sh`, and (for the terminal)
`scripts/check-terminal-guard.sh` (STD-02 R6): what a crate edge cannot
express. "Module" includes any file added later.

- **Clippy disallowed paths (`clippy.toml`):** bans `std::fs` types and
  methods, `std::env` reads (`var`, `var_os`, `vars`, `vars_os`,
  `current_dir`, `home_dir`), the process snapshot `cw_app::env::Env::capture`,
  `std::process::Command::{output,status,spawn}`,
  `std::sync::mpsc::channel`, and `rusqlite::Connection::open` (including its
  `open_with_flags` forms) regardless of how a path is imported or aliased.
  Production exemptions are confined to the boundary owners:
  `scribe-store/src/db/files.rs` (`std::fs` beside `cw-fs`),
  `scribe-tools/src/fs.rs` (host `std::fs` adapter),
  `scribe-core/src/composition.rs` (environment capture, the `cw-app`
  snapshot and `PATH` metadata) and `scribe-core/src/providers/cli.rs`
  (supervised process spawn and drain channel). The binary has no exemption:
  `main` and the output layer receive captured values.
  `rusqlite::Connection::open*` has no production exemption:
  `scribe-store` opens `scribe.db` only through `cw-sqlite`.
- **Crate edges:** each crate's internal allowlist and banned crates, from the
  table above. A crate without a policy fails.
- **Import cycles and in-layer orders (STD-02 R6, R39):** the import graph
  must be acyclic between workspace crates, between the top-level modules of
  every crate, and between the sibling modules of every layer and module
  directory (resolving `super::`, `self::`, grouped `use` trees and root
  re-exports while ignoring comments and string literals). Every layer with
  more than about ten units (`apps/`, `db/`, `contracts/`, `repository/`,
  `scribe-tools`, `commands/`) obeys its declared order above; an unlisted
  module or upward import fails.
- **App order:** every directory under `apps/` is in the order above; an app
  imports only lower apps, through their `mod.rs`; no common, shared
  or util app.
- **The host and sockets, in the core:** `composition.rs` imports no app;
  only `providers/` names `rustix` and `tempfile`; nothing opens a socket
  (`std::net`).
- **The model callers:** the dependency guard allows only
  `apps/memory/refresh.rs`, `apps/group/close.rs`, their apps' `mod.rs` files,
  and `runtime/errors.rs` to name `providers`, `Models`, `ModelError`,
  `ModelsOnce` or `ModelsEach`. The flows no longer invoke an in-process
  model; the remaining `ModelError` references are in `apps/group/mod.rs` and
  `runtime/errors.rs`. `providers/` imports no app and no runtime, and the
  model shapes import none of their callers.
- **The store:** only `db/` names `cw_sqlite` or `rusqlite` (through
  `cw_sqlite::rusqlite`), and no scribe source sets journal mode,
  `synchronous`, `busy_timeout`, `user_version`, `secure_delete`,
  `PersistWal` or `wal_checkpoint` itself; `db/files.rs` names nothing of the
  store but its error; no module of the store re-implements a file mechanism:
  `cw-fs` is the file layer, and only the store and `apps/init/install.rs`
  name it; `db/codec.rs` and `repository/audit.rs` do no file I/O (neither
  names `db/files.rs`, `cw-fs` or `cw-sqlite`); `contracts/` imports no `db`
  or `repository`, has no `check_*` function and reads no clock.
- **The tools:** nothing names `scribe_core` or `std::net`, calls
  `Store::write`, reads the clock or draws randomness.
- **The binary:** names no `scribe_store`, `scribe_tools` or
  `scribe_core::runtime` path; `output/` imports no `app`, `cli` or
  `commands`; only `output/` touches stdout or stderr and asks whether a stream
  is a terminal, and only `composition.rs` names the color, width, format and
  log variables (captured, then passed down; `scripts/check-terminal-guard.sh`).
- **History:** a persisted timestamp never defaults to the current time
  (R16), and the deleted paths (`scribe-domain`, cases, the binary's moved
  files, the core's retired module names) stay deleted (R17).
