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
paths: ["crates/**"]
related_features: [scribe]
related_artifacts: [ORB-21744]
---

# scribe — Overview

scribe is a small, standalone Orbit plugin. It records only what Daniel
explicitly asks it to record, catalogs only the sessions he turns on, and
lets any agent ask what Daniel said or decided, answering with verbatim
citations. It replaces the scribe built inside orbit-office, without the
Library, to-dos or calendar around it.

> **State.** The repository was generated from the constellation Rust CLI
> template on 2026-11-02. The code and the other design docs
> ([2_design.md](./2_design.md), [3_vision.md](./3_vision.md),
> [4_decisions.md](./4_decisions.md)) still describe the template's
> note-keeping example. The first slice replaces them.

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
| `#scribe <instruction>` | In a user-typed turn, in any session whether on or off, adds exactly one entry to the named tag. It quotes Daniel verbatim, cites the message, and echoes the tag, text and id. An unknown tag is refused. | Small model, only to resolve "this"/"it" |
| `/scribe on` | Catalogs that session into one session record (summary, goals, outcome, next), updated on cadence and at session end. Writes no tag entries. | Haiku |
| `ask` | The one MCP tool exposed to main agents. Retrieval is model-free (search over entries, session records and Orbit tasks); a decision model answers only from the retrieved evidence, verbatim-checks every quote, says "no record" rather than infer, and reports conflicts instead of picking a side. | Decision model |
| Consistency routine | Flags contradictions and stale entries and proposes "B supersedes A". Daniel approves. A superseded entry stays on disk, is marked, and is hidden from answers by default. | Small model |

Writing stays with the provider hooks (`#scribe`, `/scribe on`); main
agents see only `ask`.

## 3. Open questions

- Whether superseded entries are ever removed, or only hidden. The directions
  say "outdated record removal"; the proposal above hides and keeps them.
- Storage layout and the search index for `ask`.
- The migration from the orbit-office scribe (its v1 and v2 records).
- Which tags ship by default beyond `preferences` and `decisions`.
