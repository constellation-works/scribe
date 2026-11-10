---
title: scribe — Design
owner: claude
last_updated: 2026-11-03
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: design
type: design
summary: How the scribe plugin is bounded (one binary; hooks write, agents only ask), how #scribe entries, /scribe on session records, ask and the consistency routine work, where everything is stored, which model does what, and the slice plan that builds it.
tags: [scribe, plugin, architecture]
paths: ["crates/**", "plugin/**", "scripts/**", "Makefile"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21778, ORB-21784, ORB-21787, ORB-21795, ORB-13117, ORB-13134, ORB-23849]
---

# scribe — Design

The design of the scribe plugin: its boundary, the four parts named in
[1_overview.md §2](./1_overview.md#2-shape), the store, the model choices,
and the slices that build it. Slices S1 (the domain, the store and the
read-only CLI), S2 (`#scribe` capture without a model, and the Claude Code
package), S3 (referents and the models port), S4 (`/scribe on` and
session records), S5 (the Orbit plugin tree and `ask`) and S6 (the
consistency routine and the settling verbs) are implemented; S7
(discussions) is not yet. The reasons behind each choice
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
| `scribe hook <event>` | Claude Code hooks (the `plugin/claude-code/` package) | Reads the hook's JSON on stdin; captures `#scribe` instructions, switches cataloging on and off, marks session passes due. The only path that writes entries. |
| `scribe plugin call` | Orbit's exec backend (`plugin/.orbit-plugin/`) | Reads the Orbit tool request on stdin, writes the `{"ok", …}` envelope on stdout. Serves `scribe.ask` and the routines' tools. |
| `scribe <noun> <verb>` | Daniel at a terminal, scripts | Read-only `entry list/show`, `session list/show`, `proposal list`, `tag list`, `question ask`, plus `session pass` (Daniel runs it by hand) and `consistency scan` (its routine's job). |

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
`systemMessage` for Daniel when any proposal is open, and nothing
otherwise. It reads only; it does not create `config.toml`
([§3](#3-storage)).

Entry writes never go through Orbit's tool surface: the hooks call the
binary. The two routine tools write session records and proposals, but they
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

## 3. Storage

The root is `--root` > `SCRIBE_ROOT` > `~/.scribe`, as in the template. The
hooks, the CLI and the Orbit backend resolve the same root, so one store
serves every workspace on the host. The plugin's config key `root` (default
`~/.scribe`) is passed to the backend as `--root`.

```
~/.scribe/                       0700, owner-checked on every load
├── config.toml                  tags, models, cadence (Daniel edits; read-only to scribe)
├── entries/<tag>/<id>.md        one file per entry: YAML frontmatter + verbatim body
├── sessions/<yyyy>/<mm>/<session-id>.md   one per opted-in session: frontmatter + sections
├── discussions/<yy-mm>/<slug>.md   one per distilled discussion (§4.5), Daniel may edit
├── control.json                 {"format":3,"sessions":{"<id>":{"on","since","due","final","consecutive_failures","retry_after","pending_links"}}}
├── proposals.json               {"format":3,"proposals":[…]}
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

**Metadata.** Entries and session records are Markdown files whose YAML
frontmatter is the metadata `ask` and the CLI filter on, so a lookup narrows
by fields before any ranking. Every record carries:

| Field | Meaning |
|---|---|
| `format`, `kind` | `3`; `entry` or `session` |
| `id` | `decisions-12`, or the session id |
| `status` | entry: `active`, `superseded`, `retired`, `forgotten`; session: `open`, `final` |
| `project` | the repository slug from the cwd's git remote, else the cwd's basename; `global` when Daniel says so |
| `topics` | 0–5 lowercase kebab-case labels (`ci`, `crew-routing`, `release`) |
| `related` | references the record mentions: `task:ORB-123`, `pr:<owner>/<repo>#7`, `repo:<slug>`, `session:<id>` (model-extracted for a session) |
| `links` | sessions only: `[{type, session}]`, `type` `spawned_from` or `continues`; the session structure Daniel asserted ([§5.4](#54-session-links)), stored on the child only |
| `terms` | sessions only: phrases Daniel coins or defines, each `{term, said, message}` where `said` is a verbatim span of Daniel's own turn that defines or first uses it |
| `provider`, `host` | where it was recorded |
| `created`, `updated` | RFC 3339 UTC |

`topics` are filter labels and are distinct from a tag, which names where an
entry goes (`decisions`, `preferences`). An entry's topics are the extra
hashtags on its `#scribe` line (`#scribe add this to decision #ci`) and
nothing else, since capture calls no model. A session's topics come from
the `session` model at each pass, which is given the existing topic list and
told to reuse it before coining one. The consistency routine proposes
merging near-duplicate topics (`ci` and `continuous-integration`) as it does
superseding entries; nothing is renamed without Daniel's approval.

**Entry** (`entries/<tag>/<id>.md`):

```markdown
---
format: 3
kind: entry
id: decisions-12
tag: decisions
status: active
project: orbit
topics: [merge-policy]
related: [task:ORB-123]
provider: claude-code
host: dk-server-1
created: 2026-11-02T19:05:41Z
updated: 2026-11-02T19:05:41Z
said: "#scribe add this to decision #merge-policy"
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
  keeps the id and `cite` but blanks `said` and `content`
  ([4_decisions.md](./4_decisions.md#superseded-and-stale-entries-are-hidden-not-removed)).
- `review_after` comes from the tag's `stale_after_days` (none by default
  for `decisions`, 180 days for `preferences`).
- Ids are `<tag>-<n>`, `n` being the tag's max + 1 under the lock: short,
  self-describing in an echo, and never reused.

**Session record** (`sessions/<yyyy>/<mm>/<id>.md`): the common
frontmatter plus `links` (sessions only; `related` says what a session
mentions, `links` the structure Daniel asserted, [§5.4](#54-session-links)),
`title`, `cwd`, `started`, `on_since`, `scribed_through`
(the last transcript message folded in), `passes` (count) and `next_open`
(the number of `next` items); `status: final` after the session-end pass.
The body has four sections, `## Summary`, `## Goals`, `## Outcome` and
`## Next` (character budgets 1500/600/2500/1000). The four text fields are model-written
summaries, labelled as such in `ask` evidence, never quoted as Daniel's words.

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

**Proposal** (`proposals.json`): `id` (`p-<n>`), `kind` (`supersedes` or
`stale`), `older`, `newer` (entry ids), `reason` (model text, at most 300
characters), `quotes` (verbatim spans from both entries), `state`
(`open`, `approved`, `rejected`), `opened_at`, `settled_at`, `settled_by`
(a `cite` of Daniel's approving turn).

**Config** (`config.toml`): `[tags.<name>]` with `aliases`, `description`
and `stale_after_days`; `[sources]` with `discussions_readonly`, a list of
directories `ask` reads as extra discussion evidence and never writes
(default: the constellation almanac's `15-discussions`, when present); `[models]` with `ask`, `distill`, `session`, `consistency`,
`referent` and `credentials` (the variables model calls are given, default
`CLAUDE_CODE_OAUTH_TOKEN` and `ANTHROPIC_API_KEY`); `[catalog]` with
`every_minutes` (20), `every_turns` (10), `window_messages` (40) and
`window_chars` (15000), all positive integers; `[redact]` with `env`, further
variables whose values are masked. A missing file or table is the defaults;
an unknown key or bad value is refused, naming the file and line
([specs/store.md](./specs/store.md#config)).
The file is created by `scribe hook session-start` on first run with the
defaults ([4_decisions.md](./4_decisions.md#only-decisions-and-preferences-ship-by-default))
if absent; scribe never rewrites it after that. As built (S6), it is not:
`session-start` reads only, and a missing file is the defaults
([4_decisions.md](./4_decisions.md#session-start-reads-only)).

**Redaction.** Before any write of `said`, `content`, a session record's
title, text fields and terms, or a proposal's `reason` and quotes, the store masks
the live values of the variables in `[redact] env` and `[models] credentials`
and high-confidence credential shapes (`Authorization: Bearer …`,
`ghp_`/`github_pat_`, `sk-ant-`/`sk-`, `AKIA…`, `xox[bp]-`, PEM
private-key blocks); a `[redact] env` name with no value in the writer's
environment refuses the write instead of storing text unmasked
(STD-05 R13, R14;
[4_decisions.md](./4_decisions.md#persisted-free-text-passes-one-redactor)).

## 4. `#scribe`

### 4.1 Detection

`scribe hook user-prompt-submit` receives `session_id`, `transcript_path`,
`cwd` and `prompt`. `capture::instructions` finds every line of `prompt`
that starts with `#scribe ` (after optional leading whitespace), skipping
lines inside code fences, blockquotes and `<pasted_content>` blocks (the
orbit-office `markers.rs` rule). Each such line is one instruction; a turn
with two lines writes two entries. A bare `#decision`, `#preferences` or any
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
before Claude Code has written the transcript. A failed check records
nothing; each `#scribe` line gets a `could not record` reply and an audit
line with `error_code: provenance_failed`. The `Stop` hook first checks,
reading only, whether any entry of the session still has `message: null`;
when none does (the usual case) it returns without opening the transcript,
locking or writing. Otherwise it parses the transcript
([§2](#2-layering), `transcript`: user and assistant text only, tool
payloads, sidechain and meta lines dropped), hashes each user message's whole
text and each of its text blocks, and gives each waiting cite the uuid of the
matching message written closest to the cite's `at`; a cite whose prompt is
not in the transcript yet stays `null` for the next `Stop`.

### 4.2 Parsing an instruction

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
both declared in config (`decision`, `decisions`; `preference`,
`preferences`, `pref`). An instruction whose tag matches nothing is refused:
no entry, and the reply lists the configured tags. An instruction that
matches no form is refused with the forms. A refusal is still audited
(`status: failure`, `error_code: unknown_tag` or `unparsed_instruction`).

Exactly one entry per instruction, also under retries: the idempotency key
is `(session, prompt_sha256, line)`. A repeat finds the existing entry and
echoes it instead of writing again.

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
words, with or without a verb (`#scribe that to decisions`); `#scribe add
this rule to decisions` is form 2 with the content `this rule`.
The tag word of form 2 may end in `.`, `!`, `,` or `;`.

### 4.3 Resolving "this" and "it"

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
decisions-3 to decisions (guessed: <why>; recorded the assistant message
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
same text, and never blocks the turn, so the rest of the prompt still
reaches the agent:

```
scribe: added decisions-12 to decisions — "squash-merge agent PRs; keep merge commits for releases"
scribe: refused "#scribe add it to rituals" — no tag "rituals"; tags: decisions, preferences
```

The agent-facing text adds "recorded by scribe; no action needed", so the
main agent does not try to record it again.

As built (S2): the reply is one line of hook JSON,
`{"systemMessage": …, "hookSpecificOutput": {"hookEventName":
"UserPromptSubmit", "additionalContext": …}}`, with one echo line per
`#scribe` line in prompt order; a prompt without one prints nothing. A
replayed line echoes `scribe: already recorded <id> in <tag> — "…"`. When
the store or `config.toml` cannot be used, each line echoes `scribe: could
not record "…" — <error>` and the hook still exits 0, so the prompt goes
through. Quotes in an echo are flattened to one line and cut at 200
characters (`scribe entry show <id>` has the whole text). The `Stop` hook
prints nothing.

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

## 5. `/scribe on`

### 5.1 Switching on and off

A prompt whose first line is `/scribe on`, `/scribe off` or `/scribe status`
(or a link command, [§5.4](#54-session-links)) is intercepted by the `UserPromptSubmit` hook, which updates `control.json`
and returns `decision: "block"` with the status as `reason`, so no model
turn is spent. Every session starts off. `on` records `on_since` and marks a
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

As built (S4): the `Stop` hook reads `control.json` first and goes no
further for a session that is not on or already due. The last pass is the
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
   blocks as pasted.
2. Split messages after `scribed_through` into consecutive windows, oldest
   first: at most `[catalog] window_messages = 40` messages or
   `window_chars = 15000` Unicode characters of content, whichever binds
   first. An individually oversized message is cut with a marker inside
   the character limit. No older messages are discarded to fit later ones.
3. Send each window and the record so far to the `session` model (Haiku)
   for the whole updated `{title, summary, goals, outcome, next, topics,
   related, terms}` record. A session with links also sends the latest
   title and summary of each linked session that has a record, read once
   at the start of the pass, framed as data, with the instruction to
   summarize only this session's own work and not retell the linked one
   ([§5.4](#54-session-links)). The first window of a first pass has no prior
   record. Plain message headers retain role and id; pasted data and the
   record use plain delimiters too. Messages are data, never instructions.
   Offer stored topics and terms, including those from earlier windows,
   for reuse. Validate field limits and shape on every window, with one
   fresh retry naming the refusal. A term must name a user message in the
   current or an earlier window and quote its own text verbatim; unknown,
   future, assistant and pasted citations are dropped.
4. After every valid window, redact and atomically write the whole record
   under the store lock, advancing `scribed_through` and `passes` (successful
   record updates). Feed the persisted record into the next window. Only
   the last window clears `due` and applies final status. A failure keeps
   all prior windows, stays due, and starts bounded retry backoff; the next
   pass resumes at the failed window. One CLI invocation processes all
   windows; the routine may yield between them at its deadline.

A pass writes no tag entries, even if the transcript contains `#decision`
or an unprocessed `#scribe` line. There is no backfill: sessions that were
never switched on are never read.

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

## 6. `ask`

### 6.1 Contract

Input: `{"question": string, "scope"?: {"tags"?: [..], "topics"?: [..],
"project"?: string, "kind"?: "entry" | "session", "since"?: date},
"include_hidden"?: bool}`. Output:

```json
{
  "status": "answered | no_record | conflict",
  "answer": "…",
  "claims": [{"text": "…", "quote": "…", "cite": "decisions-12"}],
  "conflicts": [{"cites": ["preferences-3", "preferences-9"], "note": "…"}],
  "evidence_considered": 14,
  "dropped_quotes": 0
}
```

`cite` is an entry id, `session:<id>` or `task:<id>`. A lookup
([§6.5](#65-lookups-records-by-recency-or-id)) also sets `"status":
"record"` and returns `records`, the stored records themselves. `scribe question ask
"<question>"` is the same operation for Daniel at a terminal (`--json`
prints this payload).

### 6.2 Retrieval (model-free)

0. Filter by frontmatter first. `scope` fields (`project`, `topics`,
   `tags`, `since`, `kind`) and any project name, topic or date the
   question names ("decisions on ci this month") restrict the candidate
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
   fields for sessions, title + description for tasks; entries get a 1.5×
   weight because they are Daniel's explicit words.
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
| the entries of a tag in a period ("decisions this week") | those entries, newest first |
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
  covers "decisions this week"). Stemming strips `-ing`, `-ed`, `-es`, `-s`
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

## 7. Consistency routine

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
`scribe: 3 proposals to review — scribe proposal list`) and settles each in a
typed turn: `#scribe approve p-4` sets the older entry `superseded` (or
`retired` for a stale proposal) and records `superseded_by`; `#scribe reject
p-4` closes it for good. `#scribe retire <entry-id>` hides an entry directly;
`#scribe forget <entry-id>` blanks its text for good. Agents cannot settle
proposals: settling is a `#scribe` instruction, which only a user-typed turn
carries. The orbit-office contradiction detector
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

The settling verbs run in `scribe hook user-prompt-submit`, in prompt
order after the transcript check, one locked `Store::settle` each:
`approve` needs an open proposal whose older entry is still `active`
(refused `entry_not_active` otherwise), and writes `proposals.json` before
the entry, so a crash between them is what the next scan re-applies;
`reject` of an approved proposal or `approve` of a rejected one is refused
`proposal_settled`; `retire` takes an `active` entry; `forget` takes any
entry not yet forgotten and also blanks every proposal quote of it and the
reason of every proposal about it, so its words leave the store. A repeated
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
| Session pass | `models.session` | `claude-haiku-5-5` | `scribe.session-pass` job | 120 s |
| Consistency judgement | `models.consistency` | `claude-haiku-5-5` | `scribe.consistency-scan` job | 120 s per batch of 10 pairs |
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
changed nothing (no `#scribe` line, a replay, a `Stop` with nothing waiting)
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
- A store with a newer `format`, a group-writable root or a linked file is
  refused with the template's errors; golden tests cover every help page.
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
- Lines in fences, blockquotes and `<pasted_content>`, and bare `#decision`
  markers, write nothing; an unknown tag is refused with the configured
  list; replaying the same payload writes nothing new.
- Every write and refusal is audited with ids and error codes only.
- `plugin/claude-code/` installs locally and a manual check in a live
  Claude Code session confirms the reply is shown to Daniel and the agent;
  the result is recorded in this doc ([§9](#9-concerns--honest-limitations)).
- `scribe hook stop` fills the cite's message uuid from the transcript.

**S3 — Referents and the models port.** Form 3 and `ClaudeCliModels`.

- With a fake `Models`, `#scribe add this to decisions` records the chosen
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

**Outside this repository.** Retiring orbit-office's scribe (its hooks,
`office.scribe-*` tools and `/scribe` command) is orbit-office work, filed
there once S4 is in use, so two scribes never intercept the same
`/scribe` prompt.

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

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
