# scribe

A standalone Orbit plugin that keeps Daniel's explicitly marked preferences and decisions, catalogs the sessions he turns on, and answers agents' questions about him with verbatim citations. Design: [docs/design/scribe/1_overview.md](docs/design/scribe/1_overview.md).

A `#scribe` line Daniel types in Claude Code is recorded as an entry by the
`UserPromptSubmit` hook (forms `#scribe <tag>: <content>` and `#scribe add
<content> to <tag>`, and `#scribe add this to <tag>`, whose "this" a model
resolves). `/scribe on` in a session catalogs it: `scribe session pass`
writes one record per session whose pass is due. The CLI reads the store,
and `scribe question ask` answers from it, quoting each record it cites
verbatim; the Orbit plugin in `plugin/.orbit-plugin/` serves the same
answer to agents as `scribe.ask` (install steps in its
[README](plugin/.orbit-plugin/README.md)). `scribe consistency scan` (the
weekly consistency routine's job) proposes superseding an entry a later one
replaced, and retiring an entry due for review; Daniel settles each with
`#scribe approve p-<n>` or `#scribe reject p-<n>` in a prompt, and hides or
blanks an entry with `#scribe retire <id>` or `#scribe forget <id>`.

```sh
scribe entry list                       # a table on a terminal, tab-separated lines in a pipe
scribe entry list --tag decisions --status active
scribe entry list --project constellation-works/scribe --topic release
scribe entry list --json | jq -r '.entries[].content'
scribe entry show <id>
scribe session list --project constellation-works/scribe
scribe session show <id> --json | jq -r .next
scribe session pass --json
scribe proposal list
scribe consistency scan --json
scribe tag list
scribe tag list --json | jq '.tags[] | select(.configured | not)'
scribe question ask "how do we merge agent PRs?"
scribe question ask "can you fetch me last session summary?" --json   # a lookup: no model call
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
refused. Every `entry`, `session`, `proposal`, `tag` and `question` command
except `session pass` only reads, and writes nothing, not even an audit line;
`session pass` and `consistency scan` write and are audited.

## The Claude Code package

`plugin/claude-code/` registers four hooks that run the `scribe` binary on
`PATH`, and the `/scribe` command: `SessionStart` runs `scribe hook
session-start`, which tells Daniel how many proposals are open;
`UserPromptSubmit` runs `scribe hook user-prompt-submit`, which
records each `#scribe` line of the typed prompt (lines in code fences,
blockquotes and pasted blocks are skipped), applies each `#scribe
approve|reject|retire|forget`, and replies to Daniel and the
agent with what it recorded, settled or refused; `Stop` runs `scribe hook stop`,
which fills each new entry's cite with the transcript message's id and, for
a session that is on, marks a pass due every `every_minutes` or
`every_turns` (`[catalog]` in `config.toml`); `SessionEnd` runs `scribe
hook session-end`, which marks the last pass due and final. A prompt whose
first line is `/scribe on`, `/scribe off` or `/scribe status` is answered
by the `UserPromptSubmit` hook and never reaches the model; so are
`/scribe on --from <id>` and `/scribe link <id> [--type continues]`, which
link this session to the one it was spawned from (or continues), refusing
an unknown id, a self-link or a cycle. Claude Code's hook input and
transcripts name no parent session, so links are only ever typed. No hook calls a
model except for a `#scribe add this` line; the pass does. Each
write and refusal leaves one line in `audit.jsonl` with ids and error codes
only. Install it from this checkout after `make install`:

```sh
claude plugin install scribe --marketplace ./plugin
```

`plugin/.claude-plugin/marketplace.json` is the local marketplace that
command adds; `claude plugin validate plugin/claude-code` checks the package.

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
                        session.rs, proposal.rs, record.rs, config.rs,
                        capture.rs, control.rs, pass.rs, ask.rs, transcript.rs,
                        verbatim.rs and models.rs (business
                        logic, no I/O), store/
                        (persistence: facade, format, redact, audit log,
                        fsio), error.rs
crates/scribe/        the binary: cli.rs (root flags), app.rs (composition),
                        commands/ (one module per noun: flags and handlers;
                        hook/ has one file per Claude Code hook event,
                        session/pass.rs writes session records, question.rs
                        answers, plugin.rs is the Orbit backend), models.rs
                        (the claude CLI adapter), tasks.rs (the Orbit task
                        adapter),
                        audit_middleware.rs (audits mutating commands),
                        output/ (sink, rendering, errors)
crates/*/tests/         integration tests; crates/scribe/tests/goldens/ holds the
                        help and output goldens, tests/fixtures/store/ the
                        fixture store they read
plugin/claude-code/     the Claude Code package (hooks, /scribe command), and plugin/.claude-plugin/
                        the local marketplace that installs it
plugin/.orbit-plugin/   the Orbit plugin tree (manifest, launcher, schemas,
                        conformance goldens and the manifest contract test)
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
