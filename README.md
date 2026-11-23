<h1 align="center"><a href="https://github.com/constellation-works/scribe">scribe</a></h1>

<p align="center">A local record of what you asked to remember.</p>

<p align="center">
  <a href="https://github.com/constellation-works/scribe/releases"><img src="https://img.shields.io/github/v/release/constellation-works/scribe" alt="Release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License: MIT" /></a>
  <a href="https://github.com/constellation-works/scribe/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/constellation-works/scribe/ci.yml?branch=agent-main" alt="CI" /></a>
</p>

Scribe keeps the preferences you mark and the chat sessions you choose to catalog, on your own machine. An agent that asks it a question gets an answer that quotes those records.

Scribe is pre-1.0. It runs on Linux and macOS; a Windows host is not supported.

## Why

- You mark a sentence in a chat, and scribe stores your words.
- You turn a session on, and scribe writes a record of it.
- You ask a question, and the answer quotes the record it used.
- The files stay in one directory on your machine.
- You and an agent read and write them with one command-line tool.

## How it works

```text
You:     Keep the public API stable. #scribe
scribe:  After the reply, stores your sentence and a short statement of it.
You:     /scribe on
scribe:  Catalogs this session.
Agent:   scribe question ask "what should stay stable?"
scribe:  Answers, and quotes your sentence.
```

`/scribe on` is the Claude Code command. Codex accepts the same marker, and an exact `$scribe on`. Both installs are in the [guide](docs/guide.md).

## Quick start

**You need:** Rust at or above the `rust-version` in [Cargo.toml](Cargo.toml), on Linux or macOS, and a checkout of this repository.

```sh
make install
scribe brief
scribe entry list --tag preferences --status active
```

1. Install with the block above. `make install` copies the `scribe` binary to `~/.cargo/bin`.
2. Read the page from `scribe brief`. An empty data directory still prints it.
3. Read `scribe entry list`. It prints a table on a terminal and tab-separated lines in a pipe.
4. To mark a sentence or catalog a session, install the Claude Code or Codex package. The steps are in the [guide](docs/guide.md).
5. Ask with `scribe question ask "preferences this week"`. That lookup does not call a model. Other questions use the configured model and quote each record they cite.

## Session groups

A group links named seats to the ordered sessions that held them. Open it from the session where you asked for it, quoting your typed words; each agent joins from its own session under its own author.

```sh
scribe group open --title 'Release review' --kind review --project scribe --quote 'open a release review'
scribe group join <id> --as editor --role reviewer --author aide
scribe group show <id> --json
```

<details>
<summary>Other ways to set it up</summary>

`scribe init` scaffolds the skill and hooks into a project you already have. The Orbit plugin answers the same questions for an agent. Both are in the [guide](docs/guide.md).

</details>

## Learn more

- [Guide](docs/guide.md) — Claude Code, Codex and Orbit install, the commands, the data directory, and the tmux popup.
- [Overview](docs/design/scribe/1_overview.md) — what scribe is for.
- [Design](docs/design/scribe/2_design.md) — how the pieces work.
- [Store and configuration](docs/design/scribe/specs/store.md) — directory layout and every config key.
- [Catchers](docs/design/scribe/specs/catchers.md) — what a session pass keeps.
- [Output contract](docs/design/scribe/specs/output-contract.md) — JSON shapes, streams and exit status.
- [Architecture](ARCHITECTURE.md) — crate layers and what may call what.
- [Changelog](CHANGELOG.md) — notable changes.
- [Agent skill](skills/scribe/SKILL.md) — how an agent reads and writes the records.
- [Orbit plugin](plugin/.orbit-plugin/README.md) — the `scribe.ask` tool and its install.
- [Codex Orbit package](plugin/orbit-codex/README.md) — the Codex-backed plugin variant.

## Contributing

Working rules and gates are in [AGENTS.md](AGENTS.md). Before review, `make ci-fast`, `make ci-lint` and `make goldens` pass; `make ci` is the full run.

## Licence

[MIT](LICENSE).
