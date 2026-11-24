---
title: scribe — Vision
owner: claude
last_updated: 2026-11-21
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: vision
type: design
summary: Where the scribe plugin may go after its slices — more providers (Claude Code and Codex only, decided 2026-11-21), cross-host stores, better retrieval, an opt-in import of orbit-office decisions, a view for humans now the tmux popup is removed — and what it deliberately leaves out.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-23892, ORB-26219, ORB-26240, ORB-26256]
---

# scribe — Vision

Speculative. Nothing here is designed or committed; the plan is the slice
plan in [2_design.md §10](./2_design.md#10-slice-plan).

## 1. Open Questions

1. **Other providers.** Claude Code is first because its hooks see the
   user's typed turn before the model does. Codex and other providers need
   an equivalent pre-model hook, or a transcript watcher that only reads
   sessions Daniel switched on. Which provider comes second, and does a
   watcher keep the "user-typed only" rule honest?
2. **Two hosts.** Sessions run on the Mac and on dk-server-1, each with its
   own `~/.scribe`. Should one host own the store and the other forward
   `#scribe` writes to it (over the SSH transport Daniel already uses), or
   should `scribe search` read both? Either changes the provenance story.
3. **Retrieval beyond BM25.** When paraphrase misses start to show up in
   `scribe search`, is a query-expansion step (which keeps the store
   index-free) enough, or does the store need
   embeddings and a persisted index after all
   ([4_decisions.md](./4_decisions.md#ask-ranks-in-memory-with-no-persisted-index))?
4. **Importing orbit-office decisions.** An opt-in, one-shot importer could
   turn each human-decided ledger line with a verbatim quote into a
   proposal Daniel approves one by one. Is that worth building, given
   [the no-migration decision](./4_decisions.md#no-migration-of-orbit-office-scribe-records)?
5. **Where Daniel reviews.** Proposals are listed at session start and
   settled with `#scribe approve`. If the queue grows, does it want a
   digest (an Orbit notice, a mail through orbit-decision-mail) rather than
   a session-start line?
6. **Session records as evidence.** They are model summaries, so search
   labels them and never quotes them as Daniel's words. Should search show
   the underlying transcript lines instead, which would mean keeping a
   parsed copy of each opted-in transcript?

7. **A view for humans.** The tmux popup was removed (ORB-27911): Daniel
   called it "just bad way to do things". If a human view is wanted again,
   it starts from what he needs to see, not from the popup.
8. **Box workers and the agent surface.** The `scribe` skill serves Mac
   sessions only. If box workers need scribe, does that wait for a
   cross-host store (question 2), or does the skill learn to reach the Mac
   over SSH?
9. **Memory for a user who lives elsewhere.** Answered 2026-11-21: scribe
   reads Claude Code and Codex sessions only. Daniel, of a regular user
   ("She is bounded to chatGPT and resistant to change because of
   memory"), then: "no hosted server - I am not managing other's memory.
   claude code and codex support only then. so be it". There is no
   ChatGPT-export import, app connector or hosted server, as future work
   or otherwise ([4_decisions.md](./4_decisions.md#scribe-reads-claude-code-and-codex-sessions-only)).
   The third route, always-on capture of a session that is not switched on
   with `/scribe private` as the off switch, was designed for the sessions
   scribe already reads
   ([2_design.md §13.12](./2_design.md#1312-a-personal-base-and-a-work-layer-per-session))
   and is parked, with its personal base, light catalog entries, `people`
   and recurring dates
   ([4_decisions.md](./4_decisions.md#the-personal-base-people-recurring-dates-and-light-catalog-entries-are-parked)).

## 2. Prior Work

- **orbit-office scribe** (v1 and v2, `docs/design/office/specs/scribe.md`
  in orbit-office): session records with Summary/Goals/Outcome/Next under
  character budgets, line-start marker detection that ignores fences,
  quotes and pasted content, an exact-substring quote check, the
  model-free "not captured" answer, a Stop-hook cadence of 20 minutes or
  10 turns, and a contradiction view that orders sides by date. scribe
  reuses these rules; it drops the Library, the git commit per pass, the
  decision-ledger extractor and the auto-backfill.
- **Orbit plugins** (`docs/design/plugins/` in orbit): the plugin tree,
  exec backend, `mcp_scope`, capability floor and seeded-disabled routines
  this plugin is built on.

## 3. What May Be Distinctive

Daniel's words have no agent in their path. Until S12 the write path had
no agent at all; from S12 agents write too, through the CLI, but every
record says who wrote it, only Daniel's typed turns add his entries or
settle and forget them, and every answer carries a quote that code, not
the model, has checked against the record.

## 4. Deliberately Out of Scope

- Binding anything Daniel did not mark: no inference of **preferences**
  from behaviour, and no tag entries from session passes; a preference
  binds agents only once he approves it.

  Amended 2026-11-21 ([ORB-26219]) for **descriptive memory only**: the
  session pass writes memory items (`profile`, `projects`, `working`, `now`,
  `milestones`) from his typed turns in sessions he switched on, with his
  verbatim quotes and no approval step
  ([2_design.md §13](./2_design.md#13-memory-profile-projects-working-now-and-milestones-proposed)).
  Daniel, chief's chat, 2026-11-21: "I think we should expand scribe's role
  to manage memory system … What we are lacking is a holistic memory, e.g.
  user profile, interests, projects, working preferences, etc." A memory
  item describes; it never binds an agent. A recurring `working` item
  reaches `preferences` only as a candidate he approves
  ([4_decisions.md](./4_decisions.md#scribe-keeps-a-descriptive-memory-the-pass-writes-without-approval)).
- A desktop or web UI. The CLI and the SessionStart line are the views
  (the tmux popup was one until [ORB-27911](./4_decisions.md#ask-the-orbit-task-search-and-the-tmux-popup-are-removed)).
- Scanning or cataloging sessions on scribe's own initiative. Backfill
  catalogs only sessions Daniel names.

  Amended 2026-11-21 ([ORB-26240]) for **interactive sessions only**: every
  session a human types in feeds the personal memory base and, when it is
  not switched on, leaves a light catalog entry (title, date, short
  summary), unless he types `/scribe private`
  ([2_design.md §13.12](./2_design.md#1312-a-personal-base-and-a-work-layer-per-session),
  [§13.15](./2_design.md#1315-what-a-session-that-is-not-switched-on-leaves)).
  Daniel, chief's chat, 2026-11-21: "I am fine with using personal as the
  base", and to a light entry for sessions not switched on: "Yes, light
  entry". Work sessions stay chosen one by one with `/scribe on`; backfill
  still catalogs only sessions he names; headless runs are never read.
- Tasks, calendars and the Library: orbit-office keeps those.

## 5. References

**Project-internal**

- [1_overview.md](./1_overview.md), [2_design.md](./2_design.md),
  [4_decisions.md](./4_decisions.md)

**External**

- orbit-office: `docs/design/office/specs/scribe.md`,
  `docs/design/claude-code-scribe/4_decisions.md`,
  `app/packages/server/src/features/scribe/contradictions.ts`.
- orbit: `docs/design/plugins/1_scope.md`, `docs/design/routines/2_design.md`.

## Task References

- [ORB-21747] — designed the standalone scribe plugin.
- [ORB-21744] — the orbit-office scribe redesign this plugin supersedes (archived).
- [ORB-26219] — designed the descriptive memory and amended §4 for it.
- [ORB-26240] — amended §4 and question 9 for the personal base and light
  catalog entries.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
