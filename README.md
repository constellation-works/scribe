# scribe

A standalone Orbit plugin that keeps Daniel's explicitly marked preferences and decisions, catalogs the sessions he turns on, and answers agents' questions about him with verbatim citations. Design: [docs/design/scribe/1_overview.md](docs/design/scribe/1_overview.md).

> The code below is still the Rust CLI template's note-keeping example; the first slice replaces it.

Keep short notes in a local store.

```sh
scribe note add "Call the plumber" --tag home --priority high
scribe note list                  # a table on a terminal, tab-separated lines in a pipe
scribe note list --json | jq '.notes[].title'
scribe note show <id>
```

Notes live in one JSON file under the data directory: `--root <dir>`, else
`$SCRIBE_ROOT`, else `~/.scribe`. The directory is created owner-only,
and a store that another user owns or could write is refused. Each `note add`
appends one line to `audit.jsonl` beside it (what ran, on which note, how it
ended; never the note's text). Reading commands write nothing.

| Setting | Flag | Environment | Default |
|---------|------|-------------|---------|
| Data directory | `--root <DIR>` | `SCRIBE_ROOT` | `~/.scribe` |
| Output format | `--format auto\|table\|json`, `--json` | `SCRIBE_FORMAT` | `auto` |
| Color | — | `NO_COLOR`, `CLICOLOR_FORCE`, `TERM` | on for a terminal |
| Log level (stderr) | — | `SCRIBE_LOG` (e.g. `debug`) | `warn` |

Exit status is 0 on success, 1 when a command fails and 2 on a usage error.
`--json` output is a stable contract: a list is
`{"notes": [...], "total": N, "truncated": false}`, one note is an object,
and errors in JSON mode are one `{"error", "code"}` object on stderr.

## Starting point

This repository was created from the constellation Rust CLI template, either
with GitHub's "Use this template" on `constellation-works/rust-cli-template`
followed by `scripts/rename-template.sh <name>`, or inside the constellation
umbrella by `operations/scripts/new-rust-cli.sh` (the template's source is
`operations/templates/rust-cli/` there). The note commands are a working example
of the engineering standards (STD-01 CLI surface, STD-02 Rust architecture,
STD-03 concurrency and process safety, STD-04 testing and verification,
STD-05 security boundaries); replace them with the real domain and keep the
structure, gates and tests as the pattern to follow. Fill in `about.md` and
the summary above. The command lines in this README are parsed by a test, so
keep them runnable.

## Layout

```
crates/scribe-core/   domain library (no clap, no terminal): note.rs and query.rs
                        (business logic, no I/O), store/ (persistence: facade,
                        format, audit log, fsio), error.rs
crates/scribe/        the binary: cli.rs (root flags), app.rs (composition),
                        commands/ (one module per noun: flags and handlers),
                        audit_middleware.rs (audits mutating commands),
                        output/ (sink, rendering, errors)
crates/*/tests/         integration tests; crates/scribe/tests/goldens/ holds the
                        help and output goldens
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
