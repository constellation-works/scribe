---
title: scribe — Overview
owner: claude
last_updated: 2026-11-10
last_validated: 2026-11-02
status: Draft
stage: building
next: S9 segment-writing passes and session rebuild, then S10–S14 (S8 store format 4 built); S7 discussions unfiled
feature: scribe
doc_role: overview
type: design
summary: A standalone Orbit plugin that keeps Daniel's explicitly marked preferences and decisions, catalogs the sessions he turns on or names (each a header over an append-only log), answers agents' questions about him with verbatim citations, and lets agents search and write through its CLI under one skill.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-21404, ORB-23849, ORB-23892]
---

# scribe — Overview

scribe is a small, standalone Orbit plugin. It records only what Daniel
explicitly asks it to record, catalogs only the sessions he turns on, and
lets any agent ask what Daniel said or decided, answering with verbatim
citations. Agents on the Mac also search and write scribe through its CLI,
taught by one skill; Daniel browses it in a tmux popup. It replaces the scribe built inside orbit-office, without the
Library, to-dos or calendar around it.

> The design ([2_design.md](./2_design.md),
> [4_decisions.md](./4_decisions.md)) describes the plugin. Delivery state
> lives only in the [slice plan](./2_design.md#10-slice-plan); this overview
> does not restate it.

## 1. Directions (Daniel, 2026-11-02)

Quoted verbatim; these are the requirements this design starts from.

- "I don't think auto-backfilling is a good idea. like I said, too noisy. a
  better method is manually specifying which conversation I would like for
  the scribe to catalog. and when there is an explicit tag mention during
  conversations, e.g. #preferences or #decision."
- "I think two should stay separate. \"/scribe on\" catalogs the chat
  session. for the other, we should use \"#scribe add this to decision\" or
  \"#scribe add it to my preference\", and it doesn't catalog the whole
  session and just adds new entry to whatever tag. so, preferences,
  decisions, other tags, are only explicitly added on the instruction of the
  user."
- "I think scribe should be a standalone small orbit plugin. and \"asking\"
  probably better with a decision model; and a routine that checks for
  inconsistencies, outdated record removal, and small model like haiku for
  \"scribe on\"."
- "let's keep #scribe writing. I would rather keep mcp surface small for my
  main agents - they can just have \"ask\" mcp."
- "I just want to make sure that orchestrator agents can ask the scribe on
  anything, e.g. \"can you fetch me last session summary? (works as a
  session handoff)\"."
- "for chat session summaries, we are probably better off using a good
  metadata system in the frontmatter, e.g. tags to filter by" and "same for
  decisions and preferences I suppose".
- "I am trying to graduate to level 4 (1:1:many:many resident orchestrator
  and manager setup), which means the \"agent\" that I will be primarily
  talking to won't be orchestrators anymore. it will be manager, whom I will
  discuss designs, ideas, etc, just like we are doing right now. So, by
  scribing chat sessions, when I mention \"graduate to level 4\", a scribe
  can pull up the session summary on that terminology."
- "I like this kind of setup: knowledgebase/almanac/15-discussions. I used
  to keep selective discussions where we discussed something meaningful.
  and I can reference it later in chats. I want scribe to fill that gap."

### Directions of 2026-11-10 ([ORB-23892])

- "limiting each session to 2500 is insufficient for long sessions - e.g.
  after writing first version of the session using all 2500 characters,
  the next update must remove a part of the original. So over multiple
  re-writes, the beginning session's context will get lost, defeating the
  purpose."
- "we should add \"isActive\" field to sessions so that scribe or based on
  configurable duration limit can periodically deactivate stale sessions",
  and "A feature that no longer exists (if it exists in session store)
  should be deactivated."
- "I don't want to keep having to manually search for specific session
  id's to hand to agents; and I am not sure about adding another set of
  MCP tools. so I think a single scribe skill that teaches terminal
  commands will suffice."
- "we need a very simple desktop UI or tmux UI for humans. perhaps tmux
  will be better - having another desktop app will be cumbersome."
- "we need to backfill some of important chat sessions on orbit and scribe
  projects. not everything but certain long ones", then "we don't need to
  go all the way back to aug. those will just add noise. the primary goal
  is to understand the context of existing features, to provide continuity
  ... month of oct will be a good starting point."

## 2. Shape

| Part | Does | Model |
|---|---|---|
| `#scribe <instruction>` | In a user-typed turn, in any session whether on or off, adds exactly one entry to the named tag. It quotes Daniel verbatim, cites the message, and echoes the tag, text and id. An unknown tag is refused with the configured list. A bare `#decision` captures nothing. | None, unless the instruction says "this"/"it": then Haiku picks the referenced message |
| `/scribe on` | Catalogs that session into one session record (summary, goals, outcome, next), refreshed on cadence and at session end. Writes no tag entries. No backfill of other sessions. `/scribe on --from <id>` or `/scribe link <id>` links it to the session it was spawned from or continues ([§5.4](./2_design.md#54-session-links)); its pass reads that session's summary as context. | Haiku |
| `ask` | The one MCP tool exposed to main agents. Retrieval is model-free (lexical ranking over entries, session records and Orbit tasks); a decision model answers only from the retrieved evidence, every quote is verbatim-checked, it says "no record" rather than infer, and reports conflicts instead of picking a side. | Decision model (Sonnet by default) |
| Consistency routine | Flags contradictions and stale entries and proposes "B supersedes A" or "retire A". Daniel approves with `#scribe approve <id>`. A superseded entry stays on disk, is marked, and is hidden from answers by default. From S10 it also deactivates sessions whose feature a later record says was removed, and sessions past a generous age limit unless pinned. | Haiku, on model-free candidate pairs |
| CLI agent surface (S11–S13) | Agents on the Mac search (`scribe record search`, BM25), filter lists by date, status, topic and task, and add or update entries, tags and proposals with `--author`; one `scribe` skill teaches it. Agent writes are marked `author.kind: agent`, never quoted as Daniel's words, and never settle an entry Daniel wrote. | None |
| tmux popup (S13) | `scribe-tmux` lists sessions, entries, proposals and backfill candidates in fzf with `show` as the preview. | None |
| Backfill by name (S14) | `scribe transcript list --since 2026-10-01` lists long uncataloged sessions; Daniel picks and runs `scribe session catalog <id>`. | Haiku (the session pass) |

Daniel's own entries and settlements come only from his typed `#scribe`
turns. Agents see only `ask` over MCP; from S12 they also write through
the CLI, each write recording its author. A session record becomes a
rewritable header over an append-only log of segments (S8, S9), and a
session can be inactive: hidden, not removed (S8, S10).

## 3. Core concepts

- **Tag** — a named list Daniel writes to, such as `decisions` or
  `preferences`, declared in the config with its aliases.
- **Entry** — one record in a tag: Daniel's instruction verbatim, the
  content it records (verbatim from his instruction or from the message it
  points at), and a citation of the message.
- **Session record** — the one catalog record of a session Daniel turned on
  or named: a header (title, summary, goals, next) over a log of segments.
- **Segment** — one sealed or open stretch of a session record's log, cited
  `session:<id>#<n>`.
- **Author** — who wrote an entry or proposal: Daniel (`#scribe`), an agent
  (the CLI) or scribe's routine.
- **Proposal** — a consistency finding ("B supersedes A", "A is stale")
  waiting for Daniel's approval.
- **Evidence** — what `ask` retrieved and handed to the decision model; an
  answer may only quote evidence.

Full definitions: [references/glossary.md](./references/glossary.md).

## 4. Questions settled by this design

Each was open when the repository was created; each is now a decision Daniel
can override.

| Question | Decision |
|---|---|
| Whether superseded entries are ever removed, or only hidden (the directions say "outdated record removal") | [Superseded and stale entries are hidden, not removed](./4_decisions.md#superseded-and-stale-entries-are-hidden-not-removed) |
| Storage layout and the search index for `ask` | [One Markdown file per record, metadata in YAML frontmatter](./4_decisions.md#one-markdown-file-per-record-metadata-in-yaml-frontmatter), [`ask` ranks in memory with no persisted index](./4_decisions.md#ask-ranks-in-memory-with-no-persisted-index) |
| The migration from the orbit-office scribe (its v1 and v2 records) | [No migration of orbit-office scribe records](./4_decisions.md#no-migration-of-orbit-office-scribe-records) |
| Which tags ship by default beyond `preferences` and `decisions` | [Only decisions and preferences ship by default](./4_decisions.md#only-decisions-and-preferences-ship-by-default) |

## 5. At a glance

| Concern | Where it will live | Slice |
|---------|--------------------|-------|
| Entries, session records, proposals; the store | `crates/scribe-core/src/{entry,session,consistency}.rs`, `store/` | S1 |
| `#scribe` parsing and capture | `crates/scribe-core/src/capture.rs`, `crates/scribe/src/commands/hook/` | S2, S3 |
| `/scribe on` and session passes | `crates/scribe-core/src/{session,transcript}.rs`, `commands/hook/`, `commands/session.rs` | S4 |
| Session links (`spawned_from`, `continues`) | `crates/scribe-core/src/{session,control}.rs`, `store/`, `commands/hook/scribe_command.rs`, `commands/session/` | ORB-23849 |
| `ask` | `crates/scribe-core/src/ask/`, `commands/question.rs` | S5 |
| Consistency routine | `crates/scribe-core/src/consistency.rs`, `commands/consistency.rs` | S6 |
| Orbit plugin tree | `plugin/.orbit-plugin/` | S5 |
| Claude Code hooks package | `plugin/claude-code/` | S2 |
| Store format 4: session header and log, `active`, authors (the tags store is S12) | `crates/scribe-core/src/{session,record,activity}.rs`, `store/` | S8 |
| Segment-writing passes, `session rebuild` | `crates/scribe-core/src/pass.rs`, `commands/session/rebuild.rs` | S9 |
| Deactivation routes | `crates/scribe-core/src/{activity,consistency}.rs`, `commands/session/activate.rs` | S10 |
| `record search`, list filters, id prefixes | `crates/scribe-core/src/search.rs`, `commands/record.rs` | S11 |
| Agent write verbs and the author guard | `commands/{entry,tag,proposal}.rs` | S12 |
| The `scribe` skill, the tmux popup | `plugin/claude-code/skills/scribe/`, `plugin/tmux/` | S13 |
| `session catalog`, `transcript list/show` | `commands/session/catalog.rs`, `commands/transcript.rs` | S14 |

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-21744] — the orbit-office scribe redesign this plugin supersedes (archived).
- [ORB-21404] — the on-call task the scribe redesign was spawned from.
- [ORB-23849] — session links between session records.
- [ORB-23892] — designed S8–S14: session header and log, the `active`
  flag, the CLI agent surface and skill, the tmux popup, backfill by name.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
