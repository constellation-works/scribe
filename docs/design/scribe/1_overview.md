---
title: scribe — Overview
owner: claude
last_updated: 2026-11-02
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: overview
type: design
summary: A standalone Orbit plugin that keeps Daniel's explicitly marked preferences and decisions, catalogs the sessions he turns on, and answers agents' questions about him with verbatim citations.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-21404]
---

# scribe — Overview

scribe is a small, standalone Orbit plugin. It records only what Daniel
explicitly asks it to record, catalogs only the sessions he turns on, and
lets any agent ask what Daniel said or decided, answering with verbatim
citations. It replaces the scribe built inside orbit-office, without the
Library, to-dos or calendar around it.

> **State.** The design ([2_design.md](./2_design.md),
> [4_decisions.md](./4_decisions.md)) describes the plugin; nothing of it is
> built yet. The code is still the Rust CLI template's note-keeping example,
> and [specs/](./specs/) still specifies that example's output and store
> until slice S1 of the [slice plan](./2_design.md#10-slice-plan) replaces
> both.

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

## 2. Shape

| Part | Does | Model |
|---|---|---|
| `#scribe <instruction>` | In a user-typed turn, in any session whether on or off, adds exactly one entry to the named tag. It quotes Daniel verbatim, cites the message, and echoes the tag, text and id. An unknown tag is refused with the configured list. A bare `#decision` captures nothing. | None, unless the instruction says "this"/"it": then Haiku picks the referenced message |
| `/scribe on` | Catalogs that session into one session record (summary, goals, outcome, next), refreshed on cadence and at session end. Writes no tag entries. No backfill of other sessions. | Haiku |
| `ask` | The one MCP tool exposed to main agents. Retrieval is model-free (lexical ranking over entries, session records and Orbit tasks); a decision model answers only from the retrieved evidence, every quote is verbatim-checked, it says "no record" rather than infer, and reports conflicts instead of picking a side. | Decision model (Sonnet by default) |
| Consistency routine | Flags contradictions and stale entries and proposes "B supersedes A" or "retire A". Daniel approves with `#scribe approve <id>`. A superseded entry stays on disk, is marked, and is hidden from answers by default. | Haiku, on model-free candidate pairs |

Writes stay with the provider hooks (`#scribe`, `/scribe on`) and the
plugin's own routines; main agents see only `ask`.

## 3. Core concepts

- **Tag** — a named list Daniel writes to, such as `decisions` or
  `preferences`, declared in the config with its aliases.
- **Entry** — one record in a tag: Daniel's instruction verbatim, the
  content it records (verbatim from his instruction or from the message it
  points at), and a citation of the message.
- **Session record** — the one catalog record of a session Daniel turned on.
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
| Storage layout and the search index for `ask` | [One JSON file per tag and per session](./4_decisions.md#one-json-file-per-tag-and-per-session-under-scribe), [`ask` ranks in memory with no persisted index](./4_decisions.md#ask-ranks-in-memory-with-no-persisted-index) |
| The migration from the orbit-office scribe (its v1 and v2 records) | [No migration of orbit-office scribe records](./4_decisions.md#no-migration-of-orbit-office-scribe-records) |
| Which tags ship by default beyond `preferences` and `decisions` | [Only decisions and preferences ship by default](./4_decisions.md#only-decisions-and-preferences-ship-by-default) |

## 5. At a glance

| Concern | Where it will live | Slice |
|---------|--------------------|-------|
| Entries, session records, proposals; the store | `crates/scribe-core/src/{entry,session,proposal}.rs`, `store/` | S1 |
| `#scribe` parsing and capture | `crates/scribe-core/src/capture.rs`, `crates/scribe/src/commands/hook/` | S2, S3 |
| `/scribe on` and session passes | `crates/scribe-core/src/catalog.rs`, `commands/hook/`, `commands/session.rs` | S4 |
| `ask` | `crates/scribe-core/src/ask/`, `commands/ask.rs` | S5 |
| Consistency routine | `crates/scribe-core/src/consistency.rs`, `commands/consistency.rs` | S6 |
| Orbit plugin tree | `plugin/.orbit-plugin/` | S5 |
| Claude Code hooks package | `plugin/claude-code/` | S2 |

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-21744] — the orbit-office scribe redesign this plugin supersedes (archived).
- [ORB-21404] — the on-call task the scribe redesign was spawned from.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
