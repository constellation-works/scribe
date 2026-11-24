# Architecture

The layer model that [`scripts/check-dependency-direction.sh`](scripts/check-dependency-direction.sh) enforces
([STD-02](docs/standards/STD-02-rust-architecture-and-errors/STD-02.md) R1, R6).
A lower layer never imports a higher one. A new crate, edge or domain module
changes this file and that script in the same commit.

Why four crates: [the four-crate decision](docs/design/scribe/4_decisions.md#four-crates-domain-store-use-cases-and-cli)
(ORB-27707). Other decisions are in [4_decisions.md](docs/design/scribe/4_decisions.md).

## Crates

```
scribe-domain  <-  scribe-store  <-  scribe-core  <-  scribe
```

| Crate | Owns | Internal dependencies | Banned |
|-------|------|-----------------------|--------|
| `scribe-domain` | Records and the pure rules over them: entries, sessions, proposals, tags, transcripts, the parsing of prompts and config, the checks a write runs, redaction, and the request and answer shapes of the `Models` port. Values only, no I/O. | — | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile`, `rustix` |
| `scribe-store` | The data root and `scribe.db`: the `Store` (`open`, `read() -> Snapshot`, `write(\|tx\| …)` and the repositories), locks, atomic writes, format stamps, the import of the file store's archives and the audit record. The only crate that touches the data root. | `scribe-domain` | `clap`, `tracing-subscriber` and any terminal crate |
| `scribe-core` | The use cases (`cases/`): each loads what it needs from a snapshot, plans with domain functions, calls the ports and commits in one store write. Its root re-exports `scribe-domain` and `scribe-store` at the paths they had in it. | `scribe-domain`, `scribe-store` | `clap`, `tracing-subscriber`, any terminal crate, `rusqlite`, `tempfile`, `rustix` |
| `scribe` (binary) | Arguments, dispatch, the audit guard, rendering, and the model adapter. | `scribe-core` | `rusqlite` |

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

## Tiers inside `scribe`

| Tier | Unit | Owns | May depend on |
|------|------|------|---------------|
| Composition | `scribe::app`, `::models` | Resolves roots and configuration, builds the `Store` and the model adapter. | `scribe-core` |
| Surface | `scribe::cli`, `::commands`, `::output`, `::audit_middleware` | Argument declarations, dispatch, the audit guard around mutating commands, rendering, and the only stdout, stderr and terminal access. | composition, `scribe-core` |

## Rings inside `scribe-domain`

The domain's modules are flat files, each in one ring. A module imports only
modules of its own ring or an earlier one, by path, in a grouped import, or
through a name the crate root re-exports. The script holds the module→ring
map; a new module fails the check until it is given a ring.

| Ring | Modules |
|------|---------|
| 1. base | `record`, `text`, `verbatim`, `provider`, `origin` |
| 2. records | `session`, `activity`, `control`, `entry`, `handoff`, `tag`, `proposal` |
| 3. transcript | `transcript` (with `transcript/codex`) |
| 4. rules | `trigger`, `voice`, `claim`, `brief`, `capture`, `consistency`, `pass`, `instruction`, `models`, `config`, `error`, `checks`, `search`, `group`, `memory`, `message`, `action`, `annotation`, `redact` |

These are the rings the code has, and they differ from the approved lists in
ORB-27707 where an import forces it:

- **`group`, `memory`, `message`, `action`, `annotation` are in rules, not
  records.** Each imports a rule or a model shape:
  - `group` imports `instruction` (`claude_session_conversation`) and
    `models::CloseEvidence`;
  - `memory` imports `claim::own_span` and `models::{MemoryAnswer, PassMessage}`;
  - `message` imports `claim::PassClaim`, `models::PassMessage` and
    `memory::{finance_word, sensitive_word}`;
  - `action` imports `voice::is_pass_captured`;
  - `annotation` imports `capture::{scan, marks, …}` and the annotation shapes
    in `models`.
- **`transcript` comes before rules, not after.** The rules read
  transcripts: `capture` imports `transcript::Message`, and `instruction`,
  `pass`, `memory`, `message` and `action` import from it too. In turn,
  `transcript` imports only the records ring (`control::parse`, `entry`,
  `session`) and base (`origin`, `text`, `record`).
- **`config` is in rules, not a ring of its own after transcript.** It imports
  `trigger::MAX_TRIGGERS` and `voice::{UserName, Instructions}`, and `pass`,
  `error` and `redact` import it back, so it cannot sit after rules.
- **Modules the lists did not name:**
  - `origin` is in base and imports nothing;
  - `control` is in records, since it imports `activity::CatalogedBy` and
    `session`;
  - `error` (imports `config::ConfigError`), `checks` (imports `pass`,
    `instruction`, `group`), `search` (imports `voice`), `models` (imports
    `voice`) and `redact` (imports `config`, `group::NewGroup`) are in rules.
- **Naming:** `verbatim` is its own module in base, beside `text`. Ids live
  with their records, so there is no `ids` module.

`models` stays in `scribe-domain` with the `Models` port until ORB-27911
step 2 removes scribe's in-process model calls. The other open targets are
listed in the four-crate decision.

## Module rules

Checked by grep over non-test sources in `scripts/check-dependency-direction.sh`
and `scripts/check-terminal-guard.sh`: what a crate edge cannot express. "Module"
includes any file added later.

- `scribe-domain` and `scribe-core` do no file, network or process I/O (`std::fs`, `std::net`, `std::process`): `scribe-core` reaches files through `scribe-store`.
- `scribe-domain`, `scribe-store` and `scribe-core` read no environment variables and discover no paths (`std::env::`, `home_dir`, `current_dir`); the caller passes resolved roots.
- The `claim` module owns lexical claim and provenance predicates below the `pass` orchestrator; config owns pass window limits. Their callers (`config`, `annotation`, `consistency`, `instruction`, `memory`, `message`) do not import `pass`.
- A port imports none of the modules that call it; the `Models` port does not import `memory` or `group`.
- In `scribe-store`, `fsio.rs` imports no business module and nothing else from the store. `codec.rs` and `audit.rs` do no file I/O. `archive.rs` reads the file store's archives through `fsio.rs` and reaches their tables through `db/archive.rs`.
- In `scribe-store`, only `db.rs` and `db/` use `rusqlite` (and `testing.rs`, test support).
- `app.rs` and `models.rs` with `models/` import no `cli`, `commands`, `output` or `audit_middleware`.
- Only `models.rs` with `models/` (the model transports) spawns processes.
- `output/` imports no `app`, `cli`, `commands` or `audit_middleware`; `audit_middleware.rs` imports no `cli` or `commands`.
- `commands/` writes no file (`std::fs::write`, `create`, `rename`, `remove`, `copy`, `set_permissions`, `OpenOptions`); every write goes through the store.
- Only `output/` touches stdout or stderr, asks whether a stream is a terminal, or reads the color and format variables.

The crate edges replaced these grep rules:

- business modules name no store and no use case;
- the store names no use case;
- `tempfile` and `rustix` stay out of the business modules;
- `rusqlite` stays out of the domain, the use cases and the CLI.
