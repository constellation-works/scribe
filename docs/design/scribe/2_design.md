---
title: scribe — Design
owner: claude
last_updated: 2026-11-21
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: design
type: design
summary: How the scribe plugin is bounded (one binary; Daniel writes through hooks, agents read and write through the CLI taught by one skill, and the MCP surface stays ask), how #scribe entries, /scribe on session records (a rewritable header over an append-only log of segments), active and inactive sessions, ask and search, the consistency routine, backfill by name and the tmux popup work, where everything is stored, which model does what, and the slice plan that builds it; §11–§13 are proposed designs (claims and issues, settling on instruction, and the descriptive memory written from /scribe on sessions; its personal base for sessions that are off, people, recurring dates and light catalog entries are designed but parked 2026-11-21, and scribe reads Claude Code and Codex sessions only); §14 proposes `#scribe` as an annotation anywhere in a typed turn, cataloged at the end of the turn with a statement and a verbatim quote.
tags: [scribe, plugin, architecture]
paths: ["crates/**", "plugin/**", "scripts/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21778, ORB-21784, ORB-21787, ORB-21795, ORB-13117, ORB-13134, ORB-23849, ORB-23892, ORB-23995, ORB-24005, ORB-26214, ORB-26204, ORB-26219, ORB-26240, ORB-26256, ORB-26349, ORB-26265, ORB-26510]
---

# scribe — Design

The design of the scribe plugin: its boundary, the four parts named in
[1_overview.md §2](./1_overview.md#2-shape), the store, the model choices,
and the slices that build it. Slices S1 (the domain, the store and the
read-only CLI), S2 (`#scribe` capture without a model, and the Claude Code
package), S3 (referents and the models port), S4 (`/scribe on` and
session records), S5 (the Orbit plugin tree and `ask`) and S6 (the
consistency routine and the settling verbs) are implemented; S7
(discussions) is not yet. S8–S14 ([ORB-23892], designed 2026-11-10; see the slice-specific
As built notes) turn a session record into a header over an append-only log, add the
`active` flag, give agents a CLI read and write surface taught by one skill,
add a tmux popup for Daniel, and catalog chosen past sessions by name. The reasons behind each choice
are in [4_decisions.md](./4_decisions.md); open directions are in
[3_vision.md](./3_vision.md). Where this doc and the template's engineering
rules meet (atomic writes, the store lock, the audit guard, output modes,
gates), the template's mechanisms carry over unchanged and are not restated:
see [ARCHITECTURE.md](../../../ARCHITECTURE.md) and the
[store-module decision](./4_decisions.md#persistence-is-one-module-inside-the-core-crate).

## 1. Boundary and surfaces

One Rust binary, `scribe`, has three entry points. Each is a noun under the
STD-01 grammar:

| Entry point | Called by | Does |
|---|---|---|
| `scribe hook <event>` | Claude Code hooks (the `plugin/claude-code/` package) | Reads the hook's JSON on stdin; captures `#scribe` instructions, switches cataloging on and off, marks session passes due. The only path that writes Daniel's entries (`author.kind: daniel`) and settles proposals about them. |
| `scribe plugin call` | Orbit's exec backend (`plugin/.orbit-plugin/`) | Reads the Orbit tool request on stdin, writes the `{"ok", …}` envelope on stdout. Serves `scribe.ask` and the routines' tools. |
| `scribe <noun> <verb>` | Daniel at a terminal, scripts, and agents through a shell (from S11) | Read-only `entry list/show`, `session list/show`, `proposal list`, `tag list`, `question ask`, plus `session pass` (Daniel runs it by hand) and `consistency scan` (its routine's job). From S8–S14: `record search`, list filters, the agent write verbs ([§1.1](#11-the-cli-agent-surface)), `session rebuild`, `session activate/deactivate`, `session catalog`, `transcript list/show` and `tmux` (the tmux popup, [§1.2](#12-the-tmux-popup)). |

**The Orbit plugin tree** is `plugin/.orbit-plugin/`: `plugin.yaml`
(`schemaVersion: 2`, `kind: Plugin`, `metadata.name: scribe`), `bin/scribe`
(a launcher that execs the installed binary as `scribe plugin call`), `schemas/`
and `definitions/` (the two routines and their jobs). The backend is
`type: exec`, `timeout_ms: 720000`: the routine tools may need 600 s (five
sessions at 120 s) and 480 s (four consistency batches at 120 s), and `ask`
is bounded by its own 90 s budget. Tools:

| Tool | `execution_kind` | `agent_callable` | `mcp_scope` |
|---|---|---|---|
| `scribe.ask` | `read_only` | — | `global` |
| `scribe.session-pass` | `mutating` | `false` | `none` |
| `scribe.consistency-scan` | `mutating` | `false` | `none` |

Orbit defaults `mcp_scope` to `workspace`, so every tool except `ask` sets
`none` explicitly, and a conformance test in `plugin/.orbit-plugin/tests/`
asserts that `ask` is the only advertised tool. Main agents therefore see
exactly `mcp__orbit__scribe_ask` once orbit-office's scribe is retired (see
*Outside this repository* in [§10](#10-slice-plan)); until then its
`office.scribe-*` tools are on the list too. `ask` is `global` because the
store is Daniel's, not a workspace's. The two mutating tools run only from the
plugin's routines (Runner capability). Permissions: `fs.read` and `fs.write`
on the store root, `network: any` for the model, `env_pass` for the model
credential names ([§8](#8-models)), and `orbit_tools: [orbit.search,
orbit.task.show]` (read-only) for `ask`'s task evidence.

**The Claude Code package** is `plugin/claude-code/`
(`.claude-plugin/plugin.json`, `hooks/hooks.json`), installed with
`claude plugin install`. It registers `UserPromptSubmit`, `Stop`,
`SessionStart` and `SessionEnd`, each running `scribe hook <event>` with a
10-second timeout (30 seconds for `UserPromptSubmit`, which may call Haiku,
[§4.3](#43-resolving-this-and-it)). It ships no MCP server and no slash
command file: `/scribe` is intercepted by the `UserPromptSubmit` hook
([§5.1](#51-switching-on-and-off)). Orbit's plugin manifest has no hook or
command key, which is why this is a separate package
([4_decisions.md](./4_decisions.md#hooks-call-the-scribe-binary-directly-not-orbit)).
As built in S2, it registers `UserPromptSubmit` (30 s) and `Stop` (10 s)
only; `SessionStart` and `SessionEnd` join with the slices that give them
something to do (S4, S6), since a registered hook whose command does not
exist fails on every session. `plugin/.claude-plugin/marketplace.json` is a
one-plugin local marketplace, so the package installs from a checkout with
`claude plugin install scribe --marketplace ./plugin`.
As built in S4, `SessionEnd` (10 s) joins them, and the package ships one
command file, `commands/scribe.md`, whose body is `/scribe $ARGUMENTS` and
whose only effect is the same hook call: Claude Code knows the command, and
whichever form reaches the hook, typed or expanded, starts with `/scribe`
([4_decisions.md](./4_decisions.md#scribe-ships-a-command-file-that-expands-to-itself)).
As built in S6, `SessionStart` (10 s) joins them: `scribe hook session-start`
prints `scribe: <n> proposals to review — scribe proposal list` as a
`systemMessage` for Daniel when any proposal is open, adds one line
counting action entries left unpicked for over an hour
([§4.6](#46-action-tags)), and, when this session is not opted in and a
session in the same project was recorded within the last day, adds
`scribe: this may continue <id> (recorded) — type /scribe on --from <id> to keep recording`
([§5.4](#54-session-links), [§7](#7-consistency-routine)). It prints nothing
otherwise. It reads only; it does not create `config.toml`
([§3](#3-storage)).
(Proposed, not built: it also gives the agent the memory page as
`additionalContext` in interactive sessions,
[§13.6](#136-reading-back). Recurring dates computed into its `now` part,
[§13.14](#1314-recurring-dates), are parked.)

Entry writes never go through Orbit's tool surface: the hooks call the
binary, and from S12 agents call the CLI ([§1.1](#11-the-cli-agent-surface)). The two routine tools write session records and proposals, but they
are `mutating` and not agent-callable, so no tool needs `agent_callable` or an
operator override, and the MCP list stays at one tool.

As built (S5): the plugin tree is `plugin.yaml`, `bin/scribe-plugin` (a POSIX
sh launcher that execs the `scribe` on `PATH` as `scribe plugin call`, and
answers a `not_installed` envelope when there is none), `schemas/` (the
config, `ask`'s input and output, and placeholders for the routine tools),
`tests/conformance/scribe.yaml` (the `orbit plugin test` goldens) and
`tests/conformance.rs`, the manifest contract test, compiled into
`crates/scribe/tests/plugin.rs`. The three tools are declared as the table
says; `scribe.session-pass` and `scribe.consistency-scan` answer
`not_implemented` until S6 wires them with the routines, so there is no
`definitions/` yet. Three departures, each in
[4_decisions.md](./4_decisions.md#the-plugin-backend-serves-ask-alone-until-s6):
`timeout_ms` is 150000, since Orbit caps it at 300000 and only `ask` is live
(30 s of task reads, then 90 s of model); the store root is the plugin's
`[plugins.scribe] root` setting, an absolute path, granted `fs.read` only
(the write grant is `{{plugin_state}}`, unused), and `ask` answers
`not_configured` until it is set; and task evidence comes from the workspace
the call is bound to unless `task_workspaces` names others. `requires.programs`
is `[scribe, claude]`. `plugin call` reads the envelope, runs `scribe.ask`
through the same code as `question ask`, and always prints one envelope line
and exits 0: errors are `invalid_envelope`, `unknown_tool`, `invalid_input`,
`not_configured`, `not_implemented`, a model code (`model_timeout` and
`model_failed` retryable) or a store code. It is read-only and not audited.

As built (S6): the two routine tools are wired to the CLI's code, and the
tree gains `routines/session_pass.yaml` and `routines/consistency.yaml`
(seeded by `orbit plugin enable` as `scribe-session-pass`, `*/5 * * * *`,
and `scribe-consistency`, `0 6 * * 1`, both `enabled: false`) and
`jobs/session_pass.yaml` and `jobs/consistency.yaml` (`scribe-session-pass`
and `scribe-consistency`, one `plugin.tool_call` step each, 300 s), listed
under `spec.definitions` rather than in a `definitions/` directory.
`scribe.session-pass` runs `session pass` and `scribe.consistency-scan`
runs `consistency scan`, each answering the report its `--json` prints
(`schemas/routine.output.json`). Departures, each in
[4_decisions.md](./4_decisions.md#the-routine-tools-start-no-unit-after-a-deadline-and-audit-inside-the-call):
`timeout_ms` is 300000, Orbit's cap, not 720000, so a tool call starts no
session after 50 s and no batch after 170 s (the first always starts) and
leaves the rest for the next run; the store root is granted `fs.write` as
well as `fs.read`, and a new `transcripts` setting (Claude Code's projects
directory, absolute) is granted `fs.read` and must be set for
`scribe.session-pass`; the `root` default is `/var/tmp`, which means unset,
because Orbit grants writes only to a directory that exists outside the
install tree; and the routine tools are audited inside the call, in the
store their envelope names, under the CLI verbs' command names
(`plugin call` itself stays unaudited). The tools take only `{}`.

### 1.1 The CLI agent surface

Designed 2026-11-10 ([ORB-23892]); built by S11, S12 and S13. Daniel: "I
don't want to keep having to manually search for specific session id's to
hand to agents; and I am not sure about adding another set of MCP tools. so
I think a single scribe skill that teaches terminal commands will suffice."
Agents "should able to filter sessions by topics, tags, bm25 search, date
range, add/update proposals, tags and entries directly", and `scribe.ask`
stays for intensive and vague searches.

The MCP surface does not change: `scribe.ask` is still the one advertised
tool. Agents with a shell use the CLI, and one skill teaches it
([4_decisions.md](./4_decisions.md#agents-read-and-write-through-the-cli-taught-by-one-skill)).

| Want | Command | Model |
|---|---|---|
| Words to look for | `scribe record search "<terms>" [--kind entry\|session\|discussion] [--project <p>] [--topic <t>]… [--since <date>] [--until <date>] [--include-hidden] [--limit <n>]` | none ([§6.8](#68-record-search)) |
| Records by metadata | `entry list` and `session list` with `--since <date>`, `--until <date>`, `--status <status>`, repeated `--topic <t>` (every one must match) and repeated `--related <kind:target>` (any may match); `entry list` also `--author <name>` and `--author-kind daniel\|agent`; `session list` also `--include-inactive`; `proposal list` with `--state`, `--since`, `--until`, `--entry <id>` and `--author-kind daniel\|agent\|scribe` | none |
| One record | `entry show <id>`, `session show <id> [--log] [--segment <n>]`; `session show session:<id>#<n>` shows one segment | none |
| An answer in prose | `scribe question ask "<question>"`, or the `scribe.ask` MCP tool | decision model |
| Record a note | `scribe entry add --tag <tag> --author <name> [--topic <t>]… [--related <r>]… [--project <p>] <content>` | none |
| Change a note | `scribe entry update <id> --author <name> [--content <text>] [--add-topic <t>]… [--remove-topic <t>]… [--add-related <r>]… [--remove-related <r>]…`; `scribe entry retire <id> --author <name> --reason <text>` | none |
| A tag | `scribe tag add <name> --author <name> --description <text> [--alias <a>]… [--stale-after-days <n>]`; `scribe tag update <name> --author <name> [--description <text>] [--add-alias <a>]… [--remove-alias <a>]… [--stale-after-days <n>]` | none |
| A proposal | `scribe proposal add --kind supersedes\|stale --older <id> [--newer <id>] --reason <text> --author <name>`; `scribe proposal update <p-id> --author <name> --reason <text>`; `scribe proposal settle <p-id> --approve\|--reject --author <name> [--reason <text>]` | none |
| A session's relevance | `scribe session deactivate <id> --author <name> --reason <text> [--cite <ref>]`; `scribe session activate <id> --author <name> [--pin\|--unpin] [--reason <text>]` ([§5.7](#57-active-and-inactive-sessions)) | none |

Rules every verb keeps:

- **Ids.** Every command that takes an id takes a unique prefix of it (at
  least four characters). An exact id wins over a prefix. An ambiguous
  prefix is refused with exit 2 and `id_ambiguous`, listing up to ten
  candidates; an input that is not an id and is shorter than four
  characters is a usage error (exit 2) for every noun; no match is the
  noun's `*_not_found` (exit 1), whose message names the id and the store
  path searched (`entries/…`, `sessions`, `proposals.json`, the
  transcripts directory) so a wrong `--root` shows.
- **Argument checks come first.** A write verb refuses a bad argument
  (unknown tag, blank content or reason, an id that matches nothing, a
  repeated tag) before it reports an unset `[redact] env` name, so the
  first error is about what the caller can fix in the command line.
- **Authors.** Every write verb requires `--author <name>` (lowercase
  `[a-z0-9._-]`, 1–40 characters, by convention the agent's crew or model,
  such as `opus`) and records `{kind: agent, name}`. `kind: agent` means
  "written through the CLI"; Daniel's channel is still `#scribe`, which
  records `kind: daniel` ([4_decisions.md](./4_decisions.md#every-entry-and-proposal-records-its-author)).
  The names `daniel`, `scribe` and `consistency` are reserved for the
  non-agent channels: an agent write using one is refused
  `reserved_author` (reads still accept them as filters), with or without
  a terminal: no CLI write records `kind: daniel`. Daniel catalogs a past
  session with `/scribe catalog <id>` in his typed prompt
  ([§5.8](#58-cataloging-a-past-session),
  [4_decisions.md](./4_decisions.md#daniel-writes-only-through-his-typed-prompt-agent-names-are-attribution)).
- **The guard.** `ask` never presents an agent's entry as Daniel's words
  and gives it no 1.5× weight ([§6.7](#67-segments-authors-and-inactive-sessions)).
  Agents never change the content of, retire or settle a proposal about an
  entry Daniel wrote: `entry update` of a Daniel entry may change only its
  topics and related references; `entry retire` and `proposal settle` are
  refused `daniel_only` when the entry (the proposal's `older`) is
  Daniel's. Agents may open a proposal against a Daniel entry; Daniel
  settles it with `#scribe approve|reject`. `#scribe forget` stays
  Daniel's alone ([4_decisions.md](./4_decisions.md#entries-daniel-wrote-are-settled-only-by-daniel)).
  Built since: with a checked instruction of his, an agent may settle a
  candidate and retire or forget his entry
  ([§12](#12-settling-and-deleting-on-daniels-instruction-proposed)); a
  proposal about his entry stays his to settle.
  Between agents, author names are attribution only: any agent may update
  or retire another agent's entry, update an open agent proposal or settle
  one, including its own, and the record keeps who changed it
  (`updated_by`, `settled_author`). A non-active agent entry refuses
  updates (`entry_not_active`). Settling with `--reason` replaces the
  proposal's reason, so on a proposal Daniel or scribe opened it is
  refused `proposal_not_agent`; settling without `--reason` keeps the
  original. Tags stay shared vocabulary: any agent may update a store
  tag, recorded in `updated_by`
  ([4_decisions.md](./4_decisions.md#daniel-writes-only-through-his-typed-prompt-agent-names-are-attribution)).
- **Audit and redaction.** Each write verb is a mutating command: one
  audit line per change or refusal (ids and codes only), store lock,
  `write_atomic`, and every free-text argument passes the redactor before
  it is stored. A repeated write that changes nothing prints `nothing
  changed` and writes no audit line.
- **Output.** Each write prints the record it wrote in the `show` shape
  (`--json`: the record object), so an agent can read back the id.
- **Grammar.** Names follow STD-01 R1–R2: the search is `record search`
  (`record` is any stored record) rather than a bare `scribe search`,
  which would mix verb-first and noun-first orders; backfill candidates
  are `transcript list` and `transcript show`. `session rebuild` replaces
  a record's text, so it refuses without `--confirm` (STD-01 R5).

Division of work (Daniel confirmed 2026-11-10): scribe's own job is
summarizing and cataloging sessions, and its model work stays there; the
session pass keeps its model. Agents do the direct writes. scribe never
writes an entry, a tag or a proposal on an agent's behalf from a model
call.

**The skill.** One `scribe` skill ships in the Claude Code package as
`plugin/claude-code/skills/scribe/SKILL.md`. It teaches: which question
goes to `search` or a list (a known word, a date range, a topic, a task
id) and which to `ask` (vague, needs weighing or a synthesized answer);
`session show <id>` versus `--log`, and that segments cite as
`session:<id>#<n>`; the write verbs, `--author`, and the guard; that
session records are model summaries and agent entries are not Daniel's
words; and that `--json` is the stable machine form. A route for other
providers lives in the constellation umbrella at `.agents/skills/scribe/`
(outside this repository, [§10](#10-slice-plan)).

**Limit.** The store is on the Mac, so the skill serves sessions on the
Mac. Box workers still cannot read scribe
([§9](#9-concerns--honest-limitations)).

### 1.2 The tmux popup

Daniel: "we need a very simple desktop UI or tmux UI for humans. perhaps
tmux will be better - having another desktop app will be cumbersome."
`scribe tmux` runs the popup from inside the binary: a POSIX shell script
of about 150 lines, embedded at build time and run with `sh -c`, over
`fzf`. Nothing else is installed (an earlier standalone script had to be put
on `PATH` and never was), and its child commands run the executable that
was started, not whichever `scribe` is first on `PATH`. It needs `fzf` on
`PATH` and adds no Rust dependency. Bound to a tmux key, it opens a popup:

```tmux
bind-key S display-popup -E -w 90% -h 85% scribe tmux
```

- It lists sessions first (`session list`); `ctrl-s`, `ctrl-e`, `ctrl-p`
  and `ctrl-b` reload the list with sessions, active entries, open
  proposals or backfill candidates (`transcript list --since <30 days
  ago>`, [§5.8](#58-cataloging-a-past-session)). Rows are for reading, not
  for ids: a start date (`MM-DD`, or `YYYY-MM-DD` when the year is not the
  current one), then for a session its status (`open` green, `final`
  dimmed), its title and its project (dimmed), newest started first;
  an entry shows its tag and the first line of its text, a proposal its
  state and what it proposes, a candidate its project and first prompt.
  Inactive sessions (after `alt-a`) are dimmed. A list that is empty or fails is
  one notice row (id `-`) that says which ("no active entries; alt-a shows
  every status", "no open proposals"); its preview says the same and Enter and
  `ctrl-y` do nothing on it. The id is a hidden first
  field of the row (`kind<TAB>id<TAB>display`, fzf `--with-nth=3`) so the
  preview, Enter and `ctrl-y` act on the same record; topics follow the
  columns past any terminal width, where `--no-hscroll` never shows them but
  fzf still searches them. Rows are built in Rust, from the same list
  commands' records, by a hidden `scribe tmux rows <view>` mode.
- The preview pane is a hidden `scribe tmux preview <kind> <id>`: for a
  session its title and summary, then goals, next steps, metadata and log;
  for an entry its text followed by the same metadata as `entry show`;
  for a proposal what it proposes, its reason, then its fields; a
  candidate is `transcript show --format table`. Enter opens the whole
  record with `--format table` (`--log` for a session), because `show`
  prints only its list line when stdout is not a terminal.
- Enter opens the full record in `$PAGER` (else `less`).
  On a candidate, Enter puts `/scribe catalog <id>` in the tmux paste
  buffer and says so; typed into a Claude Code prompt, it catalogs the
  session as Daniel's ([§5.8](#58-cataloging-a-past-session)).
- `ctrl-y` copies the id: `tmux set-buffer`, and `pbcopy` when present.
- `alt-a` toggles inactive sessions and hidden entries (adds
  `--include-inactive` to the session list and drops `--status active`
  from the entry list).
- Without `fzf` it exits 1 with an install hint (`fzf_missing`).

It reads only: `Command::audit` is `None`, it never opens the store, and it
passes the data directory to its children as `SCRIBE_ROOT` (and drops
`SCRIBE_FORMAT`, which would reshape the lists it parses). A real TUI is
built only if this proves too small ([3_vision.md](./3_vision.md)).

## 2. Layering

The template's two crates and tiers stay
([4_decisions.md](./4_decisions.md#a-domain-library-crate-and-a-cli-crate)).
The note domain is deleted in S1 and replaced by:

| Module (`scribe-core`) | Owns | I/O |
|---|---|---|
| `entry` | `Entry`, `EntryId`, `TagName`, `Cite`, entry status; the next-id rule | none |
| `tag` | the tag table and alias resolution | none |
| `capture` | parsing a typed turn into `#scribe` instructions; tag resolution; the referent request and its validation | none |
| `session` | `SessionRecord`, the control state, the due rule | none |
| `proposal` | `Proposal`, its id, kind, state and quotes | none |
| `record` | the metadata every record shares: `Project`, `Topic`, `Related`; list selection (`Listing`) | none |
| `config` | parsing `config.toml` text into tags, models, cadence and redaction settings | none (takes text) |
| `transcript` | parsing a Claude Code transcript (JSONL) into typed messages, dropping tool payloads and synthetic turns | none (takes bytes) |
| `verbatim` | the one quote check ([§6.4](#64-the-verbatim-check)) | none |
| `ask` | tokenizing, ranking, evidence packing, answer validation | none |
| `consistency` | candidate pairing, staleness, proposal validation | none |
| `models` | the `Models` port (a trait) and the typed requests/responses for each use | none |
| `store` | the `Store` facade over `format`, `audit`, `fsio`, `redact`; the only disk access | disk |

The CLI crate adds `commands/{entry,session,proposal,tag,question,consistency,plugin}.rs`
and `commands/hook/` (one file per event), plus two
adapters composed in `app.rs`: `ClaudeCliModels` (the `Models` port,
[§8](#8-models)) and `OrbitTasks` (task evidence for `ask`). Adapters that
spawn processes live in the CLI crate so the core stays free of
`std::process`, as the dependency-direction script already requires. The
adapters are a new module family; the slice that adds the first one adds
them to `ARCHITECTURE.md` and the script. S1 built `entry`, `tag`,
`session`, `proposal`, `record`, `config`, `verbatim` and `store` and the
`entry`, `session`, `proposal` and `tag` commands. S2 built `capture`,
`transcript`, the store's first writes (`Store::capture`,
`Store::fill_cite_messages`) and `commands/hook/` (`user_prompt_submit.rs`,
`stop.rs`, plus `payload.rs` for the stdin shapes and the provenance check
and `project.rs` for the project lookup). S3 built `models` (the `Models`
port, its referent request and answer, and `ModelError`), form 3 in
`capture`, and the first adapter, `ClaudeCliModels` in
`crates/scribe/src/models.rs`, which `app.rs` composes and the
`user-prompt-submit` hook calls; `ARCHITECTURE.md` and the dependency script
name it as the only module that spawns a process. The config's model ids
are `ModelIds`, so the port alone is `Models`.

S8–S14 add no crate and no process-spawning module, except that `tmux` (the tmux popup) adds `popup.rs`, the adapter that runs its embedded script on the caller's terminal. In the core, `session`
gains the header, the `Segment` log and the sealing rule; `record` gains
`Author` (`{kind, name}`) and the date and related filters of `Listing`;
`activity` (new, no I/O) holds the `active` flag, the age rule and the
removed-feature candidate pairing; `ask` exposes its tokenizer and BM25
ranking to `search` (new, no I/O), which ranks without a model; `tag`
merges config tags with the store's `tags.json`; `transcript` gains the
model-free statistics `transcript list` reports (Daniel's turns, size,
path-mention counts). The CLI gains `commands/record.rs` (`record search`),
`commands/transcript.rs`, the write verbs in `commands/{entry,tag,proposal}.rs`
and `commands/session/{rebuild,activate,catalog}.rs`, and an id-prefix
resolver shared by every command that takes an id. The tmux script and the
skill are files in `plugin/`, not code.

## 3. Storage

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`, as in the template. The
hooks, the CLI and the Orbit backend resolve the same root, so one store
serves every workspace on the host. The plugin's config key `root` (default
`~/.scribe`) is passed to the backend as `--root`.

```
~/.scribe/                       0700, owner-checked on every load
├── config.toml                  tags, models, cadence (Daniel edits; read-only to scribe)
├── entries/<tag>/<id>.md        one file per entry: YAML frontmatter + verbatim body
├── sessions/<yyyy>/<mm>/<session-id>.md   one per opted-in session: frontmatter + header sections + ## Log (S8)
├── discussions/<yy-mm>/<slug>.md   one per distilled discussion (§4.5), Daniel may edit
├── control.json                 {"format":4,"sessions":{"<id>":{"on","since","due","final","consecutive_failures","retry_after","pending_links","cataloged_by"}}}
├── proposals.json               {"format":4,"proposals":[…]}
├── tags.json                    {"format":4,"tags":[…]} agent-created tags (S12); optional
├── pickups.json                 {"format":8,"pickups":[…]} consumers' pickups of action entries (§4.6); optional
├── audit.jsonl                  one line per mutating command (template shape)
└── .lock
```

Every file except `audit.jsonl` is replaced whole through
`fsio::write_atomic` under `.lock`; nothing is appended or edited in place.
A `#scribe` creates one new entry file; a session pass replaces one session
file.
Reads take no lock and write nothing (STD-01 R31), so `ask` works on a
read-only root. Each document now writes `format: 3`. Format 1 remains readable: its
control switches gain `consecutive_failures: 0` and `retry_after: null` in
memory. Format 2 remains readable too: its session records gain an empty
`links` and its control switches an empty `pending_links` in memory
([§5.4](#54-session-links)). The next mutation writes format 3. Entry and
proposal shapes are unchanged. The template's `notes.json` is not carried over.

**Format 4** (S8, [ORB-23892]) changes four shapes in one bump, appended as
one step to `store/format.rs::UPGRADES`
([4_decisions.md](./4_decisions.md#store-format-4-carries-the-log-active-and-author-in-one-step)):
a session record becomes a header over an append-only `## Log` of segments,
with `active`, `keep_active`, `active_change` and `cataloged_by`; an entry
gains `author` and `updated_by`; a proposal gains `author` and
`settled_author`; a control switch gains `cataloged_by`. `tags.json` is
new. The 3-to-4 step converts in memory, as the earlier steps do: a
session's `## Summary` stays the header summary, its summary and
`## Outcome` become sealed segment 1 (`origin: upgrade`), and it is
`active: true`; every existing entry is `author: {kind: daniel, name:
daniel}`, since every entry so far came from a typed `#scribe` line; every
existing proposal is `author: {kind: scribe, name: consistency}`, and a
settled one `settled_author: {kind: daniel, name: daniel}`. The next write
of each document persists format 4; an older build refuses it. A record
whose transcript still exists can then be rebuilt into real segments
([§5.6](#56-rebuilding-a-record)).

**Metadata.** Entries and session records are Markdown files whose YAML
frontmatter is the metadata `ask` and the CLI filter on, so a lookup narrows
by fields before any ranking. Every record carries:

| Field | Meaning |
|---|---|
| `format`, `kind` | `3` (`4` from S8); `entry` or `session` |
| `id` | `preferences-12`, or the session id |
| `status` | entry: `active`, `superseded`, `retired`, `forgotten`; session: `open`, `final` |
| `project` | the repository slug from the cwd's git remote, else the cwd's basename; `global` when the line opens with `#scribe/global` instead of `#scribe` ([§4.2](#42-parsing-an-instruction)) |
| `topics` | 0–5 lowercase kebab-case labels (`ci`, `crew-routing`, `release`) |
| `related` | references the record mentions: `task:ORB-123`, `pr:<owner>/<repo>#7`, `repo:<slug>`, `session:<id>` (model-extracted for a session) |
| `links` | sessions only: `[{type, session}]`, `type` `spawned_from` or `continues`; the session structure Daniel asserted ([§5.4](#54-session-links)), stored on the child only |
| `terms` | sessions only: phrases Daniel coins or defines, each `{term, said, message}` where `said` is a verbatim span of Daniel's own turn that defines or first uses it |
| `provider`, `host` | where it was recorded |
| `author` | entries and proposals, from S8: `{kind, name}`; `kind` is `daniel` (a typed `#scribe` turn), `agent` (a CLI write verb, `name` from `--author`) or `scribe` (scribe's own routine, proposals only) |
| `active` | sessions only, from S8: `true` while the record still describes something current ([§5.7](#57-active-and-inactive-sessions)); distinct from `status` |
| `created`, `updated` | RFC 3339 UTC |

`topics` are filter labels and are distinct from a tag, which names where an
entry goes (`preferences`). An entry's topics are the extra
hashtags on its `#scribe` line (`#scribe add this to preferences #ci`) and
nothing else, since capture calls no model. A session's topics come from
the `session` model at each pass, which is given the existing topic list and
told to reuse it before coining one. The consistency routine proposes
merging near-duplicate topics (`ci` and `continuous-integration`) as it does
superseding entries; nothing is renamed without Daniel's approval.

**Entry** (`entries/<tag>/<id>.md`):

```markdown
---
format: 4
kind: entry
id: preferences-12
tag: preferences
status: active
project: orbit
topics: [merge-policy]
related: [task:ORB-123]
provider: claude-code
host: dk-server-1
author: {kind: daniel, name: daniel}
updated_by: null
created: 2026-11-02T19:05:41Z
updated: 2026-11-02T19:05:41Z
said: "#scribe add this to preferences #merge-policy"
content_from: {kind: message, role: assistant, message: <uuid>, at: 2026-11-02T19:04:11Z}
cite: {session: <session-id>, message: <uuid>, at: 2026-11-02T19:05:40Z, prompt_sha256: "…", line: 3, cwd: "…"}
superseded_by: null
review_after: 2027-05-01
---
squash-merge agent PRs; keep merge commits for releases
```

The body is `content`, verbatim.

- `said` is the full `#scribe` line exactly as Daniel typed it.
- `content` is what the entry records. `content_from.kind` is
  `instruction` when it is a verbatim substring of `said`, or `message` when
  it is a verbatim span of an earlier message the instruction pointed at
  ("this", "it"); then `role` says whose words it is, and `ask` must never
  present an assistant's words as Daniel's.
- `status` is `active`, `superseded`, `retired` or `forgotten`; `forgotten`
  keeps the id and `cite` but clears `said`, `content`, `evidence`, `as_of`,
  `quote`, `window` and `retirement_reason`
  ([4_decisions.md](./4_decisions.md#superseded-and-stale-entries-are-hidden-not-removed)).
- `review_after` comes from the tag's `stale_after_days` (180 days for
  `preferences`; none for a tag that sets no `stale_after_days`).
- Ids are `<tag>-<n>`, `n` being the tag's max + 1 under the lock: short,
  self-describing in an echo, and never reused.
- `author` (from S8) says who wrote the entry. A `#scribe` line writes
  `{kind: daniel, name: daniel}`. `scribe entry add` writes `{kind: agent,
  name: <--author>}`, `said: ""` (there is no typed line), `content_from:
  {kind: agent}` and `cite: null`; the agent's session, when it gives one,
  is a `related` `session:<id>`. `updated_by` is the author of the last
  `entry update` or `entry retire`, else `null`.
- `quote` and `window` (store format 8, [§4.6](#46-action-tags)) are `null`
  except on action entries: `window` is the transcript window a `#scribe`
  line under an action tag keeps, `quote` the human's correction an agent
  quoted under a tag with `require_quote`.
- `evidence` and `as_of` (store format 10, [§4.7](#47-findings)) are `null`
  except on a finding: the pointer its result can be checked against and
  the date the result was true. An assistant turn kept by approving a
  session-pass candidate is the second agent shape beside the direct CLI
  note: author `{kind: agent, name: assistant}`, `said` the canonical
  approving command, `content_from: message` (assistant) and the turn's
  cite; the persisted-shape check accepts exactly these two agent shapes.

**Session record** (`sessions/<yyyy>/<mm>/<id>.md`): the common
frontmatter plus `links` (sessions only; `related` says what a session
mentions, `links` the structure Daniel asserted, [§5.4](#54-session-links)),
`title`, `cwd`, `started`, `on_since`, `scribed_through`
(the last transcript message folded in), `passes` (count) and `next_open`
(the number of `next` items); `status: final` after the session-end pass.
Up to format 3 the body has four sections, `## Summary`, `## Goals`,
`## Outcome` and `## Next` (character budgets 1500/600/2500/1000). The four text fields are model-written
summaries, labelled as such in `ask` evidence, never quoted as Daniel's words.

From format 4 (S8) a session record is a **header** the passes rewrite and
a **log** they only append to
([4_decisions.md](./4_decisions.md#a-session-record-is-a-rewritable-header-over-an-append-only-log)).
Daniel, 2026-11-10: "limiting each session to 2500 is insufficient for long
sessions - e.g. after writing first version of the session using all 2500
characters, the next update must remove a part of the original. So over
multiple re-writes, the beginning session's context will get lost, defeating
the purpose." One record per session still holds: a long session grows its
log, it does not start a second record.

```markdown
---
format: 4
kind: session
id: 7c2b9d14-8e5f-4a3b-b1c6-2f9e8d7a6b02
status: open
active: true
keep_active: false
active_change: null
cataloged_by: null
project: scribe
topics: [session-records, backfill]
related: [task:ORB-23892]
links: []
terms: []
provider: claude-code
host: daniels-mac
created: 2026-11-10T02:11:09Z
updated: 2026-11-10T05:40:31Z
title: Session records with a log, an active flag and a CLI agent surface
cwd: /Users/daniel/workspace/constellation
started: 2026-11-10T02:10:44Z
on_since: 2026-11-10T02:11:09Z
scribed_through: <uuid>
passes: 6
next_open: 2
log:
  - {n: 1, heading: "Why one 2500-character outcome loses history", first_message: <uuid>, last_message: <uuid>, from: 2026-11-10T02:10:44Z, to: 2026-11-10T03:02:17Z, state: sealed, windows: 2, origin: pass, topics: [session-records], related: []}
  - {n: 2, heading: "Agents write through the CLI", first_message: <uuid>, last_message: <uuid>, from: 2026-11-10T03:04:50Z, to: 2026-11-10T05:39:58Z, state: open, windows: 1, origin: pass, topics: [agent-surface], related: [task:ORB-23892]}
---
## Summary
1. Why one outcome section loses a long session's start; the header-and-log shape. 2. Agents read and write through the CLI…
## Goals
…
## Next
- …
## Log
### 1. Why one 2500-character outcome loses history
…
### 2. Agents write through the CLI
…
```

- **Header**, rewritten by every pass: `title` (for the whole session, not
  the latest window; 120 characters), `## Summary` (at most 1200
  characters, written as a table of contents: what the session covered, in
  order, pointing at segment numbers), `## Goals` (600) and `## Next`
  (1000, a bullet list), plus the header's `topics`, `related` and `terms`.
  There is no `## Outcome`: what happened is the log's job
  ([4_decisions.md](./4_decisions.md#outcome-folds-into-the-log)).
- **Log**, appended: `## Log`, then one `### <n>. <heading>` per segment,
  in order, each followed by its text. A segment's metadata is the
  frontmatter list `log`: `n` (from 1, no gaps), `heading` (at most 80
  characters, equal to the body heading), `first_message` and
  `last_message` (the transcript message ids it covers; `null` only for an
  upgraded segment whose start is unknown), `from` and `to` (those
  messages' times), `state` (`open` or `sealed`; only the last segment may
  be `open`), `windows` (how many pass windows wrote it), `origin` (`pass`,
  `rebuild`, `catalog` or `upgrade`), and its own `topics` (0–5) and
  `related`. A pass writes a segment's text in at most 900 characters,
  asking the model for about 800; an upgraded segment keeps its converted
  text whatever its length.
- Only the open segment is ever rewritten. Sealed segments are never sent
  back to the model to rewrite and never change, except by `session
  rebuild`, which replaces the whole record ([§5.6](#56-rebuilding-a-record)).
- **Activity** (sessions only): `active` (default `true`), `keep_active`
  (default `false`; the exemption from automatic deactivation) and
  `active_change` (`null`, or the last change of `active` or `keep_active`:
  `{at, by, route, reason, cite}`, where `by` is an author, `route` is
  `age`, `removed` or `manual`, `reason` at most 300 characters and `cite`
  the record that justified it, such as `session:<id>#<n>` or an entry id,
  else `null`). Every change is also an audit line
  ([§5.7](#57-active-and-inactive-sessions)).
- `cataloged_by` is `null` for a session Daniel switched on with
  `/scribe on`, or `{at, by}` for one cataloged by name with `session
  catalog` ([§5.8](#58-cataloging-a-past-session)).

**Discussion** (`discussions/<yy-mm>/<slug>.md`): the distilled record of
a conversation that concluded something, in the shape of Daniel's almanac
discussions (`almanac/15-discussions/`). Frontmatter: the common fields with
`kind: discussion`, plus `title`, `status` (`seed`, `growing`, `settled`),
`sources` (the `session:<id>` and `message` ids each point came from) and
`terms`. Body, in order: a thesis paragraph; `## The arc`, numbered
`### <n>. <claim>` points, each opening with one blockquoted verbatim quote
of Daniel (`> "…" — Daniel`) followed by one interpreting paragraph, a
change of mind headed `Correction #<n> —`; `## Through-lines`;
`## The one-line takeaway`; `## References` (sessions, tasks, related
discussions as `[[slug]]`). The quotes are Daniel's words; everything else
is the model's interpretation and is labelled so in `ask` evidence.

**Proposal** (`proposals.json`): `id` (`p-<n>`), `kind` (`supersedes`,
`stale` or `candidate`), `older`, `newer` (entry ids), `reason` (at most 300
characters; model text for consistency proposals, a quote preview for candidates), `quotes` (verbatim spans from both entries), `state`
(`open`, `approved`, `rejected`), `opened_at`, `settled_at`, `settled_by`
(a `cite` of Daniel's approving turn). From format 4: `author` (who opened
it: `{kind: scribe, name: consistency}` for the routine, `{kind: agent,
name}` for `proposal add`) and `settled_author` (`{kind: daniel, name:
daniel}` for a `#scribe` settle, `{kind: agent, name}` for `proposal
settle`, else `null`). An agent's settle has `settled_by: null`, since no
typed turn exists to cite.

Format 9 adds `candidate`, null for prior kinds, otherwise `{tag, quote,
cite, project, provider, host}`. A candidate's `older` reserves the future
entry id; no file exists for it until approval. All entry allocators count
reservations, including rejected ones. `newer` is null and `quotes` is empty;
the candidate's quote and cite live in its payload. Its author is
`{kind: scribe, name: session-pass}`. `settled_by` cites the approving or
rejecting turn; the eventual entry cites the original statement.
Reads upgrade formats 1–8 in memory; writes use format 9. Prior proposal
fields, values and attribution are preserved, with `candidate: null` added.
Format 10 adds `role` (`user` for an upgraded candidate) and `evidence`
(`null`) to a candidate, so a pass can also quote an assistant turn
([§4.7](#47-findings)); entry files gain `evidence` and `as_of`, both
`null` on upgrade. Writes use format 10.

**Tags store** (`tags.json`, from S12): `{"format": 4, "tags": [{"name",
"aliases", "description", "stale_after_days", "author", "created",
"updated"}]}`, the tags agents created with `scribe tag add`. The tag table
scribe uses is the config's tags merged with these. Config tags win: a
store tag may not take a name or alias a config tag claims (`tag add` and
`tag update` refuse it, `tag_conflict`), a word a later config edit claims
is dropped from the store tag with a warning, and a config tag cannot be
changed by `tag update` (`tag_configured`). A store tag is used exactly
like a config tag, by `#scribe` lines and by `entry add`
([4_decisions.md](./4_decisions.md#agent-created-tags-live-in-the-store-and-config-tags-win)).

**Config** (`config.toml`): `[tags.<name>]` with `aliases`, `description`,
`stale_after_days`, `action`, `require_quote`, `ask_first`,
`require_evidence` and `[tags.<name>.capture]` (`from`, `route`,
`triggers`, `criteria`, `not`; [§11.2](#112-capture-rules-per-tag)). Config
tags merge with the shipped set (`preferences`, `idea`, `standard`): declaring
a new tag adds it, and declaring a shipped tag overrides only the fields it
sets and inherits the rest. A name or alias the file sets that a shipped tag
also holds is Daniel's: the shipped tag loses that alias, or is left out when
the word is its name, and `tag list` warns. Two config entries that claim one
word are refused. No config syntax removes a shipped tag except claiming its
name
([4_decisions.md](./4_decisions.md#config-tags-merge-with-the-shipped-set)).
`[sources]` with `discussions_readonly`, a list of
directories `ask` reads as extra discussion evidence and never writes
(default: the constellation almanac's `15-discussions`, when present); `[models]` with `ask`, `distill`, `session`, `consistency`,
`referent` and `credentials` (the variables model calls are given, default
`CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`); `[catalog]` with
`every_minutes` (20), `every_turns` (10), `window_messages` (40) and
`window_chars` (15000), all positive integers, and from S9 `segment_windows`
(3, [§5.5](#55-the-log-segments-and-sealing)); from S10 `[sessions]` with
`inactive_after_days` (120, a positive integer;
[§5.7](#57-active-and-inactive-sessions)); from S14 `[projects.<slug>]`
with `paths`, the path fragments whose mentions attribute a transcript to
that project ([§5.8](#58-cataloging-a-past-session); none by default);
`[redact]` with `env`, further
variables whose values are masked. A missing file or table is the defaults;
an unknown key or bad value is refused, naming the file and line
([specs/store.md](./specs/store.md#config)).
The file is created by `scribe hook session-start` on first run with the
defaults ([4_decisions.md](./4_decisions.md#only-decisions-and-preferences-ship-by-default))
if absent; scribe never rewrites it after that. As built (S6), it is not:
`session-start` reads only, and a missing file is the defaults
([4_decisions.md](./4_decisions.md#session-start-reads-only)).

**Redaction.** Before any write of `said`, `content`, a session record's
title, text fields and terms (from S8 also each segment's heading and text,
and `active_change.reason`), a proposal's `reason` and quotes, or (from
S12) a store tag's description, the store masks
the live values of the variables in `[redact] env` and `[models] credentials`
and high-confidence credential shapes (`Authorization: Bearer …`,
`ghp_`/`github_pat_`, `sk-ant-`/`sk-`, `AKIA…`, `xox[bp]-`, PEM
private-key blocks); a `[redact] env` name with no value in the writer's
environment refuses the write instead of storing text unmasked
(STD-05 R13, R14;
[4_decisions.md](./4_decisions.md#persisted-free-text-passes-one-redactor)).

## 4. `#scribe`

### 4.1 Detection
> Proposed change: [§14](#14-scribe-as-an-annotation-proposed) ([ORB-26510]) replaces this rule; the text here is what is built.


`scribe hook user-prompt-submit` receives `session_id`, `transcript_path`,
`cwd` and `prompt`. `capture::lines` finds every line of `prompt`
that starts with `#scribe` or `#scribe/` (after optional leading
whitespace). A `#scribe` line must end there or continue with whitespace;
`#scribe/` is the global-marker attempt, including a malformed one.
Lines inside code fences, blockquotes and `<pasted_content>` blocks are
skipped (the orbit-office `markers.rs` rule). Each such line is one
instruction; a turn with two lines writes two entries. A bare `#preference`,
`#preferences` or any
other hashtag is not an instruction and captures nothing, with no reply.

Only Daniel writes scribe: a user-role turn that is not plainly typed by
him records nothing, settles nothing and does not run `/scribe`. The hook
skips a turn whose prompt carries an agent or harness wrapper
(`<agent-message>`, `<task-notification>`, `<side-sessions-event>`, a
scheduled/loop wrapper, or any prompt opening with a markup tag), or whose
payload origin field (`source`, `prompt_source`, `origin`,
`prompt_origin`, where the harness provides one) is not a plain typed
value. The reply is empty and nothing is audited. This is an accident
guard like the provenance check below (STD-05 R5), not a security
boundary: a process running as Daniel can forge a plain payload.

Provenance check (an accident guard, not a security boundary — STD-05 R5):
before writing, the hook confirms `transcript_path` lies under
`~/.claude/projects/`, belongs to the user, and that its session id matches
`session_id`. The prompt itself is not yet in the transcript when the hook
runs, so the message uuid in `cite` is filled by the next `Stop` hook, which
finds the user message whose text hashes to `prompt_sha256`. A process that
can run `scribe hook` with forged stdin can forge an entry; that is
documented in [§9](#9-concerns--honest-limitations).

As built (S2): the transcripts directory is `$CLAUDE_CONFIG_DIR/projects`,
else `$HOME/.claude/projects`. The path must be absolute, without `.` or `..`
components, inside that directory and named `<session_id>.jsonl`; when the
file exists it must be a regular file (not a link) owned by this uid. A
missing file passes, because a session's first prompt can reach the hook
before Claude Code has written the transcript. Where a command goes on to
read the transcript (`session catalog`, `transcript show` and `list`,
`session rebuild`), the check and the read share one handle: the file is
opened once, every component below the transcripts root without following
links and without blocking on a pipe, and its type, owner and bytes come
from that open file, so a path swapped for a link after discovery is refused
rather than read (STD-03, STD-05 R7). A failed check records
nothing; each `#scribe` line gets a `could not record` reply and an audit
line with `error_code: provenance_failed`. The `Stop` hook checks the transcript's provenance and scans its user
messages for entry instructions the prompt hook missed, including when the
session is off. It uses the same parser, origin guard, redactor, store capture
and echoes as the prompt hook. Tool payloads, assistant messages, sidechain
and meta lines are excluded. Original origin fields are retained for the
guard; each recovered entry cites the transcript message's uuid, timestamp,
prompt hash, line number and cwd (the Stop payload's optional cwd is the
fallback when the transcript line omits it). If neither provides a cwd,
Stop reports the missing metadata and records nothing from that message.

Recovery scans the whole conversation on each Stop and uses existing cites
as its progress record rather than advancing a cursor past an unwritten
line. It skips lines already captured for `(session, prompt_sha256, line)`,
including individual text blocks of a multi-block message; the store checks
the key again under its lock. It also skips a line an entry covered at that
turn's timestamp, including an entry retired or superseded since: coverage
is decided as of the turn (`covers_turn`), not from the entries active when
Stop runs, so retiring the covering entry does not make a later Stop write
the line. The active window ends at the entry's `inactive_since` (format
12), which a later metadata write does not move, and both sides are
compared in whole seconds because stored times are whole seconds while
transcript times are not. A form-3 referent gets the same check once it is
resolved (`Store::covered_draft`). A line typed after the entry was retired
is not covered by that rule and is still captured. A forgotten entry's words are blank, so it
cannot answer. This retries partial failures and remains
silent and write-free when everything is recorded. Refusals and failures
are echoed and audited, and retried on later Stops; they are never treated
as successful captures. No model runs during recovery: referents use the
existing guessed fallback over only the messages before that prompt, and
the echo says why. Session controls (`/scribe …`) and settling commands
are not replayed from history; they remain prompt-hook operations.

Stop also fills waiting cites from prompt-hook captures: it hashes each user
message's whole text and each of its text blocks, and gives each waiting cite
the uuid of the matching message written closest to the cite's `at`; a cite
whose prompt is not in the transcript yet stays `null` for the next Stop.

### 4.2 Parsing an instruction
> Proposed change: [§14](#14-scribe-as-an-annotation-proposed) ([ORB-26510]) replaces this rule; the text here is what is built.


Model-free grammar, tried in order:

1. `#scribe <tag>: <content>` — explicit tag, content after the colon.
2. `#scribe [add|record|note] <content> (to|in|under) [my|the] <tag>` — the
   tag is the last word group, matched against names and aliases. `<content>`
   is never a bare `this`, `that`, `it` or `these`; those lines are form 3.
3. `#scribe [add|record] (this|that|it|these) (to|in|under) [my|the] <tag>`
   — a referent instruction ([§4.3](#43-resolving-this-and-it)).
4. `#scribe approve <p-id>` · `#scribe reject <p-id>` ·
   `#scribe retire <entry-id>` · `#scribe forget <entry-id>` — settle a
   proposal or hide/blank an entry ([§7](#7-consistency-routine)).

Tag names and aliases are compared case-insensitively, singular and plural
both declared in config (`preference`, `preferences`, `pref`). An instruction whose tag matches nothing is refused:
no entry, and the reply lists the configured tags. An instruction that
matches no form is refused with the forms. A refusal is still audited
(`status: failure`, `error_code: unknown_tag` or `unparsed_instruction`).

Exactly one entry per instruction, also under retries: the idempotency key
is `(session, prompt_sha256, line)`. A repeat finds the existing entry and
echoes it instead of writing again. That same-line replay wins over the
duplicate check below.

The same locked `Store::capture` refuses a statement an active entry of
the same tag and project already covers, using the one lexical predicate
`covers_statement` ([§5.3](#53-a-pass)). Nothing is written and no id is
consumed. The hook echoes `scribe: already covered by <id> in <tag> —
"…"`, exits 0, and writes no audit line. Retired, superseded and forgotten
entries do not cover a new line. Stop recovery still treats a retired or
superseded entry as covering a turn it was active for
([§4.1](#41-detection)). A different tag or project still writes. The check
does not match a short typed line against the long paragraph it
summarizes, and it does not catch a paraphrase.

As built (S2): hashtags at the end of the line, after the tag, are the
entry's topics, lowercased and deduplicated (more than five is refused as
`unparsed_instruction`); they are not part of `content`.
`#scribe distill …` ([§4.5](#45-distilling-a-discussion)) is recognised
and refused with `error_code: unsupported_instruction` until S7 builds it,
so it is never misread as form 2. Form 4 is built since S6: a settling
verb takes exactly one id of its kind (a proposal id for `approve` and
`reject`, an entry id for `retire` and `forget`, trailing `.`, `!`, `,` or
`;` allowed), and anything else after the verb is refused as
`unparsed_instruction` (`#scribe approve takes a proposal id, such as
p-4`). Form 3 is resolved since
S3 ([§4.3](#43-resolving-this-and-it)); a form-3 line whose tag is unknown
is refused as `unknown_tag` before any model call, as in the reply example
below. Form 3 is a line whose content is exactly one of the four referent
words, with or without a verb (`#scribe that to preferences`); `#scribe add
this rule to preferences` is form 2 with the content `this rule`.
The tag word of form 2 may end in `.`, `!`, `,` or `;`.

A line records `project: global` when its first word is `#scribe/global`
instead of `#scribe`. The rest of the line is one of the three capture
forms: `#scribe/global <tag>: <content>`,
`#scribe/global [add|record|note] <content> (to|in|under) [my|the] <tag>`,
and `#scribe/global [add|record] (this|that|it|these) (to|in|under) [my|the] <tag>`.
A line that opens with `#scribe` records the cwd's project, as before.
`author.kind` stays `daniel` either way: this is still Daniel's typed line,
not `scribe entry add`. The opener is not a topic hashtag and not a tag
name. `#global` at the end of a line is still a topic, and a tag or alias
named `global` is still named by the tag word (`#scribe global: …`,
`#scribe add … to global`). Anything else in that first word
(`#scribe/Global`, `#scribe/all`, a bare `#scribe/`) is refused with
`unparsed_instruction` and writes nothing. The same refusal applies when
`#scribe/global` opens a line that does not record an entry (`approve`,
`reject`, `retire`, `forget`, `distill`): nothing is settled and nothing is
written. A refused line, marked or not, writes no entry. The forms text in
that refusal (`FORMS`) includes `#scribe/global`.

### 4.3 Resolving "this" and "it"
> Proposed change: [§14](#14-scribe-as-an-annotation-proposed) ([ORB-26510]) replaces this rule; the text here is what is built.


Only form 3 uses a model. The hook parses the transcript, takes the last 12
messages before the prompt (user and assistant text, tool payloads dropped),
and asks the `referent` model (Haiku) for `{message, span}`: which message
the instruction points at and the exact passage to record. The answer is
accepted only if `message` is one of the 12 and `span` passes the verbatim
check against that message; then `content` is the span and `content_from`
cites it. On a timeout (20 s), a failed check or a model error, the entry is
still written, once, with `content` set to the previous assistant message's
first 600 characters, `content_from.kind = "message"`, and the echo says the
referent was guessed so Daniel can `#scribe retire` it. One instruction never
produces zero or two entries.

As built (S3): the window is the transcript's last 12 messages with text
(the [§4.1](#41-detection) parse: user and assistant text, tool payloads,
sidechain and meta lines dropped); if a retried hook finds its own prompt
already in the transcript (by `prompt_sha256`), that message and everything
after it are left out. A message longer than 8,000 characters is cut in the
prompt and the cut marked `[… cut]`; the span is still checked against the
whole message. The model's span is trimmed and becomes `content`;
`content_from` is `{kind: message, role, message, at}` of the message it
named, `user` or `assistant`. The fallback is the previous assistant
message, or the latest message when no assistant has spoken yet, cut to 600
characters; it is cited the same way. Its echo reads `scribe: added
preferences-3 to preferences (guessed: <why>; recorded the assistant message
before your prompt instead) — "…"`, where `<why>` is the timeout, the model
error, a message not in the window, or a span that is not word for word in
its message. A wrong guess is visible with `scribe entry show <id>`, and
`#scribe retire <id>` (since S6) hides it. Two cases differ from the paragraph
above: a session with no earlier message (the first prompt, or a transcript
not written yet) has nothing to point at, so the line is refused as
`no_referent` and the model is not called
([4_decisions.md](./4_decisions.md#a-referent-with-nothing-before-it-is-refused));
and a replayed form-3 line is found by its cite before the transcript is
read, so a retried hook echoes `already recorded` without a second model
call. The model runs outside the store lock; the entry is written by the
same `Store::capture` call as forms 1 and 2.

### 4.4 The reply

The hook returns `systemMessage` (shown to Daniel) and
`hookSpecificOutput.additionalContext` (shown to the main agent) with the
same echo lines (the agent's copies carry the per-line notes below), and
never blocks the turn, so the rest of the prompt still reaches the agent:

```
scribe: added preferences-12 to preferences — "squash-merge agent PRs; keep merge commits for releases"
scribe: refused "#scribe add it to rituals" — no tag "rituals"; tags: idea, preferences, standard
```

The agent-facing text adds a note to each echo line: a recorded (or
already recorded, or settled) line adds "recorded by scribe; no action
needed", so the main agent does not try to record it again; a refused or
failed line adds "not recorded by scribe; relay this to Daniel", so the
agent relays the refusal or failure to Daniel instead of silently losing
the line. The note is per line, so a recorded line never lends its
"no action needed" to a refused line in the same prompt.

As built (S2): the reply is one line of hook JSON,
`{"systemMessage": …, "hookSpecificOutput": {"hookEventName":
"UserPromptSubmit", "additionalContext": …}}`, with one echo line per
`#scribe` line in prompt order; a prompt without one prints nothing. A
replayed line echoes `scribe: already recorded <id> in <tag> — "…"`. A line
an active entry of the same tag and project already covers echoes `scribe:
already covered by <id> in <tag> — "…"` and writes no audit line; the
agent note is the recorded note. The `Stop` hook does not repeat that
covered echo on a later turn, and a prompt the prompt hook missed stays
quiet when every `#scribe` line is already covered, including a line
whose covering entry has since been retired or superseded: coverage is
as of the turn ([§4.1](#41-detection)). When the store or `config.toml`
cannot be used, each line echoes `scribe: could
not record "…" — <error>` and the hook still exits 0, so the prompt goes
through. Quotes in an echo are flattened to one line and cut at 200
characters (`scribe entry show <id>` has the whole text). The `Stop` hook
emits the same `systemMessage` echoes for recovered or refused instructions;
it prints nothing when recovery has nothing to report. Its Claude Code reply
does not include prompt-only `additionalContext` or a blocking decision.

### 4.5 Distilling a discussion

Two more `#scribe` forms, explicit like the rest; nothing is distilled
unless Daniel asks.

- `#scribe distill this discussion [as "<title>"]` reads the current
  session's turns since the last distill point (or the whole session) and
  writes a new discussion with `status: growing`.
- `#scribe add this to <slug or title>` reads the turns since that
  discussion was last updated in this session and extends it.

The `distill` model (the decision model; distilling is judgment) returns
the thesis, arc points with quotes, through-lines, takeaway, topics and
terms as JSON. Every quote must pass the verbatim check against one of
Daniel's own turns; a point whose quote fails is dropped and counted in the
echo. Related discussions are proposed from topic and term overlap with the
store and the read-only sources.

Extending never rewrites what exists, because Daniel may have edited the
file: new arc points are appended after the last numbered point,
`sources`, `terms`, `topics` and `updated` are merged, and the thesis,
through-lines and takeaway are left as they are unless the extension adds a
`Correction`, in which case the echo says the thesis may need Daniel's edit.
The echo names the file path, the points added and any dropped quotes.

### 4.6 Action tags

> Proposed change ([§11.6](#116-idea-capture-after-finding-is-gone),
> not built): `idea` switches from `propose` to `write` capture once §11.1
> lands. (`remind` stopped shipping in step (a2), [§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup).)

[ORB-25112]. Most tags hold the human's memory: preferences
`ask` answers from. A decision is not a shipped tag: it is recorded on its
Orbit task with the human's words quoted, or in a feature's `4_decisions.md`
("Decisions are recorded on their Orbit task, not in a shipped tag"). An **action tag** holds work items instead, entries a
consumer outside scribe picks up: an intake that turns a `standard` entry
into a change to the constellation standards. Work and friction are not
action tags: agents file them in Orbit (`orbit.task.add`, `orbit.friction`)
([4_decisions.md](./4_decisions.md#task-and-friction-no-longer-ship-as-action-tags)).
scribe never calls Orbit or any consumer; it keeps the entries and exposes them to read.

**Which tags.** Config marks a tag `action = true` (`[tags.<name>]`), with
two optional switches: `require_quote` (an agent's entry must quote the
human) and `ask_first` (agents ask before writing). `standard`
(`require_quote`) and `idea` ship, with no
aliases. A config merges with those shipped tags. A config tag of the same
name overrides only the fields it sets and inherits the rest (`action = false`
turns it into a memory tag). A different config entry that claims the shipped
name, as its own name or as an alias, leaves that shipped tag out, and `tag
list` warns; the file stays readable
([4_decisions.md](./4_decisions.md#action-tags-are-marked-in-config-and-three-ship),
[4_decisions.md](./4_decisions.md#config-tags-merge-with-the-shipped-set)).
A new action tag needs no code change. `tag list` shows each action tag's
mode (`free` or `ask-first`, `,quote`) and its counts of retired and
rejected agent entries, the evidence for flipping a tag to ask-first.

**The window.** A `#scribe` line under an action tag (instruction or
referent form) also stores the transcript turns since the last user
message that carried a `#scribe` line, oldest first, then the prompt
itself, so the consumer sees what led to it. It is model-free. Each turn is
cut to 4,000 characters (`cut: true`); then the oldest turns are dropped
until at most 40 turns and 12,000 characters remain, and the window records
`truncated: true` and how many it `omitted`; the prompt is always kept
([4_decisions.md](./4_decisions.md#an-action-entrys-window-is-capped-at-40-turns-and-12000-characters)).
Window text passes the redactor. Memory tags keep no window. An agent's
`entry add` keeps none either; it names its session in `--related
session:<id>`.

**Agent entries.** An agent writes under an action tag with `entry add`
like any agent note. The skill tells it to do so unasked, with one visible
line naming the id and how to cancel, but only in an interactive session
the human is in; every other session writes none; under an `ask_first`
tag it asks first. Three guards sit in the CLI and store:

- **Cancel window.** An agent-written action entry cannot be picked up for
  five minutes after it was written
  ([4_decisions.md](./4_decisions.md#agent-entries-have-a-five-minute-cancel-window)),
  so the human's `#scribe retire <id>` (which already works on agent
  entries) cancels it before any consumer acts. The read surface reports
  `cancel_window_ends` and `past_cancel_window`; `entry pickup` refuses
  inside it (`cancel_window`). The human's own entries have no window.
- **Quote.** Under a `require_quote` tag `entry add` needs `--quote '<the
  human's words>'` and a `--related session:<id>`; the CLI finds that
  session's transcript on this host and refuses the entry unless the quote
  is a verbatim span (§6.4's check) of one of the human's typed turns
  (`quote_required`, `quote_not_found`). The quote is stored on the entry.
- **Ask-first** is a switch the skill obeys and `tag list` shows; the CLI
  does not enforce it, since it cannot tell whether the human said yes.

**The read surface.** An intake reads `entry list --tag <tag> --after
<cursor> --json`: `--after` is an RFC 3339 instant, strictly after; the
window is left out unless `--window` is given (`window_turns` always says
its size). Each action entry's `action` object reports its age, cancel
window and pickups. `ask` leaves action entries out unless the question's
scope names their tag; the skill leaves them out of the session brief.

**Pickups.** A consumer records what it did with `entry pickup <id>
--author <name> [--result <ref>] [--rejected]`, appended to `pickups.json`
under the lock; consumers never edit entries. The same note twice changes
nothing. A pickup of an entry that is not under an action tag
(`not_action_tag`), inside its cancel window, or not active (unless
`--rejected`: the result of an entry picked up and later rejected) is
refused.

**Stuck entries.** The `SessionStart` hook adds one line counting active
action entries older than one hour with no pickup, per tag, and nothing
when there are none
([4_decisions.md](./4_decisions.md#an-action-entry-is-stuck-after-one-hour-without-a-pickup)).

**Reaching the owner host.** The consumer runs on dk-server-2, and entries
are captured on the Mac. The owner reads the Mac's store over SSH, with the
same read surface; there is no replica
([4_decisions.md](./4_decisions.md#the-owner-host-reads-action-entries-over-ssh)).

### 4.7 Evidence-backed findings (configured tags)

[ORB-25177], updated by [§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup).
`finding` no longer ships as a default tag and the session pass no longer
reads assistant turns. The `evidence` and `as_of` entry fields, the
`require_evidence` config flag and its guards, and the agent-claim reading
behaviour stay: a config that declares `[tags.finding] require_evidence = true`
(or any memory tag with `require_evidence = true`) still works as below, and
existing `finding` entries stay listed, shown, searched and cited as that
agent's claim.

**Which tag.** Config marks a memory tag `require_evidence = true`
(`[tags.<name>]`; an action tag refuses the flag). It has no consumer, no
transcript window and nothing for an intake to read: `entry pickup` refuses
it (`not_action_tag`), and the `SessionStart` stuck-entries line never
counts it.

**Writing one.** Under a `require_evidence` tag, an agent writes:

```sh
scribe entry add --tag finding --author <crew> --related session:<id> \
  --evidence '<path, task, run, experiment or commit>' [--as-of YYYY-MM-DD] \
  '<the result>'
```

The entry records the result as content, the `--evidence` pointer, and
the as-of date (the write's date without `--as-of`). Two guards sit in
the store, under the lock, so every route to `add_agent_entry` shares
them:

- **Evidence required.** A `require_evidence` entry without a non-empty
  pointer is refused and writes nothing (`evidence_required`); an
  `--evidence` under any other tag is refused the same way. A result with
  nothing to check it against is just an agent's memory of itself.
- **Per-session cap.** At most **3 findings per writing session**
  (`MAX_FINDINGS_PER_SESSION`), counted over every finding already held
  for the session a write names in `--related session:<id>`; a write that
  names no session is counted under its author and the write's day,
  against that author's other unnamed-session findings of that day. The
  fourth is refused (`finding_cap`) and the refusal names the cap.

**Reading one.** `ask` evidence labels a finding `agent claim (<author>)
as of <date>; evidence: <pointer>`, and the deterministic attribution
check copies that label into each surviving claim, so an answer can
never present it as Daniel's words (§6.4). `record search` returns the
same marking in the snippet plus the hit's `evidence` and `as_of`
fields; `entry show`/`list` carry both fields too.

**No pass route.** The session pass reads only Daniel's turns
([§5.3](#53-a-pass)) and opens no candidates from assistant turns, for
`finding`, `remind`, `idea` or any other tag. Previously stored format-10
assistant-turn candidates still approve into an agent-authored entry
(`{kind: agent, name: assistant}`).

## 5. `/scribe on`

### 5.1 Switching on and off

A prompt whose first line is `/scribe on`, `/scribe off` or `/scribe status`
(or a link command, [§5.4](#54-session-links), or `/scribe catalog <id>`,
[§5.8](#58-cataloging-a-past-session)) is intercepted by the `UserPromptSubmit` hook, which updates `control.json`
and returns `decision: "block"` with the status as `reason`, so no model
turn is spent. Every session starts off. (Designed, then parked
2026-11-21, not built: an interactive session that is off would still feed
a personal memory base and leave a light catalog entry, with `/scribe
private` to stop that for one session,
[§13.12](#1312-a-personal-base-and-a-work-layer-per-session) and
[§13.15](#1315-what-a-session-that-is-not-switched-on-leaves). While they
are parked an off session writes nothing.) `on` records `on_since` and marks a
pass due; `off` stops further passes and keeps the record. `/scribe on` in a
session that is already on is a no-op that says so.

As built (S4): `control::parse` reads the prompt's first line only; the
word is `/scribe` or the plugin-namespaced `/scribe:scribe`, the argument
case-insensitive, and a bare `/scribe` is a status. The payload passes the
same transcript check as a `#scribe` line first. `on` sets `since` and
`due`; `off` also clears `due` and `final`, so no pass runs after it
([4_decisions.md](./4_decisions.md#scribe-off-clears-a-pending-pass));
any other argument is refused with `unknown_scribe_command` and the usage.
A status reports the switch, whether a pass is due, and the record's status,
pass count and last write. The reply wording is in
[specs/output-contract.md](./specs/output-contract.md#hook-replies).

### 5.2 Cadence

`scribe hook stop` runs after each assistant reply. It does no model work:
it reads `control.json`, and when the session is on and either
`every_minutes` have passed since the last pass or `every_turns` user turns
have arrived, it sets `due` under the lock. `scribe hook session-end` sets
`due` with `final: true`. The plugin ships `scribe-session-pass` seeded
disabled for each host to review. Once enabled with `orbit routine enable`,
it runs every 5 minutes (`*/5 * * * *`) through the `scribe.session-pass`
job. Daniel can also run `scribe session pass` by hand where `claude` is on
`PATH`. Both take at most five due sessions per run, with a 120-second
budget per model call. Daniel reversed the manual-only choice on
2026-11-10: "perhaps routine was not a bad idea then? let's add that back"
([4_decisions.md](./4_decisions.md#consecutive-windows-and-bounded-retries-keep-session-passes-cheap)).
Failures keep the session due and delay automatic retries by 5, 10, 20,
40, then at most 60 minutes; a completed pass resets this state.
`scribe session pass --session <id>` bypasses backoff.
No process is detached from a hook ([4_decisions.md](./4_decisions.md#session-passes-run-from-an-orbit-routine-not-a-detached-hook-child)).

For cadence (S4), after capture recovery and cite filling, the `Stop` hook
reads `control.json` and goes no further for a session that is not on or
already due. The last pass is the
record's `updated`, else `since`; the minutes rule is checked before the
transcript is read, and the turns rule counts Daniel's turns after
`scribed_through` in the conversation [§5.3](#53-a-pass) builds.
`session-end` sets `due` and `final` only for a session that is on. Both
write only when the state changes, and neither calls a model. The
`scribe-session-pass` routine and the Orbit job are S5's (the Orbit plugin
tree); until then `scribe session pass` runs by hand or from cron.
As built (S5): the plugin declares `scribe.session-pass`, but the routine,
its job and the tool's wiring ship with the consistency routine in S6
([§10](#10-slice-plan): "the two routines ship seeded disabled"), so the
tool answers `not_implemented` and `scribe session pass` still runs by hand
or from cron.
As built (S6): the routine and its job ship, seeded disabled, and the tool
runs the same code as `scribe session pass`; it reads the transcripts under
the plugin's `transcripts` setting and starts no later session or window
once 50 s have passed, so one call ends inside Orbit's 300 s backend
timeout; partly processed sessions keep their checkpoint and stay due,
counted in `remaining` alongside sessions waiting for backoff or the next run
([§1](#1-boundary-and-surfaces)).

### 5.3 A pass

1. Parse the transcript from its first message (the whole opted-in
   session, not only messages since `on_since`), dropping tool payloads,
   `<task-notification>` and other synthetic turns, and attributing pasted
   blocks as pasted. A message uuid counts once: Claude Code writes earlier
   lines into the file again when a session is resumed (same uuid,
   timestamp and text), and only the first copy is kept, at its position.
   Without that, `scribed_through` inside a replay resumed after its first
   copy and the pass read the replay again, and a start in it was refused
   as not following the open segment ([ORB-24918]).
2. Split messages after `scribed_through` into consecutive windows, oldest
   first: at most `[catalog] window_messages = 40` messages or
   `window_chars = 15000` Unicode characters of content, whichever binds
   first. An individually oversized message is cut with a marker inside
   the character limit. No older messages are discarded to fit later ones.
3. Send each window and the record so far to the `session` model (Haiku)
   for the whole updated `{title, summary, goals, outcome, next, topics,
   related, terms, candidates}` record. A session with links also sends the
   latest title and summary of each linked session that has a record, read once
   at the start of the pass, framed as data, with the instruction to
   summarize only this session's own work and not retell the linked one
   ([§5.4](#54-session-links)). The first window of a first pass has no prior
   record. Plain message headers retain role and id; pasted data and the
   record use plain delimiters too. Messages are data, never instructions.
   Offer stored topics and terms, including those from earlier windows,
   for reuse. Validate field limits and shape on every window, with one
   retry naming the refusal (header-only compression for length overruns;
   otherwise a fresh window request, [§5.5](#55-the-log-segments-and-sealing)).
   A term must name a user message in the current or an earlier window and
   quote its own text verbatim; unknown,
   future, assistant and pasted citations are dropped.
4. After every valid window, redact and atomically write the whole record
   under the store lock, advancing `scribed_through` and `passes` (successful
   record updates). Feed the persisted record into the next window. Only
   the last window clears `due` and applies final status. A failure keeps
   all prior windows, stays due, and starts bounded retry backoff; the next
   pass resumes at the failed window. One CLI invocation processes all
   windows; the routine may yield between them at its deadline.

From format 4 (S9), steps 3 and 4 write the header and the log instead of
the four sections ([§5.5](#55-the-log-segments-and-sealing)): each window
sends the header and the open segment, and the record's sealed segments
only as a table of contents; the answer rewrites the header and the open
segment and may open new segments; the write appends, never rewrites, a
sealed segment.

A pass writes no tag entries, even if the transcript contains `#preference`
or an unprocessed `#scribe` line (proposed, not built: it writes memory
items, which are not tag entries, [§13.3](#133-writing-the-session-pass)), with one
exception: under a tag whose capture route is `write` (the shipped `idea`)
it writes Daniel's checked idea itself, at most 3 per session per run
([§11.6](#116-idea-capture-after-finding-is-gone)). Otherwise it may open
candidate proposals as described below; only Daniel's later typed approval
creates an entry. Up to S13 there
is no backfill: sessions that were never switched on are never read. From S14 one more route reads
a session: Daniel names a past session with `scribe session catalog <id>`
([§5.8](#58-cataloging-a-past-session)). scribe still never scans or
catalogs a session on its own; `transcript list` reads transcripts to help
Daniel choose and writes nothing.

As built (S4), in `scribe-core::pass` and `commands/session/pass.rs`:

- Candidates are the sessions `control.json` marks on and due, by id, five
  per run, excluding sessions whose `retry_after` is still in the future;
  skipped sessions remain due and count in `remaining`. The transcript is
  found by id under Claude Code's projects directory and passes the hooks' provenance check
  ([4_decisions.md](./4_decisions.md#a-pass-finds-its-transcript-by-session-id)).
  No transcript or no message yet leaves the pass `waiting` and due.
- The conversation drops tool payloads, thinking, turns that open with a
  synthetic tag (`<task-notification>`, `<system-reminder>`,
  `<command-name>` and the like) and `/scribe` command turns; a
  `<pasted_content>` block is kept apart as pasted text, offered to the
  model marked as pasted and never accepted as a term's `said`.
- Consecutive windows start after `scribed_through` and stop at either
  configured bound. A single oversized message is cut with a marker;
  the character count includes pasted content and the marker, but excludes
  prompt headers, instructions and the previous record. Nothing after
  `scribed_through` clears `due` without a model call; at session end it
  marks the record final.
- Budgets: title 120 characters, summary 1500, goals 600, outcome 2500,
  next 1000 (rendered as a bullet list); a `## ` heading inside a section or
  an empty summary also fails, as do invalid topics. A bad `related`
  reference is dropped. A term is kept when its `said` (at most 400
  characters) is verbatim in one of Daniel's own turns, not his pasted
  text; its `message` must name that turn, up through the current window.
  A term already in the record is not repeated.
- The previous record keeps its `created`, `started`, `cwd`, `project`,
  `provider` and `host`; `on_since` is the switch's `since`; `passes`
  counts up, `scribed_through` becomes the
  window's last message, and only the last window of the final pass sets
  `status: final`. Each window is checkpointed under the lock; the last
  settles `due` and resets retry state. A failed pass keeps the latest
  checkpoint and leaves `due` set with bounded retry backoff:
  the run prints its report and then exits 1 naming each failed session
  and its code, and `scribe.session-pass` answers `session_pass_failed`
  ([4_decisions.md](./4_decisions.md#a-failed-pass-stays-due-and-daniel-runs-the-pass-by-hand)).
- `scribe session pass --session <id>` (repeatable) passes the sessions
  named whether or not they are due, ignoring backoff, each once and
  without the five-per-run cap, so a session whose `due` was cleared can be passed
  again; a session that is not on is refused (`session_not_on`) and never
  read, and the switch's `final` flag counts as for a due pass.

#### Unmarked preferences

> Proposed change ([§11](#11-claims-in-context-capture-rules-and-issues-proposed),
> not built): a claim carries a statement that becomes the approved entry's
> content while the quote stays attached ([§11.1](#111-a-claim-carries-a-statement)),
> candidates whose tag is no longer in the tag table close automatically
> ([§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup)), and `idea`
> switches to `write` capture ([§11.6](#116-idea-capture-after-finding-is-gone)).
> Per-tag capture rules ([§11.2](#112-capture-rules-per-tag)), dropping
> `remind` and `finding`, and removing assistant-turn candidates are built.

An on session's ordinary pass (including an explicitly requested catalog's
windows) offers candidates beside the record in the same model call.
`session rebuild` leaves this disabled. Never-on sessions remain unread by
a pass; an explicit catalog is still required for past sessions.

- **Bounds (STD-08 §R1, §R3).** At most **3 checked candidates per session
  per pass invocation**, shared across all its windows; duplicate or otherwise
  refused checked suggestions consume a slot too. At most **20 open candidate
  proposals store-wide**, checked under the same lock as every proposal write.
  Other proposal kinds do not count. At the cap or when no Daniel turn in the
  window fires a capture-rule trigger, the pass still updates its record,
  requesting zero candidates. Rejecting or approving frees an open slot.
- **Added output per window.** `candidates` is an array of at most 3
  `{quote, message, tag}` objects. Each quote is 8–400
  characters, each message id at most 128, and each tag at most 32:
  at most **1,680 value characters plus JSON syntax**, and no extra model
  call or timeout budget.
  A header compression retry keeps the original candidates locally. Older
  model adapters may omit the field, which means none. Relevant recorded or
  proposed statements add at most 6,000 value characters of input (15 spans
  of at most 400), selected by word overlap with the window.
- **Evidence.** Only Daniel's turns that fire a tag's capture-rule trigger
  ([§11.2](#112-capture-rules-per-tag)) may supply a candidate, under the
  tag named for that turn. Assistant turns are never used for candidates.
  Each candidate must name a firing turn in the current window and quote
  its own text exactly. The pass checks again against the original
  transcript and under the store lock against the config then on disk, and
  derives `admitted`, the cite, timestamp, prompt hash, line, cwd, project,
  provider and host itself. Pasted blocks, fenced or indented code,
  blockquotes, marked `#scribe` lines, unknown or future message ids, and
  truncation markers cannot supply a candidate; tool calls and tool results
  never reach a message's text (§3, transcript parsing), so no candidate is
  ever opened from a tool payload.
  Plain pasted prose without provider or textual paste markers cannot be
  distinguished from typed prose; this remains an accident guard.
- **Duplicates.** The model is instructed to exclude statements an entry or
  proposal already records, including paraphrases, using the bounded relevant
  context (`candidate_exclusions`). Under the lock, every readable entry and
  every candidate in any state is checked again, across tags and projects,
  with the one lexical predicate `covers_statement`. Entry creation uses
  the same predicate through `covering_entry`, narrowed to the oldest
  active entry of the same tag and project. Shared normalization and
  tokenization (one definition, STD-02 §R24): NFC and whitespace folding,
  trim, lowercase, curly apostrophes folded to `'`, then ordered tokens. A
  token is a run of alphanumeric characters; an interior apostrophe stays,
  so `don't` is one token (`dont` is the apostrophe-less spelling). Equal
  token sequences match at any length, so case and punctuation collapse.
  Otherwise an existing statement covers a new one only when the new one
  adds nothing: it is a contiguous window of the existing tokens, it has at
  least four tokens, and the two sides use the same counts of `not`,
  `never`, `no`, `don't`, `dont`, `can't`, `cant`, `cannot`, `won't`,
  `wont`, `shouldn't`, `shouldnt`, `doesn't`, `doesnt`, `didn't`, `didnt`,
  `isn't`, `isnt`, `avoid`, `except`, `unless`, `only` and `without`.
  A longer refinement, a reordering, a match inside a word, or a polarity
  or exception change is kept. Candidate opening uses this predicate, not
  symmetric containment or Jaccard, so the same definition cannot suppress
  a refinement or an opposite as a candidate; only the set of records it
  consults is broader than entry creation. Meaning-level comparison of a
  paraphrase stays the model's same-call exclusions. The lexical check does
  not match a short statement against the long raw paragraph it summarizes.
  Unreadable entries
  suppress candidate creation for that window. Quotes needing redaction are
  omitted so approved content can remain an exact quote.
- **Persistence and consent.** Candidates are written before the record cursor
  advances, after the record revision and sealed segments pass their checks.
  Interrupted writes replay without duplicate proposals. No entry exists yet.
  Session-start's open proposal count includes candidates, and `proposal list`
  shows them with their source quote and turn. Only a typed `#scribe approve
  <id>` creates the reserved entry as Daniel's, with the quote as content and
  the original turn's cite and `content_from: message` (user). `said` records
  the canonical approving command; the proposal also keeps the approving
  turn's cite. Previously stored format-10 assistant-turn candidates still
  approve into one agent-authored entry (`{kind: agent, name: assistant}`),
  with `content_from: message` (assistant), the candidate's evidence
  pointer (or `session:<id>` for a finding that named none) and the
  turn's date as its as-of — never an entry attributed to Daniel; its
  approval echo names the cancel. Replaying approval creates no second
  entry. Approving a candidate whose statement an active entry of the same
  tag and project already covers — an entry added after the candidate
  opened — writes no second entry. The proposal becomes `rejected` and
  `newer` stays empty, because a candidate's stored shape rejects a
  `newer` value. The reply names the covering id (`scribe: approved <id> —
  already covered by <entry>; no new entry`); replaying that close is the
  ordinary already-rejected refusal and does not name the id again. The
  covering id is not stored on the proposal, and that typed close has no
  closed reason. An interrupted approval whose entry file is
  missing is closed by a repeated approval or by `consistency
  scan`, which reports it in `covered` and writes one audit line whose
  target is the proposal id; since format 15 it keeps its settlement (and
  an agent's instruction) and records `closed_reason: covered`, as does an
  approval on Daniel's instruction that finds its claim covered
  ([§12](#12-settling-and-deleting-on-daniels-instruction-proposed)). Approved proposals
  journal the entry creation, recoverable by a repeated approval or the
  consistency scan. Rejection writes no entry and keeps the statement in the
  suppression set. Agents cannot edit candidate proposals, and settle them
  only on his checked instruction ([§12](#12-settling-and-deleting-on-daniels-instruction-proposed)). Forgetting
  the resulting entry blanks the candidate's copy too (its `quote`, `evidence`
  and `admitted.matched`) and recovery preserves
  the forgotten entry.

The in-session skill offers ready-to-type preference lines only
when the session is off or unknown. When on, it leaves proposals to this pass;
action entries continue through their separate agent-write route.

### 5.4 Session links

As built ([ORB-23849]): Daniel links a session to the one it was spawned
from or continues, by typing `/scribe on --from <id> [--type <type>]` or
`/scribe link <id> [--type <type>]` in the child. The link is stored once,
on the child's record (`links`), or on its switch's `pending_links` until
the first pass writes the record; children are derived at read time. A
linked session's pass reads each linked record's latest title and summary
as data. The refusals and replies are in
[specs/output-contract.md](./specs/output-contract.md#hook-replies); the
reasons in
[4_decisions.md](./4_decisions.md#session-links-are-typed-stored-on-the-child-and-manual).
Neither the hook payloads nor the transcripts carry a parent session id,
so nothing is linked automatically. The session-start brief can remind
Daniel of the command, with the previous id filled in, when a new session
is not itself opted in and an active session in the same project (the
working directory's git remote name, else its directory name) was updated
within the last day — the one updated most recently
([§7](#7-consistency-routine)). The reminder writes nothing: the link
still exists only when he types it. No line when this session is already
opted in (its switch is on, or it already has a record), when no recorded
session in the project is that recent, or when the store cannot be read.

### 5.5 The log: segments and sealing

Designed for S9 ([ORB-23892]). A pass keeps the window loop of
[§5.3](#53-a-pass); what each window sends and writes changes.

**Sent.** The header (title, summary, goals, next, topics, terms), the open
segment in full (number, heading, message range, text), and the sealed
segments as a table of contents: number, heading, dates, never their text.
Then the window's messages, and linked sessions' summaries as before.
The prompt asks for a summary of at most 900 characters, leaving room below
the 1200-character hard limit, and states the current summary's Unicode
character count (0 for a first pass). The summary stays a compact overview;
the log carries the details as the session grows. `next` and segment text
get the same headroom ([ORB-24918]): `next` aims for 750 of its 1000
characters, counting a `- ` prefix per item and a newline between items,
with the current `next`'s count and the instruction to drop done or
superseded steps rather than only add; segment text aims for 700 of 900,
with the open segment's count, and a segment that would pass 700 is left
as it is and a new one started. A long session carries its `next` forward
near the limit, so without a target each added item overshot it.

**Answered.** `{title, summary, goals, next, topics, related, terms, open,
new}`: the rewritten header; `open`, the open segment's rewritten
`{heading, text, topics, related}`; and `new`, 0–3 further segments, each
`{starts_at, heading, text, topics, related}`, where `starts_at` is the id
of the window message the segment begins with. Giving a new segment seals
the one before it. When the record has no open segment (a first pass, or
scribe sealed the last one), ask for `open: null` and at least one `new`
segment starting at the window's first message.

scribe tolerates an omitted or null `open` when the request had an open
segment: keep its heading, text, topics and references unchanged. Its range
and window count still advance normally, and new segments or the usual
sealing rules can seal it. When there was no open segment, an unexpected
`open` object becomes the first `new` segment, starting at the window's
first message. It must pass the same content, ordering and total 3-segment
limit as every new segment. Refuse conversion when it is impossible: an
empty window, a duplicate first-message start, too many segments, or invalid
segment content. Neither fallback changes any sealed segment.

**Checked.** Header budgets (title 120, summary 1200, goals 600, next 1000;
a `## ` line inside a section, an empty summary or an invalid topic is
refused, as today); segment headings at most 80 characters, texts 1–900
characters, no `#` heading line in a text; `starts_at` ids in the window,
in transcript order, each after the open segment's last message, and no
two segments starting at one message. A refused answer gets one retry
naming the refusal. If the only defects are header length overruns, send
only its prose header (`title`, `summary`, `goals`, `next`) to a compression
request, with its current summary length and the budgets (still asking for
900 summary characters). No messages, linked records, segments or terms
are resent. Keep the first answer's topics, references, terms and segments,
replace only those four prose fields, then validate the complete answer
again against all hard limits. Any other defect, including one hidden
behind an overlong header, uses the fresh full-window retry. Both paths
share the same one-retry allowance. After that retry, scribe repairs an answer
whose only remaining defects are these budget and start misses:

- Cut overlong `title`, `summary` and `goals` at the last sentence boundary
  within their Unicode-character budgets. Cut `next` at the last complete
  bullet within 1000 characters, counting `- ` prefixes and newlines. If no
  such boundary fits, cut at the last word boundary with `…` inside the
  budget (at a Unicode character when one word itself is too long).
- Cut each segment text over 900 at a sentence boundary, with the same word
  fallback. When the open text was cut and there are no new segments, seal
  it at this window's end. When there are new segments, keep all of them
  with their cut text and continue the normal range and sealing rules.
- A `new` start outside the window or at/before the previous open segment's
  last message becomes a continuation of that open segment: append its text
  with a blank line when the combined text fits 900. Keep the open heading,
  merge references and retain the first five distinct topics in input order.
  If the combined text would exceed 900, keep the new segment and start it
  at the first window message that follows the open segment's last message
  in the transcript and precedes every start the model kept (never one the
  final check would refuse). When no window message qualifies, the content
  ends the open segment instead: the combined text is cut at 900 as above
  and the open segment seals at this window's end. With no open segment,
  keep the new segment and start it at the window's first message, ahead of
  valid later starts. Never discard another new segment to make room.

The uncut answer must still pass all other shape checks: truncation cannot
hide a heading line or invalid topic. Reversed or duplicate starts within
the window, excessive segment counts and collisions after reanchoring are
still refused. The repaired answer is checked again before record creation;
redaction and store validation still run before the atomic write. Sealed
history is never repaired. Each persisted repair is reported in the pass's
`warnings` (and on stderr), naming the invocation's window and the repaired
field or segment. The same path serves routine passes, rebuild and catalog.
A nonrepairable second refusal or a failed model retry fails the pass and
keeps the previous checkpoint. The bounded, redacted `error` includes the
initial refusal and the retry's refusal or model failure.

**Written.** Message ranges follow from `starts_at`: a segment runs to the
message before the next segment's start, the last one to the window's
last message. The open segment's `windows` counts up. scribe seals the
open segment itself when its `windows` reaches `[catalog] segment_windows`
(3), when an overlong open text was repaired without a new segment, or when
the final pass's last window is written (a `final` record has no open
segment). The store refuses a write that would
change a sealed segment's metadata or text from what is on disk
(`sealed_segment_changed`, an internal error that fails the pass and keeps
the record), so the append-only rule is code, not prompt.

Why three windows: one segment then covers at most about 120 messages or
45,000 characters before scribe seals it, so a run of rewrites can
compress at most that much into 800 characters, and the model is never
asked to keep more than one segment's worth of history in mind
([4_decisions.md](./4_decisions.md#a-segment-seals-when-it-is-full-when-the-thread-changes-or-after-three-windows)).

`session show <id>` prints the header and the log's table of contents
(number, heading, dates, state); `--log` adds every segment's text;
`--segment <n>` prints one segment. `session show session:<id>#<n>` is the
same as `--segment <n>`, so a citation can be pasted as is.

### 5.6 Rebuilding a record

Designed for S9. `scribe session rebuild <id> --author <name> --confirm`
rebuilds a record from its transcript, from the first message, with the
format-4 pass ([§5.5](#55-the-log-segments-and-sealing)) and no prior
record. It is how an upgraded record (one sealed segment holding the old
summary and outcome) gets a real log, and how a record written before a
prompt fix is redone.

- The transcript is found and checked as a pass finds it. A session whose
  transcript is gone is refused `transcript_missing`, and its record stays
  as the upgrade left it.
- Without `--confirm` it refuses (`confirm_required`) and says what it
  would replace: the number of segments and passes in the current record
  (STD-01 R5).
- It runs every window with no deadline and no five-session cap. Each
  window is checkpointed to `sessions/<yyyy>/<mm>/.<id>.rebuild.md`, a
  dot-file every read skips, so an interrupted rebuild resumes there on
  the next run; the record itself is untouched until the last window.
- The last step, under the lock, replaces the record with the rebuilt one
  only if the record's `updated` is what rebuild read when it started;
  if a pass wrote in the meantime, rebuild folds in the newer messages
  first and tries again. Ordinary passes mirror this: before each
  window's write they compare, under the store lock, the `updated`
  revision they loaded with the record on disk, and on a mismatch refuse
  that write with `record_changed`, keep the session due and leave the
  record on disk unchanged, so the next pass starts from the rebuilt
  record and a pass that loaded its record before a rebuild finished
  cannot overwrite it. It keeps `id`, `created`, `links`, `active`,
  `keep_active`, `active_change` and `cataloged_by`; segments are
  `origin: rebuild`; `scribed_through` is the transcript's last message;
  `status` is `final` when the switch's `final` is set or the session was
  cataloged as ended.
- A session that is on and due is refused `session_busy`; `scribe session
  pass --session <id>` first, or wait for the routine.
- It is a mutating command: one audit line (`target` `session:<id>`).
- Its tests replay recorded model answers through the built binary with no
  live model ([specs/replay-fixtures.md](specs/replay-fixtures.md)): a
  checked-in synthetic recording reproduces each budget, `starts_at` and
  open-segment failure seen on a real long session, and
  `scripts/redact-recording.sh` turns a real recording into one that holds
  no readable text ([ORB-23995]).

### 5.7 Active and inactive sessions

Designed for S8 (the flag and the read paths) and S10 (the routes). Daniel,
2026-11-10: "we should add \"isActive\" field to sessions so that scribe or
based on configurable duration limit can periodically deactivate stale
sessions", and "A feature that no longer exists (if it exists in session
store) should be deactivated."

`status` says whether the session has ended (`open`, `final`). `active`
says whether its content still describes something current. A final
session about a feature that still exists stays active; an open session can
be deactivated. An inactive session is hidden, not removed, as superseded
entries are: `session list`, `record search` and `ask` leave it out by
default and show it with `--include-inactive` (lists), `--include-hidden`
(search) or `include_hidden` (`ask`); `session show` always shows it, with
its `active_change`; `ask` counts hidden matches and says one exists, as
for entries.

Three routes change `active`, each audited and each recording `{at, by,
route, reason, cite}` in `active_change`:

| Route | By | When | Reason / cite |
|---|---|---|---|
| `removed` | the consistency routine, `{kind: scribe, name: consistency}` | a later entry Daniel wrote says the session's feature was removed or replaced ([§7](#7-consistency-routine)) | the model's reason; cite the later entry (its entry id) |
| `age` | the consistency routine | the session's last message (its last segment's `to`) is older than `[sessions] inactive_after_days` (120) | `no messages since <date> (limit <n> days)`; no cite |
| `manual` | an agent or Daniel: `scribe session deactivate <id> --author <name> --reason <text> [--cite <ref>]`, `scribe session activate <id> --author <name> [--reason <text>] [--pin\|--unpin]` | any time | the given reason and cite |

The goal is context for features that still exist, and such a session can
be old. So removal of the feature is the main route, the age limit is
generous, and there is an exemption: `keep_active: true`, set with
`session activate <id> --pin` and cleared with `--unpin`, exempts a
session from both automatic routes. A manual `deactivate` of a pinned
session also unpins it, and says so. Age is measured from the last message,
not `updated`, because a rebuild, a link or a catalog rewrites `updated`
without new conversation
([4_decisions.md](./4_decisions.md#removal-of-the-feature-is-the-main-deactivation-route-age-is-a-generous-backstop)).

scribe stays generic: it never reads a repository to decide whether a
feature exists. That check belongs to the agent or routine that calls
`deactivate` and gives the reason; the consistency routine judges only from
scribe's own records. Activating an active session, or deactivating an
inactive one, changes nothing and writes no audit line. Deactivation never
changes a record's text, its `updated`, or a pass's cadence; an inactive
session that is on is still passed.

### 5.8 Cataloging a past session

Designed for S14. Daniel, 2026-11-10: "we need to backfill some of
important chat sessions on orbit and scribe projects. not everything but
certain long ones", then "we don't need to go all the way back to aug.
those will just add noise. the primary goal is to understand the context of
existing features, to provide continuity ... month of oct will be a good
starting point." The scope is Claude Code sessions on the Mac from
2026-10-01 for orbit, scribe and constellation-site; long ones only; Daniel
picks. scribe never scans or catalogs on its own
([4_decisions.md](./4_decisions.md#backfill-is-by-name-a-session-daniel-picks)).

**Choosing: `scribe transcript list`.** `scribe transcript list --since
<date> [--until <date>] [--project <slug>]… [--min-turns <n>] [--limit
<n>]` lists the Claude Code transcripts under the projects directory that
have no record and no switch, model-free, newest first. A row is
`{session, project, project_mentions, cwd, started, ended, bytes,
daniel_turns, first_prompt}`: `daniel_turns` counts Daniel's typed turns
(the pass's rule for whose turn is whose), `--min-turns` defaults to 20 so
only long sessions show, and `first_prompt` is his first turn cut to 300
characters. It reads transcripts and writes nothing: no lock, no audit.
`scribe transcript show <id>` prints one row with Daniel's first five
turns, each cut to 300 characters, for the tmux preview.

Project. Most sessions start from the constellation root (463 of 947
transcripts on the Mac), so the cwd alone does not say which project a
session was about. `project_mentions` counts, per configured project, how
many times the transcript's text mentions any of that project's
`[projects.<slug>] paths` (for example `codebases/orbit`,
`codebases/scribe`, `constellation-site`), and `project` is the slug with
the highest count, else the cwd's slug, as today. `--project` filters on
that attribution. The paths are Daniel's config; scribe knows no project
names of its own.

**Cataloging: `scribe session catalog <id>`.** `scribe session catalog <id>
--author <name> [--project <slug>]` switches the named session on as a
cataloged-by-name session and runs its passes to the end. Who cataloged
it is a channel. The CLI records `kind: agent` under the caller's name and
refuses the reserved names (`daniel`, `scribe`, `consistency`)
`reserved_author`, with or without a terminal. Daniel catalogs as himself
by typing `/scribe catalog <id>` (an id or a unique prefix) in a Claude
Code prompt: the `UserPromptSubmit` hook makes the same checks, writes the
same switch with `cataloged_by: {at, by: {kind: daniel}}` and blocks the
prompt with what it did, but runs no window, because a catalog can be dozens
of model calls. The next `session pass` runs them (a pass of a switch with
`cataloged_by` runs as a catalog: `origin: catalog`, the attribution
kept, final and off when the transcript has ended), or `session catalog
<id>` in a shell runs them now; either keeps Daniel's attribution
([4_decisions.md](./4_decisions.md#daniel-writes-only-through-his-typed-prompt-agent-names-are-attribution)):

- The transcript is found and checked as a pass finds it. A session that
  already has a record or a switch is refused `session_cataloged` (use
  `session pass --session` or `session rebuild`), unless the switch is one
  `catalog` wrote (`cataloged_by` set) and still due: then `catalog`
  resumes it.
- It writes the switch with `since` now, `due`, and `cataloged_by: {at,
  by}`, then runs every window ([§5.5](#55-the-log-segments-and-sealing))
  with no 50 s routine deadline and no five-session cap. Segments are
  `origin: catalog`.
- `project` is `--project`, else the `transcript list` attribution. The
  record's `on_since` is the switch's `since`; `started` is the
  transcript's first message.
- When the transcript has not been written for 24 hours the session has
  ended: the last window marks the record `final` and the switch off, so
  no hook ever passes it again. A transcript written to within 24 hours is
  left on and due, and the hooks carry on as for `/scribe on`.
- A failure keeps the checkpoint and the switch due, as a pass failure
  does, and running `catalog` again resumes it.
- Audited like a pass, plus one line for the switch when the catalog
  wrote it and no window failed. A failed catalog is one failure line
  (`target` `session:<id>`, `error_code` such as `model_failed`), not a
  success line for the switch and then a failure: the switch it left due
  is that session's, named by the failure line.

The result is a model-written session record, labelled as a summary
wherever it is evidence. Cataloging creates no entries. A session only
about a feature since removed is skipped when Daniel picks, or deactivated
if it was cataloged ([§5.7](#57-active-and-inactive-sessions)). The few that
matter most are also distilled into discussions ([§4.5](#45-distilling-a-discussion),
S7). It runs by hand from the CLI, a few sessions at a time.

## 6. `ask`

### 6.1 Contract

Input: `{"question": string, "scope"?: {"tags"?: [..], "topics"?: [..],
"project"?: string, "kind"?: "entry" | "session", "since"?: date},
"include_hidden"?: bool}`. Output:

```json
{
  "status": "answered | no_record | conflict",
  "answer": "…",
  "claims": [{"text": "…", "quote": "…", "cite": "preferences-12"}],
  "conflicts": [{"cites": ["preferences-3", "preferences-9"], "note": "…"}],
  "evidence_considered": 14,
  "dropped_quotes": 0
}
```

`cite` is an entry id, `session:<id>`, `session:<id>#<n>` (a segment,
from S8) or `task:<id>`. A lookup
([§6.5](#65-lookups-records-by-recency-or-id)) also sets `"status":
"record"` and returns `records`, the stored records themselves. `scribe question ask
"<question>"` is the same operation for Daniel at a terminal (`--json`
prints this payload).

### 6.2 Retrieval (model-free)

0. Filter by frontmatter first. `scope` fields (`project`, `topics`,
   `tags`, `since`, `kind`) and any project name, topic or date the
   question names ("preferences on ci this month") restrict the candidate
   records before ranking.
1. Load every `active` entry (plus `superseded`/`retired` when
   `include_hidden`), every session record, every discussion (the store's
   and the read-only sources'), and up to 20 Orbit tasks from
   `orbit.search` with the question's terms (title, status, and the first
   1,000 characters of the description).
2. Tokenize: lowercase, split on non-alphanumerics, drop stopwords and
   tokens under three characters, light suffix stripping (`-s`, `-es`,
   `-ing`, `-ed`). Tag names and aliases in the question boost their tag.
3. Rank with BM25 over `content` + `said` + tag for entries, the four text
   fields for sessions (from format 4, the header and each segment as
   separate units, [§6.7](#67-segments-authors-and-inactive-sessions)),
   title + description for tasks; Daniel's entries get a 1.5×
   weight because they are his explicit words (agent entries do not).
4. Keep the top 20 (entries first on ties), packed newest-first within a
   48k-character evidence budget.

If nothing scores above zero, `ask` returns `no_record` without calling a
model.

### 6.3 Answering

The `ask` model (the decision model, Sonnet by default) receives the
question and the numbered evidence, each item labelled with its kind, date,
whose words it is (`Daniel`, `assistant (recorded by Daniel)`, `session
summary`, `Orbit task`) and its status. The system prompt requires: answer
only from the evidence; every claim carries a verbatim quote and a cite; say
`no_record` when the evidence does not answer; when two items disagree,
return `conflict` with both cites rather than choose; never present a
summary or an assistant's words as Daniel's. The response is a JSON schema
(`claims`, `conflicts`, `status`, `answer`).

### 6.4 The verbatim check

`verbatim::contains(source, quote)`: both strings are NFC-normalized, `\r\n`
becomes `\n`, and runs of whitespace collapse to one space; the quote's own
leading and trailing space is dropped, so a span cut at a line break still
matches. The quote must then be at least 8 characters and a substring of
the source. No case
folding, no punctuation stripping. The same function checks referent spans
([§4.3](#43-resolving-this-and-it)), proposal quotes ([§7](#7-consistency-routine))
and answers. In `ask`, a claim whose quote fails is dropped and counted in
`dropped_quotes`; if every claim drops, the status becomes `no_record`. A
claim whose cite is not in the evidence is dropped the same way. When the
evidence contains a hidden entry that matched (and `include_hidden` is
false), the answer notes that a superseded record exists, without quoting it.

### 6.5 Lookups: records by recency or id

`ask` takes any question, and orchestrators use it for handoffs: "can you
fetch me last session summary?" must return the latest session record, not
a ranked guess. Word-overlap ranking cannot answer that ("last" matches
nothing), so before §6.2 a model-free classifier recognises lookups:

| Asks for | Returns |
|---|---|
| the latest/last/previous session (optionally for a project or topic) | the newest session record matching that frontmatter, by `updated` |
| a named session (`session:<id>`, or a title match) | that session record |
| open next steps / what is pending | the `next` items of the newest records in scope, newest first |
| the entries of a tag in a period ("preferences this week") | those entries, newest first |
| what was concluded about a subject ("what did we conclude about level 4?") | the discussions whose title, topics or terms match, newest `updated` first, with thesis and takeaway verbatim; the store's before the read-only sources' |
| a term Daniel uses ("graduate to level 4", "what did I mean by resident orchestrator") | the session records whose `terms` contain it (case and whitespace normalised), newest first, each with the defining `said` span, plus entries whose body contains it |
| the last orchestrator or on-call session for a workspace | the newest `orchestration`-tagged assignment task in `ws_constellation` whose title names that workspace, with its execution summary verbatim, cited `task:<id>` |

Terms exist because Daniel's main conversational agent is a manager he
discusses designs and ideas with, and his vocabulary carries meaning across
those sessions: when he says "graduate to level 4", any agent can `ask` for
the session where he defined it instead of guessing. A summary written by
Haiku may paraphrase a term away, so terms are kept in Daniel's exact words.

A lookup returns `"status": "record"` and `records` with each record's
stored fields verbatim (summary, goals, outcome, next, timestamps, cite),
without a model call and without paraphrase. A question that is not a
lookup, or a lookup that matches nothing, falls through to §6.2. When the
classifier is unsure it falls through too, so a lookup never hides an
answer.

### 6.6 As built (S5)

`scribe_core::ask::answer` runs the whole operation over records the caller
loaded and two ports: `Models` (`ask` request, `models.ask`, 90 s) and
`Tasks`, which the CLI implements as `OrbitTasks` over `orbit tool run
orbit.search` (`kind: task`, `all: true`) and `orbit.task.show` for each hit,
15 s a call and 30 s a search, from a cleared environment of `PATH`, `HOME`,
`LANG` and the `ORBIT_*` variables (inside the plugin, Orbit's callback
channel). A task source that fails is logged and costs only its evidence.
`question ask` and `plugin call` both call `commands::question::answer_with`,
so they return the same payload; it adds `hidden_matches`, `lookup` (`null`
or `last_session`, `named_session`, `next_steps`, `tag_period`, `term`,
`orchestrator_session`), `records` and `warnings` to §6.1's fields
([specs/output-contract.md](./specs/output-contract.md#ask)).

- **Lookups**, tried in this order, each falling through when it finds
  nothing: a session named by `session:<id>` or by its whole title (at
  least 8 characters) in the question; the last orchestrator session (a
  recency word, `session`, `on-call`/`orchestrator`/`orchestration`, and the
  workspace as the word after the last `for`/`in`/`of`): the newest
  `orchestration`-tagged task whose title has that word, preferring one with
  an execution summary; the last session (a recency word and `session`),
  narrowed to a project (slug or its last segment) or topic the question
  names; open next steps (`next steps`, `pending`, `open items`): the newest
  records with `next_open > 0`; a tag (name or alias) with `today`, `this
  week` (from Monday), `this month` or `this year`: its entries created
  since, newest first; and a term: the question, less a leading "what did I
  mean by", "what is", "define" and the like, equal (case and whitespace
  folded) to a stored term, returns those sessions with the matching terms,
  then entries whose body has the phrase. At most 10 records. Discussions
  are S7's. `evidence_considered` counts the records returned.
- **Retrieval** follows §6.2 with these rules: `tags` keeps entries only;
  `tags`, `topics` or `kind` leave tasks out; `since` applies to `updated`
  (a task without one is dropped). A project or topic the question names
  narrows the store's records only when one of them still matches a query
  word; dates the question names narrow nothing yet (the tag-period lookup
  covers "preferences this week"). Stemming strips `-ing`, `-ed`, `-es`, `-s`
  (not after `s`) and then a final `-e`, so `merge` and `merging` meet.
  Superseded and retired entries are scored either way: without
  `include_hidden` they are left out and counted in `hidden_matches`, and
  the answer ends with a note saying so. Forgotten entries are never
  evidence.
- **The check** drops a claim whose cite is not in the evidence or whose
  quote is not verbatim in that item, and a conflict naming fewer than two
  evidence items. A surviving conflict makes the status `conflict`; no
  surviving claim (or the model's own `no_record`) makes it `no_record` with
  the fixed answer "Nothing recorded answers this.". A model failure is an
  error (`question ask` exits 1 with its code).

### 6.7 Segments, authors and inactive sessions

Segments and inactive sessions are built (S8); authors' weights and labels
are built in S12.

- **Units.** From format 4 a session is ranked as several units: its header
  (title, summary, goals, next), cited `session:<id>`, and each segment
  (heading and text), cited `session:<id>#<n>`. Each unit carries the
  record's frontmatter for §6.2 step 0, and a segment its own topics and
  `from`/`to` dates as well, so "what did we decide about X in October"
  narrows to segments of that month. A claim citing a segment is checked
  against that segment's text; one citing the header against the header.
  Lookups ([§6.5](#65-lookups-records-by-recency-or-id)) still return the
  record; a `last_session` or `named_session` record carries the header and
  the log's table of contents, and `records[].session.log` holds every
  segment (output contract).
- **Inactive sessions** are left out of evidence and lookups unless
  `include_hidden`, and counted in `hidden_matches` like hidden entries.
- **Authors.** Daniel's entries keep the 1.5× weight; agent entries get
  1.0×, like session units. Evidence labels an agent entry `agent note
  (<name>)`, never `Daniel`, and the system prompt forbids presenting it
  as Daniel's words. A `conflict` between a Daniel entry and an agent entry
  is reported as such, with both labels.

### 6.8 `record search`

Designed for S11. `scribe record search "<terms>" [--kind
entry|session|discussion] [--project <p>] [--topic <t>]… [--since <date>]
[--until <date>] [--include-hidden] [--limit <n>]` runs §6.2's filtering,
tokenizing and BM25 ranking with the §6.7 units and weights, and stops
there: no lookup classifier, no Orbit tasks, no model. It is the fast,
offline path for a known word; `ask` stays for vague questions and
synthesized answers. Hidden entries and inactive sessions are left out
unless `--include-hidden`; forgotten entries never match. The default limit
is 20. Each hit is `{kind, cite, id, segment, score, title, snippet,
project, topics, author, active, updated}`: `cite` is what `ask` would
cite, `segment` the segment number or `null`, `snippet` up to 240
characters around the best-matching terms, `author` an entry's author else
`null`. The `--json` envelope is the list envelope with the key `hits`,
best first. It reads only and writes no audit line. Discussions are
searched once S7 builds them, together with the read-only sources.

As built (S11): `scribe_core::search` builds the units with `ask`'s own
`store_docs` and ranks them with its `rank` (the named-tag boost and the
§6.2 step 0 narrowing included), so the two cannot drift; only the Orbit
tasks are missing from the corpus, so a score can differ slightly from the
one inside `ask`, never the method. `--kind` takes `entry` and `session`
until S7 builds discussions. Terms with no word ranking can use (all
stopwords or shorter than three letters) are a usage error. `--topic`
repeats and every one must match, as in the lists; a segment carries its
session's topics and its own. `--since`/`--until` keep an entry created on
a day in the range, a session header whose `started`–`updated` span
overlaps it, and a segment whose `from`–`to` does. A hit's `title` is a
session's title, a segment's heading or `<id> (<tag>)` for an entry;
`active` is `null` for an entry, and an additive `status` field carries
the entry's or session's status. When hidden records matched and are left
out, stderr says how many.

## 7. Consistency routine

> Proposed change ([§13.5](#135-consolidation-the-weekly-consistency-routine),
> not built): the scan also merges and supersedes memory items and
> completes their promotions, directly and without proposals.

The `scribe-consistency` routine (`0 6 * * 1`, weekly, seeded disabled)
runs `scribe.consistency-scan`. First it re-applies each approved proposal
whose older entry is still `active`: approval writes two files, and this
repairs a crash between them
([4_decisions.md](./4_decisions.md#one-markdown-file-per-record-metadata-in-yaml-frontmatter)). Then:

1. **Candidates, model-free.** Within each tag, pair active entries whose
   token sets overlap (Jaccard ≥ 0.25 over content tokens) or share an
   explicit subject term; at most 40 pairs per run, newest pairs first.
   Entries past `review_after` are stale candidates. Pairs already settled
   (rejected or approved) are never proposed again.
2. **Judgement.** The `consistency` model (Haiku) labels each pair
   `contradicts`, `refines` or `unrelated`, with a reason and a verbatim
   quote from each side. A label survives only if both quotes pass the
   verbatim check.
3. **Proposals.** `contradicts` and `refines` become `supersedes`
   proposals (the newer entry supersedes the older by default; the model
   may not reverse the order); stale candidates become `stale` proposals.
   Nothing changes on an entry yet.

Daniel sees open proposals at session start (the `SessionStart` hook returns
`scribe: 3 proposals to review — scribe proposal list`). The same hook adds
one further line when the session starting is not opted in and an active
session in that working directory's project was recorded within the last
day: `scribe: this may continue <id> (recorded) — type /scribe on --from <id> to keep recording`,
naming the record updated most recently. It does not write the link
([§5.4](#54-session-links)). There is no such line when this session is
already opted in (its switch is on, or it already has a record), when no
active record in the project was updated that recently, or when the
sessions cannot be read; a read error is logged and the session still
starts. The agent is not told. Daniel settles each open proposal in a
typed turn: `#scribe approve p-4` sets the older entry `superseded` (or
`retired` for a stale proposal) and records `superseded_by`; `#scribe reject
p-4` closes it for good. `#scribe retire <entry-id>` hides an entry directly;
`#scribe forget <entry-id>` blanks its text for good. Up to S11 agents
cannot settle proposals: settling is a `#scribe` instruction, which only a
user-typed turn carries. From S12 an agent may settle, with `scribe proposal
settle`, a proposal whose older entry is agent-authored; one about an entry
Daniel wrote stays his, settled only by `#scribe approve|reject`
([§1.1](#11-the-cli-agent-surface)). The orbit-office contradiction detector
(`app/packages/server/src/features/scribe/contradictions.ts`) contributes the
shape — newer side by date, held items, a resolve step — but not its
inputs, which were Orbit tasks fed by a ledger extractor that no longer
exists here.

As built (S6): `scribe consistency scan` (and `scribe.consistency-scan`)
runs the steps above in `scribe_core::consistency`, pure, with
`Store::reapply_approved`, `Store::open_proposals` and `Store::settle` the
three locked writes. Candidates are same-tag active pairs that share a
topic hashtag (the "explicit subject term") or whose content tokens
(`ask`'s tokenizer: lowercased, stopwords out, light stemming) overlap by
at least one in four; a pair with a proposal in any state, in either
order, is never offered again, nor is an entry with a `stale` proposal.
The 40 pairs go to the model ten per call (`CONSISTENCY_SCHEMA`: one
judgement per pair, `pair` its number in the batch), each call within
120 s; a failed batch is reported (`failed_batches`) and audited, and its
pairs are offered again next scan. A label for an unknown pair, a second
label for one pair, an unknown label or a quote that is not verbatim in its
entry is dropped and counted (`dropped_labels`); `unrelated` is counted,
not stored, so an unrelated pair is judged again by later scans
([§9](#9-concerns--honest-limitations)). A `stale` proposal's reason is
scribe's own sentence, and its quote the entry's first 200 characters. The
store numbers the drafts `p-<max+1>` under the lock, checking again that
both entries are still active and the subject has no proposal, and masks
their text with the redactor. The scan changes an entry only by
re-applying an approval. A pair the model labels `refines` becomes a
`supersedes` proposal like `contradicts`; topic-merge proposals are not
built.

**Deactivating sessions** (designed for S10, [§5.7](#57-active-and-inactive-sessions)).
After the entry steps the scan runs two session steps, skipping pinned
(`keep_active: true`) and inactive sessions:

1. **Age, model-free.** Every session whose last message is older than
   `[sessions] inactive_after_days` is deactivated, route `age`.
2. **Removed features, model judgement.** Candidates are model-free pairs
   of an active session S and a later entry R Daniel wrote (`author.kind`
   is `daniel`, active, whose date is after S's last message, written or
   updated in the last 8 days, so a weekly scan sees each new entry once)
   that share a topic or a term with S or overlap its header by at least
   one token in four, and whose text has a removal cue (`removed`,
   `deleted`, `dropped`, `decommissioned`, `retired`, `replaced`,
   `superseded`, `ripped out`, `sunset`). **Evidence rule:** only
   Daniel-authored entries count. Session headers and segments are
   model-written — a pass summarizes a transcript that may hold injected
   text — and agent entries are agent-written (`entry add`), so counting
   either would let supplied words always satisfy the verbatim quote
   check and hide a session from `ask`. Sealed segments are excluded too:
   sealing proves a segment never changed after it was written, not that
   Daniel wrote it
   ([4_decisions.md](./4_decisions.md#removal-evidence-is-daniel-authored-entries-only)).
   At most 20 pairs per scan, ten per call, within 120 s each.
   The `consistency` model labels each pair `removed` (R says the feature S
   is about was removed or replaced), `still_current` or `unrelated`, with
   the feature's name and a verbatim quote from R. A `removed` label whose
   quote passes the verbatim check against R deactivates S, route
   `removed`, reason the model's (at most 300 characters), cite R. Other
   labels are counted, not stored.

The routine deactivates directly rather than proposing: an inactive
session is hidden, not changed, and `session activate` reverses it. Each
deactivation is one audit line (`target` `session:<id>`) and an
entry in the scan report's `deactivated`.

The settling verbs run in `scribe hook user-prompt-submit`, in prompt
order after the transcript check, one locked `Store::settle` each:
`approve` of a consistency proposal needs an open proposal whose older
entry is still `active` (refused `entry_not_active` otherwise), and writes `proposals.json` before
the entry, so a crash between them is what the next scan re-applies;
`reject` of an approved proposal or `approve` of a rejected one is refused
`proposal_settled`; `retire` takes an `active` entry; `forget` takes any
entry not yet forgotten, clears `said`, `content`, `evidence`, `as_of`,
`quote`, `window` and `retirement_reason`, and clears every
proposal quote of it and the `reason`, candidate `quote`, `evidence` and
`admitted.matched` of every proposal about it. Session records are not erased. A repeated
instruction echoes `… — nothing changed` and is not audited; a change is
one audit line whose target is the proposal or entry, a refusal one line
with its code (`proposal_not_found`, `entry_not_found`,
`proposal_settled`, `entry_not_active`). `settled_by` is the typed turn's
cite with `message: null`: the `Stop` hook fills entries' message ids, not
proposals'.

## 8. Models

| Use | Config key | Default | Called from | Budget |
|---|---|---|---|---|
| Referent of "this"/"it" | `models.referent` | `claude-haiku-5-5` | `UserPromptSubmit` hook | 20 s |
| Session pass (also `session rebuild` and `session catalog`, from S9/S14) | `models.session` | `claude-haiku-5-5` | `scribe.session-pass` job, the CLI | 120 s |
| Consistency judgement (entry pairs; from S10 also removed-feature session pairs) | `models.consistency` | `claude-haiku-5-5` | `scribe.consistency-scan` job | 120 s per batch of 10 pairs |
| `ask` | `models.ask` | `claude-sonnet-5-5` | `scribe.ask`, `scribe question ask` | 90 s |
| Distilling a discussion | `models.distill` | `claude-sonnet-5-5` | `UserPromptSubmit` hook, run as a detached job like a session pass | 180 s |

All five go through one `Models` port with one adapter, `ClaudeCliModels`:
it runs `claude -p --safe-mode --tools "" --model <id> --output-format json --json-schema <schema-json>`
with the prompt on stdin, in its own process group, from a cleared
environment holding only `PATH`, `HOME`, `LANG` and the credential names
Daniel configures (`CLAUDE_CODE_OAUTH_TOKEN` or `ANTHROPIC_API_KEY`;
STD-05 R10), killed and reaped at the budget (STD-03 R11–R13).
`--safe-mode` turns off the hooks and plugins of the user's Claude Code setup,
so a model call never fires `scribe hook`; `--tools ""` removes the built-in
tools, so the model never receives tools. `--bare` is not used: it reads only
`ANTHROPIC_API_KEY`, and Daniel's login is OAuth. S3 checks both flags against
a real `claude -p` run. Why the CLI rather than an HTTP client:
[4_decisions.md](./4_decisions.md#models-are-called-through-the-claude-cli).
Tests use a fake `Models` implementation; no test calls a model.

As built (S3), in `crates/scribe/src/models.rs`: the argv is exactly
`-p --safe-mode --tools "" --model <id> --output-format json --json-schema
<schema>` (`--tools ""` is one empty argument); the environment is `PATH`,
`HOME`, `LANG`, then each name in `[models] credentials` that is set, and
never a `SCRIBE_*` name even when configured (STD-05 R11). The child leads
its own process group; its stdin is written from a thread and its stdout
(1 MiB) and stderr (64 KiB) drained while it runs. At the budget the group
gets SIGTERM, then SIGKILL after a 1-second grace; on every path, a clean
exit included, the group is swept with SIGKILL and the child reaped before
the call returns. The leader's exit is observed with `waitid(WNOWAIT)`,
which leaves it unreaped, so the group id is never signalled after it could
be reused (STD-03 R14). The answer is the result object's
`structured_output`, else its `result` text parsed as JSON (a ```` ```json ````
fence is unwrapped); `is_error: true`, a non-zero exit (with the last line
of stderr, cut to 200 characters), output that is not JSON or not the
schema's shape, and no `claude` on `PATH` are each a `ModelError`, which the
referent turns into the guessed fallback. Tests run a fake `claude` shell
script on `PATH` under a temporary `HOME`, recording its argv, environment
and process group.

Mutating commands — every `hook` event that writes, `session pass`,
`consistency scan` — are audited through the template's `AuditGuard`. Audit
lines hold ids and error codes only; `said`, `content` and model text never
enter the audit log. A hook run writes one line per entry written
(`target` the entry id), per refusal and per failed line (`target`
`session:<id>`, with its `error_code`) and per cite filled; a run that
changed nothing (no `#scribe` line, a replay, a `Stop` with no missed instruction or waiting cite)
writes none.

## 9. Concerns & Honest Limitations

- **Provenance is an accident guard.** Any process running as Daniel can
  pipe forged JSON into `scribe hook user-prompt-submit` or edit `~/.scribe`.
  The transcript checks stop a confused agent, not a hostile one (STD-05 R5).
  Agents have shell access, so "main agents see only `ask`" holds for the MCP
  surface, not for the shell.
- **Claude Code hook details are partly verified.** S2 confirmed against
  Claude Code 2.1.317 on dk-server-1 that `plugin/claude-code/` validates
  (`claude plugin validate`, also `--strict`), installs from the local
  marketplace into a throwaway `HOME`, and is listed with two hooks,
  `UserPromptSubmit` and `Stop`. Whether the reply's `systemMessage` reaches
  Daniel and its `additionalContext` the agent needs a live session and is
  the S2 manual check below; `decision: "block"` and whether `/scribe`
  reaches the hook without a command file are S4's to confirm; if `/scribe`
  needs a command file, S4 adds one whose only effect is the same hook call.
  S4 added `commands/scribe.md` and registered `SessionEnd`.
- **S4 manual check in a live session: passed, headless (2026-11-03, Claude
  Code 2.1.317, macOS).** Run by the on-call orchestrator (ORB-21760) with
  `claude -p` against an isolated `CLAUDE_CONFIG_DIR` and `SCRIBE_ROOT`,
  plugin `scribe@scribe-local` installed from `plugin/claude-code`
  (`claude plugin details` lists the `UserPromptSubmit`, `Stop` and
  `SessionEnd` hooks and the `scribe` command). `/scribe on` is delivered to
  the hook and blocked before the model (`UserPromptSubmit operation blocked
  by hook:` followed by the status line, no model turn, cost 0); in one
  session (`--session-id`, then `--resume`) a second `/scribe on` said
  "already on since <time> — nothing changed", and `/scribe status` reported
  "the final pass is due; no record yet". A fresh session reported "off —
  never switched on". `-p` ends a session after each call, so every call set
  `final: true` in `control.json`; `/scribe:scribe on` was not needed. After
  a chat in which the prompt defined "level 4" in its own words,
  `scribe session pass --json` reported one `written` pass with zero dropped
  terms, and the session record carried `terms: [{term: "level 4", said:
  <the typed sentence>, message: <uuid>}]`; a session turned on but with no
  chat was reported `unchanged`. A session never switched on has no entry in
  `control.json` or `sessions/`. Not checked: the interactive terminal
  rendering of the block message, and `final` through a real interactive
  `SessionEnd`.
- **S2 manual check in a live session: passed, headless (2026-11-03, Claude
  Code 2.1.317, macOS).** Run by the on-call orchestrator (ORB-21760) with
  `claude -p` against an isolated `CLAUDE_CONFIG_DIR` and `SCRIBE_ROOT`, the
  plugin installed from the local marketplace (`scribe@scribe-local`). A
  prompt with `#scribe decisions: S2 live check` wrote `decisions-1`; asked
  to repeat any line starting with `scribe:`, the agent quoted
  `scribe: added decisions-1 to decisions — "S2 live check"` from the hook's
  `additionalContext`, so the agent sees the reply. After the reply
  `cite.message` was a uuid, not `null`, so the `Stop` hook found the prompt
  by its hash. `#scribe add it to rituals` was refused (no `rituals` tag in
  the defaults) and the agent relayed the refusal. Not confirmed: that the
  `systemMessage` renders in the interactive terminal for Daniel, which `-p`
  mode does not show; check it the first time `#scribe` is used in an
  interactive session.
- **S3 manual `claude -p` check: passed (2026-11-03, Claude Code 2.1.317,
  macOS).** Run by the on-call orchestrator (ORB-21760) from `zsh -lc` with
  the command the adapter builds: a cleared environment holding only
  `PATH`, `HOME`, `LANG` and `CLAUDE_CODE_OAUTH_TOKEN`, then
  `claude -p --safe-mode --tools "" --model claude-haiku-5-5
  --output-format json --json-schema <schema-json>` with the prompt on
  stdin. The CLI accepted `--safe-mode` and `--tools ""` together and
  `--json-schema` in the inline `<schema-json>` form, authenticated from the
  OAuth token alone, exited 0 with `is_error: false`, and returned the answer
  object `{"message":"m-1","span":"hello world"}` in `structured_output`
  (also as a JSON string in `result`). With the scribe plugin installed in the
  configuration directory used (`claude plugin list` shows it), the prompt's
  embedded `#scribe` line fired no hook: the store kept one entry and the
  audit log kept three lines. The run took two turns (`stop_reason:
  tool_use`), which is the CLI's own structured-output step, not a user tool.
  Not checked: that the model had no tools beyond that step (the output
  lists none, and `permission_denials` was empty).
- **The prompt hash must match the transcript's text.** The `Stop` hook
  finds the prompt by hashing the transcript's user text. If Claude Code
  records a prompt differently from what it passed the hook (expanded
  pasted blocks, attachments), the cite stays `message: null`; the entry
  is still written and cited by session, line and hash.
- **One host, one store.** Sessions on the Mac and on dk-server-1 write
  separate `~/.scribe` stores; `ask` on one host does not see the other.
  Sync is open ([3_vision.md](./3_vision.md)).
- **Lexical retrieval misses paraphrase.** "merge strategy" will not find an
  entry that only says "squash". BM25 over a few thousand short entries is
  the start; [3_vision.md](./3_vision.md) records when to revisit.
- **The model sees Daniel's records.** Entries and session summaries go to
  Anthropic through the Claude CLI, as the sessions themselves already do.
  Nothing else leaves the host.
- **An unrelated pair is judged again.** `unrelated` labels are not
  stored, so a pair the model found unrelated is offered on every scan
  until one entry stops being active; the cap of 40 pairs, newest first,
  bounds the cost, but an old unrelated pair can keep a newer one waiting.
- **One routine call does less than the CLI run.** Orbit caps a plugin
  call at 300 s, so `scribe.session-pass` starts no later session or window
  after 50 s and
  `scribe.consistency-scan` no batch after 170 s; the rest waits for the
  next run (every five minutes once enabled, or next week for the scan).
- **Cataloging requires host opt-in.** The routine ships disabled; records
  wait until the host enables it or Daniel runs `scribe session pass`.
  Repeated failures can delay retries up to an hour; `--session <id>`
  retries immediately. `scribe session list` shows `due` so the gap is visible.
- **A guessed referent is still an entry.** When Haiku fails, the fallback
  records the previous assistant message; the echo says so, but an
  unattended guess stays until Daniel retires it with `#scribe retire`.
- **A referent costs the prompt up to about 21 seconds.** The hook waits
  for the model (20 s budget, 1 s grace) before the prompt reaches the
  agent; the hook's own timeout is 30 s, so two referent lines that both
  time out in one prompt overrun it and Claude Code ends the hook.
- **Whole-file rewrites.** Each tag is one file rewritten per entry; fine
  for thousands of entries per tag.
- **Stale is a date, not a judgement.** `review_after` flags age; whether a
  preference still holds is Daniel's call through the proposal.
- **The plugin is unverified live (S5).** `orbit plugin validate --render`
  and `orbit plugin test` pass against Orbit 0.44.0, but no workspace has the
  plugin installed. The rendered sandbox grants writes to
  `{{plugin_state}}` and Orbit's own state only, so a `claude -p` that must
  write under `~/.claude` may fail inside the plugin while it works from the
  CLI; `orbit.search` and `orbit.task.show` through the callback channel are
  exercised only against a fake `orbit`. The operator's live check
  ([ORB-21760]) installs the plugin and asks one lookup and one model
  question through `mcp__orbit__scribe_ask`.
- **Orbit tasks are the bound workspace's by default.** The orchestrator
  lookup reads `orchestration` tasks from the workspace the call is bound to
  (or `task_workspaces`); §6.5's `ws_constellation` holds them only if the
  plugin is called there or that setting names it, and Orbit refuses a
  federated search inside a managed run.

- **The agent surface serves the Mac only.** The store is on the Mac, so
  the `scribe` skill helps sessions there; box workers still cannot read or
  write scribe. A worker that needs Daniel's context gets it in its task.
- **An agent's name is attribution, not identity.** `--author` is a label
  an agent chooses; `kind: agent` is what scribe can know ("written through
  the CLI"). scribe enforces no ownership between agents: any agent may
  update or retire another's note or settle any agent proposal, and the
  record keeps who did. Daniel's words come only from his typed prompt
  through the hook, so no CLI write records `kind: daniel`; a process
  running as Daniel can still edit `~/.scribe` (STD-05 R5). The guard that keeps agents from settling Daniel's entries
  is a refusal in the CLI, not a permission. The audit log records every
  write's command and target.
- **Removed-feature judgement can be wrong.** Haiku may read "removed the
  old flag" as the removal of the session's feature. The quote is checked
  against a Daniel-authored entry only — model-written session text and
  agent-written entries never count, so injected or agent-supplied words
  cannot satisfy the check — the session is only hidden, and
  `session activate` brings it back, but a wrong deactivation is silent
  until someone looks with `--include-inactive`.
  `session list --include-inactive` shows `active` and the scan report
  lists every deactivation.
- **Path mentions guess a project.** `transcript list` attributes a
  constellation-root session by counting path mentions; a session that
  reads many orbit files to change scribe is attributed to orbit. Daniel
  picks, and `--project` on `catalog` overrides it.
- **An upgraded record has one long segment.** Until `session rebuild`
  runs, a record converted from format 3 has a single sealed segment of up
  to about 4,000 characters, ranked as one unit.
- **Rebuilds and catalogs cost model calls.** A long session is dozens of
  windows; rebuild and catalog run them with no deadline from the CLI.
  Daniel runs them a few at a time.
- **The tmux popup needs fzf.** Without it `scribe-tmux` prints how to
  install it and exits 1. Copying to the system clipboard needs `pbcopy`
  (macOS); elsewhere the id goes to the tmux buffer only.

## 10. Slice plan

Each slice is one Orbit task on `agent-main`, in order; each must pass
`make ci-fast`, `make ci-lint`, `make goldens` and the tests it adds, and
update these docs and [specs/](./specs/) for what it builds.

**S1 — Domain and store.** Replace the note example with entries, tags,
session records, control state and proposals, and the read-only CLI.

- `note.rs`, `query.rs`, the `note` noun, `notes.json` and their tests and
  goldens are gone; `entry`, `tag`, `session`, `verbatim` and `store` exist
  as in [§2](#2-layering)–[§3](#3-storage) and [§6.4](#64-the-verbatim-check).
- Entries and session records are Markdown files with the frontmatter in
  [§3](#3-storage); a record with missing or unknown frontmatter keys is
  refused, naming the file.
- `scribe entry list [--tag <tag>] [--status <status>] [--project <p>]
  [--topic <t>]`, `entry show <id>`, `session list [--project <p>]
  [--topic <t>]`, `session show <id>`, `proposal list`, `tag list` work in
  table, plain and `--json` modes against a fixture store, and against a
  read-only root.
- A store with a newer `format`, a root open to group or others, or a linked
  file is refused with the template's errors; golden tests cover every help
  page.
- The redactor masks a configured credential value in every free-text
  field it covers, and a unit test pins that inventory.
- `specs/store.md` and `specs/output-contract.md` describe the new
  documents and payloads; `ARCHITECTURE.md`, the dependency script and the
  code section of `AGENTS.md` (the `note.rs`/`query.rs` and `notes`
  envelope wording) and `README.md` describe the new modules and envelopes.

**S2 — `#scribe` capture (no model).** The `UserPromptSubmit` hook and the
Claude Code package.

- `scribe hook user-prompt-submit` with a fixture hook payload writes exactly
  one entry per `#scribe` line for forms 1 and 2, with `said`, `content`,
  `cite` and an echo carrying tag, text and id.
- Lines in fences, blockquotes and `<pasted_content>`, and bare `#preference`
  markers, write nothing; an unknown tag is refused with the configured
  list; replaying the same payload writes nothing new.
- Every write and refusal is audited with ids and error codes only.
- `plugin/claude-code/` installs locally and a manual check in a live
  Claude Code session confirms the reply is shown to Daniel and the agent;
  the result is recorded in this doc ([§9](#9-concerns--honest-limitations)).
- `scribe hook stop` fills the cite's message uuid from the transcript.

**S3 — Referents and the models port.** Form 3 and `ClaudeCliModels`.

- With a fake `Models`, `#scribe add this to preferences` records the chosen
  span with `content_from.role`; a span failing the verbatim check, a
  timeout and a model error each still write exactly one entry, the
  fallback, with an echo that says it was guessed.
- `ClaudeCliModels` launches from a cleared allowlisted environment in its
  own process group and is killed and reaped at its budget (tested with a
  fake `claude` script on `PATH` under a temp `HOME`).
- The argv carries `--safe-mode` and `--tools ""` (asserted on a fake `claude`
  that records its argv), and a manual `claude -p` run with those flags is
  recorded in [§9](#9-concerns--honest-limitations).

**S4 — `/scribe on` and session records.**

- `/scribe on|off|status` updates `control.json`, blocks the prompt and
  reports status; `on` twice is a no-op that says so.
- `scribe hook stop` marks due by minutes or turns; `session-end` marks
  final; neither calls a model.
- `scribe session pass` with a fake `Models` writes one record per due
  session within budgets, advances `scribed_through`, writes no entries,
  and keeps the previous record when the model output is invalid.
- A session never switched on is never read (a test transcript for it is
  untouched and absent from the store).
- A pass keeps a term only when its `said` is a verbatim span of one of
  Daniel's turns; a term quoting an assistant turn is dropped.

**S5 — The Orbit plugin tree and `ask`.**

- `plugin/.orbit-plugin/` passes `orbit plugin validate --render` and
  `orbit plugin test`; a conformance test asserts `scribe.ask` is the only
  tool with an `mcp_scope` other than `none`.
- With a fake `Models` and fixture tasks, `ask` returns `no_record` without
  a model call when nothing matches, drops claims whose quote or cite fails
  and counts them, returns `conflict` with both cites for two disagreeing
  entries, and excludes hidden entries unless `include_hidden`.
- `scribe question ask "<question>" --json` and the `scribe.ask` tool
  return the same payload for the same store.
- "can you fetch me last session summary?" returns `"status": "record"`
  with the newest session record in scope, its fields byte-identical to the
  stored record, and makes no model call; a non-lookup question still goes
  through retrieval and answering.
- "graduate to level 4" returns the session records whose `terms` hold that
  phrase with their defining spans, and "last on-call session for orbit"
  returns the newest matching assignment task's execution summary verbatim,
  both without a model call.

**S6 — Consistency routine.**

- `scribe consistency scan` with a fake `Models` turns a contradicting pair
  into one `supersedes` proposal, a stale entry into one `stale` proposal,
  and never re-proposes a settled pair; and a scan re-applies an approved
  proposal whose older entry is still `active`.
- `#scribe approve p-<n>` marks the older entry `superseded` with
  `superseded_by`; `reject` closes the proposal; `retire` and `forget`
  behave as [§3](#3-storage) says; none is reachable from any Orbit tool.
- The `SessionStart` hook reports the open-proposal count; the two routines
  ship seeded disabled.

**S7 — Discussions.**

- `#scribe distill this discussion as "<title>"` with a fake `Models` writes
  one discussion in the [§3](#3-storage) shape whose every quote passes the
  verbatim check against Daniel's turns; a point quoting an assistant turn
  is dropped and counted in the echo.
- `#scribe add this to <slug>` on a discussion Daniel edited by hand
  appends new points after the last one and leaves every existing line
  byte-identical.
- `ask` answers "what did we conclude about <subject>" from a store
  discussion and from a fixture `discussions_readonly` directory, and scribe
  never writes into that directory.

S8–S14 were designed on 2026-11-10 ([ORB-23892]) and are filed one Orbit
task each, in this order; S7 does not block them. Acceptance is observable
CLI behaviour; tests follow the repository's rules in `AGENTS.md`.

**S8 — Store format 4 and its read paths.** Depends on nothing.

- `FORMAT` is 4 and `UPGRADES` gains one appended step; a format-3 store
  loads unchanged on disk, and the next write of each document persists
  format 4. A format-3 session record reads as a header (its summary,
  goals and next) over one sealed segment `origin: upgrade` holding its
  summary and outcome, `active: true`; a format-3 entry reads `author:
  {kind: daniel, name: daniel}`; a format-3 proposal reads `author: {kind:
  scribe, name: consistency}`. An older build refuses a format-4 store.
- Session records parse and write in the [§3](#3-storage) shape: the
  `log` metadata and the `## Log` body agree (numbers, headings, order),
  only the last segment may be open, and a record that breaks this is
  `store_corrupt` naming the file. `active`, `keep_active`,
  `active_change` and `cataloged_by` parse and write; entries carry
  `author` and `updated_by`, proposals `author` and `settled_author`,
  control switches `cataloged_by`; `#scribe` lines write `{kind: daniel}`.
  Redaction covers segment headings and texts and `active_change.reason`.
- `session show <id>` prints the header and the log's table of contents;
  `--log` adds every segment's text; `--segment <n>` and `session show
  session:<id>#<n>` print one segment; an unknown segment is
  `segment_not_found`. `session show --json` adds `active`, `keep_active`,
  `active_change`, `cataloged_by` and `log` (each segment with its text);
  `outcome` stays in the payload as `""`.
- `session list` leaves inactive sessions out unless `--include-inactive`;
  `ask` leaves them out of evidence and lookups unless `include_hidden` and
  counts them in `hidden_matches`.
- `ask` ranks session headers and segments as separate units and cites a
  segment as `session:<id>#<n>`; a claim quoting a segment is checked
  against that segment.
- `specs/store.md`, `specs/output-contract.md` and the goldens describe
  the new shapes.

As built (S8):

- Passes are S9's to rewrite, so S8 keeps the session model's answer
  shape and maps it onto the log: the answer's `outcome` becomes the text
  of the open segment (its heading the title, cut to 80 characters;
  `windows` counted up; `to` and `last_message` from the window), a log
  with no open segment gains segment `n + 1` when the outcome is not
  empty, and the final pass (or a `SessionEnd` with nothing new) seals the
  last segment. A pass carries `active`, `keep_active`, `active_change`
  and `cataloged_by` over from the record on disk. The model still sees
  the open segment's text as the previous record's outcome.
- The format-3 body is converted while the record is decoded, because
  `UPGRADES` steps see the frontmatter only; the step fills the segment's
  metadata and the decoder its text.
- `tags.json` is S12's ([§3](#3-storage)), not S8's: nothing reads or
  writes it yet. The agent-entry shape (`content_from: {kind: agent}`,
  `cite: null`) and the `Store::set_active` write are S12's and S10's;
  S8 parses and writes the fields they will set.
- `hidden_matches` counts inactive sessions that retrieval matched; a
  lookup that skips an inactive session does not count it, as a lookup
  never counted hidden entries.
- A lookup's session record carries every segment in `--json`; its table
  view prints the header and the log's table of contents.
- `session list` without `--include-inactive` prints a stderr notice when
  inactive sessions matched; the JSON envelope's `total` counts only the
  sessions listed.

**S9 — Passes write segments; `session rebuild`.** Depends on S8.

- With a fake `Models`, a pass sends the header, the open segment and the
  sealed segments' headings only, never a sealed segment's text; writes
  the header and the open segment; and opens each `new` segment at its
  `starts_at`, sealing the one before. Message ranges, `from`, `to` and
  `windows` follow [§5.5](#55-the-log-segments-and-sealing).
- An answer whose segment text exceeds 900 characters, whose heading
  exceeds 80, whose `starts_at` is outside the window or out of order, or
  whose header breaks a budget gets one retry naming the refusal, then
  applies the deterministic budget/start repairs of §5.5 if the retry still
  misses only those rules. Other invalid answers fail with
  `session_answer_invalid`, keeping the previous checkpoint. Header-only
  length failures use compression rather than resending the window; all
  other failures use the full-window retry.
- scribe seals the open segment after `[catalog] segment_windows` windows
  (3), after cutting an overlong open text with no successor, and at the
  final pass's last window; a sealed segment is
  byte-identical on disk after every later pass.
- `scribe session rebuild <id> --author <name> --confirm` replaces an
  upgraded record with one built from its transcript (`origin: rebuild`),
  keeping `id`, `created`, `links` and the activity fields; without
  `--confirm` it refuses with `confirm_required` and changes nothing; a
  session without a transcript is `transcript_missing`; an on-and-due
  session is `session_busy`; an interrupted rebuild resumes from its
  checkpoint and the record is untouched until the last window.

As built (S9):

- The model port now answers with the header, `open` and `new` of §5.5;
  the legacy `outcome` is no longer requested. The public record JSON keeps
  `outcome: ""`. Header summary is bounded to 1200 characters; the prompt
  targets 900 and gives the current length. Header-only compression and
  the tolerant `open` rules follow §5.5. Segment
  content, ordered starts, ranges, timestamps and sealing are checked in
  `scribe-core::pass`; a refusal retries once, then repairs only the allowed
  budget/start misses. Unrepairable failures retain the bounded, redacted
  pass failure reason introduced before S9.
- `[catalog] segment_windows` parses as a positive integer (default 3).
  The store compares every sealed segment before any record replacement
  by a pass and retains its bytes through redaction. Pass reports add
  `segments` and `segments_sealed`; a final pass with no new messages also
  counts the segment it seals without calling a model.
- `session rebuild` checkpoints the same session shape in
  `.<id>.rebuild.md`, with `updated` pinning the visible record revision.
  A separate owner-only `.<id>.rebuild.lock` serializes rebuilders; it is
  released by the OS on interruption and skipped by readers. Every window
  is redacted and written under the store lock. Final replacement compares
  the visible revision, and a mismatch re-reads the transcript, folds in
  newer messages and retries. The successful replacement removes the
  checkpoint and preserves the latest identity, links and activity fields.
- `--author` names the requesting agent in the rebuild's pass report;
  `cataloged_by` keeps its original attribution. Rebuild does not alter
  control state. Its refusal and success each write one audit line with
  the session target. Repeating the confirmed command resumes after a
  failed model window. No persisted shape changes beyond S8 were needed.

**S10 — Active routes.** Depends on S8.

- `scribe session deactivate <id> --author <name> --reason <text> [--cite
  <ref>]` and `scribe session activate <id> --author <name> [--reason
  <text>] [--pin|--unpin]` change `active` and `keep_active`, record
  `active_change` with route `manual`, and write one audit line; a repeat
  is `nothing changed` with no audit line; deactivating a pinned session
  unpins it and says so.
- `[sessions] inactive_after_days` (default 120) parses; zero or a
  non-integer is `config_invalid`.
- `scribe consistency scan` deactivates every active, unpinned session
  whose last message is older than the limit (route `age`), and, with a
  fake `Models`, a session whose removed-feature pair is labelled
  `removed` with a verbatim quote (route `removed`, cite the later entry);
  a label whose quote fails is dropped and counted; pinned sessions are
  never deactivated by the scan. The report lists `deactivated`.

As built (S10):

- `activity::set_active` applies one route to a loaded record and
  `Store::set_active` writes it under the lock; the scan's deactivations
  go through `Store::set_active_many`, one lock for the batch, each
  checked again on disk. `session activate` without `--reason` records
  scribe's sentence (`activated by hand`, `pinned by hand`, `activated
  and pinned by hand`, …); a `--reason` over 300 characters is refused as
  a usage error rather than cut (a model's reason is cut). `--cite` is
  one token of at most 200 characters and must name a record that
  exists: `session:<id>` (a unique prefix is stored in full),
  `session:<id>#<n>` (segment `n` must exist) or an entry id; anything
  else, or a record that is not in the store, is a usage error and nothing
  is written. Activity belongs to the whole session, so a segment cite as
  the id is a usage error.
- Ids were exact (or `session:<id>`) in S10; S11 added unique-prefix
  resolution (`id_ambiguous`), the CLI agent surface's rule
  ([§1.1](#11-the-cli-agent-surface)), to both verbs.
- A record with no segment yet counts its age from the later of `started`
  and `on_since`, never from `updated`.
- Removed-feature candidates are built only from active entries Daniel
  wrote; session headers, segments and agent-written entries are never
  evidence. One unit of each later entry is paired with a session (its newest), ties
  are broken by session id, and the model call is a separate
  `Models::removal` request (`REMOVAL_SCHEMA`: `pair`, `label`, `feature`,
  `reason`, `quote`) on `models.consistency`. A `removed` label with an
  empty reason gets scribe's sentence naming the feature. The report's
  `deactivated` items also carry the `quote`, and a failed batch is
  listed in `failed_removal_batches`, numbered apart from the entry
  batches. In the plugin call, a removed-feature batch after the scan's
  first model call starts only before the call's deadline, as entry
  batches do. A failed per-session store write is listed in
  `failed_deactivations` and audited with its session id and error code;
  other sessions in the scan continue.

**S11 — CLI read surface.** Depends on S8.

- `scribe record search "<terms>"` returns hits ranked as `ask` ranks
  them, with no model call and no Orbit call, in table, plain and `--json`
  (envelope key `hits`); `--kind`, `--project`, `--topic`, `--since`,
  `--until`, `--include-hidden` and `--limit` narrow it; hidden entries
  and inactive sessions appear only with `--include-hidden`.
- `entry list` and `session list` take `--since`, `--until`, `--status`,
  repeated `--topic` (all must match) and repeated `--related` (any may
  match); `entry list` takes `--author` and `--author-kind`; `proposal
  list` takes `--state`, `--since`, `--until`, `--entry` and
  `--author-kind`. A bad date is a usage error.
- Every command that takes an id accepts a unique prefix of at least four
  characters; an exact id wins; an ambiguous prefix exits 2 with
  `id_ambiguous` listing the candidates.

**As built (S11).** `record search` is `commands/record.rs` over
`scribe_core::search` ([§6.8](#68-record-search)). `EntryFilter`,
`SessionFilter` and the new `ProposalFilter` hold the filters, with the
dates as `record::DateRange` (both days included): an entry's day is its
`created`, a proposal's its `opened_at`, and a session matches when any
day from `started` to `updated` is in the range. A session's `--topic`
and `--related` match its record's or any segment's (format-4 records
keep them per segment). `--status` on `session list` is `open` or `final`;
`--until` before `--since` is a usage error like a bad date. Id prefixes
resolve in the store (`store/ids.rs`), reading only: entries and
proposals as S12 built them, and sessions against both their records and
their switches, for `session show|deactivate|activate|rebuild` and `session
pass --session`; a write's audit line names the full id. The plugin's
`session pass` call and the hooks (`#scribe approve|reject|retire|forget`,
`/scribe link`) still take exact ids: they are not commands an agent types
at a shell, and Daniel's ids there come from scribe's own replies.

**S12 — CLI write verbs with authors.** Depends on S8.

- `entry add`, `entry update`, `entry retire`, `tag add`, `tag update`,
  `proposal add`, `proposal update` and `proposal settle` exist as in
  [§1.1](#11-the-cli-agent-surface); each requires `--author`, records
  `{kind: agent, name}`, writes one audit line per change or refusal, and
  prints the record it wrote.
- The guard: changing a Daniel entry's content, retiring it, or settling
  a proposal whose older entry is Daniel's is refused `daniel_only` and
  changes nothing; opening a proposal against a Daniel entry works, and
  `#scribe approve` settles it.
- `tags.json` holds agent tags; `tag list` shows config and store tags
  with a `source` column; a store tag clashing with a config name or alias
  is refused `tag_conflict`; `tag update` of a config tag is
  `tag_configured`; `#scribe … to <store tag>` files under it.
- `ask` labels agent entries `agent note (<name>)` and weights them 1.0×;
  with a fake `Models`, an agent entry is never cited as Daniel's words.

**As built (S12).** All eight CLI write verbs are implemented, with
unique entry/proposal prefixes, agent authors, no-op output and audit
suppression, redaction and locked atomic writes. Daniel entries accept
only metadata patches through the CLI; human hook settlement is unchanged.
Config and store tags share routing, with source and author in `tag list`.
The store writes **format 5**, superseding this design's format-4 sketch:
the workspace guide requires a version bump for nullable agent citations,
the new provenance variant and additional metadata. Entry retirement
reasons and proposal updaters are retained; tag updaters are retained too.
`entry add` of a statement an active entry of the same tag and project
already covers writes nothing and exits 1. Plain text and `--json` name
the existing id, and `--json` carries the stable code `already_covered`
(the message is `already covered by <id>; nothing written`). The audit
line carries that code, not the statement. Retired, superseded and
forgotten entries do not block the new entry.
See the S12 decision and the current store/output specs. `ask` gives agent
entries a 1.0× base weight (the existing named-tag boost still applies).
When agent evidence is packed, answers use checked quotes with source
labels instead of forwarding potentially misattributed model prose.

**S13 — The skill and the tmux popup.** Depends on S11 and S12.

- `plugin/claude-code/skills/scribe/SKILL.md` ships in the Claude Code
  package and `claude plugin validate` accepts it; it teaches the commands
  in [§1.1](#11-the-cli-agent-surface), when to `ask` instead, the
  `--author` rule and the guard, and every command it names exists with
  the flags it shows.
- The popup lists sessions, entries, proposals and
  transcripts in fzf with `show` as the preview, opens a record with
  `--log` in the pager on Enter, copies the id on `ctrl-y`, and exits 1
  with an install hint without `fzf`; the `display-popup` binding is
  documented in `scribe tmux --help`. (Shipped first as
  `plugin/tmux/scribe-tmux`; see the note below.)

**As built (S13).** The skill and the popup ship as designed, with two
honest adaptations to what the binary offers today. There is no
`proposal show` (a proposal is read through `proposal list`), so the
popup's proposal preview and Enter view format the proposal's list row
field by field, which carries every field a proposal has. S14 has
since landed: the backfill view runs `transcript list` for the last 30
days and shows one notice row when that list is empty or fails, and
Enter on a candidate leaves `/scribe catalog <id>` in the tmux paste buffer
(`scribe session catalog <id> --author daniel` until ORB-24030).
The popup was first a standalone script, `plugin/tmux/scribe-tmux`, that
nothing installed; it is now the `scribe tmux` subcommand (the same
script embedded in the binary, with its README folded into
`scribe tmux --help`), and the Enter and preview views request
`--format table` explicitly, because `show` through a pipe prints only its
list line, so the first version's preview and Enter views showed no
summary, log text or entry detail. The list rows then started with the
36-character id and cut the title off; rows now lead with the date, status
and title as above (hidden `tmux rows` and `tmux preview` modes of the same
binary build them; `session list`, `entry list`, `proposal list` and
`transcript list` print what they always did, and the popup reads their
records in process). The hidden modes are not listed in help (STD-01 R25,
R31: they are read-only, write nothing and exit 0, 1 or 2 like any command,
and `--help` on them works) and follow the output rules: payload on stdout,
color only when asked for and allowed by `NO_COLOR` and `TERM`. The script prefixes each row with a hidden kind field so one fzf
invocation's `ctrl-s`/`ctrl-e`/`ctrl-p`/`ctrl-b` reloads, its
`alt-a` hidden-toggle and its `ctrl-y` copy all know the record's noun
and id; `crates/scribe/tests/tmux_popup.rs` drives `scribe tmux` through a fake
`fzf` against the real binary. The skill names only commands and flags
the built binary has (checked against its `--help`). It teaches
`transcript list` and `transcript show` as read-only, and it says
`session catalog` is Daniel's.

**S14 — Backfill by name.** Depends on S9.

- `scribe transcript list --since <date>` lists uncataloged transcripts
  with `project`, `project_mentions`, `started`, `ended`, `bytes`,
  `daniel_turns` and `first_prompt`, at least `--min-turns` (20) of
  Daniel's turns, attributed by `[projects.<slug>] paths`; it writes
  nothing and works on a read-only root. `transcript show <id>` prints one
  row with Daniel's first five turns.
- `scribe session catalog <id> --author <name>` switches a never-switched
  session on with `cataloged_by`, runs every window with no deadline,
  marks it `final` and off when its transcript is a day old, and writes no
  entries; a session with a record or a `/scribe on` switch is refused
  `session_cataloged`; a failed catalog resumes on the next run.

**As built (S14).** `transcript list` and `transcript show` read only: no
lock, no audit line, and they work when the store root is not writable.
`--since` is required. `--until`, a repeated `--project` (any of them)
and `--limit` are accepted, as §5.8 specifies; the task text named only
one `--project`. Rows are newest first by `started`. A transcript is
uncataloged when it has no session file — a file that did not parse still
counts — and no switch. A provenance failure is skipped with no warning.
`project_mentions` counts non-overlapping, case-sensitive path fragments
and includes zeros; a tie takes the lexicographically smaller slug, else
the cwd's project. `first_prompt` and each of the five show turns are
whitespace-collapsed and cut to 300 characters, with no ellipsis. A file
with no parsed times is left out, because a date bound is always set.
`cwd` is on the row, as §5.8 specifies.

`session catalog` checks the transcript and the redactor before it writes
the switch. `cataloged_by.by` is `{kind: agent, name}` under the caller's
name, and a reserved name is refused `reserved_author` before the
transcript is resolved; `{kind: daniel}` comes only from `/scribe catalog
<id>` (ORB-24030, which replaced an interactive-terminal `--author daniel`
channel that a faked pty could pass). Age is the last kept message's timestamp, not the file mtime;
at least 24 hours old, the last window is final and the switch turns off
in the same store lock. A newer transcript stays on and due, and that
write clears retry backoff. An empty conversation writes no record, leaves
the switch due, and calls no model. `--project` applies only when the
session has no record yet. A failed catalog stays due; the next `catalog`
resumes it without moving `since` or `cataloged_by.at`, and it does not
wait out retry backoff. Segments are `origin: catalog`. No entries are
written. The pass report's JSON stays `{passes, remaining}`. The run is
audited like a pass, plus one success line when this run wrote the switch.

**Outside this repository.** Retiring orbit-office's scribe (its hooks,
`office.scribe-*` tools and `/scribe` command) is orbit-office work, filed
there once S4 is in use, so two scribes never intercept the same
`/scribe` prompt. With S13, the constellation umbrella adds the `scribe`
route under `.agents/skills/scribe/` for providers other than Claude Code,
pointing at the same commands; that is a constellation change.

## 11. Claims in context, capture rules and issues (proposed)

[ORB-26214], designed 2026-11-21 and **not built**. Nothing in this
section is implemented until Daniel approves it and its implementation
tasks land, in the order of [§11.7](#117-order-and-sequencing). Where it
changes a rule stated earlier (§4.6, §4.7, §5.3), the earlier text
describes what is built today and carries a pointer here. The reasons and
the alternatives are in
[4_decisions.md](./4_decisions.md#claims-carry-a-statement-tags-carry-capture-rules-and-issues-are-their-own-record).

Daniel, chief's chat, 2026-11-21: "few issues with current cataloging. 1.
they are out of context - I propose having the scribe summarize the context
and quote verbatim where appropriate 2. decisions and tasks are just noise
- good thing we got rid of them 3. to reduce noise, we need a set of
trigger words and criteria for each tags - start with sensible defaults and
able to be modified 4. let's also get rid of remind 5. let's introduce
"issues" …".

**Observed need** (the Mac store, 2026-11-21): 20 open candidate
proposals, which is the store-wide cap of §5.3, so every pass asks the model
for none. 14 of them sit under `decisions` (7) or `task` (7), tags that no
longer ship, and 2 under `finding`. Several cannot be read alone, such as
p-8: "ok that makes sense. then let's keep it like this."

### 11.1 A claim carries a statement

A claim is anything the pass captures from a turn: a candidate proposal
and a pass-written `idea` entry (built in step (b2), [ORB-26265],
[§11.6](#116-idea-capture-after-finding-is-gone)). Each one gains a
**statement** beside its **quote**.

- **Statement**: one or two sentences, at most 300 characters, written by
  the pass's `session` model, that stand alone. It names the subject (the
  page, task, feature, tool or host the turn is about) and states the rule
  or idea. "Daniel prefers that session records keep the whole session's
  history in an append-only log rather than one rewritten outcome", not
  "ok that makes sense".
- **Subject**: the statement's referent as a short noun phrase (at most 80
  characters), returned as its own field. An empty subject means the
  window does not name what the turn is about; the pass then opens no
  claim. This is the model's judgement: no deterministic check can tell
  that a referent is right, only that one was given
  ([§11.8](#118-limits-and-what-is-not-built-until-seen)).
- **Quote**: unchanged. A verbatim span of the named turn, checked against
  the original transcript exactly as §5.3 does today, with every exclusion
  (pasted, quoted, fenced, marked lines) kept.

**Bound (STD-08 §R1, §R3).** The statement and subject add at most 380
value characters per claim, so at most 1,140 per window across §5.3's three
claim slots, in the same model call with no extra call or timeout budget.
The slots and the 20-open cap are unchanged.

The statement and subject pass the redactor. A statement whose redaction
changes it is kept redacted; only the quote must stay unredacted (§5.3's
rule that a quote needing redaction is omitted stands).

**Approval changes what an entry's content is.** Today an approved
candidate's entry has the quote as its content (§5.3, "Persistence and
consent"). With a statement, the approved entry's **content is the
statement**, and its `quote` field (an entry field since store format 8,
until now set only on action entries) holds the verbatim quote. The cite,
`content_from: message` (user) and the author `{kind: daniel}` are
unchanged: Daniel approved the claim, and the quote is his. A new entry
field `stated_by` records whose words the content is: `{kind: scribe,
name: session-pass}` for an approved statement, `null` (the content is the
author's own words) for every other entry.

**Reading it.** `ask` evidence and `record search` snippets show an entry
with `stated_by` set as "scribe's statement of Daniel's turn" followed by
his quote, labelled as his words. The deterministic attribution check of
§6.4 accepts a claim attributed to Daniel only when the cited span is in
the `quote`, never in a scribe statement. `entry show` prints both.
`proposal list` shows the statement first and the quote under it, so
Daniel settles a claim he can read alone.

**Older candidates.** A candidate opened before this change has
`statement: null` and `subject: null`. Approving one still writes the
quote as content with `stated_by: null`, as today. No old claim is
rewritten by a model.

`#scribe` lines are unchanged here; [§14](#14-scribe-as-an-annotation-proposed) proposes cataloging them with a statement too: what Daniel types or points at is the
content, verbatim. The statement is for what the pass captures, because
only there did context go missing.

**Store format.** Format 12, after [§11.2](#112-capture-rules-per-tag)'s
format 11: a candidate payload gains
`statement` and `subject` (`null` on upgrade); an entry gains `stated_by`
(`null` on upgrade). One append-only step in `store/format.rs::UPGRADES`;
`specs/store.md` describes the fields when the step lands.

**Built (step b1, ORB-26264, store format 14).** Formats 12 and 13 were
taken by §5.3's `inactive_since` and §12.7, so the step is 13→14
(`claim_statement`). The pass's schema asks for `statement` and `subject` on
every claim; the pass opens nothing for an empty or over-long statement or
subject (an empty statement is refused as an empty subject is), and its
duplicate check compares the statement as well as the quote against what
active entries and earlier candidates say. `#scribe forget` on a candidate
clears its statement and subject with its quote. `proposal list` in table
mode puts the quote on a row under the statement; plain mode joins them with
` | `. The stored `reason` is unchanged. In `ask`, a claim whose quote lies in
a stated entry's quote is labelled `Daniel`, one whose quote lies only in the
statement is labelled scribe's statement, and the answer is built from those
labels as it is for agent evidence. A stated entry's `record search` snippet
is the statement and the quote, each cut to the snippet bound. The
statement and subject bounds count the model's words before redaction, so a
mask that lengthens a value keeps it. A `#scribe` line that matches a stated
entry's quote is covered by that entry, live and in Stop recovery, as it
would be by its content. The tmux popup's proposal row and preview show a
candidate's statement in place of the raw quote (ORB-26489).
`specs/store.md` and `specs/output-contract.md` describe the fields.

### 11.2 Capture rules per tag

A tag's **capture rules** decide which turns the pass may turn into a
claim under it. A turn must pass two gates, in order:

1. **Trigger**, model-free: one of the tag's trigger phrases occurs in the
   turn.
2. **Criteria**, model judgement: the turn meets the tag's `criteria` and
   none of its `not`.

Config, under the tag (`[tags.<name>.capture]`):

```toml
[tags.preferences.capture]
from = "daniel"                  # whose turns; only "daniel" is accepted for now
route = "propose"                # propose | write | agent
triggers = ["always", "never", "should not", "from now on", "I prefer", "by default"]
criteria = "A standing rule from Daniel that applies beyond the work at hand."
not = "A choice about one page or task (it goes on its Orbit task or the feature's 4_decisions.md); an acknowledgement."
```

- `from`: whose turns are read. Only `daniel` (his typed turns, with
  §5.3's exclusions) is accepted; `assistant` is refused
  (`config_invalid`) until a need for assistant-turn capture is seen again
  ([§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup)).
- `route`: what a claim becomes. `propose` opens a candidate proposal that
  Daniel settles (§5.3); `write` has the pass write the entry itself, with
  no proposal ([§11.6](#116-idea-capture-after-finding-is-gone)); `agent`
  has the prompt hook tell the agent in the chat, and the pass captures
  nothing under the tag ([§11.4](#114-issues)).
- `triggers`: phrases matched case-insensitively on word boundaries.
  Inside a phrase, `*` stands for zero to four words, and the match must
  lie within one sentence (a sentence ends at `.`, `!`, `?`, a line
  break or a NUL). A phrase that contains one of those is refused
  (`config_invalid`); the error names each of them. Other punctuation
  (a comma, hyphen, underscore or parentheses) is a word separator in
  both the phrase and the text, so `always` matches `always_on` and
  `by default` matches `by-default`. An apostrophe between letters stays
  inside the word (`here's`). Matching is in `scribe-core` and does no
  I/O. An empty list means the pass captures nothing under the tag.
- `criteria` and `not`: plain text, at most 300 characters each, given to
  the `session` model as the tag's definition.

Each field overrides the shipped default for that field alone; `triggers`
replaces the whole list. `[tags]` merges with the shipped set (`preferences`,
`idea`, `standard`): a declared tag that is not shipped is added, and a
declared shipped tag overrides only the fields it sets and inherits the rest
(`aliases`, `description`, `stale_after_days`, `action`, `require_quote`,
`ask_first`, `require_evidence`, and each capture field). For `preferences`,
omitting `aliases`, `description` or `stale_after_days` — including when only
`[tags.preferences.capture]` is set, or when `preferences` is not mentioned —
keeps the shipped values (aliases `preference` and `pref`, the shipped
description, stale after 180 days). An explicit `aliases = []` clears the
aliases; an explicit `stale_after_days` replaces 180. Omitting
`stale_after_days` on any tag that ships no interval still means the entry
never goes stale. A word the config sets (a tag name, or an alias in an
`aliases` list the file writes) that a shipped tag also holds is Daniel's.
The shipped tag loses that alias, or is left out when the word is its name,
and `tag list` names both tags on stderr. Omitting `aliases` does not claim
the inherited ones, so another config tag may take one. Two config entries
that claim one word are `config_invalid`. There is no way to remove a shipped
tag except by claiming its name. A tag with no capture rules, shipped or
configured, is never captured by the pass, which is how `standard` stays on
its own routes (§4.6). Capture rules apply only to the pass and the prompt
hook; a `#scribe` line Daniel types is captured whatever its words. Store
tags (`tags.json`) take no capture rules (not built until seen). `tag show`
reports `config` only for capture fields the file sets, and `shipped` for the
rest.

**Shipped defaults.**

| Tag | Route | Triggers | Qualifies | Not |
|---|---|---|---|---|
| `preferences` | `propose` | "always", "never", "should not", "from now on", "I prefer", "by default" | a standing rule from Daniel that applies beyond the work at hand | a choice about one page or task (its Orbit task or `4_decisions.md`); an acknowledgement |
| `idea` | `write` | "have * an idea", "had an idea", "here's an idea", "what do you think about * idea", "what if we" | he puts forward something to consider, not an instruction to do it now | a direction to act now; a preference; a question of fact |
| `issue` | `agent` | "can you investigate", "look into why", "rca", "root cause", "find root cause" | he asks for the cause of something that went wrong | how something works; a feature request |

**In the pass.** Before a window's model call, the pass matches every
capture tag's triggers against Daniel's turns in the window. If none
fires, the call requests zero claims, as at the cap, which saves the
output and removes the main source of noise. Otherwise the call names each
firing turn with its tag and trigger, and gives the tags' `criteria` and
`not`; the model returns claims only for those turns. Under the store
lock the pass checks again that the claim's turn fires a trigger of the
claim's tag, and refuses the claim otherwise.

**Provenance.** Each claim records what admitted it: `admitted: {trigger,
matched, criteria}`, the trigger phrase as configured, the matched words
of the turn (verbatim, at most 120 characters) and the `criteria` text in
force. A later config change does not rewrite it. `proposal list` and
`entry show` print it.

**Reading the rules.** `scribe tag show <tag>` (new, read-only, no lock,
no audit) prints the tag's settings and its effective capture rules, each
field with its origin: `shipped`, `config` (with the file and line) or
`none`. `--json` gives `{tag, action, capture: {from, route, triggers,
criteria, not, origin: {<field>: …}}}`. `tag list` adds the route
column.

**Store format.** Format 11 (on today's 10): a candidate payload gains
`admitted` (`null` on upgrade); a proposal gains `closed_reason`
([§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup); `null` on
upgrade).

**Built (step a1).** The trigger matcher is `scribe-core/src/trigger.rs`;
the rules are `tag::CaptureRules`, read from config by `config.rs` with each
field's line. The pass sends the model only the Daniel turns that fire a
trigger (with tag, trigger and matched words) and those tags' `criteria`
and `not`; when none fires it requests no claims. The store checks each
claim's turn again under its lock against the config then on disk and
stores the recomputed `admitted`. Route `agent` is accepted in config but
captures nothing; route `write` writes the entry itself since step (b2)
([§11.6](#116-idea-capture-after-finding-is-gone)). Because a tag without rules is never
captured and `from` takes only `daniel`, the pass no longer opens
assistant-turn candidates; stored ones still approve. Removing the
`finding` and `remind` tags, the leftover assistant-candidate code and the
§4.7 and §5.3 wording is step (a2).

### 11.3 The shipped tag set and retired-tag cleanup

After the change scribe ships `preferences` (memory), `idea` and `standard`
(action, as today) and `issue` (its own record kind, §11.4). It stops
shipping `remind` and `finding`.

- `remind`: Daniel, 2026-11-21: "let's also get rid of remind". A
  check-back is an Orbit task with a not-before date. `remind` leaves
  `SHIPPED_ACTION_TAGS`; nothing else about action tags changes.
- `finding`: Daniel, 2026-11-21: "finding - let's delete this for now."
  The tag leaves the shipped set, the skill stops telling agents to write
  findings, and the session pass stops reading assistant turns at all:
  the assistant-turn candidates of §4.7 ("The pass as safety net") and
  §5.3 go, for `finding`, `remind` and `idea` alike. The entry fields
  `evidence` and `as_of`, the `require_evidence` flag and its guards stay
  (they are persisted contract and cost nothing idle), so a config that
  declares `[tags.finding] require_evidence = true` still works as §4.7
  says, and existing findings still read as an agent's claim.
- `standard`: unchanged here (Daniel: "ok let's keep standard, and route
  it to a larger model."; the routing is the constellation intake's,
  ws_constellation ORB-26216). It ships no capture rules, so the pass never
  proposes it.

As with `task`, `friction` and `decisions`, an entry already stored under
a tag that no longer ships stays listed, shown, searched, cited and
settleable by id; no store-format change is needed for the tag set.

**Retired-tag cleanup.** An open candidate whose tag is no longer in the
tag table (config, shipped or `tags.json`) can never be approved into a
listed tag, yet it holds a slot of the 20-open cap. It is closed
automatically: its state becomes `rejected`, `settled_author` is
`{kind: scribe, name: tag-retired}`, `settled_by` is `null`, and the new
field `closed_reason` is `tag_retired`. This runs under the store lock in
the candidate write path before the cap is counted, so every pass frees
the slots, and in `consistency scan`; each close is one audit line. If the
command fails after those closures are already on disk, it still writes one
success audit line per closed proposal, then its failure line. It
needs no model and writes no entry. A closed candidate stays in the
suppression set like any rejection, and `proposal list --state rejected`
shows its reason. On 2026-11-21 it would close 16 of the 20 (7 `decisions`,
7 `task`, 2 `finding`).

**Built (step a2).** `remind` left `SHIPPED_ACTION_TAGS`, `finding` left
the shipped memory tags (`TagTable::shipped_finding` removed; `[tags.finding]`
without `require_evidence = true` is a plain tag), `EntryCandidateAnswer`
and `SESSION_SCHEMA` dropped the assistant-candidate `evidence` field, the
three skill copies stopped telling agents to write findings or reminders,
and §4.7 and §5.3 match the new behaviour. `evidence`, `as_of`,
`require_evidence` and stored `finding`/`remind` records and format-10
assistant candidates remain intact.

**Built (step a3).** `close_retired_tag_candidates` runs under the store lock
in the candidate write path (`open_pass_candidates`, before counting the
20-open cap; `session pass` also filters out retired-tag candidates when
computing `candidate_limit`) and in `consistency scan`
(`close_retired_candidates`), closing open candidates whose tag is absent from
the effective tag table as `rejected` with `settled_author: {kind: scribe,
name: tag-retired}`, `settled_by: null`, and `closed_reason: tag_retired`. Each
close writes one audit line (`target` `p-<n>`), `proposal list` displays
`CLOSED_REASON` in table and plain output, and closed candidates stay in the
suppression set.

### 11.4 Issues

An **issue** is the memory of an incident that recurs across projects and
what fixed it: "the GitHub API answered 429", "CPU exhausted on
dk-server-1". Daniel, 2026-11-21: "we will catalog issues we've faced, e.g.
github api 429 request issue we faced yesterday, cpu resource exhaustion
incidents. and upon similar incidents, the scribe will append the new
incident under "occurrences" section with date and summary, fixes." On who
writes it: "agent should write it - summarize context and add verbatim
quotes."

It is a record kind of its own, not an entry, because it grows: an entry's
content is one statement, an issue gains an occurrence each time.

**File** (`issues/<id>.md`, id `issue-<n>`, allocated like entry ids):

```markdown
---
format: 17
kind: issue
id: issue-3
title: GitHub API secondary rate limit (HTTP 429) during drains
status: open                       # open | mitigated | resolved
signature: ["HTTP 429", "secondary rate limit", "API rate limit exceeded"]
projects: [orbit]
hosts: [dk-server-1]
topics: [github, rate-limit]
related: [task:ORB-…, pr:<owner>/<repo>#…]
author: {kind: agent, name: <crew>}
updated_by: {kind: agent, name: <crew>}
created: …
updated: …
occurrences:
  - {n: 2, date: 2026-11-21, host: dk-server-1, project: orbit, session: <id>, request_message: <uuid>, recurred: false, moved_from: null, author: {kind: agent, name: <crew>}}
  - {n: 1, …}
---
## Cause
<the best-known cause, or "cause not found">
## Fix
<the current best fix or mitigation>
## Occurrences
### 2. 2026-11-21 — dk-server-1, orbit
<context summary, written by the agent>
**Daniel asked:** "can you investigate why the drain keeps failing"
**Error lines (agent evidence):**
    HTTP 429 …
**Cause:** … **Fix:** …
**Links:** session:<id>, task:ORB-…, pr:…
### 1. …
```

Header: `title` (120 characters), `signature` (1–5 symptom or error
strings of at most 200 characters; what a later search matches), `status`,
`## Cause` and `## Fix` (600 characters each). Occurrences, newest first,
each with its date, host and project, a context summary (1200 characters),
Daniel's request word for word, up to 10 key error lines (300 characters
each), the root cause and fix or "cause not found", and links to the
session and to Orbit tasks, frictions and PRs. Every free-text field
passes the redactor.

**Trigger.** Daniel's typed investigation request. On a turn that passes
the hook's existing provenance guards (§4.1) and is not a `#scribe` or
`/scribe` line, `scribe hook user-prompt-submit` matches the `issue` tag's
triggers without a model. On a match it adds one line to the turn's
`additionalContext`: that this turn asks for a root cause, the trigger that
fired, and the commands below. It writes no record, so it runs in every
interactive session, whether or not `/scribe on` is set; each fire is one
audit line, so recall and precision can be counted (**Counting**, below).
(§13.12 proposed adding the hint only in a session that is on; that is
parked 2026-11-21, so the hint stays as stated here.)

**Writer.** The agent in the chat, the one that gives Daniel the answer:

1. Before diagnosing, it searches existing issues: `scribe issue search
   '<symptoms or error text>'` (model-free, over titles, signatures and
   occurrences; also `record search --kind issue`).
2. When it answers, it writes the result: `scribe issue add` for a new
   issue, or `scribe issue occur <issue-id>` for an occurrence on the
   matching one, each with `--author <crew> --related session:<id>
   --request '<Daniel's words>'`, the context summary, error lines, cause
   and fix. It prints one line naming the id and how to cancel
   (`#scribe forget <issue-id>` or `<issue-id>#<n>`).
3. If no cause is found, it writes the occurrence with "cause not found".
   A later occurrence that finds it updates `## Cause` and `## Fix`
   (`scribe issue update <issue-id> --cause … --fix … --status …`).

**Checks.** `--request` must be a verbatim span of one of Daniel's typed
turns in the named session's transcript on this host (§4.6's quote check;
`quote_required`, `quote_not_found`). Error lines are the agent's
evidence, labelled so wherever they are shown, and are not checked against
the transcript (tool output is not in the parsed conversation). An
occurrence written after the issue is `resolved` sets the status back to
`open` and marks the occurrence `recurred: true`, shown as "recurred after
fix". A wrongly matched occurrence is moved with `scribe issue move
<issue-id>#<n> --to <issue-id>` (or `--new`), which keeps `moved_from`.
Occurrences are never edited or removed except by move or Daniel's
`#scribe forget`; the file is replaced whole through `write_atomic` under
the lock, as every record is.

**No approval.** An issue is an agent entry of cited facts, append-only.
Daniel does not settle it; he forgets an occurrence or an issue that is
wrong. Issues are not in the `SessionStart` brief or line; `ask` and
`record search` return them, labelled as the agent's account with his
quoted request. `issue` is a reserved tag name: `entry add --tag issue`
and `#scribe issue:` are refused, naming the `issue` commands, so the
capture rules live under `[tags.issue.capture]` like every tag's.

**Boundary with Orbit.** A friction is a defect report against one product,
and it closes. An issue is the memory of an incident across projects and
what fixed it. scribe files no work: if a fix needs doing, the agent files
the Orbit task or friction itself and links it from the occurrence.

**Store format.** Format 13 if it lands after §11.1 and §11.2: the
`issues/` directory and the `issue` kind are new; an older build refuses a
store holding one. No existing document changes shape.

**Counting** (step c2). Daniel, 2026-11-21, approving step (c): "let's
circle back later whether issue writes will require approval - we will need
to first run it without approval and see its recall/precision". So writes
stay unapproved and both halves are counted, with no model and no prompt
text:

- Each fire is one audit line (`hook user-prompt-submit`, `issue: {verb:
  fire, session, trigger, request_sha256}`): the session, the trigger
  phrase as configured and the prompt's sha256. The turn has no transcript
  uuid when `UserPromptSubmit` runs, so the hash (the key `#scribe` cites
  already use, §4.1) stands in for the message uuid; `request_message` is
  `null` on a fire.
- Each `issue add` or `occur` line carries, beside the request's uuid, the
  sha256 of the turn the quote check found it in (its whole text and each
  text block). A fire and a write name the same request when they share
  the session and a hash, and the write came after the fire.
- `scribe issue stats [--since <date>]` (model-free, read-only, no lock)
  counts from the log: fires; fires followed by a write on the same turn;
  writes; writes no fire matched; writes later forgotten (`#scribe forget`
  of the occurrence or the whole issue) or moved (`issue move`), each also
  counted once as withdrawn. It derives two ratios and names each a proxy:
  **precision ≈ writes neither forgotten nor moved ÷ writes**, and
  **trigger yield ≈ fires that led to a write ÷ fires**. Plain, table and
  `--json` ([specs/output-contract.md](specs/output-contract.md)).

What the counts cannot measure: an investigation no trigger fired on and
no agent recorded leaves no line, so recall is seen only from the side of
the writes (`writes_without_fire` are investigations the trigger missed but
the agent caught; the ones both missed are invisible). A write never
withdrawn is not shown to be right, only not yet judged wrong, so the
precision proxy is an upper bound that rises with Daniel's inattention. A
fire with no write may be a missed write or a trigger that fired on
something that was no investigation; the counts cannot tell which. A write
whose request the quote check found in a turn edited after it fired (or a
write before audit format 5) matches no fire. Lines lost to a failed
append are not counted.

*Built (step c2):* the trigger is `issue_trigger::fire` over the rules
`TagTable::issue_capture` returns: `[tags.issue.capture]` when configured,
else the shipped ones (route `agent`, the triggers in §11.2's table), which
the pass never takes, and `issue` stays out of the tag table. It matches
Daniel's own words only: a harness line or `/scribe` line is no turn, and
pasted, fenced, indented, quoted and `#scribe` lines are left out, as for
the pass (§11.2). A turn that fails the transcript check, or a config that
does not load, gives no hint and no line. The audit line is format 5
(`trigger`, `request_sha256`); the store format is unchanged.

*Built (record and CLI half, step (c)):* formats 13–16 were taken, so issues
are **format 17** (`issue_kind`, a no-op step). An issue file starts at
format 17 and an older build refuses one `store_too_new`; a format-16 build
never walks `issues/`, so it does not see them at all. Links take the
existing kinds (task, pr, repo, session); a friction is linked through its
task or PR until `related` gains a friction kind. The audit line (format 4)
carries `issue: {verb, id, occurrence, session, request_message,
moved_to}`. The trigger hint in `hook user-prompt-submit` came with step
c2 (**Counting**, above). The shapes are in [specs/store.md](specs/store.md) (**Issue
files**) and [specs/output-contract.md](specs/output-contract.md).

### 11.5 What Daniel still approves

Only `preferences` claims, through the candidate proposals of §5.3, and
`standard` entries, through their existing route (a `#scribe` line he
types, or an agent entry quoting him, then the intake's change he
reviews). Ideas are written by the pass, issues by the agent; he cancels
either with `#scribe retire|forget <id>`.

### 11.6 Idea capture after `finding` is gone

[ORB-25987] (the pass captures Daniel's ideas itself) was to reuse the span
route of [ORB-25983] (the pass writes findings), in which the model points
at a span and the pass copies it. ORB-25983 was archived on Daniel's "Stop
and close it" (2026-11-21), so no span route exists. Idea capture is built
on the **existing candidate route** instead: the model returns a quote and
a message id, and the pass checks the quote verbatim against the original
transcript, as §5.3 does for preferences. The only difference is the
route: under `route = "write"` the checked claim is written as an active
`idea` entry in the same locked step that would otherwise open a
candidate, with no proposal. Before that write, the pass calls the shared
`covering_claim` check under the store lock ([§5.3](#53-a-pass),
[§11.7](#117-order-and-sequencing) step (b2)). On this direct-write path
the only meaning-level match is the model's exclusion of existing
statements in the same session-model call (the bounded
`write_exclusions`); the store applies only the
lexical check and does not call a model again. A paraphrase the model
still returns is written unless that lexical check catches it. The lexical
check does not match a short statement against the long raw paragraph it
summarizes. The per-pass bound
(ORB-25987's 3 ideas per session per invocation, separate from the
candidate slots) and the stuck-line exemption are as ORB-25987 states.

Two of ORB-25987's points change with this design; Daniel approved both
(2026-11-21, [4_decisions.md](./4_decisions.md#the-session-pass-writes-daniels-ideas-itself-with-their-outcome)),
and step (b2) builds them:

- **Content.** ORB-25987 makes the content "byte-for-byte his span". With
  §11.1 the content is the statement, the span is the entry's `quote`, and
  `stated_by` says the pass wrote the statement. The span is still checked
  byte-for-byte.
- **Assistant ideas.** ORB-25987 point 4 captures an assistant suggestion
  Daniel defers. With `from = "daniel"` the triggers fire only on his
  words, and no assistant-turn route remains, so point 4 is not built until
  seen.

**Built (step b2).** The shipped `idea` tag has `route = "write"` (its
criteria: Daniel proposes something that could be built, changed or tried,
and the session does not carry it out; its `not`: a direction to act now, a
preference, a question of fact, an idea the session goes on to carry out).

- **What is written.** A claim under a write-route tag, from a Daniel turn
  that fired its trigger, whose quote is checked byte-for-byte against the
  transcript and passes every check a candidate passes (admission again
  under the lock, statement and subject, provenance, redaction), is written
  as an active entry in the same locked step, with no proposal: content is
  the statement, `quote` the span, `author` and `stated_by` are
  `{kind: scribe, name: session-pass}`, `content_from` is his message, and
  `said` is empty. It is never listed under `--author-kind daniel` (it is
  `--author-kind scribe`), and `ask`, `record search` and `entry show`
  label it "captured by the session pass", apart from a typed
  `#scribe idea:` entry. Assistant turns never fire a trigger (`from =
  "daniel"`), so an assistant's idea is never written.
- **Outcome.** Each such entry has an `outcome` (format 16,
  [specs/store.md](./specs/store.md)): `open`; `parked` with
  Daniel's words, kept only when they are a span of the turn the model
  names and redaction leaves them unchanged (otherwise `open`); or `task`.
  An idea the model says became a task in the same session writes no entry,
  and only when the task id it names appears in the session's text;
  otherwise it is written `open`. The pass never writes `task`; the state is
  reserved for a later settling route (not built).
- **Bounds.** At most 3 ideas per session per pass invocation (`MAX_IDEAS`),
  counted apart from the 3 candidate slots and the 20 open candidates, in
  the same session-model call (no extra call). The schema allows 6 claims
  per window; the pass and the store each cut write claims at 3. Across a
  pass's windows only ideas the store wrote spend the 3: a drafted idea
  dropped as a task or a duplicate leaves a later window its slot.
- **Guards, and what each misses.**
  1. *Model exclusions* (`write_exclusions`): the active entries of the
     firing write tags in the record's project, each its content and, for a
     stated entry, Daniel's words, ranked by words shared with the window,
     at most 10 of 400 characters, sent as data the model must not return.
     It is the only meaning-level match, so it catches a paraphrase of an
     existing idea, but it is a model's judgement: not deterministic, and
     an idea ranked out of the 10 is not seen.
  2. *Prior candidates* (`claim_covers` under the lock): a claim whose quote
     or statement an earlier candidate of any state already says is
     dropped. Lexical, so it misses a paraphrase.
  3. *Active entries* (`covering_claim`, the shared approval check): an
     active entry of the same tag and project whose content or quote says
     the claim's quote or statement. It catches the same or a contained
     text, and misses a paraphrase and a short typed idea against the long
     raw paragraph the claim quotes.
  4. *Retries* (`written_from`): an entry the pass already wrote from the
     same turn under the tag, with the same quote or forgotten. It stops a
     retried pass from writing it twice and a forgotten idea from coming
     back, and misses nothing within one turn; a re-statement in a later
     turn is left to guards 1 to 3.
- **Not counted as stuck.** The `SessionStart` stuck line skips
  pass-captured ideas: nobody took them on purpose, so they are not waiting
  for an intake.
- **Cancelling.** Daniel retires or forgets one with `#scribe retire|forget
  <id>`; forgetting blanks the quote and the parked words too.
- Not built: off sessions, a digest, settling an earlier idea's outcome
  later, `related` links, and any write to nebula.

**ORB-26204** (an agent Daniel instructs approves claims and deletes
records, designed in [§12](#12-settling-and-deleting-on-daniels-instruction-proposed)) sequences after step (a): the cleanup and the trigger gate cut
the queue he must settle, from 20 open claims to about 4, before any new
settle route is designed against it. Its design may proceed in parallel;
its implementation lands after (a), and if it lands after (b) its approval
writes the statement as content like a typed approval.

### 11.7 Order and sequencing

1. **(a)** Capture rules, the retired-tag cleanup, and dropping `remind`
   and `finding` (format 11).
2. **(b)** The statement on every claim (format 12).
3. **(b2)** The pass writing entries itself (`route = "write"`, not an
   approved or filed proposal), revised as in §11.6 (built, format 16).
   Before it writes, it calls the shared `covering_claim` check under the
   store lock. On that
   direct-write path the only meaning-level match is the model's exclusion
   of existing statements in the same session-model call (the bounded
   `write_exclusions`); the store applies only
   the lexical check and does not call a model again. A paraphrase the
   model still returns is written unless the lexical check catches it. The
   lexical check does not match a short statement against the long raw
   paragraph it summarizes.
4. **(c)** Issues (format 17; the record and CLI half is built).

Each step is its own task, lands with its `specs/store.md`,
`specs/output-contract.md`, skill and golden updates, and leaves the
store readable by the next step's build.

### 11.8 Limits and what is not built until seen

- **A trigger is a word, not a meaning.** "never" fires on "I never got
  that error"; the criteria gate removes it, at the cost of the model call
  the trigger gate was to save. A preference phrased without any trigger
  is missed; Daniel adds a trigger in config, or types `#scribe`.
- **The subject check is the model's.** An empty subject is refused; a
  wrong one is not caught. Daniel reads it before approving a preference;
  an idea's wrong subject is caught only when read.
- **The hook line is advice.** An agent can ignore it, and no issue is
  written.

Not built until seen:

- a backstop when an agent ignores the issue hook line (for example the
  pass noticing an investigation with no issue written);
- issue triggers on symptom words ("429", "out of memory") rather than his
  request;
- `from = "assistant"` capture for any tag, and ORB-25987's deferred
  assistant ideas;
- capture rules on store tags (`tags.json`);
- a model that matches an occurrence to an issue (the agent searches and
  chooses);
- issues in the session-start brief, and any bound on an issue's number of
  occurrences or compaction of old ones.

## 12. Settling and deleting on Daniel's instruction (proposed)

[ORB-26204], designed 2026-11-21 and **not built**. Nothing in this
section is implemented until Daniel approves it and its implementation
tasks land after [§11.7](#117-order-and-sequencing) step (a). Where it
changes a rule stated earlier ([§1.1](#11-the-cli-agent-surface)'s guard,
[§5.3](#53-a-pass)'s "Agents cannot settle or edit candidate proposals"),
the earlier text describes what is built today and carries a pointer here.

**Built (task 1 of 6, ORB-26242).** The check of §12.2 and §12.5 and its
refusal codes: `scribe-core/src/instruction.rs` (word lists, freshness,
entry points; no I/O) and `scribe/src/commands/instruction.rs` (finding and
reading the transcript). `proposal settle` and `entry retire` take
`--session` and `--instruction` and run the check before the lock. In this
task a passing check did not yet widen what they accept; tasks 4 and 3
below did (retiring and forgetting his entries, and settling a candidate),
and they built `entry forget` and the instruction record of §12.3.

**Built (task 4 of 6, ORB-26245).** The entry rows of §12.1 and §12.4: with
a passing check, `entry retire` accepts an entry Daniel wrote, hides it from
`entry list --status active` and `ask` with `retirement_reason` set, keeps
its text and records the instruction with the redacted quote (since format
16 in `removal_instruction`, apart from an approval's `instruction`; below).
`entry forget
<id> --author <name> --session <id> --instruction '<words>' --confirm`
refuses `usage_error` (exit 2) without an instruction and `confirm_required`
without `--confirm`, both before the transcript is read; it then applies the
same forget as `#scribe forget` (`consistency::forget`), keeping the
instruction's session, message and time but not its quote. Content changes
to his entries stay refused (`entry update` takes no instruction). Each
change or refusal writes one audit line with the command, target and
`--author`, never the instruction's words; since task 3 it also carries the
instruction's session and message ids (below).

**Built (task 3 of 6, ORB-26244, store format 15).** The proposal rows of
§12.1 and §12.3: with a passing check, `proposal settle <p-id>
--approve|--reject` accepts an open `candidate` (`Store::change_agent_proposal`,
`settle_candidate_on_instruction` in `store/agent.rs`), re-checking its state
under the lock. An approval writes the reserved entry as a typed `#scribe
approve` would (his for a claim from his turn, `{kind: agent, name:
assistant}` for one from an assistant's), adding `approved_by: {kind: agent,
name}`, the `instruction` (quote redacted) and, as `said`, the canonical
command `scribe proposal settle p-<n> --approve --author <name> --session
<id> --instruction '<his words, redacted>'`. A rejection writes no entry and
sets `closed_reason: instructed`; the claim stays in the pass's suppression
set. Both record `settled_author` (the agent), `settled_by` (the instruction
turn's cite: session, message, time, the turn's SHA-256 and the quote's line;
`cwd` is empty, as the transcript's is not read) and `instruction`. The same
command again changes nothing; any other settlement of a settled claim is
`proposal_settled`. A consistency proposal about his entry is still refused
`daniel_only`, instruction or not, and `--reason` cannot replace a claim's
reason (`proposal_not_agent`). The entry is rebuilt from the proposal, so the
scan's recovery of an interrupted instructed approval writes the same
attribution, never a typed `#scribe approve` line. An approved claim whose
reserved entry is missing when an active entry already covers it (at
approval, in the scan's recovery, or on a repeated typed approval) is closed
`rejected` with the new `closed_reason: covered`, keeping its settlement and
instruction, for typed approvals too: it never reads as a rejection nobody
made. Every `proposal settle`, `entry retire` and `entry forget` audit line
now carries `instruction_session` (from `--session`) and
`instruction_message` (the turn the check found, also when the check then
refuses it as stale, mismatched or not interactive; `null` only when no
typed turn held the instruction or that turn has no message id), audit
format 3.

**Built (task 5 of 6, ORB-26246).** Reading it (§12.3): `entry show` prints `approved by <aide> on your instruction "<quote>" (<session>, <date>)` under the cite for an entry an agent approved on his instruction, and nothing for one he approved by typing. `record search` ends such an entry's snippet with the same line, and `proposal list` adds `approved by` or `rejected by …` to an instructed candidate's reason. `ask` keeps the entry as his words with his 1.5× weight and adds the line to its evidence item as `<approval>approved by <aide> on Daniel's instruction …</approval>`, after the item's text: the model reads it, so it names him in the third person, and no claim can quote it as the entry's words. A prompt rule tells the model to say an agent approved it when it uses the item. `--json` is unchanged: `approved_by` and `instruction` were already on entries and proposals.

**Fixed (ORB-26503, store format 18).** Review ORB-26500 of task 3 found
five defects, now fixed: a format-14 entry decodes again through typed
YAML, so a local YAML tag reads as before (format 16's `EntryV14`; a
fixture test pins it); an instruction whose turn has no
message id is refused `instruction_not_found` before anything is written;
an approval or rejection is refused `instruction_stale`, under the lock,
when the instruction was typed before the claim's `opened_at` (check 3's
"claims opened since"); a refusal after the turn was found audits its
message id; and retiring or forgetting an approved entry records its
instruction in `removal_instruction`, leaving the approval's `instruction`
in place (§12.7).

The reasons and alternatives are in
[4_decisions.md](./4_decisions.md#an-agent-settles-claims-and-deletes-records-on-daniels-checked-instruction).

Daniel, chief's chat, 2026-11-21: "I will be interacting with the scribe
primarily to manage scribe artifacts, e.g. instruct scribe to approve
certain claims, delete certain artifacts, etc. the scribe will be using cli
tool to perform these operations." "The scribe" is the formation's aide, an
agent session he types to; a claim is an open candidate proposal (§5.3).

**What the CLI refuses today** (read at 5362ac2):
`Store::change_agent_proposal` (`crates/scribe-core/src/store/agent.rs`)
refuses every `candidate` with `daniel_only` ("Daniel settles candidate
proposals through #scribe approve or reject"), and a proposal whose older
entry is Daniel's the same way; `entry retire` and content changes by
`entry update` are refused on his entries; there is no CLI `forget` and no
delete verb for any record. **Observed need**: 20 open proposals on the
Mac store, all candidates (11 from his turns, 9 from assistants' turns),
about 4 once §11.3's cleanup closes the retired-tag ones.

### 12.1 The route

The aide lists what Daniel asks about (`proposal list`, `entry list`,
`record search`), names the ids in its reply, and on his answer runs one
command per target with his words attached:

| Daniel says | The aide runs |
|---|---|
| approve or reject a claim | `scribe proposal settle <p-id> --approve\|--reject --author <name> --session <id> --instruction '<his words>'` |
| delete a claim | the same, `--reject` |
| delete an entry | `scribe entry retire <id> --author <name> --reason <text> --session <id> --instruction '<his words>'` |
| forget an entry for good | `scribe entry forget <id> --author <name> --session <id> --instruction '<his words>' --confirm` (new verb) |
| delete a session | `scribe session deactivate <id> --author <name> --reason <text>` (unchanged, [§5.7](#57-active-and-inactive-sessions)) |

`--instruction` and `--session` come together or not at all (a usage
error, exit 2). Without them every verb behaves as built today. With them,
and only after the checks of §12.2 pass, `proposal settle` accepts a
`candidate` and `entry retire` accepts an entry Daniel wrote. `entry
forget` exists only with them. Nothing else widens: an instruction does not
let an agent change his entry's content, settle a consistency proposal
about his entry, or write any entry other than the one a candidate already
reserved.

### 12.2 Question 1 — how his instruction is evidenced, and what the store verifies

The agent passes the session it is running in (`--session <id>`) and a
verbatim span of Daniel's instruction (`--instruction`). The CLI, before
it takes the store lock, finds that session's transcript on this host and
runs these checks; any failure refuses the command and changes nothing.

1. **Provenance.** The transcript passes §4.1's checks (under the
   transcripts root, owned by this uid, named `<id>.jsonl`, one no-follow
   handle for check and read), as `verify_quote` does today
   (`crates/scribe/src/commands/entry.rs`). No transcript on this host:
   `instruction_not_found`.
2. **His typed turn.** `--instruction` is a verbatim span (§6.4's check) of
   one user turn that §4.1's origin guard accepts as plainly typed:
   no agent or harness wrapper, not inside a fence, blockquote or
   `<pasted_content>` block, not a sidechain or meta line. Otherwise
   `instruction_not_found`. A short answer below §6.4's 8 characters
   ("yes") passes when it is his whole turn; when several of his turns
   hold the span, the latest is the instruction.
3. **Fresh.** That turn is one of the session's last three typed user turns
   and was written at most 30 minutes before the command runs, never in the future. Otherwise
   `instruction_stale`. An old instruction cannot be reused to settle
   claims opened since.
4. **It names the target.** The target's id (exact, or the unique prefix
   the command resolved) appears either in that turn or in the assistant
   message immediately before it, the reply he answered. Otherwise
   `instruction_mismatch`. "Approve p-4 and p-9" names its targets; so
   does "yes" after "Approve p-4 and p-9?".
5. **It names the act.** Model-free word lists in `scribe-core`
   (`instruction.rs`, no I/O):

   | Verb | His turn contains one of |
   |---|---|
   | approve | approve, accept, keep, yes, ok, okay, go ahead, do it |
   | reject | reject, decline, drop, discard, delete, remove |
   | retire | retire, delete, remove, hide, drop |
   | forget | forget, erase, purge, permanently, for good |

   For approve, when his turn's only match is an affirmative (yes, ok,
   okay, go ahead, do it), the reply he answered must contain "approve" or
   "accept" next to the ids (on the same line as the id). Words match
   whole and case-insensitively ("keeper" is not "keep"). For reject,
   retire and forget, the verb's word
   must be in his own turn; an affirmative alone does not delete anything.
   The check also refuses `instruction_mismatch` if his turn or the assistant
   message it answers contains a negation or exception marker anywhere. It
   shares the duplicate check’s markers (including not, no, never, don’t,
   dont, can’t, cannot, won’t, shouldn’t, except, unless and without), and
   also refuses but, stop, cancel, other than, avoid and only ("approve only
   p-4" is refused too). The scan covers the whole turn and the whole answered
   message, not just the line that names the target; this over-refusal is
   kept as designed, since it never accepts a write it should refuse. This
   conservative rule refuses “Don’t forget to approve p-4” even when approval was intended;
   ask for a plainer instruction. Otherwise `instruction_mismatch`.

The store, under its lock, checks only that an instructed write carries an
instruction record (§12.3) and re-checks the target's state; reading the
transcript is I/O and stays in the CLI, as the action-tag quote check does
([4_decisions.md](./4_decisions.md#the-quote-check-runs-in-the-cli-against-this-hosts-transcript)).

This is an accident guard (STD-05@1 §R5), like every provenance check in
scribe: it stops an agent acting on its own, a stale or misread
instruction, and an instruction meant for another record. It is not a
security boundary: a process running as Daniel can write a transcript
file. The marker check refuses some intended instructions and does not
understand sarcasm or associate each act with its target.

### 12.3 Question 2 — whose entry an approval writes, and how it reads

**A candidate from his turn** is approved into the entry the candidate
reserved, **as his**: `author: {kind: daniel}`, the original turn's cite
and `content_from: message` (user), exactly as a typed `#scribe approve`
writes it, because the content is words the pass already checked against
his turn (or, once §11.1 lands, a statement with his checked words in
`quote` and `stated_by` set). What differs is who settled it:

- the proposal records `settled_author: {kind: agent, name: <aide>}`,
  `settled_by` the instruction turn's cite (session and message uuid,
  both known from the transcript), and a new `instruction: {session,
  message, at, quote}` (the quote redacted);
- the entry records a new `approved_by: {kind: agent, name}` (`null` when
  he typed the approval) and `said` is the canonical command with
  `--instruction`, not a `#scribe` line: `scribe proposal settle p-<n>
  --approve --author <name> --session <id> --instruction '<his words>'`,
  the words as stored (redacted) and shell-quoted.

This refines the rule of
[4_decisions.md](./4_decisions.md#daniel-writes-only-through-his-typed-prompt-agent-names-are-attribution)
that no CLI path records `{kind: daniel}`: the CLI still never records
words as his that are not already checked as his typed words; it only
settles a claim made of them, on a typed instruction it has checked.

**A candidate from an assistant's turn** is approved into one
agent-authored entry, as a typed approval writes it today (§5.3), with the
same proposal fields. **A rejection** writes no entry and keeps the claim
in the suppression set, recording the same fields.

**Reading it.** `entry show` prints `approved by <aide> on your
instruction "<quote>" (<session>, <date>)` under the cite. `ask` and
`record search` treat the entry as his words, with his 1.5× weight, and
add the same line to the evidence, so an answer can say that an agent
approved it (as built, `ask`'s line names "Daniel's instruction" and sits
outside the item's quotable text). `proposal list --state approved|rejected` shows the
instruction. `--json` adds the fields; none is renamed or retyped.

### 12.4 Question 3 — what "delete" means for each record kind

| Kind | "Delete" does | Reversible | What stays |
|---|---|---|---|
| Proposal (any open claim) | `reject`: closed, no entry, `closed_reason: instructed` ([§11.3](#113-the-shipped-tag-set-and-retired-tag-cleanup)'s field) | no; the suppression set keeps it from being proposed again | the proposal with its quote, reason and instruction |
| Entry | `retire`: hidden from lists and `ask`, `retirement_reason` set | no un-retire verb exists, but the text is intact and can be added again as a new entry | the whole entry, `updated_by`, the instruction (`removal_instruction`, apart from an approval's `instruction`) |
| Entry, "forget" or "for good" | `forget`: entry text and evidence cleared, with every proposal quote of it and related candidate quotes, evidence and reasons (§7) | no | id, tag, dates, author, `updated_by` and the instruction's session and message ids (`removal_instruction`; an approval's `instruction` keeps its ids too); **not** their quotes, which may repeat the words he wants gone |
| Session | `deactivate`: hidden from `ask` and lists unless `--include-inactive` | `session activate` | the record and its log |

"Delete" never removes a file. A removed proposal would leave the
suppression set and come back on the next pass; a removed entry would
break the cites and `superseded_by` links of others. **Audit**: one line
per change or refusal, as every write verb (§1.1): the command, the
agent's name, the target id, the instruction's session and message ids,
and the error code on a refusal (`instruction_not_found`,
`instruction_stale`, `instruction_mismatch`, plus the existing codes). The
instruction's words are never in the audit line; they live on the record
(and, after `forget`, not on the entry or its proposals; session records may retain them).

### 12.5 Question 4 — only an interactive session he is in

Three checks, all in the CLI before the lock:

1. **Interactive.** The instruction turn's transcript line records the
   harness entry point of an interactive session. For Claude Code that is
   `entrypoint` `"cli"` (the terminal) or `"claude-desktop"` (the desktop
   app); `"sdk-cli"` (`claude -p` and the Agent SDK, which every Orbit run,
   routine and drain uses) and any other or no value is refused
   `not_interactive`. On the Mac over the 7 days to 2026-11-21 the
   transcripts were `sdk-cli` 336, `claude-desktop` 62 and `cli` 0, so
   Daniel's own sessions are `claude-desktop`; the 40 most recent
   transcripts on dk-server-1 that day were all `sdk-cli`. For Codex the
   entry point is the rollout's `session_meta.payload.source`: `"cli"`
   (the terminal UI, `originator: codex-tui`) and `"vscode"` (the IDE and
   desktop app) are interactive; `"exec"` (`codex exec`,
   `originator: codex_exec`) and a subagent's source are refused
   `not_interactive` before the rollout is parsed. Of 400 rollouts
   sampled on dk-server-1 on 2026-11-21, 353 were `exec`, 28 a Codex
   Desktop `guardian` subagent and 2 `cli`; the parse already admits only
   `cli` and `vscode` main threads (§4.1), so the mapping refuses nothing
   capture accepts.
2. **Typed by him and fresh** — §12.2 checks 2 and 3: a turn he typed in
   the last 30 minutes, among the last three, so the session is one he is
   in now.
3. **On this host.** The transcript is read from this host's transcripts
   root, so the aide must run where the store is (the Mac). An aide on
   another host is refused `instruction_not_found`.

The CLI cannot tell that `--session` is the caller's own session: an
agent in another session could name his. Checks 2 and 4 of §12.2 bound
that to the targets and act his recent typed words name. The skill tells
the aide to pass its own session id.

### 12.6 The aide's side (the skill)

The skill's settling section tells the aide to: list before acting and
name every id in its reply; act only on his explicit instruction in this
session; run one command per target; echo each result in one line (`p-4
approved as e-…`, `e-12 retired`), and the reversal: a wrongly approved
claim's entry is retired (`#scribe retire <id>`, or on his instruction);
a wrong retire is repaired by adding the text again as a new entry; a wrong reject or
forget cannot be undone. It never runs `entry forget` unless his words say
forget, erase or for good.

**Built (task 6 of 6, ORB-26247).** The aide's side is the section "Settling
and deleting on the human's instruction" at the end of
`skills/scribe/SKILL.md`, copied to the two plugin skills by
`scripts/sync-skills.py`. It sets the §12.1 command shapes, the order (list;
name each id and its reversal in the reply; act only on an explicit
instruction; one command per target), the one-line echoes and the reversals
above. Where the skill is narrower than the CLI, the skill wins for the aide:
it runs `entry forget` only on the words forget, erase or for good, while
§12.2 point 5's list also takes purge and permanently. A wrong retire is
repaired by the human typing the `#scribe <tag>:` line again, because the aide
never writes an entry of the human's. The reply that names the ids is checked
for the same negation and exception words as the human's turn, so the skill
tells the aide to keep that reply free of them. A session deactivation is
reversed by `session activate` (§12.4).

### 12.7 Store format

**Built (task 2 of 6, ORB-26365, store format 13).** Step 12→13
(`instructed_settlement`): a proposal gains `instruction` (`null` on upgrade);
an entry gains `approved_by` and `instruction` (`null` on upgrade).
`closed_reason` comes from §11.2's format 11, gaining the value `instructed`.
`specs/store.md` and `specs/output-contract.md` describe the fields and format
13.

**Built (task 3 of 6, ORB-26244, store format 15).** Step 14→15
(`instructed_approval`) changes no stored value: it admits an entry whose
`said` is the canonical `scribe proposal settle p-<n> --approve …` command,
only beside its `approved_by`, and `closed_reason: covered` on a settled
candidate, so a format-14 build refuses a store holding either as too new
rather than corrupt.

**Built (ORB-26503, store format 18).** Step 17→18 (`removal_instruction`)
adds an entry's `removal_instruction`, the checked instruction that retired
or forgot it, so `instruction` is only ever the instruction `approved_by`
approved on and a removal never reads as evidence for an approval. An entry
without `approved_by` moves its `instruction` there; an approved one keeps
it. [4_decisions.md](./4_decisions.md#an-entry-keeps-its-approval-instruction-apart-from-its-removal)
gives the reasons.

### 12.8 Limits and what is not built until seen

- **Word lists, not meaning.** Negation and exception markers cause a
  conservative refusal, including “Don’t forget to approve p-4”. Sarcasm
  still passes, and a turn naming two acts ("approve p-4, reject p-5")
  passes both lists for both ids, so the aide’s choice decides which id
  gets which verb.
- **Same user.** An agent able to write a transcript file can forge an
  instruction; this is the accident guard of §4.1, not a boundary.
- **Mac only.** The aide must run on the host of the store and of its own
  transcript.

Not built until seen:

- settling a consistency proposal about his entry, or changing his
  entry's content, on instruction (all 20 observed claims are candidates);
- removing a session record or blanking its text (`session forget`):
  entry forget does not erase the session record, including its summary and
  segment text; deactivation is the delete;
- forgetting a proposal's words without an entry (a rejected claim keeps
  its quote for suppression);
- settling many ids in one command or by a description ("all the tmux
  claims") resolved by the store rather than by the aide listing ids;
- instructions from a non-interactive or remote session, and any token or
  confirmation dialog beyond the transcript check.

## 13. Memory: profile, projects, working, now and milestones (proposed)

[ORB-26219], designed 2026-11-21. Steps (a) and (b) of
[§13.10](#1310-order-and-sequencing) are built: (a) (ORB-26258, store
format 19) the `memory` kind, pass extraction with its checks and sensitive
gate, `scribe memory show|ack|retire|forget`, and the session-start page;
(b) (ORB-26259, store format 20) the consistency scan's merge and
supersede, the `now` to `milestones` promotion rules and `scribe memory
rules`. As built, a merge and a promotion **move** the occurrences (the
merged or promoted item keeps none, and `forget` follows the link), a pair
judged `unrelated` is kept on the older item (`unrelated_to`) so it is
never offered again, and the pass's `milestones` element that no rule (a)
or (c) holds for is written as a `now` item. Step (c) is **not built**
until its implementation task lands,
after
[§11.7](#117-order-and-sequencing) step (b). Where it changes a rule stated
earlier (§5.3's "A pass writes no tag entries" and its bounds, §7's
steps, the `SessionStart` hook of §1, which today tells only Daniel and
never the agent), the earlier text describes what is built today and
carries a pointer here. It reverses
[3_vision.md §4](./3_vision.md#4-deliberately-out-of-scope) for descriptive
memory only. The reasons and the alternatives are in
[4_decisions.md](./4_decisions.md#scribe-keeps-a-descriptive-memory-the-pass-writes-without-approval).

[ORB-26240], designed 2026-11-21 and **not built**, added
[§13.12](#1312-a-personal-base-and-a-work-layer-per-session) to
[§13.16](#1316-limits-of-the-addendum-and-what-is-not-built-until-seen):
memory split per session into a **personal base** that every session a
human types in would feed (`profile`, `now`, `milestones`, a sixth section
`people`, and recurring dates) and a **work layer** that only sessions
switched on with `/scribe on` feed, with a **light catalog entry** for a
session that is off. **Those parts are parked, 2026-11-21 ([ORB-26256]).**
Daniel, chief's chat, 2026-11-21, of the consolidated task list of this
section: "park 2-5, keep 1, 6, 7". Parked, each marked in its own section
with his words and its text kept for when he unparks it: the base pass for
off sessions with `/scribe private` (§13.12), `people` (§13.13), recurring
dates (§13.14) and light catalog entries (§13.15). Kept: the three steps of
[§13.10](#1310-order-and-sequencing), (a) the `memory` kind and the
session-start page, (b) consolidation and promotion, and (c) `working`
items proposed as `preferences` candidates. The reason: Daniel decided the
same day that scribe reads **Claude Code and Codex sessions only** ("no
hosted server - I am not managing other's memory. claude code and codex
support only then. so be it", [4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)),
so there is no surface whose sessions a base would serve beyond what
`/scribe on` already covers. The decision is in
[4_decisions.md](./4_decisions.md#the-personal-base-people-recurring-dates-and-light-catalog-entries-are-parked).

What the parking means for the rest of this section:

- **Sources.** While the base is parked, memory items are written **only
  from sessions switched on with `/scribe on`** (or named with `session
  catalog`, §5.8), including the personal sections (`profile`, `now`,
  `milestones`). An off session writes nothing, as today. Where §13.3 and
  §13.8 say "a session he switched on" as the only source, that is the rule
  in force; the pointers to §13.12 describe the parked design.
- **Fields.** Slice (a) does not add `layer`, `person` or `recurs` to an
  item: only parked parts read them. They come, in a store format step of
  their own, with the part that needs them (`layer` with the base pass,
  `person` with `people`, `recurs` with recurring dates).
- **Settings.** `[memory] base`, `base_idle_minutes`, `[memory.sections]
  people` and `[memory.recurring]` exist only for parked parts and are not
  shipped with (a).
- **`[memory] personal`** ships **true** (§13.1). It decides whether the
  pass of an on session may keep personal elements; it does not choose
  which sessions feed memory.
- **Sections and page.** Five sections, a page of 2,400 characters, no
  `people` part, no derived recurring lines, and a recent-sessions list of
  the sessions on record.

Daniel, chief's chat, 2026-11-21: "I think we should expand scribe's role
to manage memory system. we already keep track of sessions, preferences,
and issues … What we are lacking is a holistic memory, e.g. user profile,
interests, projects, working preferences, etc." Then: "let's just include
profile, projects, and working." Thinking of a user other than himself:
"memory --- provides a holistic overview of the user -- akin to how a human
builds a \"sense\" of another person; preferences, projects, recent
events/sessions (list last n title and session_id?)". On time-bound events:
"don't fade it out until the due date … It feels nice when other people
remember details about you … even after the event (not reference it but at
least be able to recall)", and "we add \"events\" or something, for big life
events like getting married, having a child, graduating, promotion, moving
to a new city, etc. and we can promote some of \"now\"", named "milestones".

**Observed need** (ws_constellation ORB-26194, 2026-11-21): about 60
Claude-only auto-memory notes on this host (60 files under
`~/.claude/projects/*/memory/`, counted 2026-11-20) against 2 scribe
`preferences` entries, so other providers read none of what agents have
learnt about how he works; about 8 of those notes exist only to say what a
repository is or that it is retired; the old hand-run profile
(`knowledgebase/almanac/about-me`, last distilled 2026-07-08) still named a
host that has since changed role, and nothing injected it at session start.
Handoffs carry today's state ("a release under way", "dk-server-1
unreachable") by hand. The `now` need of a regular user (Daniel's example
of his wife) is stated, not yet observed in use.

### 13.1 Three layers and five sections

(A sixth section, `people`, is designed in [§13.13](#1313-people) and
parked 2026-11-21. "Layer" in this table is the kind of record, not the
personal base and work layer of the parked §13.12.)

| Layer | What it holds | Lifetime | How an agent reads it |
|---|---|---|---|
| Sessions ([§5](#5-scribe-on)) | the dump of history | permanent | the last 5 on the page; the rest through `record search` and `ask` |
| `memory` | a sense of the user | by section, below | the session-start page; `scribe memory show`; `ask` |
| `preferences` (an entry tag) | rules that bind agents | until retired | as today; Daniel approves each |

A **memory item** is one statement about the user with the occurrences
that support it. Its **section** is exactly one of:

| Section | Holds | Example statement | Scope | On the page |
|---|---|---|---|---|
| `profile` | role, skills, hosts, environment (and, with `personal` on, household as he states it) | "Runs his agents from a Mac and two Linux boxes." | global | until superseded; no decay |
| `projects` | a project's purpose, status and current focus, with an as-of date; only what Orbit does not hold (why it exists and where it stands, never its tasks) | "scribe: being widened from preferences to a whole memory (as of 2026-11-21)." | one project | the current project's only; 90-day decay |
| `working` | how he steers agents, as observed | "Asks for the design to be filed as a task before any build." | global | 90-day decay |
| `now` | upcoming things and recent happenings | "Said his sister's wedding is on 2026-12-12." | global | [§13.4](#134-now-and-milestones) |
| `milestones` | big, dated life or project events | "2026-06-14 — married." "2026-10-02 — pulsar decommissioned." | global | always, no age cutoff |
| `people` ([§13.13](#1313-people), **parked 2026-11-21, not part of (a)**) | brief records of people and pets close to the user, made only when the user brings them up | "Sam (his husband): works as a nurse." | one person | always, one line each |

`working` **describes**; `preferences` **binds**. An agent may adapt to a
`working` item but must not treat it as a rule; only an approved
`preferences` entry is one (§11.5 unchanged). A `working` item that recurs
is offered to him as a preference ([§13.7](#137-from-working-to-preferences)).

The section names are fixed in code (`scribe-core::memory::Section`, a
closed enum, five members; `people` joins it only if
[§13.13](#1313-people) is unparked); config can switch a section off, not
add one. An `interests` section is not built (Daniel dropped it).

**Every section is switchable, and none is unconditional.** Each section
has its own setting (`[memory.sections] <name> = true|false`), and
`[memory] enabled = false` switches all five off. A section switched off is
neither written by the pass (the model is not asked for it, and an element
naming it is refused, `memory_section_off`) nor shown on the page;
`memory show` lists its items only with `--include-off-page`, labelled
`section off`; its items stay in the store, unread,
until it is switched on again or they are forgotten. The last-n sessions
list is not a section and has its own setting (`recent_sessions = 0`).

**Work and personal.** A second setting, `[memory] personal`, decides
whether personal life is kept at all, and it ships **true** (the default of
[ORB-26240], kept when its base was parked; ORB-26219 had shipped false).
Memory is written only from sessions switched on with `/scribe on`, and
`personal = true` means the pass of such a session may keep the personal
elements it finds in what he typed: household facts in `profile`, personal
events in `now` (§13.4), personal milestones, and the stated events
[§13.8](#138-sensitive-content) allows. Each element the model returns
carries a transient personal mark, checked when written and not stored.
For Daniel, whose on sessions are work, these stay empty. `personal =
false` refuses every element the sensitive word list of §13.8 flags, or
that the model marks personal, in every section (`memory_personal_off`), so
`now` holds work state ("a release under way", "dk-server-1 unreachable")
and `milestones` work milestones ("pulsar decommissioned"); no family
event, loss or household fact is written. The setting does not choose
**which** sessions feed memory: that is only `/scribe on` while the base
([§13.12](#1312-a-personal-base-and-a-work-layer-per-session)) is parked.
How a store chooses these settings as a whole was first to be `work` and
`personal` presets; [ORB-26240] replaced them with a per-session split,
which is parked, so no preset is built. This section only requires that
every setting it names can be set alone.

### 13.2 The item and its occurrences

A record kind of its own, like an issue (§11.4) and for the same reason: an
item grows an occurrence each time the user says it again, while an entry
is one statement. **File** `memory/<id>.md`, id `m-<n>` allocated like
entry ids, frontmatter only (no body):

```markdown
---
format: <n>
kind: memory
id: m-14
section: now                       # profile | projects | working | now | milestones
status: active                     # active | merged | superseded | promoted | retired | forgotten
statement: Said his sister's wedding is on 2026-12-12.
project: null                      # projects only: the project key sessions use
event_date: 2026-12-12             # now and milestones; null otherwise
sensitive: false                   # §13.8
sensitive_kind: null               # loss | health | relationship | family
acknowledged: null                 # now only: {at, session, author}
promoted_from: null                # milestones: the now item it came from
promotion: null                    # {rule, to, profile_from, profile_to} on the promoted item
merged_into: null
superseded_by: null
preference_proposal: null          # §13.7: the candidate opened for a working item
author: {kind: scribe, name: session-pass}
updated_by: {kind: scribe, name: session-pass}
created: 2026-11-21T05:40:00Z
updated: 2026-11-21T05:40:00Z
occurrences:
  - {n: 2, date: 2026-11-28, session: <id>, message: <uuid>, host: <host>, project: <key>, quote: "the wedding is two weeks out and I'm still on the seating", author: {kind: scribe, name: session-pass}}
  - {n: 1, date: 2026-11-21, session: <id>, message: <uuid>, host: <host>, project: <key>, quote: "my sister's wedding is on december 12", author: {kind: scribe, name: session-pass}}
---
```

- **Statement**: at most 200 characters, written by the pass's `session`
  model, standing alone (§11.1's rule: it names its subject). It reports
  what the user said or did, in the third person; it passes the redactor.
- **Occurrence**: one turn that supports the statement: its date (the
  turn's timestamp, not the pass's), session, message uuid, host, project,
  and the user's **verbatim quote** (8–400 characters, checked as in
  §13.3). Occurrences are newest first and are never edited; they are
  removed only by `forget` ([§13.9](#139-correction)) or moved by a merge.
- **Author**: `{kind: scribe, name: session-pass}` on the item and on each
  occurrence it writes; `{kind: scribe, name: consistency}` for a merge,
  supersede or promotion the routine makes. Never `{kind: daniel}`: the
  statement is scribe's, and only the quote is his
  ([4_decisions.md](./4_decisions.md#daniel-writes-only-through-his-typed-prompt-agent-names-are-attribution)
  is unchanged).
- **Derived, never stored**: whether an item is on the page, and why not
  (decayed, past its window). It is a pure function of the item and the
  day, in `scribe-core`, so no routine has to run for an item to leave the
  page, and `SessionStart` stays read-only
  ([4_decisions.md](./4_decisions.md#session-start-reads-only)).

### 13.3 Writing: the session pass

Writing is implicit: no `#scribe` line, no proposal, no approval. In an
on session's ordinary pass (including the windows of a session Daniel
names with `session catalog`, §5.8), the same `session` model call that
updates the record returns a `memory` array beside `candidates`.
`session rebuild` leaves it disabled, as it does candidates.

- **Source.** Only the user's typed turns of a session he switched on (or
  named for catalog; a session that is off writes nothing, the base pass of
  the parked [§13.12](#1312-a-personal-base-and-a-work-layer-per-session)
  not being built), with every exclusion of §5.3: pasted blocks, fenced
  or indented code, blockquotes, marked `#scribe` lines, synthetic and
  meta turns. Assistant turns are never a source (§11.3 already stops the
  pass reading them).
- **Bound.** At most **3 memory items per session per pass invocation**,
  shared across its windows and separate from the 3 candidate slots;
  refused items consume a slot. Each array element is `{section,
  statement, quote, message, event_date?, project?, same_as?,
  sensitive_kind?, promote?}`: statement 200, quote 400, message 128,
  section 12, event date 10, project 64, `same_as` 16, `sensitive_kind` 16,
  `promote` 160 characters, at most **1,006 value characters per element,
  3,018 per window**, in the same call with no extra call or timeout
  budget. Input grows by the active items of the sections, at most 40
  statements of at most 200 characters (8,000), selected newest first and
  by word overlap with the window, each with its id, so the model can name
  a repeat.
- **The verbatim check.** As for candidates: the quote must be a verbatim
  span of the turn its `message` names, in the current window, checked
  again against the original transcript under the store lock, with the
  cite, timestamp, host and project derived by the pass, not the model. A
  quote the redactor would change drops the item.
- **Dates.** The window gives each turn's timestamp; the model resolves
  "next Saturday" against it. A model-free check keeps `event_date` only if
  it is a real date within a year before and two years after the turn;
  otherwise it is `null` (the item is then undated, §13.4).
- **A repeat is an occurrence, not an item.** When the model names
  `same_as: m-<n>`, the pass appends an occurrence to that item if it is
  active, in the same section, and shares at least one content token in
  four with the new statement (`ask`'s tokenizer, as §7); otherwise the
  element opens a new item. The statement of the existing item is kept;
  the routine decides whether the new words supersede it (§13.5). An
  occurrence from a session already on the item is still appended (it is
  a new turn) but counts once toward recurrence.
- **Duplicates without `same_as`.** Under the lock, a new statement whose
  normalized tokens equal, contain or overlap (Jaccard ≥ 0.8) an active
  item's in the same section becomes an occurrence of that item instead.
  That equal, contain or Jaccard rule is memory's own; it is separate from
  §5.3's candidate duplicate guard.
- **The sensitive-content gate** of [§13.8](#138-sensitive-content) runs
  last; a refused element writes nothing and is counted in the pass
  report (`memory_refused`, by code).
- **Write.** Items and occurrences are written under the store lock before
  the record cursor advances, after the record's own checks, through
  `write_atomic` per file; a replayed window appends no second occurrence
  for the same message uuid. One audit line per item created or
  occurrence added, with ids only. The pass report gains `memory_written`
  and `memory_refused`.

**Config** (`[memory]`, every key optional, shown with its default):

```toml
[memory]
enabled = true                 # false: every section off
personal = true                # ORB-26240's default, kept (ORB-26219 shipped false); false: no personal element from an on session, §13.1
decay_days = 90                # projects and working leave the page
now_undated_days = 7           # an undated now item leaves the page
now_after_days = 14            # a dated now item stays this long after its date
page_chars = 2400              # the session-start page budget
recent_sessions = 5            # the last-n sessions on the page
promote_working_sessions = 3   # §13.7; 0 switches it off

[memory.sections]              # each section switched on or off alone
profile = true
projects = true
working = true
now = true
milestones = true
```

### 13.4 `now` and `milestones`

**`now`** holds what is happening to the user: a release under way, a
host down, and, with `personal` on (§13.1), a wedding next month, a rough
day, a loss. An item stores **what the user
said, with its verbatim quote, never an inferred state**: "Said his
grandmother died on 2026-11-20", never "is in a solemn mood". The
conversing model derives the tone from the event and its age; scribe
stores no mood, diagnosis or judgement. The pass enforces this without a
model: a `now` statement containing a word from the mood list in
`scribe-core::memory` (sad, solemn, upset, depressed, anxious, stressed,
grieving, happy, excited, angry, mood, and the like) that its quote does
not itself contain is refused (`memory_inferred`).

Lifecycle, all derived from the item's dates (§13.2):

| `now` item | On the session-start page | Then |
|---|---|---|
| dated (`event_date` set), upcoming | from its first occurrence through its date | stays, below |
| dated, date passed | for `now_after_days` = **14** days after its date | leaves the page |
| undated (a happening with no date: a rough day, a trip "this week") | for `now_undated_days` = **7** days after its latest occurrence; a new occurrence restarts the count | leaves the page |

An item that leaves the page is not changed: it stays `active` in the
store, in `scribe memory show --include-off-page`, in `ask` and `record
search`, and in its session. What fades is the tone adjustment, not the
memory. A big event does not fade: it is promoted.

**The `acknowledged` mark** limits **unprompted mentions only**; it never
removes an item from the page. It records when the item was last brought
up: `{at, session, author}`. It is set when

- the agent raises the item unprompted and then runs `scribe memory ack
  <m-id> --session <id> --author <name>` (an audited agent write; the
  skill tells the agent to); or
- the user raises it himself: a new occurrence from his turn sets the mark
  to that turn.

The page labels an item **not yet raised** when it has no mark, or when
its date has passed and its mark is from before the date. So a wedding is
raised once before it ("how is the seating going") and once after
("congratulations — how did it go"), and otherwise stays on the page as
something the agent knows and ties a conversation to when it fits.

**`milestones`** are permanent and dated: married, a child, graduated,
promoted, moved city, a loss; for a project the like of "pulsar
decommissioned". A milestone never decays and never leaves the page; it is
removed only by retire or forget. The agent knows every milestone and
raises one only when the conversation goes there ("recall, don't
reference"); the page says so in its header line.

**Promotion from `now` to `milestones`**, on three shipped defaults Daniel
edits like the capture rules of §11.2:

```toml
[memory.promote]
profile_change = true          # (a) the event changes a profile fact
recurs_in_sessions = 3         # (b) 0 switches it off
user_says = ["milestone", "big day", "remember this", "never forget", "life changing"]   # (c)
```

- **(a) A `profile` fact changes** (married, a parent, a new city, a new
  job). The model returns `promote: "<the new profile statement>"` on the
  element, naming the `profile` item it replaces (`same_as`) when one
  exists. In one locked step the pass writes the milestone (with
  `promoted_from` and its date), marks the `now` item `promoted`, and
  writes the new `profile` item, setting the old one `superseded` with
  `superseded_by`. The milestone's `promotion` records every id, so a crash
  between the files is completed by the next consistency scan, as approvals
  are re-applied (§7).
- **(b) The same event recurs in three or more sessions**: model-free,
  counted over the distinct sessions of the item's occurrences after each
  write and after each merge (§13.5).
- **(c) The user says so**: one of `user_says` occurs in his turn (matched
  as §11.2's triggers), and the model names the item. Once §12 lands, the
  aide may also promote on his checked instruction (`scribe memory promote
  <m-id> --session <id> --instruction '<his words>'`, §12.2's checks).

The pass may write a milestone directly, without a `now` item, only when
rule (a) or (c) holds on that turn ("we got married last June" in passing
is a `now` item for 7 days unless it changes his profile). Each field
overrides its shipped default alone; `user_says` replaces the whole list;
`scribe memory rules` prints the effective rules with their origin
(`shipped`, `config` with file and line), like `tag show` (§11.2).

### 13.5 Consolidation: the weekly consistency routine

§7's scan gains memory steps, after the entry steps and before the session
steps, within the same routine, deadline and model:

1. **Pairs, model-free.** Within one section (and one project for
   `projects`), active items whose statements overlap by at least one
   content token in four; at most 40 pairs per scan, newest first; a pair
   judged before is never offered again.
2. **Judgement.** The `consistency` model labels each pair `same`,
   `contradicts` or `unrelated`, with a reason and one verbatim quote from
   each side's occurrences. A label survives only if both quotes pass the
   verbatim check against the items' stored quotes.
3. **Merge** (`same`): the newer item's occurrences move onto the older
   item, newest first, keeping each occurrence's author; the newer item
   becomes `merged` with `merged_into`. The older item keeps its id, so
   cites and links hold.
4. **Supersede** (`contradicts`): the newer item supersedes the older,
   which becomes `superseded` with `superseded_by`; the model may not
   reverse the order (§7's rule). "Lives in Boston" (2025) is superseded by
   "moved to Seattle" (2026).
5. **Promotion catch-up**: rule (b) recounted after merges, and any
   promotion journalled on a milestone but not completed is completed.
6. **Decay** needs no step: it is derived (§13.2). An item whose latest
   occurrence is older than `decay_days` = **90** leaves the page and stays
   in the store; a new occurrence brings it back. Decay applies to
   `projects` and `working` only. **`profile`** facts are stable and rarely
   restated, so they leave the page only when superseded; `now` follows
   §13.4; `milestones` never leave.

Unlike §7's entry steps, the memory steps change items directly rather
than proposing: memory is descriptive, scribe-authored and needs no
approval, and every change is reversible by reading the occurrences (a
merge moves them, a supersede keeps both items). Each change is one audit
line (target `memory:<id>`) and an entry in the scan report's `memory`
block. The routine stays seeded disabled; until Daniel enables it, the
pass's `same_as` and duplicate guard are the only consolidation.

### 13.6 Reading back

**The session-start page.** `scribe hook session-start` adds the page as
the hook's `additionalContext` (Claude Code's `SessionStart`
`hookSpecificOutput`), so the **agent** reads it; the lines for Daniel stay
in `systemMessage` as today. It is built read-only, with no lock and no
write, from the store and the day. It is injected **only into an
interactive session**: the hook reads Claude Code's entry point
(`CLAUDE_CODE_ENTRYPOINT=cli`, the interactive value §12.5 relies on;
`sdk-cli` for every `claude -p` run) and adds no page otherwise, so
headless Orbit runs, routines and drains carry none of it. Codex's
equivalent is mapped by the implementation task, or Codex sessions get
the page through the skill (`scribe memory show --page`) until it is.
The page does not depend on this session being on: reading needs no
switch; only writing does.

A section switched off (§13.1) has no part on the page. The example below
is a store with `personal` on (the shipped default, §13.1); with `personal` off the `now` and
`milestones` lines hold only work state and work milestones.

Layout and budget (`page_chars` = **2,400** characters, about 600 tokens,
for every part but `milestones`, whose lines come on top), sections in this
order, each line one item and its id:

```text
scribe memory — what you know about Daniel. Descriptive, not rules: rules are his preferences. Raise milestones only when the conversation goes there.
Now:
- 2026-12-12 sister's wedding (in 21 days; not yet raised) [m-14]
- release of orbit under way, said 2026-11-21 [m-15]
Milestones:
- 2026-06-14 married [m-3]
- 2026-10-02 pulsar decommissioned [m-9]
Profile: runs agents from a Mac and two Linux boxes [m-1]; …
Project scribe (as of 2026-11-21): being widened to a whole memory [m-11]
Working: files the design as a task before any build [m-6]; …
Recent sessions: <id> "scribe memory design" (2026-11-21); <id> "…" (2026-11-20); … (5)
More: scribe memory show · scribe session show <id> · scribe question ask
```

| Part | Budget | Selection |
|---|---|---|
| header and footer | 260 | fixed |
| `now` | 600 | on-page items, dated ones by date (soonest first), then undated by latest occurrence; at most 8 |
| `milestones` | no cap | every milestone, one dated line each (at most 100 characters), oldest first |
| `profile` | 400 | on-page items, most occurrences first |
| `projects` | 300 | the current project's items (the working directory's project key, as §5.4) |
| `working` | 440 | on-page items, most distinct sessions first |
| recent sessions | 400 | the last **n = 5** active sessions by last message, title and id, excluding this one |

(Parked 2026-11-21: [§13.13](#1313-people) would add a `people` part (300
characters, after `milestones`) and raise `page_chars` to **2,700**;
recurring dates of [§13.14](#1314-recurring-dates) would be lines of the
`now` part; the recent sessions part would prefer switched-on sessions
([§13.15](#1315-what-a-session-that-is-not-switched-on-leaves)). None is
part of (a): the page is the layout above.)

A part over its budget drops whole items from its end and says
`(+k more)`; unused budget passes to `working`, then `profile`.
Milestones are never cut: when their lines pass 600 characters the page is
still injected whole, and the hook logs `memory_page_over_budget` (a
warning, never a failure), which is the signal that a cap is needed (not
built until seen). Items are rendered from their statement only; quotes stay in the
store. An item marked `sensitive` (§13.8) shows its statement as stored and
no quote, like any other.

**`scribe memory show`** (read-only, no lock, no audit) renders the full
pages: every on-page item of every section (or `--section <s>`, `--project
<key>`), each with its statement, id, occurrence count, latest date and,
for `now`, its page window and mark; `--include-off-page` adds decayed,
past-window, merged and superseded items labelled with why;
`scribe memory show <m-id>` prints one item with every occurrence and its
quote; `--page` prints exactly what the hook would inject. `--json` gives
the list envelope `{items, total, truncated, warnings}`; field names are a
contract from the first release.

**`ask` and `record search`** treat items as records: `record search
--kind memory` and `ask` retrieval include them; evidence shows "scribe's
statement" followed by his quoted words with their session and date. The
attribution check of §6.4 accepts a claim attributed to Daniel only when
the cited span is in an occurrence's quote, never in a statement. An item
weighs 1.0 in `ask`, not the 1.5 of his own entries. Retired and forgotten
items are left out; off-page items are included, labelled.

### 13.7 From `working` to `preferences`

When a `working` item's occurrences span **three or more distinct
sessions** (the same count as rule (b), its own config key
`[memory] promote_working_sessions = 3`), the pass, under the lock after
the occurrence that crossed it, opens one candidate proposal under
`preferences`: the item's statement as the §11.1 statement, its latest
occurrence's quote and cite as the claim's quote, and `admitted:
{trigger: "memory:recurs", matched: "<m-id>", criteria: <the tag's>}`. It
counts against the 20-open cap (at the cap it is not opened, and is
retried at the next occurrence), passes the suppression set, and is
recorded on the item (`preference_proposal`), so it opens once. Daniel
approves or rejects it as any candidate (§5.3, or on instruction, §12);
approval writes his `preferences` entry with `stated_by` set, and the
`working` item stays as it is. A rejection is never asked again.

### 13.8 Sensitive content

Only what the user typed in a session he switched on (or named for
catalog) is read; nothing is read from tool output, pasted text or
assistant turns (§13.3). (The base pass of the parked
[§13.12](#1312-a-personal-base-and-a-work-layer-per-session) would also read
what he typed in an interactive session that is off and not private.)

**Never kept, in any section** (refused, `memory_sensitive`):

- credentials, keys, tokens, passwords, and any text the redactor changes
  (model-free: the redactor runs on statement and quote, and a change
  drops the element);
- financial details: account or card numbers, balances, salaries, debts,
  amounts owed (model-free: the redactor's account and card patterns, plus
  the `finance` word list in `scribe-core::memory`; the model is told the
  rule as well);
- health details beyond the event: diagnoses, conditions, medications,
  symptoms, test results, of the user or anyone else;
- third parties' identifying details: names of people other than public
  figures and colleagues named for work, contact details, addresses, ages.
  A statement names a person by relation ("his sister"), not by name. The
  quote stays verbatim: it is his own words, already in the session record
  he switched on, and it is never shown on the page. (The parked
  [§13.13](#1313-people) would let a `people` record keep the name the user
  calls a person or pet by, as he typed it, and nothing else identifying.)

**Kept, only when `[memory] personal` is on (the shipped default, §13.1; ORB-26219 had it off), in `now` only, with the short lifetime**: an event in his
or his family's life that he stated, as the event and at the level he said it
("said his grandmother died on 2026-11-20", "said he has surgery on
2026-12-03"), never its medical or financial detail. Such an item carries
`sensitive: true` and the kind (`loss`, `health`, `relationship`,
`family`), set by the model and forced by a model-free word list (died,
passed away, funeral, hospital, surgery, diagnosed, divorce, miscarriage,
pregnant, and the like): when the list matches, the element is sensitive
whatever the model said. A sensitive item

- is accepted only in `now` or `milestones` (in `profile`, `projects` or
  `working` it is refused, so health and family events never become
  standing facts);
- stays on the page for the §13.4 window (until its date and 14 days
  after, or 7 days after its latest mention when undated), then leaves the
  page and stays in the store, in `scribe memory show
  --include-off-page`, `ask` and its session, until removed;
- is promoted to a milestone only by rule (b) or (c), never by (a): no
  `profile` fact is derived from it;
- is shown in `scribe memory show` like any item, so he can see exactly
  what is kept;
- is removed with `#scribe forget <m-id>`, or by the aide on his word
  (§13.9), which blanks its statement and every quote.

How the pass enforces it, in order, after the verbatim check and before
the write: the section switch (model-free), the redactor (model-free), the
word lists (model-free; `finance` refuses, `sensitive` forces the flag, and
with `personal` off a flagged element is refused), the section rule for sensitive items
(model-free), the mood rule of §13.4 (model-free), and the model's
instruction to drop health detail and third parties' details (model
judgement, the only gate that reads meaning; [§13.11](#1311-limits-and-what-is-not-built-until-seen)).
Refusals are counted in the pass report and audited by code only, never
with the text.

### 13.9 Correction

What exists, extended to memory ids (and, if the parked
[§13.13](#1313-people) is built, to `people` records' `p-` ids):

- `#scribe retire <m-id>` hides an item from the page, `ask` and lists
  (`retired`); `#scribe forget <m-id>` blanks its statement and every
  occurrence's quote for good (`forgotten`; id, section, dates and authors
  stay), and `#scribe forget <m-id>#<n>` one occurrence. The settling
  verbs of §7 accept `m-` ids; a repeated instruction echoes "nothing
  changed".
- Items are scribe-authored, so an agent may retire or forget one without
  his instruction check, as it may retire agent-authored entries (§1.1):
  `scribe memory retire <m-id> --author <name> --reason <text>`, `scribe
  memory forget <m-id>[#<n>] --author <name> --confirm` (STD-01@2 §R5).
  The skill tells the aide to run them when he says an item is wrong or
  asks for it to be forgotten, and to echo the result in one line.
- A wrong statement with right quotes is corrected by retiring it; the
  next occurrence opens a fresh item. There is no edit verb.

Every change is one audit line with the id and the code, never the words,
so a forgotten item's words leave the store.

### 13.10 Order and sequencing

After [§11.7](#117-order-and-sequencing) step (b), whose statement and
`stated_by` this reuses:

1. **(a)** The `memory` record kind, pass extraction with its checks and
   the sensitive gate, `scribe memory show` and `ack`, retire and forget,
   and the session-start page with the last-n sessions (one store format
   step, numbered when it lands: the `memory/` directory and the `memory`
   kind; an older build refuses a store holding one). Items carry no
   `layer`, `person` or `recurs` field, and (a) ships none of `[memory]
   base`, `base_idle_minutes`, `[memory.sections] people` or
   `[memory.recurring]`: only the parked parts read them (§13.12–§13.14),
   so each field and setting comes, with its own store format step where it
   is persisted, with the part that needs it. `[memory] personal` ships
   true (§13.1).
2. **(b)** Consolidation: the consistency scan's merge and supersede, the
   `now` to `milestones` promotion rules and `scribe memory rules`, and
   the derived decay and page windows (decay ships in (a) as derived; (b)
   adds the routine steps).
3. **(c)** A recurring `working` item proposed as a `preferences`
   candidate.

Each lands with its `specs/store.md`, `specs/output-contract.md`, skill and
golden updates. Daniel, 2026-11-21, of the consolidated list: "park 2-5,
keep 1, 6, 7". (a), (b) and (c) are kept. [ORB-26240]'s steps (a1)–(a4)
(§13.16) are parked with §13.12–§13.15 and are not scheduled.

### 13.11 Limits and what is not built until seen

- **Sections, events and sensitivity are the model's judgement.** The
  model-free gates catch credentials, listed finance and loss words, a mood
  word not in the quote, and a sensitive item outside `now` or
  `milestones`; a health detail phrased without a listed word, or a third
  party named in a statement, is caught only by the model, and by Daniel
  reading `scribe memory show`.
- **The mood rule is a word list.** "Sounded tired" passes if "tired" is
  not listed.
- **The page is in every interactive session's context**, about 600
  tokens, whether or not the session needs it.
- **Acknowledgement depends on the agent** running `memory ack`; an agent
  that does not will raise an item again in the next session.
- **Promotion (a) trusts the model's profile statement**; a wrong one
  supersedes a right profile fact until he retires it.
- **Only Claude Code and Codex sessions, and only those switched on.** A
  user whose conversations are elsewhere (Daniel's example of a user in
  ChatGPT) has no memory, by his decision of 2026-11-21 ("claude code and
  codex support only then. so be it",
  [4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)),
  and a session that is off writes nothing while the base is parked.

Not built until seen:

- importing the old `about-me` (four months stale), or Claude's
  auto-memory notes (the better seed, if Daniel later wants one);
- export to Markdown files, or reading the store from the box;
- an `interests` section, or sections defined in config;
- `#scribe <section>:` lines that write an item by hand;
- a cap on the milestones list, or on an item's occurrences;
- an edit verb for an item's statement;
- a page for headless runs, or per-agent page budgets;
- a model-level duplicate check in the pass beyond `same_as` and the
  lexical guard;
- always-on capture with an off-the-record switch (designed and parked,
  §13.12).

### 13.12 A personal base and a work layer, per session

> **Parked 2026-11-21 ([ORB-26256]).** Daniel, chief's chat, 2026-11-21: "park 2-5, keep 1, 6, 7".
> Reason: Claude Code and Codex only ([4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)), so there is no surface whose sessions the base pass and `/scribe private` would serve beyond what `/scribe on` already covers.
> Nothing below is built or scheduled; the text is kept for when he unparks it.
> While it is parked, memory items are written only from sessions switched on with `/scribe on`, and `[memory] personal = true` only admits personal elements from those (§13.1). The settings `[memory] base` and `base_idle_minutes` and the `layer` field are not shipped with (a), and the issue hint of §11.4 stays in every interactive session.

[ORB-26240], designed 2026-11-21, **not built**. Daniel, chief's chat,
2026-11-21, after approving `work` and `personal` presets ("Yes, add
presets"): "just to be clear, let's say a user inits with personal preset.
the user is still able to do \"/scribe on\" for work right? I think that is
the right separation. work related sessions should be selectively chosen.
also, I am fine with using personal as the base - I don't ever share
anything personal anyways so there is nothing there to fill". So the
separation is **per session, not per store**: one person has both kinds of
conversation, and `work` and `personal` are not two kinds of user.

| | Personal base | Work layer |
|---|---|---|
| Fed by | every **interactive** session a human types in, on or off, unless he types `/scribe private` in it | only a session switched on with `/scribe on` (or named with `/scribe catalog`, §5.8) |
| Writes | memory items in `profile`, `now`, `milestones` and `people` with `layer: personal`, recurring dates on them (§13.14), and, for a session that is off, one light catalog entry (§13.15) | what an on session writes today (the record with its segment log, §5; claim candidates under `preferences`, `standard`, `idea` and the capture rules of §11.2; issues, §11.4) and memory items in `projects` and `working`, and work facts in `profile`, `now` and `milestones` with `layer: work` |
| Approvals | none; no tag entries and no candidates | as today: candidates wait for his approval |
| Writer | the **base pass** (below), same `session` model | the session pass of §5.3 and §13.3 |

**Every store, the same default.** The base is on in every store, Daniel's
included; for him its sections simply stay empty, as he expects. Nothing
makes a session a work session by default: he chooses each one with
`/scribe on`, as today.

**Which sessions count as interactive.** The entry point recorded on the
session's typed turns in the transcript (the field §12.5 reads): Claude
Code's `cli`, and the desktop app's and IDE extensions' values, which the
implementation task maps from real transcripts on the Mac before it ships
(Daniel's roughly 60 desktop sessions a week are the main source). Every
other value, `sdk-cli` (every `claude -p`: Orbit runs, routines, drains,
workers) and a missing entry point, is not interactive and **feeds neither
layer**, as today. Codex's equivalent is mapped by the same task; until it
is, a Codex session feeds the work layer only when switched on.

**The base pass.** For an interactive session that is off and not private,
the `Stop` hook registers it once in `control.json` (`base: {since,
cursor: null}`), a write only when the state changes, no model work, no
detached process (§5.2's rule). The `scribe-session-pass` routine and
`scribe session pass` then treat a registered session as **base-due** when
its transcript has typed turns after `cursor` and its last line is older
than `[memory] base_idle_minutes` = **30**, or when `session-end` marked it
final. One `session` model call per window, under the bounds of §13.3 (3
memory items per pass invocation, the same verbatim check, dates,
`same_as`, duplicate guard and sensitive gate), with three differences:

- the model is asked only for **personal-layer** elements in `profile`,
  `now`, `milestones` and `people`, and an element it marks `work`, or in
  `projects` or `working`, is refused (`memory_work_layer_off`), so work
  talk in a session he did not switch on leaves no work memory;
- it opens **no candidates**, writes **no tag entries** and **no issue**,
  and never reads a `#scribe` line as a claim (the `#scribe` hook keeps
  working in every session as today, since it is his explicit write, not
  the pass's: the one place the base is not "no tags");
- it writes no session record; for a session that is off it writes or
  updates the session's light catalog entry (§13.15) in the same call.

The cursor advances under the lock with the items. When the session is
later switched on, the ordinary pass reads it whole from its first message
(§5.3), so the base pass's items are not lost and a replayed message adds
no second occurrence (§13.3). After `/scribe off` the base pass resumes
from the on pass's cursor, personal layer only.

**`/scribe private`.** A new control word (§5.1's parser, the same
transcript check). It sets `private: true` on the session in `control.json`:
no base pass reads it, it leaves no light entry, and, if it was on, it is
switched off as `/scribe off` would. Items already written stay until
forgotten (§13.9). `/scribe on` clears it. `/scribe status` gains one line:
`base: reading | private | off in config | not interactive`.

**The issue hint and the page.** Issues belong to the work layer, so the
`UserPromptSubmit` issue hint of §11.4 is added only in a session that is
on. The session-start page (§13.6) is still read in every interactive
session and shows both layers: `SessionStart` fires before `/scribe on` can
be typed, and the split governs what a session writes, not what its agent
may read.

**Settings, one by one.** Each difference is a single key; there is no
`if personal` branch and no preset:

```toml
[memory]
personal = true             # was false in ORB-26219; false: no personal-layer item from any session (on sessions keep work items only)
base = true                 # false: off sessions are not read at all (no base pass, no light entries)
base_idle_minutes = 30      # a registered off session is base-due this long after its last line

[memory.sections]
people = true               # §13.13
```

**`preset` is not needed.** The word was to name a set of values chosen
once per store (`work` or `personal`). Under the per-session split every
store ships the same default, and each remaining difference is one setting
above or in §13.3, so no named set has a member to choose between.
`scribe init --preset` is not built; a preset is not built until a user
needs several settings changed together and asks for it.

### 13.13 `people`

> **Parked 2026-11-21 ([ORB-26256]).** Daniel, chief's chat, 2026-11-21: "park 2-5, keep 1, 6, 7".
> Reason: Claude Code and Codex only ([4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)), so there is no surface whose sessions a `people` record would serve beyond what `/scribe on` already covers.
> Nothing below is built or scheduled; the text is kept for when he unparks it.
> Slice (a) adds no `person` field and no `[memory.sections] people`; they come with this part.

Daniel, chief's chat, 2026-11-21: "it may be appropriate to build a brief
profiles of those close to the user, e.g. husband's profession and
birthday, if they are brought up. and pets as well." A sixth section, in
the personal base. **Need observed:** Daniel's example for a regular user;
not yet observed in use.

**The record.** A person header of its own kind, because a person is the
subject of several facts and dates, while an item is one statement. File
`people/<id>.md`, id `p-<n>` allocated like entry ids, frontmatter only:

```markdown
---
format: <n>
kind: person
id: p-2
name: Sam                          # what the user calls them, as typed; null until said
relation: husband                  # from the relation list, or null ("relation not said")
person_kind: person                # person | pet
status: active                     # active | retired | forgotten
made_by: {rule: stated_relation, session: <id>, message: <uuid>}   # or second_mention
dates:                             # at most 4
  - {n: 1, label: birthday, month_day: 03-14, recurs: yearly, acknowledged: null,
     occurrence: {date: 2026-11-21, session: <id>, message: <uuid>, quote: "Sam's birthday is march 14"}}
author: {kind: scribe, name: session-pass}
created: 2026-11-21T05:40:00Z
updated: 2026-11-21T05:40:00Z
---
```

Each **fact** is a memory item of §13.2 with `section: people` and
`person: p-2` ("Works as a nurse.", "Lives in Seattle."), with occurrences
and the user's verbatim quotes like every other item; `same_as`, the
duplicate guard, merge and supersede work on facts within one person.

**When a record is made**, model-free on the model's element (`{section:
people, person_name?, relation?, person_kind?, statement?, date?, quote,
message}`), never on a name alone:

1. **A stated relation**: the quote holds a possessive (`my`, `our`) before
   a word of the relation list in `scribe-core::memory` (husband, wife,
   partner, son, daughter, child, mother, mom, father, dad, sister,
   brother, grandmother, grandfather, aunt, uncle, cousin, friend, and for
   pets dog, cat, puppy, kitten, pet, and the like). Work relations
   (colleague, coworker, manager, boss, report, client) make no record:
   they are not "close to the user" and belong to the work layer.
2. **A second mention in another session**: a name the model returns
   without a relation is not stored; its normalized form is hashed with a
   per-store random salt (`people/pending.json`: `{hash, sessions, first,
   last}`, SHA-256, already a dependency) and kept 90 days. A mention in a
   second distinct session makes the record with `relation: null` and the
   second mention's quote. So a name said once never becomes a profile,
   and the ledger holds no name in the clear.

A later element whose name or relation matches an active record (same
normalized name, or same relation for a relation that names one person:
husband, wife, partner, mother, father) adds to that record, never a
second one; a relation that names several (son, sister, friend, dog) needs
the name to match.

**Brief, bounded.** At most **5 active facts** per person, each statement
at most **120 characters**, and at most **4 dates**. A sixth fact
supersedes the fact whose latest occurrence is oldest (it stays in the
store, `superseded`, as §13.5's supersede does); a fifth date is refused
(`memory_person_full`). The number of people is not capped (§13.16).

**Third parties.** A record holds facts about someone who did not opt in,
so:

- **only what the user said**: every fact and date carries his verbatim
  quote from his typed turn; nothing is read from assistant turns, tool
  output or pasted text (§13.3), and nothing is inferred;
- **kept brief**: relation, profession (never the employer's name), the
  city they live in (never an address), what they like or do, and dates;
- **a third party's health is not kept.** No condition, diagnosis,
  medication, pregnancy, disability or mental-health fact is ever written
  on a `people` record, and neither are their finances, religious or
  political beliefs, sexuality, immigration or legal matters, contact
  details, address, age or birth year (`memory_person_sensitive`: the
  `sensitive` and `finance` word lists refuse in `people` without a model,
  then the model's instruction). When the user tells of such an event as
  his own news ("my husband has surgery on december 3"), the only trace is
  his own `now` item under §13.8, at the level he said it, with the short
  window, never promoted and never a `people` fact;
- **shown**: `scribe memory show --section people` lists every record with
  its facts and dates, and `scribe memory show p-<n>` one record with every
  quote, so the user sees exactly what is kept;
- **removable**: `#scribe forget p-<n>` blanks the name, every fact's
  statement and quote and every date, keeping id, dates of writing and
  authors (as §13.9); `#scribe forget p-<n>#d<k>` one date; facts by their
  `m-` ids. A forgotten record's salted name and relation enter the
  suppression set, so the same person is not made again from later
  mentions. `#scribe retire p-<n>` hides it. Agents use `scribe memory
  retire|forget p-<n>` as §13.9.

**On the page**, after `milestones`, 300 characters, one line per active
person ordered by latest occurrence, most facts first within it: `People:
Sam (husband): works as a nurse; lives in Seattle [p-2]; Biscuit (dog) [p-4]`.
The page budget rises to **2,700**.

### 13.14 Recurring dates

> **Parked 2026-11-21 ([ORB-26256]).** Daniel, chief's chat, 2026-11-21: "park 2-5, keep 1, 6, 7".
> Reason: Claude Code and Codex only ([4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)), so there is no surface whose sessions a recurring-dates reminder would serve beyond what `/scribe on` already covers.
> Nothing below is built or scheduled; the text is kept for when he unparks it.
> Slice (a) adds no `recurs` field and no `[memory.recurring]`; they come with this part, and `SessionStart` adds no derived `now` lines.

Daniel, chief's chat, 2026-11-21: "Also, I forgot about recurring dates,
e.g. birthdays of the user and family members, wedding anniversary date,
etc. it would be nice if agent says happy birthday, or remind the user on
upcoming family birthdays." **Need observed:** Daniel's example for a
regular user; not yet observed in use.

**A field, not a section.** `recurs: yearly` on a date, in three places,
all in the personal base:

| Where | Date | Example | Whose |
|---|---|---|---|
| a `profile` item with `event_date` | the user's own birthday | "Said his birthday is 1990-05-02." | own |
| a `people` record's `dates` | a birthday, a pet's adoption day | Sam's birthday, 03-14 | others' |
| a `milestones` item | the anniversary of the event | "2026-06-14 — married." yields the wedding anniversary | the user's, planned for |

Who sets it: the model on a stated birthday or anniversary, checked
model-free (a real month and day; February 29 falls on February 28 in other
years; §13.3's one-year-before rule does not apply to a recurring
date, whose year may be long past). A milestone recurs **with nothing more stored** when its statement
contains a word of `[memory.recurring] anniversary_words` (shipped
`["married", "wedding"]`). A `sensitive` item (§13.8) never recurs, so no
loss is turned into a yearly reminder. The user's own year may be kept on
his `profile`; a third party's never is (§13.13), so a `people` date is a
month and day.

**Surfacing is computed, not remembered.** `scribe hook session-start`,
read-only, with no lock, no write and no model call, runs a pure function
in `scribe-core` over the dates and the day (the host's local date) and
adds a derived line to the page's `now` part for each date inside its
window:

```toml
[memory.recurring]              # every key optional, shown with its default
others_days_ahead = 14          # people dates: from 14 days before through the day
own_days_ahead = 0              # the user's own birthday: on the day only
anniversary_days_ahead = 14     # recurring milestones
anniversary_words = ["married", "wedding"]
```

```text
Now:
- 2027-03-14 Sam's birthday (husband; in 12 days; not yet raised this year) [p-2#d1]
- today: Daniel's birthday (not yet raised this year) [m-21]
```

Derived lines count in the `now` part's budget and its 8 lines, by date
like dated items. Nothing is written into `now`: a past date simply falls
out of the window, so no routine has to run.

**`acknowledged` applies per year.** The mark on a recurring date is
`{year, at, session, author}`, set by `scribe memory ack <m-id>|p-<n>#d<k>
--session <id> --author <name>` (the year is the occurrence's year, derived
by the CLI) or by the user raising it in a turn; a mark from an earlier
year does not count, so each year's birthday is "not yet raised" once
again. The skill tells the agent: on the day, say happy birthday; ahead of
someone else's date, remind the user once so he can plan; then run `ack`.

**No push.** A reminder is said only when the user next opens an
interactive session. Notifications (an Orbit notice, a mail, a phone) are
another surface and are not built until seen.

### 13.15 What a session that is not switched on leaves

> **Parked 2026-11-21 ([ORB-26256]).** Daniel, chief's chat, 2026-11-21: "park 2-5, keep 1, 6, 7".
> Reason: Claude Code and Codex only ([4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)), so there is no surface whose sessions a light catalog entry would serve beyond what `/scribe on` already covers.
> Nothing below is built or scheduled; the text is kept for when he unparks it.
> A session that is off leaves nothing; `depth: light` is not added to session headers, and the page's recent-sessions list is the plain list of §13.6.

**Decided by Daniel**, chief's chat, 2026-11-21, to "Should a session you
do not switch on still leave a light catalog entry (title and summary)?":
"Yes, light entry". A session a human typed in that is not switched on
(and not private, §13.12) leaves **its personal-base memory items and a
light catalog entry**: a title, a date and a short summary, so every
conversation is recallable, which is what a regular user's catalog of
past chat sessions needs. The full record with its segment log, tags and
claims, issues, `projects` and `working` stay with sessions switched on
with `/scribe on`.

**The entry.** A session record header with `depth: light` (absent means
`full`, so every existing record reads unchanged), written by the base
pass in the same call (§13.12): `title` (at most 80 characters), `summary`
(at most 300), `created` and `last_message`, `host`, `project`; no log
segments, no Goals or Next, no links. The summary is written from the
user's typed turns only and passes the redactor and the gates of §13.8:
a summary the redactor changes, or that the `finance` or `sensitive` word
list flags, is not stored (the entry keeps title and date), and a title
so flagged becomes `(untitled)`. A later base pass on the same session
rewrites the header (it is the rewritable part of §5.5).

- **Switching on later** turns the entry into a full record: `depth`
  becomes `full` and the first ordinary pass, reading from the first
  message (§5.3), replaces title and summary. `/scribe off` never turns
  a full record back into a light one.
- **Reading**: `session list` shows both, with a `depth` field (an added
  `--json` field; the envelope is unchanged) and `--depth full|light`;
  `session show` on a light entry prints its header and says it was not
  switched on; `record search --kind session` and `ask` include light
  entries as session summaries, labelled light, at the weight of a
  session record (never Daniel's words).
- **Correction**: a light entry is deactivated like any session (§5.7);
  `#scribe forget session:<id>` is not built (§12's limit), so a private
  conversation is kept out by `/scribe private`, before the pass reads it.

**A behaviour, not a setting.** There is no `[sessions] light_catalog`
key: the light entry is what the base pass does for a session that is off,
and it is stopped only with the base itself (`[memory] base = false`, no
off session is read) or for one session (`/scribe private`). The reason:
Daniel decided it for a regular user's catalog; a switch that keeps memory
items but no entry (or the reverse) answers no need seen, and every
setting is one more state the pass and its tests must cover. Since he
decided (a), option (b), nothing but memory items with a
`[sessions] on_by_default` switch for a regular user, is not built.

**The recent-sessions list prefers switched-on sessions** (this design's
call, the chief's proposal). For Daniel, about 60 light entries a week
would push his chosen sessions off the page's 5 slots. So the list takes
first the switched-on sessions whose last message is within
`[memory] recent_prefer_on_days` = **14** days, newest first, then fills
the remaining slots with the newest sessions of either depth; each line
says `(light)` when it is one. A regular user who never switches a session
on sees his last 5 light entries.

### 13.16 Limits of the addendum and what is not built until seen

> **Parked 2026-11-21 ([ORB-26256]) with §13.12–§13.15**, whose limits and order this section records. The "(a), amended" step below is **not in force**: (a) adds no `layer`, `person` or `recurs`, and `personal` ships true for the reason of §13.1. (a1)–(a4) are not scheduled. Text kept for when he unparks them.

- **The base reads by default.** Every interactive session is read unless
  the user types `/scribe private`; a conversation he forgot to mark is
  read, and its items stay until he forgets them.
- **Cost of reading off sessions.** About 60 desktop sessions a week on
  Daniel's Mac become base passes, each at least one `session` model call
  (bounded windows, the 30-minute idle rule), to keep a personal base he
  expects to be empty, plus 60 light entries.
- **The layer is the model's judgement** within an on session (`personal`
  or `work` per element); in an off session the gate is model-free (any
  `work` element is refused), but a work fact the model labels personal
  passes.
- **Interactive entry points must be mapped** for the desktop app, IDE
  extensions and Codex before the base reads them; an unmapped entry point
  feeds nothing.
- **People are matched by name and relation words.** Two people with the
  same first name are one record until the user says otherwise (retire and
  let the next mention open a new one); a relation said without a
  possessive ("Sam, the husband") does not make a record until the second
  mention.
- **The third-party health rule is a word list plus the model**, like
  §13.11's gates.
- **Recurring dates depend on the host's date and on the agent's `ack`**;
  an agent that does not ack repeats the greeting in each session of the
  day.

Not built until seen:

- presets (`scribe init --preset`), or any named set of settings;
- push notifications of a reminder;
- recurring dates other than yearly, reminders the user sets for a date,
  and anniversaries of sensitive events (a loss);
- a cap on the number of `people` records, people pages beyond one line,
  links between people (a relation of a relation), and importing contacts
  or a calendar;
- un-suppressing a forgotten person, and an edit verb for a person's name
  or relation;
- forgetting a light entry's words, and a setting for light entries;
- a base for non-interactive or remote sessions, and a work layer chosen
  by project or host rather than by `/scribe on`.

**Order** (against [§13.10](#1310-order-and-sequencing)):

- **(a), amended**: memory items carry `layer`, `person` and `recurs` as
  nullable fields in (a)'s store format step, and `personal` ships true.
- **(a1)** after (a): the base pass, `[memory] base` and
  `base_idle_minutes`, the interactive entry-point map, `/scribe private`
  and its status line, the issue hint gated on on sessions.
- **(a2)** after (a1): light catalog entries (`depth: light`, one store
  format step) and the recent-sessions preference.
- **(a3)** after (a1): the `people` record kind (one store format step),
  its two making rules and pending ledger, bounds, third-party gate, page
  part, `memory show`, retire, forget and suppression.
- **(a4)** after (a3): recurring dates, `[memory.recurring]`, the derived
  `now` lines and the per-year `ack`. It needs only (a)'s dated items and
  (a3)'s dates, not (b).

(b) and (c) are unchanged and independent of (a1)–(a4).

## 14. `#scribe` as an annotation (proposed)

[ORB-26510], designed 2026-11-21 and **not built**. Nothing in this section
is implemented until Daniel approves it and its tasks land, in the order of
[§14.9](#149-order-and-sequencing). Where it changes a rule stated earlier
([§4.1](#41-detection), [§4.2](#42-parsing-an-instruction),
[§4.3](#43-resolving-this-and-it), [§4.4](#44-the-reply),
[§11.1](#111-a-claim-carries-a-statement)), the earlier text describes what
is built today and carries a pointer here. The reasons, the alternatives and
each difference from his sketch are in
[4_decisions.md](./4_decisions.md#scribe-is-an-annotation-cataloged-at-the-end-of-the-turn).

Daniel, chief's chat, 2026-11-21: "current scribe's inline command only
work as a standalone comment, i.e. it doesn't work when mixed into other
sentences. … when I am writing something, I should be able to annotate
scribe messages along side it. And the scribe agent will read it, and
surrounding subsequent and previous messages if required, and catalog it
accordingly. having to write a separate scribe message is redundant. … no
more verbatim captures on "#scribe <tag>: <msg>". all entries should have
short context/summary and verbatim messages for citation. if I want to
capture verbatim notes, then I could do so manually."

**Observed need** (the Mac store and Daniel's typed Claude Code turns,
2026-11-02 to 2026-11-21):

- **Missed annotations.** Twice on 2026-11-21 he put `#scribe` inside a
  sentence, the hook saw nothing, and he typed the line again on its own:
  "so here is what I am thinking.  #scribe what follows is an idea" (05:17Z)
  became `idea-1` six minutes later, and "approve both - #scribe remind:
  let's circle back later whether issue writes …" (07:48Z) became
  `remind-2` a minute later.
- **Verbatim content that says nothing.** `standard-2` is "#scribe standard:
  add below"; its content is "add below", and the standard itself is only
  in the action window.
- **What he uses.** He typed 8 entries: 7 in form 1 and 1 approval. Forms 2
  and 3 have no entry, so the referent model and its guessed fallback
  (§4.3) have never written one.
- **Talk about scribe.** Of 4 unquoted mid-sentence `#scribe` tokens in his
  typed turns, 2 are the annotations above and 2 are talk about scribe
  ("let's keep #scribe writing", "support global #scribe entries"). Another
  5 sit in double quotes, such as his own "#scribe <tag>: <msg>" above. A
  mention cannot be told from a marker by syntax alone (§14.1).

### 14.1 The marker and the mention

**Detection, model-free** (`capture::marks`, beside `capture::lines`).
In a turn the §4.1 origin guard accepts as plainly his, a **marker** is the
token `#scribe` or `#scribe/global` that starts the turn or follows
whitespace or an opening bracket, and is followed by the end of the line,
whitespace or `:`. It may sit anywhere in a sentence. These never hold a
marker, because they are text about scribe or text he did not write:

- code fences, indented code and inline code spans (`` `#scribe` ``);
- blockquotes and `<pasted_content>` blocks (as §4.1 today);
- a span in double quotes on the same line (`"…"` or `“…”`).

A turn with a marker is a **marked turn**. Its markers are numbered from 1
in text order. The idempotency key of each is `(session, prompt_sha256,
marker)`, as `(session, prompt_sha256, line)` is today.

**The hint.** Words right after the marker may name a tag or alias
(`#scribe idea`, `#scribe pref:`): that word is the **hint**, matched
model-free as §4.2 matches a tag word, and the model may not choose another
tag. `#scribe/global` sets `project: global` as today, and any other
`#scribe/…` word is refused `unparsed_instruction`. Hashtags after the
marker on its line are topics, as today. Words that name no tag are not
grammar: they are part of his message, and the model reads them.

**Settling stays a typed command.** A line that is exactly `#scribe
approve|reject|retire|forget <id>` (§4.2 form 4) is matched first and
settled at submit, model-free, as today. A settling verb after a marker
inside a sentence ("approve both - #scribe approve p-3") settles nothing:
the hook refuses it with `unparsed_instruction` and echoes that settling
needs its own line. Settling changes records, so it must be his exact
command, never a model's reading of a sentence.

**A mention is not a marker.** Syntax removes quoted and coded mentions.
An unquoted mention ("let's keep #scribe writing") still reaches the
model, which may answer `none`: nothing is written and the echo says so
(§14.4). A mention costs one model call and an echo line. That is two in
19 days, against a missed annotation that he has to type again.

### 14.2 When it is cataloged: at the end of the turn

The **prompt hook writes nothing** for a marker. It echoes that the turn is
marked and tells the main agent that scribe records it, so the agent does
not try to (§14.4). It still settles form 4 lines and refuses a bad
`#scribe/…` word.

The **`Stop` hook of the same turn** catalogs it. Stop already reads the
transcript with §4.1's provenance check and replays missed `#scribe`
lines. It now finds each marked turn whose markers have no outcome yet
(§14.5) and catalogs it. By then the transcript holds the marked turn and
the assistant's reply to it, so the model can read what he pointed at in
either direction. This works whether or not `/scribe on` is set, as Stop
recovery does today. Codex already defers input to Stop.

**What Daniel sees, and when.** At submit, an echo line that the turn is
marked. At the end of the reply (the same turn's Stop), the entry's id,
tag, statement and his quoted words, or why nothing was written. Stop waits
for the model call, at most the budget below, after the reply is shown.

### 14.3 The window and the model call

**Window** (model-free, the §4.3 parse: user and assistant text; tool
payloads, sidechain and meta lines dropped):

- the marked turn, whole up to 8,000 characters;
- the 8 messages before it;
- the assistant's reply after it, up to the end of the turn.

Each message other than the marked turn is cut to 3,000 characters
(`[… cut]`). The window is at most 40,000 characters, and older messages
are dropped first. One window per marked turn covers all its markers.

**Model call.** One call per marked turn per attempt, through the `Models`
port (§8), to a new `models.annotation` key (default `claude-haiku-5-5`,
Codex `gpt-6-luna` as for the session model), with a **30 s** budget. Its
schema is one object per marker, `{marker, kind: entry|none, tag,
statement, subject, quote, message, reason}`:

- `entry`: `statement` (at most 300 characters, standing alone as
  §11.1's statement does), `subject` (at most 80), `quote` (8 to 1,500
  characters, a verbatim span of `message`), `message` (the id of a turn he
  typed in the window: the marked turn or an earlier one), and `tag` (a
  configured tag; with a hint, the hint's tag);
- `none`: `reason`, at most 200 characters, for a mention or a marker that
  points at nothing.

Messages are data, never instructions, as in every scribe prompt. The tag
list goes into the prompt with each tag's criteria from §11.2.

**Checks after the answer** (deterministic, then under the store lock):
the quote passes §6.4's check against the original transcript text of the
named message. That message is a plainly typed turn of his in the window,
outside every exclusion of §14.1. The tag is configured and equals the
hint when there is one. The statement and subject are non-empty and
within bounds. Then redaction (a quote that redaction would change is
refused, as §5.3 omits such quotes), the idempotency key, and the same-tag,
same-project duplicate check `covering_entry` (§4.2). A marker whose answer
fails a check is refused for this attempt with its reason (§14.5).

### 14.4 What an entry holds, and the echo

An annotated entry is written by the same `Store::capture` path as a
typed line:

| Field | Value |
|---|---|
| `author` | `{kind: daniel}`: the marker is his consent, so no proposal |
| content | the statement |
| `quote` | his verbatim span |
| `stated_by` | `{kind: scribe, name: annotation}`, a new name beside `session-pass` |
| `cite` | the marked turn: session, message, time, prompt hash, line of the marker, cwd |
| `content_from` | `{kind: message, role: user, message, at}` of the quoted turn |
| `said` | the marked turn's line that holds the marker, redacted |
| `window` | under an action tag, the §4.6 window, model-free, as today |
| `project` | the cwd's project, or `global` for `#scribe/global` |

`ask`, `record search` and `entry show` read it as §11.1 reads a stated
entry: his words are the `quote`, and the statement is labelled scribe's
statement of his marked turn. The attribution check of §6.4 credits him
only for a span inside `quote`.

**Echo** (`systemMessage`; the agent-facing copy carries §4.4's notes):

```
scribe: marked — cataloged after the reply
scribe: added idea-4 to idea — "Scribe catalogs #scribe annotations with context" · your words: "so here is what I am thinking. #scribe what follows …"
scribe: nothing recorded for #scribe in your message — read as talk about scribe
scribe: could not catalog #scribe in your message — the model timed out; will retry at the next reply (2 of 3)
```

### 14.5 Outcomes, retries and bounds

Each marker's outcome is kept in a new store document, `marks.json`
(`{session, message, prompt_sha256, marker, outcome, attempts, at}`, ids
and codes only, never words). `outcome` is `entry` with its id, `none`,
`refused` with its code, or `pending`. Stop catalogs only markers with no
outcome or `pending`, so a `none` or a refusal is never sent to the model
again.

- **Bounds (STD-08 §R1, §R3).** One model call per marked turn per Stop,
  30 s each. At most 3 attempts per marked turn, across Stops; after the
  third failure the outcome is `refused` (`model_failed`) and the echo
  says to retype it. At most 3 markers per turn: a fourth is refused
  `too_many_marks` before any call. At most 2 marked turns per Stop,
  oldest first; the rest stay `pending` for the next Stop. The window is
  at most 40,000 characters in, and the answer at most 3 × about 2,100
  value characters.
- **When the model fails** (timeout, error, a shape the schema refuses):
  nothing is written, `pending` with `attempts + 1`, and an echo. When the
  model **finds nothing** (`none`): nothing is written, `none`, and an echo
  with its reason. When an answer **fails a check**: nothing is written,
  `refused` with the check's code, and an echo. Never a guessed entry:
  §4.3's fallback goes.
- **Audit**: one line per entry written, per refusal and per `none`, as
  every hook write is (§8); none for a `pending` retry that changed
  nothing but the attempt count.

### 14.6 What happens to forms 1, 2 and 3

They fold into the marker. A line that starts with `#scribe` is a marked
turn like any other: `#scribe idea: …` is a marker with the hint `idea`,
and `#scribe add this to preferences` is a marker with the hint
`preferences` whose words the model reads. Nothing is recorded verbatim as
content any more. Daniel's exact words are always the entry's `quote`.

- The §4.2 grammar for forms 1 to 3 and the `unparsed_instruction` refusal
  for a capture line are removed. Form 4 stays (§14.1).
- §4.3's referent call at submit, its 12-message window and its guessed
  fallback are removed, and so is `models.referent`. A config that sets it
  still loads for the 30-day compatibility window, with a warning that it
  is unused.
- Stop recovery of missed forms 1 to 3 becomes the cataloging of §14.2.
- **Existing entries are unchanged.** The 8 entries he typed keep their
  verbatim content and `stated_by: null`. No model rewrites them, and they
  read as today. He can mark one again to get a stated entry.

### 14.7 The session pass

A marked turn cannot supply a candidate or a pass-written idea: §5.3
already excludes "marked `#scribe` lines", and that exclusion widens from
the line to the whole marked turn. An unmarked turn is captured as today.
The pass never reads `marks.json`.

### 14.8 Store format

One step: `marks.json` is a new document (no older form), and an entry's
`stated_by` admits `{kind: scribe, name: annotation}` with a user
`content_from` and a non-empty `quote`, beside `session-pass`. The number
is the next free one when the step lands. `specs/store.md` and
`specs/output-contract.md` describe both with the step.

### 14.9 Order and sequencing

1. **Annotations at Stop.** §14.1's marker detection and mention rules,
   the prompt hook's echo, §14.2 to §14.5 (`models.annotation`, the
   window, the checks, `marks.json`, retries and echoes) and the store
   step. Forms 1 to 3 keep working at submit until step 2, and a marker
   inside a sentence is cataloged at Stop.
2. **Fold forms 1 to 3 into the marker.** §14.6: the grammar and the
   referent call go, a line-start form is a marked turn with a hint, and
   `models.referent` is deprecated. The skill, help, goldens and
   `specs/output-contract.md` follow.
3. **The pass skips marked turns** (§14.7).

### 14.10 Limits and what is not built until seen

- **A mention costs a call.** An unquoted "#scribe" in talk about scribe
  reaches the model, which may still write an entry he did not mean. He
  retires it, and the echo shows it at once.
- **The statement is the model's.** As §11.8 says of claims, a wrong
  subject or a weak statement is caught only when he reads the echo. The
  quote keeps his exact words beside it.
- **Stop waits.** Up to 30 s after the reply before the turn ends, once
  per marked turn.

Not built until seen:

- reading his **next** turn ("subsequent messages") for a marker: the entry
  is written at the end of the marked turn, and in the two missed
  annotations what he pointed at was in the same turn;
- quoting an **assistant** message as the entry's words (§4.3's form 3
  allowed it; no entry was ever written that way);
- a **verbatim-only** form, until Daniel answers
  [§14.11](#1411-questions-for-daniel);
- several markers with different scopes in one turn beyond "what each
  marker annotates", as he has never typed two;
- rewriting old entries into stated ones.

### 14.11 Questions for Daniel

1. **"Manually" for a verbatim note.** Proposed: nothing new. Every entry
   keeps his exact words in `quote`, and `entry show` prints them.
   Alternatives: keep one explicit verbatim form, or give him a CLI command.
2. **Several markers in one turn.** Proposed: each marker is its own entry,
   and the model records what that marker annotates (its sentence, or what
   it points at: "what follows", "below", "this"). The other choice is one
   entry per marked turn, whatever the number of markers.
3. **When the echo arrives.** Proposed: at the end of the same turn's reply,
   so the model can read the reply. The other choice is to catalog at
   submit, which echoes before the reply but cannot read it and makes the
   prompt wait for the model.
4. **Existing entries.** Proposed: left as they are, verbatim (8 entries).
   The other choice is a one-off pass that adds a statement to each, with
   the original words kept in `quote`.
5. **A mention.** Proposed: the model reads it and writes nothing, and the
   echo says so. The other choice is to treat every unquoted token as a
   marker and leave it to him to retire what it writes.

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-13117] — built the Rust CLI template this project was created from.
- [ORB-13134] — brought the template to STD-01..03 v2 and STD-04/05 v1.
- [ORB-21778] — S3: referents (form 3), the `Models` port and `ClaudeCliModels`.
- [ORB-21784] — S4: `/scribe on|off|status`, session-pass cadence and session records.
- [ORB-21760] — the operator's live checks of each slice.
- [ORB-21787] — S5: the Orbit plugin tree and `ask`.
- [ORB-21795] — S6: the consistency routine, the settling verbs, the
  `SessionStart` hook and the two routines.
- [ORB-23849] — session links (`spawned_from`, `continues`), `session list --tree` and the linked-session pass context.
- [ORB-23892] — designed S8–S14: the header-and-log session record, the
  `active` flag, the CLI agent surface and skill, the tmux popup and
  backfill by name.
- [ORB-23995] — session rebuild replay fixtures: the replay harness, the
  synthetic recording and the redaction tool.
- [ORB-24030] — `/scribe catalog <id>` as Daniel's only catalog channel; agent
  author names as attribution only.
- [ORB-26214] — designed §11: claim statements, per-tag capture rules, the
  tag set without `remind` and `finding`, the retired-tag cleanup and issues.
- [ORB-25987] — the pass captures Daniel's ideas; revised by §11.6.
- [ORB-26265] — built §11.6 (step b2): the pass writes Daniel's ideas
  itself, with their outcome (format 16).
- [ORB-26204] — designed §12: an agent Daniel instructs settles claims and deletes records
  through the CLI; sequenced after §11 step (a).
- [ORB-26219] — designed §13: the `memory` record kind with its five sections, the
  pass that writes it, consolidation, the session-start page and correction.
- [ORB-26240] — designed §13.12–§13.16: the personal base and the work layer
  per session, `people`, recurring dates and light catalog entries,
  replacing the presets.
- [ORB-26256] — recorded Daniel's decisions of 2026-11-21: scribe reads
  Claude Code and Codex sessions only, and §13.12–§13.15 (the base pass,
  `people`, recurring dates, light catalog entries) are parked.
- [ORB-26510] — designed §14: `#scribe` as an annotation anywhere in a
  typed turn, cataloged at Stop with a statement and his verbatim quote;
  forms 1–3 fold into it.
- [ORB-26349] — extended the duplicate check's negation markers, and made
  Stop recovery decide coverage as of the turn.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.

## Codex integration and portable skill

The provider-neutral source is `skills/scribe/SKILL.md`, embedded by `scribe init`
and deterministically copied into both provider packages. Init takes an explicit
provider and project target, reconciles digest-owned artifacts, preserves
unrelated hook groups and refuses edits/symlinks. It never trusts hooks or enables
cataloging. The native Codex marketplace installs `plugin/codex/` independently
of project scaffolding; its launcher handles GUI PATH omissions.

Codex has separate lifecycle/schema handling in `commands/hook/codex.rs` and
pure transcript parsing in `scribe-core::codex`. A supported main user rollout
must carry per-content host user provenance and turn ids. Hooks alone are not
identity. Exact typed skill controls normalize only after that evidence is read;
ordinary prompt text is preserved. Before persistence, input is deferred to Stop.
Current-turn bootstrap and durable id-only cursors avoid historical settlement
replay. Unsupported metadata returns a nonblocking refusal. Descriptor-rooted
no-follow reads validate the opened file before parsing, bounded at 64 MiB.
`archived_sessions` remains discoverable for post-archive final passes.

Provider-tagged controls (store format 7) route shared passes and rebuilds.
Codex defaults to Luna (`gpt-6-luna`) for session, referent, ask and consistency
work. `[codex.models]` overrides those choices; `[models]` preserves Claude/global
routing unless explicitly changed. `--provider codex` selects Codex for global
CLI ask/consistency. `models/codex.rs` reuses the existing supervisor, disables
hook/plugin/tool capabilities and external instructions, uses strict structured
output with optional fields normalized on the wire, validates the result, refuses
tool events and has no fallback. Provider credentials stay in the provider's
existing store. Model temp artifacts/logs/sqlite state live in private scratch
and are reclaimed by RAII.

The default Orbit plugin optionally accepts `codex_home`; it grants read access to
active/archive transcripts and existing auth, exposes no new agent-callable
write tool, and preserves existing routine schedules and Claude-only config.
Luna-backed Orbit execution uses `plugin/orbit-codex/.orbit-plugin`, generated
from the default package. It additionally requires `codex_runtime_tmp` and
`codex_installation_id`: one directory write and one exact-file write for the
provider's startup state. Auth/config/transcripts receive no write grant. The
same identity, tools, jobs and disabled routine defaults are preserved. Explicit
settings and consent are required; no provider-specific logic enters Orbit.
See README's Codex section for current installation/trust prerequisites and the
explicit same-OS-user accident-guard limitation.
