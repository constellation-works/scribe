# Using scribe

The [README](../README.md) is the front page. This page holds the install steps,
the command list and the operating notes that used to live there. How an agent
should read and write the records is the [skill](../skills/scribe/SKILL.md).
The mechanism is the [design](design/scribe/2_design.md). Every config key is
the [store spec](design/scribe/specs/store.md).

## What a mark does

In Claude Code or Codex, a `#scribe` you type in your own turn, inside a
sentence or opening a line (`#scribe idea: …`, `#scribe add this to preferences`),
marks that sentence. The prompt hook echoes it. After the reply, Stop hands
the marked line to the agent, with no model call: the agent records it with
`scribe entry add --spoken`, its content the agent's statement and its `quote`
your words, or records nothing when the line marks nothing. A tag named by the
marker, or written at the end of the line (`… to preferences`), is the `--tag`
Stop suggests. `#scribe/global` instead of `#scribe` suggests `--project
global`, for every repository; otherwise the entry keeps the working
directory's project. A line you queue while the agent works and a slash
command's arguments are marked the same way. `scribe mark list` shows the
markers with no entry from scribe.

`/scribe on` in Claude Code, or an exact `$scribe on` in Codex, catalogs that
session. The Orbit job `scribe-session-pass` writes records for due sessions
and proposes qualifying claims from your turns. `scribe session pass` settles
empty or unchanged windows and points you to the job when it finds new work.
`scribe proposal list --state open --json` shows the quotes and the source
turns. `#scribe approve <id>` creates the entry as yours, citing the original
turn. `#scribe reject <id>` closes it for good.

`scribe search` finds records by their words, with no model call. The Orbit
plugin runs the routines ([install](../plugin/.orbit-plugin/README.md)).
`scribe consistency scan`
proposes retiring an entry due for review, with no model call. You settle each with `#scribe approve <id>` or `#scribe reject <id>`.
`#scribe retire <id>` hides an entry and keeps its text. `#scribe forget <id>`
blanks it. The scan also deactivates sessions with no messages for
`[sessions] inactive_after_days` (the default named in `scribe consistency scan
--help`). By hand:

```sh
scribe session deactivate <id> --author opus --reason 'the feature was removed'
scribe session activate <id> --author opus --pin
```

Catchers decide what the pass catches. Each tag's `[tags.<tag>.capture]` names
whose turns it reads, the trigger phrases, whether a catch is proposed or
written at once, and how many claims may wait open. `preferences` and
`standard` ship. A new catcher is config only. Examples are in
[specs/catchers.md](design/scribe/specs/catchers.md).

Action tags hold work for a consumer outside scribe. `standard` ships, and
config may mark another tag the same way. Agents write them with `entry add`,
and consumers read them with `entry list --tag <tag> --after <time> --window`
and note what they did with `entry pickup`. The rules are
[2_design.md §4.6](design/scribe/2_design.md#46-action-tags) and the
[skill](../skills/scribe/SKILL.md).

Prompts say "the user". `[user] name` in `config.toml` names you instead, and
`[instructions]` replaces what one model use is told to do. `{user}` in that
text stands for the name. The JSON shapes stay fixed in code
([store spec](design/scribe/specs/store.md),
[output contract](design/scribe/specs/output-contract.md)).

## Commands

These are the lines `scribe --help` and each command's `--help` document.
`<id>` is an id or a unique prefix of at least four characters. `--author` is
the agent name on a write; the reserved names are `user`, `scribe`,
`consistency`, `session-pass` and `tag-retired`.

```sh
scribe entry list --tag preferences --status active
scribe entry list --topic ci --limit 10
scribe entry list --since 2026-10-01 --until 2026-10-31 --author-kind user
scribe entry list --related task:<id> --related pr:<owner>/<repo>#<n>
scribe entry list --tag standard --after 2026-11-15T09:00:00Z --window --json
scribe entry show <id> --json
scribe entry show <id> --window
scribe entry add --tag preferences --author opus 'Use the stable API'
scribe entry add --tag preferences --author opus --related session:<id> --quote '<their words>' --spoken 'The user prefers short updates.'
scribe entry add --tag standard --author opus --related session:<id> --quote '<quoted words>' 'Name the unit in every size'
scribe entry add --tag decision --author opus --decided-by user --quote '<their words>' --related session:<id> --related task:<id> 'Keep the stable API'
scribe entry add --tag open-item --author opus --asked-by 0-chief --blocked 'Local checks' --answer-in hand 'Restart the dashboard'
scribe entry list --tag open-item --state open --state answered
scribe entry answer <id> --author opus --quote '<their words>' --related session:<id>
scribe entry close <id> --author opus --decision <id>
scribe entry close <id> --author opus --note 'Restarted; nothing lasting'
scribe entry pickup <id> --author standards-intake --result task:<id>
scribe entry update <id> --author opus --add-topic api
scribe entry retire <id> --author opus --reason 'No longer applies'
scribe entry retire <id> --author opus --reason 'The user asked' --session <id> --instruction '<their words>'
scribe entry forget <id> --author opus --session <id> --instruction '<their words>' --confirm
scribe tag list
scribe tag add notes --author opus --description 'Agent notes'
scribe tag update notes --author opus --add-alias note
scribe proposal list --state open --author-kind scribe
scribe proposal list --entry <id> --since 2026-11-01
scribe proposal add --kind stale --older <id> --reason 'Review needed' --author opus
scribe proposal update <id> --author opus --reason 'New evidence'
scribe proposal settle <id> --approve --author opus
scribe proposal settle <id> --reject --author opus --session <id> --instruction '<their words>'
scribe session list --project scribe
scribe session list --since 2026-11-01 --status final --topic store --topic release
scribe session show <id> --log
scribe session show <id> --json
scribe session pass --json
scribe session pass --session <id> --json
orbit job run scribe-session-pass --input 'sessions=["<id>"]'
scribe session rebuild <id> --author <name> --confirm
orbit job run scribe-session-rebuild --input '{"session":"<id>","author":"<name>","confirm":true}'
scribe session catalog <id> --author <name>
orbit job run scribe-session-catalog --input '{"session":"<id>","author":"<name>"}'
scribe session deactivate <id> --author opus --reason 'the feature was removed'
scribe session activate <id> --author opus --pin
scribe brief
scribe memory show
scribe memory refresh
scribe memory prepare --json
scribe search "merge policy" --kind session --since 2026-11-01
scribe search "merge queue" --kind message
scribe consistency scan --json
scribe --provider codex transcript list --since 2026-10-01 --min-turns 1 --json
scribe --provider codex transcript show <id> --json
```

`scribe session list` shows the newest session first, by when it was last
updated (ties by id), in the table, piped output and `--json`; `--limit N`
keeps the N most recently updated.

A session id may be given as a unique prefix of at least four characters
wherever a command or a typed `#scribe` or `/scribe` line takes one: `session
show 7c2b`, `--related session:7c2b`, `#scribe forget 7c2b`. A terminal `session
list` (flat or `--tree`) and `transcript list` table print each session id as
its shortest prefix that no other session in the store shares, at least eight
characters (the first group of a Claude Code UUID), and a prefix printed there
is accepted back. A terminal `group show` and `mark list` print a cataloged
session as its `s-<n>` alias instead. Piped output prints that alias for every
listing. A session with no alias shows its shortest prefix instead. An alias
such as `s-3` is accepted back as a session id. `--json` keeps the full
provider id. A prefix that matches several sessions is refused with
`id_ambiguous` and the matching ids, and nothing changes. A record written from
a prefix stores the full id, and a command that changes something names it.

What an id that matches no session does depends on what the command does with
it:

The manual-link row reports the hook process exit and its audited refusal code.

<!-- session-id-outcomes:start -->
| Case | Outcome | Exit | Error code |
| --- | --- | --- | --- |
| Unmatched related reference on entry add | Success; retain the literal reference | 0 | — |
| Manual link to an unknown session | Refused; no link or switch changes | 0 | `link_unknown_session` |
| Pass for an unmatched session | Refused as not switched on | 1 | `session_not_on` |
| Instruction lookup with no matching transcript | Refused when transcript is absent | 1 | `instruction_not_found` |
| Instruction lookup by unique uncataloged transcript prefix | Success; citation stores the full session ID | 0 | — |
| Instruction lookup by ambiguous transcript prefix | Refused; no entry changes | 2 | `id_ambiguous` |
<!-- session-id-outcomes:end -->


- A command that acts on a session the store knows refuses it with a not-found
  error (`session_not_found`) and changes nothing: `session show`, `session
  activate`, `session deactivate`, `session rebuild` and `#scribe forget`.
- `session pass --session <id>` refuses it as not switched on
  (`session_not_on`).
- The `/scribe link <id>` and `/scribe on --from <id>` resolvers keep an
  unmatched target as typed, but the final link check requires an otherwise
  valid target known by a session record or switch. Otherwise it refuses with
  `link_unknown_session` and changes no link or switch; a target known by a
  switch can be linked before its record is cataloged. `/scribe group add
  <slug> <id>...` keeps unmatched member ids as typed; those members need not
  have been cataloged.
- `scribe memory ack <now-id> --session <id> --author <name>` accepts and records
  the supplied id even when it has no cataloged session.
- An instruction check resolves `--session <id>` as a unique prefix across
  stored sessions and local transcripts, including uncataloged sessions. If
  there is no match, it keeps the id as typed for transcript lookup. If there
  is no matching transcript on this host, the check returns
  `instruction_not_found`. The transcript file's provenance is checked before
  its bytes are read or its Codex source is classified: a path that is not
  under this host's transcripts root or not named for the session, a provider
  that does not match `--provider`, a link, a file that is not regular, a file
  another user owns, or a file with a group or other write bit returns
  `instruction_not_found`. So a group-writable Codex `exec` rollout returns
  `instruction_not_found`, and the same rollout private to this user returns
  `not_interactive`. For Codex, before parsing the rollout or comparing its
  `session_meta` id with the requested id, scribe requires a
  `session_meta.payload.source` of `cli` or `vscode`; a missing or unsupported
  source, including a rollout with no `session_meta`, returns
  `not_interactive`. After an allowed source, a rollout that fails parsing or
  identifies another session returns `instruction_not_found`. A genuine I/O
  error finding or reading a transcript is `io_error`. Other noninteractive
  sessions, such as a private Codex `exec` rollout, return `not_interactive`.
- A `session:<id>` reference is not a lookup. `--related` on `entry add` and
  `entry update --add-related` keeps an id that matches no session as written:
  `entry add --tag preferences --related session:abcd` succeeds on a store with
  no session `abcd`, because the session may never have been cataloged.

An agent's own notes have no typed-turn citation. `scribe search` labels them
`agent note (<name>)`. Repeating an unchanged write prints the record with
`nothing changed` on stderr and writes no audit event. Only on your checked
instruction (`--session` and `--instruction`, a turn you typed in the last 30
minutes) may an agent retire or forget your entry, or approve or reject a
claim a session pass proposed. The [skill](../skills/scribe/SKILL.md) is the
procedure.

## Data directory

`--root <dir>`, else `$SCRIBE_ROOT`, else `~/.scribe`. Entries, proposals,
tags, pickups, annotation marks, the audit log and session records are rows in
`scribe.db`. Earlier builds moved a file store in at its first write and
kept the old files in `pre-sqlite-<date>/` (and converted issues in
`pre-issue-<date>/`). This build's first write, or `scribe store migrate`,
imports those directories into `scribe.db`'s archive tables and removes
them. It no longer reads a file store: one whose records never moved is
refused, so migrate it with an earlier build first. `scribe export <dir>`
writes the records out as Markdown files, or `--as jsonl` for one file per
table; neither includes the archives. Memory stays files under
`memory/`. `config.toml` holds configured tags, models, catalog cadence,
session age and redaction. The layout and every key are in the
[store spec](design/scribe/specs/store.md).

A store that another user owns or could write, a linked file, and a record
from a newer build are refused. List, show, search, brief and transcript
commands only read: they take no lock and write nothing, including audit
lines. Direct writes, session passes and rebuilds, `session deactivate` and
`session activate`, and consistency scans are audited.

The build writes the store format named `FORMAT` in
`crates/scribe-store/src/db/codec.rs` in exports and stamps, and refuses a
newer one. What each format changed is the store spec.

## Project init

The provider-neutral skill is [skills/scribe/SKILL.md](../skills/scribe/SKILL.md).
It is embedded in the binary, so installing `scribe` is enough to initialize
another project:

```sh
scribe init --provider codex --target /absolute/path/to/project
scribe init --provider claude-code --target /absolute/path/to/project
```

Init requires the selected runtime on `PATH` and an existing project. Codex
receives the skill under `.agents/skills/scribe/` and `.codex/hooks.json`. Claude
receives the skill under `.claude/skills/scribe/` and `.claude/settings.json`.
Both skill directories include `SKILL.md`, its references and evaluation
cases. The hook command names the absolute installed `scribe`. Unrelated hooks and
configuration are preserved. A private `scribe-init.json` receipt in the
provider configuration directory tracks owned skill content and hook
definitions; it is format 2 and also records the bundled skill files. This binary
reads formats 1 and 2 and refuses any other format as an unsupported receipt
format. Repeating init reports unchanged files. An upgrade replaces only
unchanged owned artifacts. Your edits produce a conflict instead of an
overwrite. The persistent `.scribe-init.lock` serializes initialization; do not
remove it while an initializer is running. Created state is owner-only.
Symlinked or foreign-writable targets are refused.

Init does not grant hook trust, turn cataloging on, or enable a scheduler.
Restart the provider after installation. Codex requires review of the hooks in
**Settings > Hooks** (desktop) or `/hooks` (CLI). A changed package needs that
review again. You type `/scribe on`, `/scribe off`, `/scribe status` or `/scribe group <slug>`, or an
exact Codex `$scribe on|off|status` or `$scribe group <slug>` (including a Codex skill mention link `[$scribe](.../scribe/SKILL.md) group <slug>`). Loading the skill changes no recording
state. Do not initialize project hooks and install the native plugin for the
same project unless you want both hook sources.

## Codex plugin

The local package is `plugin/codex/`. The marketplace manifest is
[`.agents/plugins/marketplace.json`](../.agents/plugins/marketplace.json).
Register the repository root, then install:

```sh
codex plugin marketplace add /absolute/path/to/scribe --json
codex plugin add scribe@scribe-local --json
```

This does not submit the package to a public marketplace. The packaged
launcher tries `SCRIBE_BIN` (an absolute executable), then `scribe` on `PATH`,
then `$HOME/.cargo/bin/scribe`, so a GUI environment need not have Cargo on
`PATH`. A missing binary says recording is unavailable. Authenticate the Codex
CLI and review the hooks after enabling the plugin. The skill copies in the
packages are generated by `python3 scripts/sync-skills.py`. `make ci-fast`
checks their full file lists and bytes against the [canonical skill](../skills/scribe/).

Codex transcripts are read from `$CODEX_HOME/sessions` and
`$CODEX_HOME/archived_sessions` (default `$HOME/.codex`). A supported desktop
rollout has to identify a main user thread and carry per-content user
provenance with a host turn id. User roles alone, injected skills, repository
instructions, tools, automation and subagents do not supply that provenance.
Unknown transcript versions fail closed with a nonblocking recording refusal.
The model transport is written against the Codex request shape named in
`crates/scribe-core/src/providers/cli/codex.rs`. It is not a stable transcript API.

`UserPromptSubmit` may arrive before its transcript item exists. It then
reports pending verification. Stop processes matching host-attributed inputs,
once, from the current turn. First activation does not replay earlier turns.
An existing cursor resumes later inputs. A rewritten or missing cursor refuses
replay. The host-owned file checks prevent accidents. A process with the same
OS account can still forge them. Agents must not call hooks to impersonate you
or relax the author rules.

Session pass reads each opted-in transcript from its recorded provider,
including archived Codex rollouts. Orbit selects the model through the
`session_distill_crew` and `session_header_crew` plugin settings. Both
default to `haiku`. The `scribe-session-catalog` and `scribe-session-rebuild`
jobs use the same crews and read the record's provider.

```sh
scribe --provider codex transcript list --since 2026-11-01 --min-turns 1 --json
scribe --provider codex transcript show <id> --json
scribe session pass --session <id> --json
```

Interim Codex catalog and rebuild model work defaults to Luna (`gpt-6-luna`), with no fallback. Override it
in the private store's `config.toml`. Missing keys keep the defaults in
`ModelIds::codex()` (`crates/scribe-core/src/config.rs`):

```toml
[codex.models]
provider = "codex"                 # or explicit "claude-code" transport
ask = "gpt-6-luna"
session = "gpt-6-luna"
credentials = ["OPENAI_API_KEY"]   # an existing Codex login also works
```

`[models]` keeps the Claude defaults. It also accepts `provider = "codex"` for
global routing, and then defaults its model ids to Luna. The `annotation` and `consistency` keys still load
but are unused. `--provider codex` selects the Codex
model table for those CLI calls. An explicit id is honored. Model children run
ephemeral, without your instructions, plugins, hooks, shell, apps, web search
or tools, from a private temporary directory. The supervisor bounds time and
output and terminates the process group. Schemas are validated, tool items are
refused, secrets are masked in diagnostics, and temporary schemas and logs are
removed. A missing CLI, login or model is an error, not a fall back to Claude.

Configure `codex_home` in the [Orbit plugin](../plugin/.orbit-plugin/README.md)
to read Codex transcripts. The [generated variant](../plugin/orbit-codex/README.md)
remains available. Orbit runs its configured crews under its own provider
settings. Both plugin variants ship routines disabled and preserve existing
enablement and schedules on upgrade.

Lifecycle and trust: [Codex hooks](https://learn.chatgpt.com/docs/hooks).
Packaging: [Build plugins](https://developers.openai.com/plugins/build/plugins#bundled-mcp-servers-and-lifecycle-hooks).

## Claude Code package

`plugin/claude-code/` registers four hooks that run the `scribe` binary on
`PATH`, and the `/scribe` command.

- `SessionStart` runs `scribe hook session-start`. It tells you how many
  proposals are open and how many open items wait for you (when nonzero)
  and, in an interactive session, gives the agent the brief
  (`scribe brief`: memory, the selected project's most recently opened group
  when one exists, `## Open items` across projects, the last sessions, and
  every open claim by tag; JSON adds `open_items`). Plain
  output omits the `Last group` section and JSON omits `last_group` when that
  project has no group.
- `UserPromptSubmit` runs `scribe hook user-prompt-submit`. It echoes each
  `#scribe` marker of the typed prompt (code fences, blockquotes, pasted
  blocks and quoted mentions are skipped), applies each whole-line
  `#scribe approve|reject|retire|forget`, and replies with what it marked,
  settled or refused.
- `Stop` runs `scribe hook stop`. It hands the session's open markers to the
  agent, with the `entry add` command that records each, echoes each to you,
  and fills the message id of an older entry's cite. A line an older store
  already recorded verbatim is not handed over again, and settling commands are
  not replayed. For a session that is on, Stop also marks a pass due on the
  `[catalog]` cadence in `config.toml`.
- `SessionEnd` runs `scribe hook session-end`, which marks the last pass due
  and final.

A prompt whose first line is `/scribe on`, `/scribe off` or `/scribe status`
is answered by `UserPromptSubmit` and never reaches the model. So are
`/scribe on --from <id>`, `/scribe link <id> [--type continues]`, `/scribe catalog <id>`,
and `/scribe group <slug>` (also `/scribe:scribe …`). The link forms connect
this session to the one it was spawned from or continues, and refuse an
unknown id, a self-link or a cycle. `/scribe group <slug>` creates an open
`project` group titled `<slug>` in the working directory's project if absent and
joins the calling session on an automatic seat: the first eight ID characters,
then the full ID, then the first available numbered name. Numbered names use
the first 116 ID characters plus `-2`, `-3`… (or the whole ID when shorter),
so a 128-character ID of `a`s gets 116 `a`s plus `-2` when its short and full
names are already taken. A join never displaces a seat and uses role `member`;
when typed from another session, the command joins the existing open group
(one group per session until `scribe group leave`; non-typed prompts are
refused). `/scribe group add <slug> <session-id>...` adds only the named
sessions to the group, creating it when absent. The session typing the line is
included only when its ID is explicitly named. For each named session, scribe
checks and reads a transcript on disk before storing its typed turns; if no
transcript exists, the session gets membership only.
If a present transcript fails its checks or cannot be read, the hook refuses
the whole line before writing membership; it does not treat that transcript as
absent. A Codex transcript that cannot be parsed also refuses the line. Claude
Code skips lines that are not valid JSON, so a transcript with no parseable
lines can still be added with zero typed turns. A session already in another
group also refuses the whole line. It is a typed line only; there is no
terminal command. Codex accepts the
same `/scribe group <slug>`, `$scribe group <slug>`,
`$scribe group add <slug> <session-id>...`, and skill mention link `[$scribe](.../scribe/SKILL.md) group <slug>` once the host
rollout verifies the typed turn. After a desktop clear, the transcript names
the previous session. When its newest active handoff has mode `continue`, the
hook puts that body before the brief, carries the recording switch and writes
a `continues` link. If the ancestry is missing, type
`/scribe continue <handoff-id>`. A `new` handoff starts with the ordinary
brief. No hook calls a model. The pass does.
Each write and refusal leaves one audit line in `scribe.db` with ids, the
`--author` name and error codes only.

Install from this checkout after `make install`:

```sh
claude plugin install scribe --marketplace ./plugin
```

[`plugin/.claude-plugin/marketplace.json`](../plugin/.claude-plugin/marketplace.json)
is the local marketplace that command adds. `claude plugin validate plugin/claude-code`
checks the package. An install after a plugin change runs:

```sh
claude plugin marketplace update scribe-local
claude plugin update scribe@scribe-local
```

`make update-plugins` runs those two commands and the Codex
`codex plugin add scribe@scribe-local --json` in one step.

Orbit's copy of the plugin is installed from outside the repository. The
commands and the reason are in the
[plugin README](../plugin/.orbit-plugin/README.md#install-operator).
`claude plugin update` applies on the next Claude Code restart.

The package ships the same skill, which teaches the CLI above. The skill is
for sessions on the machine where the data directory lives.

## Settings

Each variable scribe reads by name is listed here, generated from its
declaration in `scribe_core::composition` (`make goldens` fails when this copy
drifts). The default model credentials are listed. Other names that
`config.toml` sets, such as further model credentials and `[redact] env`, are
read as configured and are not listed.

<!-- env-table:start -->
| Variable | Meaning | Default | Flag |
|---|---|---|---|
| `SCRIBE_ROOT` | The data directory. | `$HOME/.<app>` | `--root` |
| `SCRIBE_FORMAT` | The output format: `json` for one JSON document, `table` for a table, and `auto` for a table on a terminal and tab-separated lines otherwise. | `auto` | `--format` |
| `SCRIBE_LOG` | The log filter for stderr, in `tracing` syntax such as `debug`. | `warn` | — |
| `NO_COLOR` | Any non-empty value turns color off. | — | — |
| `CLICOLOR_FORCE` | Any non-empty value turns color on for a terminal, even a `dumb` one; `NO_COLOR` still wins. | — | — |
| `TERM` | The terminal type, such as `dumb`. | — | — |
| `COLUMNS` | The width in columns that a table must fit, when it is a positive number. | — | — |
| `HOME` | The home directory; the default data directory is under it. | — | — |
| `CLAUDE_CONFIG_DIR` | Where Claude Code keeps its state, and so its transcripts under `projects`. | `$HOME/.claude` | — |
| `CODEX_HOME` | Codex's state directory. Codex transcripts are read from its `sessions` and `archived_sessions`. | `$HOME/.codex` | — |
| `CLAUDE_CODE_ENTRYPOINT` | Set by Claude Code on its hooks: `cli` in an interactive session, `sdk-cli` for `claude -p`. Only `cli` and `claude-desktop` get the memory page at session start. | — | — |
| `CLAUDE_CODE_SESSION_ID` | The Claude Code session a command runs in. `group` names it as the caller, and `entry add` reads it for a quote-less refresh. | — | — |
| `CODEX_THREAD_ID` | The Codex session a command runs in, which `group` names as the caller for Codex. | — | — |
| `PATH` | The directories searched for the `claude` or `codex` program. `init` refuses a runtime that is not executable on them. | — | — |
| `LANG` | The locale a model call runs under. | — | — |
| `CLAUDE_CODE_OAUTH_TOKEN` | The Claude Code sign-in token, the first default model credential: a Claude model call gets it in its environment. | — | — |
| `ANTHROPIC_API_KEY` | An Anthropic API key, the second default model credential for Claude. A model call gets it in its environment. | — | — |
| `OPENAI_API_KEY` | The OpenAI API key, the default model credential for Codex. A Codex model call gets it in its environment. | — | — |
| `COMPUTERNAME` | The host name on Windows, which records carry as their host label. | — | — |
<!-- env-table:end -->

`auto` is a table on a terminal and tab-separated lines otherwise. Color is
off in a pipe, when `NO_COLOR` is non-empty, and for `TERM=dumb`. A non-empty
`CLICOLOR_FORCE` overrides `TERM=dumb` and never overrides `NO_COLOR` or a
pipe.

Exit status is 0 on success, 1 when a command fails and 2 on a usage error.
`--json` is the contract in the
[output spec](design/scribe/specs/output-contract.md): a list is
`{"<entries|sessions|proposals|tags|hits|transcripts>": [...], "total": N, "truncated": false, "warnings": [...]}`,
one record is an object, and an error in JSON mode is one `{"error", "code"}`
object on stderr.

## Where this repository came from

This repository was created from the constellation Rust CLI template, either
with GitHub's "Use this template" on `constellation-works/rust-cli-template`
followed by `scripts/rename-template.sh <name>`, or inside the constellation
umbrella by `operations/scripts/new-rust-cli.sh`. The template's note example
has been replaced by scribe's domain. The structure, gates and tests remain
the pattern (STD-01 CLI surface, STD-02 Rust architecture, STD-03 concurrency
and process safety, STD-04 testing and verification, STD-05 security
boundaries), vendored under [docs/standards/](standards/README.md).

## Layout

[ARCHITECTURE.md](../ARCHITECTURE.md) is the layer list and the dependency
rules. The top of the tree:

```text
crates/scribe-core/    domain library, including store/
crates/scribe/         the scribe binary
plugin/claude-code/    Claude Code hooks and the /scribe command
plugin/codex/          Codex plugin
plugin/.orbit-plugin/  Orbit plugin (Claude-oriented)
plugin/orbit-codex/    Orbit plugin variant for Codex model calls
skills/scribe/         canonical skill, embedded by scribe init
docs/design/scribe/    design, decisions and specs
scripts/               gates run by make ci-fast (structure, README links)
```

## Development

```sh
make ci-fast          # fmt, structure checks, README links, standards-check; no compile
make ci-lint          # clippy -D warnings, plus cargo-deny
make test             # nextest when installed, else cargo test; then doctests
make goldens          # compare help and output with the built binary
make goldens UPDATE=1 # regenerate after an intended surface change
make ci               # all of the above; what CI runs
```

`cargo-deny` and `cargo-nextest` are optional locally (the Makefile warns and
skips or falls back). CI installs both at pinned versions, and there a missing
tool fails. `make standards-check` runs `docs/standards/check.sh`.

Every line in the root [README](../README.md) and every command example in this
guide that starts with `scribe ` is parsed by
`every_documented_command_line_parses` (`crates/scribe/src/tests/cli.rs`), so
those lines have to stay real commands. The same test parses the examples in
`--help`.
