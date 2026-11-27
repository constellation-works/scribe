---
title: scribe — Overview
owner: claude
created_date: 2026-11-02
updated_date: 2026-11-26
last_validated: 2026-11-02
status: Draft
stage: building
next: 2_design.md §19 Orbit jobs built; only §19.5 providers/ deletion and model-config migration (ORB-28144) remain; S7 discussions unfiled
feature: scribe
doc_role: overview
type: design
summary: A standalone Rust CLI and Orbit plugin that records explicit preferences, catalogs sessions Daniel turns on or names, and lets agents search and manage attributed records through the CLI and one skill.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-21404, ORB-23849, ORB-23892, ORB-25977, ORB-25980, ORB-25983, ORB-25987, ORB-27638, ORB-27655, ORB-27656, ORB-27911]
---

# scribe — Overview

scribe is a standalone Rust CLI and Orbit plugin. It records preferences
Daniel explicitly marks, catalogs sessions he turns on or names, and stores
agent-authored records with their authorship. Agents search and manage these
records through the CLI, guided by one skill. Orbit routines keep session
records and proposals current. Scribe replaces the scribe built inside
orbit-office, without the Library, to-dos or calendar around it.

> The design ([2_design.md](./2_design.md),
> [4_decisions.md](./4_decisions.md)) describes the plugin. Delivery state
> lives only in the [slice plan](./2_design.md#10-slice-plan); this overview
> does not restate it.

**Principle (Daniel, 2026-11-20).** "I think scribe's best value is from
providing insights implicitly, rather than explicitly - seamlessly
cataloging things that would've been lost otherwise without any additional
effort. I just turn on scribe, and scribe extracts insights and turn them
into value". For ongoing capture, turning a session on is the one act scribe
asks of him; naming a past session opts that session in as well. A step
that needs him to type, approve or remember has to earn its place. This
widens capture to qualifying material from sessions Daniel chose to
catalog; preferences still require his explicit instruction or checked
approval. What changes and which tasks carry it:
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
waits on Daniel ([2_design.md §18](./2_design.md#18-decisions-and-open-items)).

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

| Change | Task | State when filed |
|---|---|---|
| `task` and `friction` stop shipping; work and friction are filed in Orbit | [ORB-25977] | filed, not landed |
| `decisions` stops shipping; a decision lives on its Orbit task or in a `4_decisions.md` | [ORB-25980] | filed, not landed |
| The session pass writes findings itself, verbatim, with no proposal or approval | [ORB-25983] | filed, not landed |
| The session pass captures his ideas and what became of them, and links near ones | [ORB-25987] | filed, not landed |
| `preferences` keep his typed approval: agents act on them, so a wrong one costs more | none | unchanged |
| Amended 2026-11-23: a decision that outlives a task is a `decision` record again; open items join it | [ORB-27638], [ORB-27655], [ORB-27656], [ORB-27657] | Records, entry writes, the settling verbs (`entry answer`, `entry close`, `entry list --state`) and reading (the `## Open items` brief part and JSON, the `SessionStart` count, `search` labels) built |

Two things the principle asks for that no task covers yet:

- **Approving a preference should cost less** than typing
  `#scribe approve <id>`. Four candidates had waited since 2026-11-16.
- **The read side is not measured.** `search` returns ranked hits, but
  scribe does not record whether an entry was later consulted, so it cannot
  tell a useful catalog from a pile.

## 2. Shape

| Part | Does | Model |
|---|---|---|
| `#scribe` marker | The prompt hook marks a typed turn. At Stop, an unresolved marker is handed to the main agent, which records it through the CLI with a checked quote or explains why nothing was recorded. | Main agent |
| `/scribe on` and `scribe session catalog` | Catalog the current session or a named past session. The session pass refreshes a header and append-only log from its provider transcript. | Session model |
| `scribe search` | Ranks entries, session headers and log segments. `--kind message` searches stored turns. Search reads the local store and makes no model or Orbit call. | None |
| `entry`, `proposal`, `memory`, `group` and `brief` | Read and manage entries (including decisions and open items), proposals, memory, session groups and the session-start page. | Depends on the command |
| `consistency scan` | Reapplies approved proposals, opens proposals for stale entries, closes candidates for retired tags, and deactivates aged sessions. It does not judge entry pairs or infer feature removal. | None |
| Orbit plugin | Its jobs call the plugin's prepare/apply tools; the manifest sets every tool's `mcp_scope` to `none`. | Orbit runner |
| Backfill by name | `scribe transcript list` finds uncataloged provider transcripts; Daniel can pick one with `scribe session catalog <id>`. | Session model |

When an entry attributes words to Daniel, it cites his typed turn and keeps
the checked quote; agent-authored records identify their author. The plugin
does not expose its tools over MCP. The `ask` command was removed
([ORB-27911] removed `ask` and the tmux popup; see the [removal decision](./4_decisions.md#ask-the-orbit-task-search-and-the-tmux-popup-are-removed));
agents use the CLI to search and manage records. A session record has a
rewritable header over an append-only log of segments, and an inactive
session is hidden rather than removed.

## 3. Core concepts

- **Tag** — a named record category supplied by scribe or configured with aliases.
- **Entry** — a stored record with content, an author, status and provenance;
  some entries also carry a checked quote or structured decision, open-item
  or handoff fields.
- **Session record** — the one catalog record of a session Daniel turned on
  or named: a header (title, summary, goals, next) over a log of segments.
- **Segment** — one sealed or open stretch of a session record's log, cited
  `session:<id>#<n>`.
- **Author** — who wrote an entry or proposal: the user (`#scribe`), an agent
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
| Storage and search | [The SQLite store and its exported document formats](./specs/store.md); [`scribe search` ranks stored records without a persisted search index](./4_decisions.md#ask-ranks-in-memory-with-no-persisted-index) |
| The migration from the orbit-office scribe (its v1 and v2 records) | [No migration of orbit-office scribe records](./4_decisions.md#no-migration-of-orbit-office-scribe-records) |
| Which tags ship by default | [The shipped tag definitions](./specs/store.md) |

## 5. At a glance

| Concern | Where it lives | Slice |
|---------|--------------------|-------|
| Entries, proposals, memory and the store | `crates/scribe-core/src/apps/{entry,proposal,memory}/`, `crates/scribe-store/` | S1, later slices |
| `#scribe` markers and capture | `crates/scribe-core/src/apps/hook/`, `crates/scribe/src/commands/hook/` | S2, S3, §14 |
| `/scribe on`, catalog and session passes | `crates/scribe-core/src/apps/{session,hook}/`, `crates/scribe-tools/src/transcript/`, `crates/scribe/src/commands/session/` | S4, S14 |
| Session links (`spawned_from`, `continues`) | `crates/scribe-core/src/apps/session/rules/control.rs`, `apps/hook/scribe_command.rs`, `crates/scribe-store/`, `commands/session/` | ORB-23849 |
| Consistency scan | `crates/scribe-core/src/apps/consistency/`, `crates/scribe/src/commands/consistency.rs` | S6 |
| Orbit plugin tree | `plugin/.orbit-plugin/` | S5 |
| Claude Code hooks package | `plugin/claude-code/` | S2 |
| Store format 4: session header and log, `active`, authors (the tags store is S12) | `crates/scribe-store/src/contracts/{session,record}.rs`, `apps/session/rules/activity.rs`, `crates/scribe-store/` | S8 |
| Segment-writing passes, `session rebuild` | `crates/scribe-core/src/apps/session/rules/pass.rs`, `commands/session/rebuild.rs` | S9 |
| Deactivation routes | `crates/scribe-core/src/apps/session/activity.rs`, `apps/consistency/`, `commands/session/activity.rs` | S10 |
| `scribe search`, filters and id prefixes | `crates/scribe-core/src/apps/search/`, `crates/scribe-store/src/repository/ids.rs`, `crates/scribe/src/commands/search.rs` | S11 |
| Agent write verbs, groups and authorship | `crates/scribe-core/src/apps/{entry,tag,proposal,group}/`, `crates/scribe/src/commands/` | S12, later slices |
| The `scribe` skill | `skills/scribe/`, synced to `plugin/claude-code/skills/scribe/` | S13 |
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
- [ORB-27638] — designed the `decision` and `open-item` records.
- [ORB-27655] — built the decision and open-item entry records.
- [ORB-27656] — built open-item answer, close and state commands.
- [ORB-27911] — removed `ask`, the tmux popup and in-process model calls.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
