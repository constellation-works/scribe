---
title: scribe — Overview
owner: claude
last_updated: 2026-11-23
last_validated: 2026-11-02
status: Draft
stage: building
next: 2_design.md §14 annotations built (steps 1–3, store format 22; Codex wired, ORB-26535); S7 discussions unfiled
feature: scribe
doc_role: overview
type: design
summary: A standalone Orbit plugin that keeps Daniel's explicitly marked preferences, catalogs the sessions he turns on or names (each a header over an append-only log), answers agents' questions about him with verbatim citations, and lets agents search and write through its CLI under one skill.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-21404, ORB-23849, ORB-23892, ORB-25977, ORB-25980, ORB-25983, ORB-25987, ORB-27638]
---

# scribe — Overview

scribe is a small, standalone Orbit plugin. It records only what Daniel
explicitly asks it to record, catalogs only the sessions he turns on, and
lets any agent find what Daniel said or decided, with verbatim
citations: agents on the Mac search and write scribe through its CLI,
taught by one skill; Daniel browses it in a tmux popup. It replaces the scribe built inside orbit-office, without the
Library, to-dos or calendar around it.

> The design ([2_design.md](./2_design.md),
> [4_decisions.md](./4_decisions.md)) describes the plugin. Delivery state
> lives only in the [slice plan](./2_design.md#10-slice-plan); this overview
> does not restate it.

**Principle (Daniel, 2026-11-20).** "I think scribe's best value is from
providing insights implicitly, rather than explicitly - seamlessly
cataloging things that would've been lost otherwise without any additional
effort. I just turn on scribe, and scribe extracts insights and turn them
into value". Turning a session on is the one act scribe asks of him. A step
that needs him to type, approve or remember has to earn its place. This
reverses "records only what Daniel explicitly asks it to record" for
everything except preferences; the paragraph above still describes what is
built. What changes and which tasks carry it:
[Directions of 2026-11-20](#directions-of-2026-11-20).

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

Amended 2026-11-20 ([ORB-25980]): `decisions` no longer ships as a tag; a
decision is recorded on its Orbit task or in a feature's `4_decisions.md`
([4_decisions.md](./4_decisions.md#decisions-are-recorded-on-their-orbit-task-not-in-a-shipped-tag)).
Daniel's 2026-11-02 words above are kept as he wrote them.
Amended 2026-11-23 ([ORB-27638]): a decision that outlives one task
returns as a shipped `decision` record, beside `open-item` records for what
waits on Daniel ([2_design.md §18](./2_design.md#18-decisions-and-open-items-proposed)).

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

### Directions of 2026-11-20

Said in the chief's chat of a formation session ([ORB-25797]), except where
noted. They supersede the 2026-11-02 direction that tags "are only
explicitly added on the instruction of the user" for every tag but
`preferences`.

- The principle quoted at the top of this page.
- To the aide: "we don't really need friction and tasks - they can be
  created via orbit."
- "I think decisions are also weak. we log decisions on the orbit task
  itself mostly, quoting me verbatim."
- Pasting five analysis results from recent sessions: "below are some of
  the things that should be automatically logged into \"findings\". we
  keep forgetting. So I suggest we make these implicit, i.e. have scribes
  add these into findings automatically, instead of explicitly specificying
  them".
- "same for idea - there are probably so many forgotten ideas that never
  got resurfaced. probably mostly bad ideas to begin with but I think it
  would be good to keep them captured automatically."
- On whether preferences should still need his approval: "I agree with the
  explicit for preference."

What he was looking at, measured on his store that day: 4 entries in all
(1 `preferences`, 1 `remind`, 2 `standard`) against 31 cataloged sessions;
none under `task`, `friction`, `decisions`, `finding` or `idea`; and 20 open
proposals, the store-wide cap, so the session pass had asked for no
candidates since 2026-11-17.

| Change | Task | State on 2026-11-20 |
|---|---|---|
| `task` and `friction` stop shipping; work and friction are filed in Orbit | [ORB-25977] | filed, not landed |
| `decisions` stops shipping; a decision lives on its Orbit task or in a `4_decisions.md` | [ORB-25980] | filed, not landed |
| The session pass writes findings itself, verbatim, with no proposal or approval | [ORB-25983] | filed, not landed |
| The session pass captures his ideas and what became of them, and links near ones | [ORB-25987] | filed, not landed |
| `preferences` keep his typed approval: agents act on them, so a wrong one costs more | none | unchanged |
| Amended 2026-11-23: a decision that outlives a task is a `decision` record again; open items join it | [ORB-27638], [ORB-27655], [ORB-27656] | Records, entry writes and the settling verbs (`entry answer`, `entry close`, `entry list --state`) built |

Two things the principle asks for that no task covers yet:

- **Approving a preference should cost less** than typing
  `#scribe approve <id>`. Four candidates had waited since 2026-11-16.
- **The read side is not measured.** scribe does not record whether an
  entry was ever cited by `ask` or shown again, so it cannot tell a useful
  catalog from a pile.

## 2. Shape

| Part | Does | Model |
|---|---|---|
| `#scribe <instruction>` | In a user-typed turn, in any session whether on or off, adds exactly one entry to the named tag. It quotes Daniel verbatim, cites the message, and echoes the tag, text and id. An unknown tag is refused with the configured list. A bare `#preference` captures nothing. | None, unless the instruction says "this"/"it": then Haiku picks the referenced message |
| `/scribe on` | Catalogs that session into one session record (summary, goals, outcome, next), refreshed on cadence and at session end. Writes no tag entries. No backfill of other sessions. `/scribe on --from <id>` or `/scribe link <id>` links it to the session it was spawned from or continues ([§5.4](./2_design.md#54-session-links)); its pass reads that session's summary as context. | Haiku |
| Consistency routine | Flags contradictions and stale entries and proposes "B supersedes A" or "retire A". Daniel approves with `#scribe approve <id>`. A superseded entry stays on disk, is marked, and is hidden from answers by default. From S10 it also deactivates sessions whose feature a later entry Daniel wrote says was removed (only Daniel-authored entries count as removal evidence), and sessions past a generous age limit unless pinned. | Haiku, on model-free candidate pairs |
| CLI agent surface (S11–S13) | Agents on the Mac search (`scribe search`, BM25), filter lists by date, status, topic and task, and add or update entries, tags and proposals with `--author`; one `scribe` skill teaches it. Agent writes are marked `author.kind: agent`, never quoted as Daniel's words, and never settle an entry Daniel wrote. | None |
| tmux popup (S13) | `scribe tmux` lists sessions, entries, proposals and backfill candidates in fzf, with session logs and full entry detail in the preview. | None |
| Backfill by name (S14) | `scribe transcript list --since 2026-10-01` lists long uncataloged sessions; Daniel picks and runs `scribe session catalog <id>`. | Haiku (the session pass) |

Daniel's own entries and settlements come only from his typed `#scribe`
turns. No tool is advertised to agents over MCP: `ask`, the one that
was, is removed ([ORB-27911](./4_decisions.md#ask-the-orbit-task-search-and-the-tmux-popup-are-removed)); from S12 agents read and write through
the CLI, each write recording its author. A session record becomes a
rewritable header over an append-only log of segments (S8, S9), and a
session can be inactive: hidden, not removed (S8, S10).

## 3. Core concepts

- **Tag** — a named list Daniel writes to, such as `preferences`, declared in the config with its aliases.
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

Full definitions: [references/glossary.md](./references/glossary.md).

## 4. Questions settled by this design

Each was open when the repository was created; each is now a decision Daniel
can override.

| Question | Decision |
|---|---|
| Whether superseded entries are ever removed, or only hidden (the directions say "outdated record removal") | [Superseded and stale entries are hidden, not removed](./4_decisions.md#superseded-and-stale-entries-are-hidden-not-removed) |
| Storage layout and the search index | [One Markdown file per record, metadata in YAML frontmatter](./4_decisions.md#one-markdown-file-per-record-metadata-in-yaml-frontmatter), [`ask` ranks in memory with no persisted index](./4_decisions.md#ask-ranks-in-memory-with-no-persisted-index) |
| The migration from the orbit-office scribe (its v1 and v2 records) | [No migration of orbit-office scribe records](./4_decisions.md#no-migration-of-orbit-office-scribe-records) |
| Which tags ship by default beyond `preferences` and `decisions` | [Only decisions and preferences ship by default](./4_decisions.md#only-decisions-and-preferences-ship-by-default) |

## 5. At a glance

| Concern | Where it will live | Slice |
|---------|--------------------|-------|
| Entries, session records, proposals; the store | `crates/scribe-core/src/{entry,session,consistency}.rs`, `store/` | S1 |
| `#scribe` parsing and capture | `crates/scribe-core/src/capture.rs`, `crates/scribe/src/commands/hook/` | S2, S3 |
| `/scribe on` and session passes | `crates/scribe-core/src/{session,transcript}.rs`, `commands/hook/`, `commands/session.rs` | S4 |
| Session links (`spawned_from`, `continues`) | `crates/scribe-core/src/{session,control}.rs`, `store/`, `commands/hook/scribe_command.rs`, `commands/session/` | ORB-23849 |
| Consistency routine | `crates/scribe-core/src/consistency.rs`, `commands/consistency.rs` | S6 |
| Orbit plugin tree | `plugin/.orbit-plugin/` | S5 |
| Claude Code hooks package | `plugin/claude-code/` | S2 |
| Store format 4: session header and log, `active`, authors (the tags store is S12) | `crates/scribe-core/src/{session,record,activity}.rs`, `store/` | S8 |
| Segment-writing passes, `session rebuild` | `crates/scribe-core/src/pass.rs`, `commands/session/rebuild.rs` | S9 |
| Deactivation routes | `crates/scribe-core/src/{activity,consistency}.rs`, `commands/session/activate.rs` | S10 |
| `scribe search`, list filters, id prefixes | `crates/scribe-core/src/search.rs`, `commands/search.rs` | S11 |
| Agent write verbs and the author guard | `commands/{entry,tag,proposal}.rs` | S12 |
| The `scribe` skill, the tmux popup | `plugin/claude-code/skills/scribe/`, `crates/scribe/src/popup.rs` (`scribe tmux`) | S13 |
| `session catalog`, `transcript list/show` | `commands/session/catalog.rs`, `commands/transcript.rs` | S14 |

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-21744] — the orbit-office scribe redesign this plugin supersedes (archived).
- [ORB-21404] — the on-call task the scribe redesign was spawned from.
- [ORB-23849] — session links between session records.
- [ORB-23892] — designed S8–S14: session header and log, the `active`
  flag, the CLI agent surface and skill, the tmux popup, backfill by name.
- [ORB-25797] — the formation session in which Daniel gave the 2026-11-20
  directions.
- [ORB-25977] — stop shipping the `task` and `friction` action tags.
- [ORB-25980] — stop shipping the `decisions` memory tag.
- [ORB-25983] — the session pass writes findings itself.
- [ORB-25987] — the session pass captures ideas and their outcome.
- [ORB-27638] — design: `decision` and `open-item` records (proposed).

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
