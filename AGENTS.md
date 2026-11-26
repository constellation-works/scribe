# scribe — agent guide

scribe is a standalone Rust CLI and Orbit plugin that records explicitly marked preferences, catalogs opted-in sessions, lets agents search and read those records through the CLI, and provides the routines that keep them current. Product behavior is in [the design](docs/design/scribe/1_overview.md); code boundaries are in [ARCHITECTURE.md](ARCHITECTURE.md).

## Rules

- Work only within authorized scope. Implementation or a PR request does not authorize merging.
- Update the docs a change makes stale in the same PR as the change; stale docs are a review blocker.
- Edit this file only when a rule, command, or pointer changes; put feature descriptions in the owning design docs.
- Do not edit `CHANGELOG.md` during task work; release builds compile it.

## Branching

- `agent-main` is the integration branch; commit directly (gate `direct`). `main` is for releases only.

## Gates

- `make ci-fast`, `make ci-lint`, and `make goldens` must pass before review.
- After changing help or output, run `make goldens UPDATE=1` and review the diff as a contract change.
- `make ci` runs the full test and doctest suite in CI on Linux and macOS.

## Code

- Follow [ARCHITECTURE.md](ARCHITECTURE.md) for layer boundaries and ownership.
- Store writes go through `Store::write`, or `Store::transaction` for a whole command, so the write and its audit row commit in one `scribe.db` transaction ([db/write.rs](crates/scribe-store/src/db/write.rs)).
- Never hold a `Store::transaction` across a model call ([db/write.rs](crates/scribe-store/src/db/write.rs)).
- Reads never write or lock.
- Each app's `Command::audit` in `scribe-core` (`crates/scribe-core/src/apps/<app>/mod.rs`) decides whether its commands are audited.
- Read commands return `None` from `Command::audit`.
- The binary declares no audit.
- Audit lines carry ids and error codes, never argument values, content or model text ([decision](docs/design/scribe/4_decisions.md#mutating-commands-are-audited-through-one-guard)).
- `--json` field names, the list envelope and the store's persisted shape are contracts: add fields, never rename or retype them (STD-01 R10).
- A change to an exported document's shape bumps `FORMAT` in [db/codec.rs](crates/scribe-store/src/db/codec.rs).
- A persisted-shape change appends a database migration in [db/mod.rs](crates/scribe-store/src/db/mod.rs).
- Do not bump `FORMAT` unless a change also alters an exported document's shape.
- Product behavior and decisions belong in [scribe design docs](docs/design/scribe/); new feature docs start from [the design template](docs/design/_templates/).

## Tests

- Use temporary roots in store tests.
- Set `HOME` and `SCRIBE_*` on child commands; repository fixtures are in [tests/support](crates/scribe/tests/support/mod.rs).

<!-- constellation-standards:begin -->
<!-- Managed by the constellation's operations/scripts/sync-standards.sh; edits inside this block are overwritten. -->
## Constellation standards

This repository adopts these constellation standards, vendored read-only in `docs/standards/` (each a directory: `STD-nn.md` holds the rules, `why.md` and `checks.md` the reasons and gates):

- `STD-01@4` — [docs/standards/STD-01-cli-surface/STD-01.md](docs/standards/STD-01-cli-surface/STD-01.md): CLI surface — command grammar, inputs and effects, output modes, errors, help, and golden-tested public surfaces
- `STD-02@10` — [docs/standards/STD-02-rust-architecture-and-errors/STD-02.md](docs/standards/STD-02-rust-architecture-and-errors/STD-02.md): Rust architecture and errors — layering, visibility, typed errors, failure semantics, idiom and naming, logging, and…
- `STD-03@2` — [docs/standards/STD-03-concurrency-and-process-safety/STD-03.md](docs/standards/STD-03-concurrency-and-process-safety/STD-03.md): Concurrency and process safety — locks, channels, durable writes, state evolution, subprocess lifecycles, destructive…
- `STD-04@2` — [docs/standards/STD-04-testing-and-verification/STD-04.md](docs/standards/STD-04-testing-and-verification/STD-04.md): Testing and verification — test design, hermetic tests, honest gates, docs as contracts, and evidence-based…
- `STD-05@1` — [docs/standards/STD-05-security-boundaries/STD-05.md](docs/standards/STD-05-security-boundaries/STD-05.md): Security boundaries — authority, filesystem containment, child environments, redaction, local network surfaces, supply…
- `STD-11@3` — [docs/standards/STD-11-agent-context/STD-11.md](docs/standards/STD-11-agent-context/STD-11.md): Agent context files — skills and instruction files loaded by an agent harness
- `STD-12@4` — [docs/standards/STD-12-source-style/STD-12.md](docs/standards/STD-12-source-style/STD-12.md): Source style — names, vertical spacing, line width and the formatter gate, as a language-neutral core with one profile…
- `STD-14@1` — [docs/standards/STD-14-pre-release-legacy-support/STD-14.md](docs/standards/STD-14-pre-release-legacy-support/STD-14.md): Pre-release legacy support — no compatibility code before a project's first release tag

Follow them; they are normative. To deviate from a rule, record a decision in `docs/design/<feature>/4_decisions.md` citing `STD-nn@<version> §Rn`; never edit `docs/standards/` (`sh docs/standards/check.sh` enforces this).
Reviewers check every change against the adopted standards and report violations as `STD-nn §Rn` with file:line evidence.
<!-- constellation-standards:end -->
