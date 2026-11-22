# scribe

A standalone Orbit plugin that keeps Daniel's explicitly marked preferences, catalogs the sessions he turns on, and answers agents' questions about him with verbatim citations. Design: [docs/design/scribe/1_overview.md](docs/design/scribe/1_overview.md).

A `#scribe` Daniel types in a Claude Code prompt, inside a sentence or
opening a line (`#scribe idea: …`, `#scribe add this to preferences`), marks
what it annotates: the `UserPromptSubmit` hook echoes it, and after the reply
the `Stop` hook catalogs it with one bounded model call, as an entry whose
content is scribe's statement and whose `quote` is his exact words. A tag
named after the marker, or at the end of its line (`… to preferences`), is a
hint the model must follow. `#scribe/global` instead of `#scribe` stores
`project: global`, for every repository; without it the entry keeps the
working directory's project. Codex desktop threads work the same way once the typed turn is verified. `/scribe on` in a session catalogs it: `scribe session pass`
writes one record per session whose pass is due and proposes unmarked preferences
from Daniel's own turns (up to three per session pass, twenty open
candidates store-wide). `scribe proposal list --state open --json` shows their
quotes and source turns. `#scribe approve <id>` creates the entry as Daniel's,
with the original turn's cite; `#scribe reject <id>` closes it for good. The CLI reads the store and accepts authored agent notes,
and `scribe question ask` answers from it, quoting each record it cites
verbatim; the Orbit plugin in `plugin/.orbit-plugin/` serves the same
answer to agents as `scribe.ask` (install steps in its
[README](plugin/.orbit-plugin/README.md)). `scribe consistency scan` (the
weekly consistency routine's job) proposes superseding an entry a later one
replaced, and retiring an entry due for review; Daniel settles each with
`#scribe approve p-<n>` or `#scribe reject p-<n>` in a prompt, and hides or
blanks an entry with `#scribe retire <id>` or `#scribe forget <id>`. The
scan also deactivates (hides, never removes) sessions with no messages for
`[sessions] inactive_after_days` (120) and sessions a later record says were
removed or replaced; an agent or Daniel does the same by hand with `scribe
session deactivate <id> --author <name> --reason <text>`, and `scribe
session activate <id> --author <name> [--pin|--unpin]` shows one again or
exempts it from the scan.

Issues (`scribe issue add|occur|update|move|search|show|list`) are the
memory of incidents that recur across projects and what fixed them. When
one of Daniel's typed turns fires an `issue` trigger ("can you
investigate", "root cause", …; `[tags.issue.capture]`), the prompt hook
tells the agent the turn asks for a root cause. The agent searches issues
first and, when it answers, writes the result with his request quoted word
for word; it needs no approval, and Daniel removes a wrong one with
`#scribe forget issue-<n>` or `#scribe forget issue-<n>#<m>`. Every fire
and write is counted in the audit log; `scribe issue stats [--since
<date>]` reports fires, writes and withdrawals with proxy ratios for
precision and trigger yield.

Action tags hold work items for consumers outside scribe instead of
Daniel's memory: `standard`, `remind` and `idea` ship, and config marks
any other tag `action = true`. A `#scribe` line under one also keeps the
transcript turns since the previous `#scribe` line (at most 40 turns and
12,000 characters, cut from the oldest). Agents write them with `entry add`
in an interactive session, subject to a five-minute cancel window and,
under `standard`, a quote of Daniel's own words; consumers read them with
`entry list --tag <tag> --after <time> --window --json` and note what they
did with `entry pickup`. `ask` leaves them out unless asked for them
([2_design.md §4.6](docs/design/scribe/2_design.md#46-action-tags)).

```sh
scribe entry list                       # a table on a terminal, tab-separated lines in a pipe
scribe entry list --tag preferences --status active
scribe entry list --project constellation-works/scribe --topic release
scribe entry list --since 2026-11-01 --author-kind daniel --related task:<id>
scribe entry list --json | jq -r '.entries[].content'
scribe entry show <id>                  # or a unique prefix of at least four characters
scribe entry add --tag preferences --author opus 'Keep the API stable.'
scribe entry add --tag remind --author opus --related session:<id> 'Re-run the sync benchmark after the cache lands'
scribe entry list --tag standard --after 2026-11-15T09:00:00Z --window --json
scribe entry pickup <id> --author standards-intake --result task:<id>
scribe entry update <id> --author opus --add-topic api
scribe entry retire <id> --author opus --reason 'No longer applies'
scribe entry retire <id> --author opus --reason 'Daniel asked' --session <id> --instruction '<his words>'
scribe entry forget <id> --author opus --session <id> --instruction '<his words>' --confirm
scribe tag add notes --author opus --description 'Agent notes'
scribe tag update notes --author opus --add-alias note
scribe proposal add --kind stale --older <id> --reason 'Review needed' --author opus
scribe proposal update <id> --author opus --reason 'New evidence'
scribe proposal settle <id> --reject --author opus
scribe proposal settle <id> --approve --author opus --session <id> --instruction '<his words>'
scribe session list --project constellation-works/scribe
scribe session list --since 2026-11-01 --status final --topic release --topic store
scribe session show <id> --json | jq -r .next
scribe session show <id> --log
scribe session pass --json
scribe session rebuild <id> --author <name> --confirm
scribe session deactivate <id> --author opus --reason 'the feature was removed'
scribe session activate <id> --author opus --pin
scribe proposal list --state open --entry <id>
scribe record search "merge policy" --kind session --since 2026-11-01   # BM25, no model call
scribe issue search 'HTTP 429 secondary rate limit' --json
scribe issue occur <id> --author opus --related session:<id> --request '<his words>' --summary 'The drain failed again' --fix 'Paused the drain'
scribe consistency scan --json
scribe tag list
scribe tag list --json | jq '.tags[] | select(.configured | not)'
scribe question ask "how do we merge agent PRs?"
scribe question ask "can you fetch me last session summary?" --json   # a lookup: no model call
```

The store is a directory: `--root <dir>`, else `$SCRIBE_ROOT`, else
`~/.scribe`. Entries are Markdown files with YAML frontmatter under
`entries/<tag>/<tag>-<n>.md`; issues are `issues/issue-<n>.md`; session records are
`sessions/<yyyy>/<mm>/<session-id>.md`; `control.json` holds which sessions
are being cataloged, `proposals.json` holds routine and agent proposals,
`tags.json` holds agent-created tags, `pickups.json` holds consumers' pickups of action entries, and `config.toml` holds configured tags,
models, catalog cadence, session age limit and redaction settings. Config names and aliases
take precedence over store tags; `tag list` shows each source. The
layout and every key are in
[specs/store.md](docs/design/scribe/specs/store.md). A store that another
user owns or could write, a linked file and a record from a newer build are
refused. List, show, search and question commands only read: they take no lock
and write nothing, including audit lines. Direct agent writes, session
passes and rebuilds, `session deactivate` and `session activate`, and consistency scans are audited. Each direct write
requires `--author` and records kind `agent`, even for the name `daniel`.
Agents may annotate Daniel's topics and references, but cannot replace his
content or settle proposals about his entries; Daniel settles those through
`#scribe`. Only on his checked instruction (`--session` and `--instruction`,
naming a turn he typed in the last 30 minutes) may an agent retire or forget
his entry, or approve or reject a claim a session pass proposed. Agent notes have no typed-turn citation;
`ask` labels them `agent note (<name>)`. Repeating an unchanged write prints
the unchanged record with `nothing changed` on stderr and no audit event.
The store now writes format 17 and reads older formats without rewriting them. Under a tag whose capture route is `write` (the shipped `idea`), a session pass writes up to 3 of Daniel's ideas per session per run itself, with their outcome, as entries it authors (`--author-kind scribe`).
Passes rewrite the header and open segment and append new
segments; sealed segments stay unchanged. `[catalog] segment_windows`
(default 3) bounds each segment's rewrites. Rebuild replaces a record from
its transcript only when every window succeeds, resuming private
checkpoints on the next confirmed invocation.

## Generic skill and project initialization

The standalone, provider-neutral skill is [skills/scribe/SKILL.md](skills/scribe/SKILL.md).
It is embedded in the binary; installing `scribe` is sufficient to initialize
another project without this repository or Constellation:

```sh
scribe init --provider codex --target /absolute/path/to/project
scribe init --provider claude-code --target /absolute/path/to/project
```

Init requires the selected runtime executable on `PATH` and an existing project.
Codex receives `.agents/skills/scribe/SKILL.md` and `.codex/hooks.json`;
Claude receives `.claude/skills/scribe/SKILL.md` and `.claude/settings.json`.
The hook command names the absolute installed scribe executable. Existing unrelated
hooks/configuration are preserved. A private `scribe-init.json` receipt in the
provider configuration directory tracks owned skill content and hook definitions.
Repeating init reports unchanged files; an upgrade replaces only unchanged owned
artifacts. User edits produce an actionable conflict instead of an overwrite.
The persistent `.scribe-init.lock` serializes project initialization and may be
ignored by version control; do not remove it while an initializer is running.
Created state is owner-only, and symlinked or foreign-writable targets are refused.

Init never grants hook trust or enables cataloging or a scheduler. Restart/resume
the provider after installation. Codex requires review of the exact hooks in
**Settings > Hooks** (desktop) or `/hooks` (CLI); a changed package needs review
again. The human types `/scribe on`, `/scribe off`, `/scribe status`, or an exact
Codex `$scribe on|off|status` skill invocation. Merely loading a skill changes no
recording state. Do not initialize project hooks and install the native plugin
for the same project unless you deliberately want both hook sources.

## The Codex plugin

The local package is `plugin/codex/`; the local marketplace manifest lives at
`.agents/plugins/marketplace.json`. Register the repository root, then install:

```sh
codex plugin marketplace add /absolute/path/to/scribe --json
codex plugin add scribe@scribe-local --json
```

This performs no public marketplace submission. The packaged launcher tries
`SCRIBE_BIN` (an absolute executable), `scribe` on `PATH`, then
`$HOME/.cargo/bin/scribe`, so GUI/launchd environments need not include Cargo on
`PATH`. A missing binary says recording is unavailable. Authenticate the Codex
CLI and review/trust hooks after enabling the plugin. The generic skill copies
in both packages are generated by `python3 scripts/sync-skills.py`; `make ci-fast`
checks them against the canonical source.

Codex transcripts are read from `$CODEX_HOME/sessions` and
`$CODEX_HOME/archived_sessions` (default `$HOME/.codex`). Supported desktop
rollouts must identify a main `thread_source: user` thread and carry per-content
`user.text`/`provenance.type: user` metadata with a host turn id. User roles alone,
injected skills, repository instructions, tools, automation and subagents do not
supply typed-user provenance. Unknown transcript versions fail closed with a
nonblocking recording refusal. This is a version-sensitive adapter tested against
Codex CLI 0.167.2 and desktop 0.167.0-alpha.4.1 metadata, not a stable transcript API.

UserPromptSubmit may arrive before its transcript item exists. It then reports
pending verification; Stop processes matching host-attributed inputs, once, from
the current turn. First activation never replays earlier turns' capture or
settlement commands. An existing cursor resumes later inputs; a rewritten or
missing cursor refuses replay. Plain prompts preserve their exact hash and line
numbers. The host-owned files and no-follow descriptor checks prevent accidents;
a process with the same OS account can still forge them. Agents must not call
hooks to impersonate the human or relax existing author restrictions.

Session records and switches carry provider identity. `scribe session pass`
routes each opted-in session to its provider, including archived final passes;
`session rebuild` uses its record's provider. Discovery is read-only:

```sh
scribe --provider codex transcript list --since 2026-11-01 --min-turns 1 --json
scribe --provider codex transcript show <id> --json
scribe session pass --session <id> --json
```

Codex model work defaults to **Luna (`gpt-6-luna`)**, with no fallback. Override
Codex-origin routing in the existing private store's `config.toml`:

```toml
[codex.models]
provider = "codex"                 # or explicit "claude-code" transport
ask = "gpt-6-luna"
session = "gpt-6-luna"
consistency = "gpt-6-luna"
annotation = "gpt-6-luna"
credentials = ["OPENAI_API_KEY"]     # existing Codex login also works
```

`[models]` retains Claude defaults; it also accepts `provider = "codex"` for
explicit global routing, defaulting its model ids to Luna. Global question and
consistency jobs otherwise use `[models]`; `--provider codex` selects the Codex
model table for those CLI calls. Explicit ids are always honored. Model children
run ephemeral, without user/project instructions, plugins, hooks, shell, apps,
web search or tools, from a private temporary directory. The shared supervisor
bounds time/output and terminates the process group. Schemas are validated, tool
items refused, secrets masked in diagnostics, and temporary schemas/logs removed.
Missing CLI/auth/model availability is an error, never a Claude fallback.

For Luna-backed Orbit session passes, install the generated
[`plugin/orbit-codex/`](plugin/orbit-codex/README.md) variant. Alongside `codex_home`,
it requires explicit `codex_runtime_tmp` and `codex_installation_id` settings and
an Orbit build with exact-file plugin writes. Codex needs its `tmp` directory
and `installation_id` file writable at startup, even for ephemeral model calls;
its auth, configuration and transcripts stay read-only. Scribe copies no
credentials and requests no whole-home write or unsandboxed permission. The
default `plugin/.orbit-plugin/` remains compatible with Claude-only setups.
Both variants keep ask as the only agent-callable MCP tool and ship routines
disabled; upgrading preserves existing enablement and schedules.

Official lifecycle/trust contract: [Codex hooks](https://learn.chatgpt.com/docs/hooks);
packaging: [Build plugins](https://developers.openai.com/plugins/build/plugins#bundled-mcp-servers-and-lifecycle-hooks).

## The Claude Code package

`plugin/claude-code/` registers four hooks that run the `scribe` binary on
`PATH`, and the `/scribe` command: `SessionStart` runs `scribe hook
session-start`, which tells Daniel how many proposals are open;
`UserPromptSubmit` runs `scribe hook user-prompt-submit`, which
echoes each `#scribe` marker of the typed prompt (code fences,
blockquotes, pasted blocks and quoted mentions are skipped), applies each
whole-line `#scribe approve|reject|retire|forget`, and replies to Daniel and
the agent with what it marked, settled or refused; `Stop` runs `scribe hook
stop`, which catalogs the session's unsettled markers (one `models.annotation`
call per marked turn, at most two turns per Stop), echoes what it recorded or
why not, and fills the message id of an older entry's cite. A line an older
store already recorded verbatim is not cataloged again, and settling
commands are never replayed. For a session that is on, Stop also
marks a pass due every `every_minutes` or
`every_turns` (`[catalog]` in `config.toml`); `SessionEnd` runs `scribe
hook session-end`, which marks the last pass due and final. A prompt whose
first line is `/scribe on`, `/scribe off` or `/scribe status` is answered
by the `UserPromptSubmit` hook and never reaches the model; so are
`/scribe on --from <id>` and `/scribe link <id> [--type continues]`, which
link this session to the one it was spawned from (or continues), refusing
an unknown id, a self-link or a cycle. Claude Code's hook input and
transcripts name no parent session, so links are only ever typed. No hook calls a
model except Stop's annotation call; the pass does. Each
write and refusal leaves one line in `audit.jsonl` with ids, the `--author`
name and error codes only. Install it from this checkout after `make install`:

```sh
claude plugin install scribe --marketplace ./plugin
```

`plugin/.claude-plugin/marketplace.json` is the local marketplace that
command adds; `claude plugin validate plugin/claude-code` checks the package.
An install after a plugin change runs:

```sh
claude plugin marketplace update scribe-local
claude plugin update scribe@scribe-local
PLUGIN_SRC="$(mktemp -d)/.orbit-plugin"   # a copy outside the repository, same name
cp -R plugin/.orbit-plugin "$PLUGIN_SRC"
orbit plugin upgrade scribe "$PLUGIN_SRC" --grant fs,network,env_pass,orbit_tools
```

Orbit refuses an upgrade source inside the repository, so the command copies the
tree first; [the plugin README](plugin/.orbit-plugin/README.md#install-operator)
says why.

`claude plugin update` applies on the next Claude Code restart.

The package also ships one skill, `skills/scribe/SKILL.md`, which teaches
agents the CLI above: when a word, date or topic makes `record search` or
a list the right tool and when a vague question goes to `question ask`,
the `--author` rule for the write verbs, and the guard that Daniel's own
entries are settled only by him. The skill serves sessions on this Mac,
where the store lives.

## The tmux popup

`scribe tmux` browses sessions, entries, proposals and
backfill candidates in an `fzf` popup. The preview includes the session
summary and log or the entry text and metadata. Enter opens the record
in the pager, and `ctrl-y` copies its ID. The popup is read-only and
requires `fzf` on `PATH`. Bind a key in `~/.tmux.conf`
(`scribe tmux --help` lists the keys):

```tmux
bind-key S display-popup -E -w 90% -h 85% scribe tmux
```

Backfill candidates come from `scribe transcript list` over the last 30
days. An empty list, or a list that fails, is one notice row instead of
a failed popup.

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
                        session.rs, activity.rs, proposal.rs, record.rs, config.rs,
                        capture.rs, control.rs, pass.rs, ask.rs, action.rs, transcript.rs,
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
                        fixture store they read, tests/fixtures/replay/ the
                        session rebuild replay recordings
                        (docs/design/scribe/specs/replay-fixtures.md)
plugin/claude-code/     the Claude Code package (hooks, /scribe command), and plugin/.claude-plugin/
                        the local marketplace that installs it
plugin/.orbit-plugin/   the Orbit plugin tree (manifest, launcher, schemas,
                        conformance goldens and the manifest contract test)
scripts/                structure gates run by `make ci-fast`, and
                        redact-recording.sh (redacts a replay recording)
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
