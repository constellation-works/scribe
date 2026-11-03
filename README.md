# scribe

A standalone Orbit plugin that keeps Daniel's explicitly marked preferences and decisions, catalogs the sessions he turns on, and answers agents' questions about him with verbatim citations. Design: [docs/design/scribe/1_overview.md](docs/design/scribe/1_overview.md).

This slice reads the store; recording entries (`#scribe`), cataloging
sessions (`/scribe on`), `ask` and the Orbit plugin arrive in later slices.

```sh
scribe entry list                       # a table on a terminal, tab-separated lines in a pipe
scribe entry list --tag decisions --status active
scribe entry list --project constellation-works/scribe --topic release
scribe entry list --json | jq -r '.entries[].content'
scribe entry show <id>
scribe session list --project constellation-works/scribe
scribe session show <id> --json | jq -r .next
scribe proposal list
scribe tag list
scribe tag list --json | jq '.tags[] | select(.configured | not)'
```

The store is a directory: `--root <dir>`, else `$SCRIBE_ROOT`, else
`~/.scribe`. Entries are Markdown files with YAML frontmatter under
`entries/<tag>/<tag>-<n>.md`; session records are
`sessions/<yyyy>/<mm>/<session-id>.md`; `control.json` holds which sessions
are being cataloged, `proposals.json` the consistency routine's proposals and
`config.toml` the tags, models, catalog cadence and redaction settings. The
layout and every key are in
[specs/store.md](docs/design/scribe/specs/store.md). A store that another
user owns or could write, a linked file and a record from a newer build are
refused. Every command here only reads, and writes nothing, not even an audit
line.

| Setting | Flag | Environment | Default |
|---------|------|-------------|---------|
| Data directory | `--root <DIR>` | `SCRIBE_ROOT` | `~/.scribe` |
| Output format | `--format auto\|table\|json`, `--json` | `SCRIBE_FORMAT` | `auto` |
| Color | — | `NO_COLOR`, `CLICOLOR_FORCE`, `TERM` | on for a terminal |
| Log level (stderr) | — | `SCRIBE_LOG` (e.g. `debug`) | `warn` |

Exit status is 0 on success, 1 when a command fails and 2 on a usage error.
`--json` output is a stable contract
([specs/output-contract.md](docs/design/scribe/specs/output-contract.md)): a
list is `{"<entries|sessions|proposals|tags>": [...], "total": N,
"truncated": false, "warnings": [...]}`, where `warnings` names each record
file left out because it did not parse; one record is an object; and errors
in JSON mode are one `{"error", "code"}` object on stderr.

## Starting point

This repository was created from the constellation Rust CLI template, either
with GitHub's "Use this template" on `constellation-works/rust-cli-template`
followed by `scripts/rename-template.sh <name>`, or inside the constellation
umbrella by `operations/scripts/new-rust-cli.sh` (the template's source is
`operations/templates/rust-cli/` there). The template's note example has
been replaced by scribe's domain; its structure, gates and tests remain the
pattern to follow (STD-01 CLI surface, STD-02 Rust architecture, STD-03
concurrency and process safety, STD-04 testing and verification, STD-05
security boundaries). The command lines in this README are parsed by a test,
so keep them runnable.

## Layout

```
crates/scribe-core/   domain library (no clap, no terminal): entry.rs, tag.rs,
                        session.rs, proposal.rs, record.rs, config.rs and
                        verbatim.rs (business logic, no I/O), store/
                        (persistence: facade, format, redact, audit log,
                        fsio), error.rs
crates/scribe/        the binary: cli.rs (root flags), app.rs (composition),
                        commands/ (one module per noun: flags and handlers),
                        audit_middleware.rs (audits mutating commands),
                        output/ (sink, rendering, errors)
crates/*/tests/         integration tests; crates/scribe/tests/goldens/ holds the
                        help and output goldens, tests/fixtures/store/ the
                        fixture store they read
scripts/                structure gates run by `make ci-fast`
docs/design/            design docs: _templates/ to copy, scribe/ for this tool
```

[ARCHITECTURE.md](ARCHITECTURE.md) has the layer rules; [AGENTS.md](AGENTS.md)
has the working rules for agents.

## Development

```sh
make ci-fast          # fmt, structure checks, standards-check; no compile
make ci-lint          # clippy -D warnings + cargo-deny
make test             # nextest when installed, else cargo test; then doctests
make goldens          # compare help/output with the built binary
make goldens UPDATE=1 # regenerate after an intended surface change
make ci               # all of the above; what CI runs
```

`cargo-deny` and `cargo-nextest` are optional locally (the Makefile warns
and skips or falls back); CI installs both at pinned versions, and there a
missing tool fails. `make standards-check` runs `docs/standards/check.sh`
once the constellation standards are vendored with
`operations/scripts/sync-standards.sh`; until then it warns locally and fails
in CI (`STANDARDS_STRICT=1`).
