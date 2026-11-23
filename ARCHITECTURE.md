# Architecture

The layer model that [`scripts/check-dependency-direction.sh`](scripts/check-dependency-direction.sh) enforces
([STD-02](docs/standards/STD-02-rust-architecture-and-errors/STD-02.md) R1, R6).
A lower layer never imports a higher one. A new crate or edge changes this file
and that script in the same commit.

## Layers

| Tier | Unit | Owns | May depend on |
|------|------|------|---------------|
| 1. Business logic | `scribe-core`, except `store/` | Records, the rules that select and judge them, the parsing of prompts, config and transcripts, and the `Models` and `Tasks` ports; values only, no I/O. | nothing internal |
| 2. Persistence | `scribe-core::store` | The `Store` facade, the only reader and writer of the data root and `scribe.db`: locks, atomic writes, format upgrades, redaction and the audit record. | tier 1 |
| 3. Composition | `scribe::app`, `::models`, `::tasks`, `::popup` | Resolves roots and configuration, builds the `Store` and the adapters for models, Orbit tasks and the tmux popup. | `scribe-core` |
| 4. Surface | `scribe::cli`, `::commands`, `::output`, `::audit_middleware` | Argument declarations, dispatch, the audit guard around mutating commands, rendering, and the only stdout, stderr and terminal access. | tiers 1–3 |

Each module's doc comment states what it owns. Decisions are in
[4_decisions.md](docs/design/scribe/4_decisions.md).

## Crates

| Crate | Kind | Internal dependencies | Banned |
|-------|------|-----------------------|--------|
| `scribe-core` | library | — | `clap`, `tracing-subscriber` and any terminal crate |
| `scribe` | binary `scribe` | `scribe-core` | — |

Why two crates rather than one: [docs/design/scribe/4_decisions.md](docs/design/scribe/4_decisions.md#a-domain-library-crate-and-a-cli-crate).
Why persistence is a module inside the core rather than a third crate:
[the store-module decision](docs/design/scribe/4_decisions.md#persistence-is-one-module-inside-the-core-crate).

## Module rules inside crates

Checked by grep over non-test sources in `scripts/check-dependency-direction.sh`
and `scripts/check-terminal-guard.sh`. "Module" includes any file added later.

- `scribe-core` modules other than `store/` do no file, network or process I/O (`std::fs`, `std::net`, `std::process`, `tempfile`, `rustix`) and never import `crate::store`.
- The `claim` module owns lexical claim and provenance predicates below the `pass` orchestrator; config owns pass window limits. Their callers do not import `pass`.
- A port imports none of the modules that call it; the `Models` port does not import `ask`, `memory` or `group`.
- `scribe-core` reads no environment variables and discovers no paths (`std::env::`, `home_dir`, `current_dir`); the caller passes resolved roots.
- `store/fsio.rs` imports no business module and nothing else from `store`. `store/format.rs`, `store/redact.rs` and `store/audit.rs` do no file I/O.
- Only `store/db.rs` and `store/db/` use `rusqlite`; nothing else in either crate does. Tests are exempt.
- `app.rs`, `models.rs` with `models/`, `tasks.rs` and `popup.rs` import no `cli`, `commands`, `output` or `audit_middleware`.
- Only `models.rs` with `models/` (the model transports), `tasks.rs` and `popup.rs` spawn processes; `tasks.rs` only through `models::supervise`.
- `output/` imports no `app`, `cli`, `commands` or `audit_middleware`; `audit_middleware.rs` imports no `cli` or `commands`.
- `commands/` writes no file (`std::fs::write`, `create`, `rename`, `remove`, `copy`, `set_permissions`, `OpenOptions`); every write goes through the store.
- Only `output/` touches stdout or stderr, asks whether a stream is a terminal, or reads the color and format variables.
