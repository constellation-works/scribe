---
title: scribe — Vision
owner: claude
last_updated: 2026-11-21
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: vision
type: design
summary: Where the scribe plugin may go after its slices — more providers, cross-host stores, better retrieval, an opt-in import of orbit-office decisions, a real TUI if the tmux popup proves too small — and what it deliberately leaves out.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744, ORB-23892, ORB-26219, ORB-26240]
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
   should `ask` read both? Either changes the provenance story.
3. **Retrieval beyond BM25.** When paraphrase misses start to show up in
   `no_record` answers, is a query-expansion step by the decision model
   (which keeps the store index-free) enough, or does the store need
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
6. **Session records as evidence.** They are model summaries, so `ask`
   labels them and never quotes them as Daniel's words. Should `ask` quote
   the underlying transcript lines instead, which would mean keeping a
   parsed copy of each opted-in transcript?

7. **A real TUI.** The tmux popup ([2_design.md §1.2](./2_design.md#12-the-tmux-popup))
   is a 60-line fzf script. If Daniel finds himself wanting to act from it
   (activate, deactivate, catalog, settle) rather than read and copy ids,
   or to see a session's log beside its links, a ratatui TUI is the next
   step, at the cost of new dependencies. Build it only if the popup proves
   too small.
8. **Box workers and the agent surface.** The `scribe` skill serves Mac
   sessions only. If box workers need scribe, does that wait for a
   cross-host store (question 2), or does the skill learn to reach the Mac
   over SSH?
9. **Memory for a user who lives elsewhere.** The memory of
   [2_design.md §13](./2_design.md#13-memory-profile-projects-working-now-and-milestones-proposed)
   is built only from sessions scribe reads. Daniel, 2026-11-21, of a
   regular user: "She is bounded to chatGPT and resistant to change because
   of memory." Three routes are named here and none is designed: **importing
   a ChatGPT export** into sessions and memory items; **an app connector**
   that reads such a provider's conversations as they happen; and
   **always-on capture with an off-the-record switch**, the reverse of
   today's `/scribe on`. The first test before any of them: import one
   export with its owner's consent, build the memory page from it, and
   have the owner read the page. Each would need its own provenance story,
   since none of these turns is typed through a scribe hook.
   (The personal base of [2_design.md §13.12](./2_design.md#1312-the-personal-base-and-the-work-layer),
   proposed by [ORB-26240], is the third route for memory only and only
   for providers scribe already reads, with `/scribe private` as its
   off-the-record switch.)

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
- A desktop or web UI. The CLI, the SessionStart line and, from S13, the
  tmux popup are the views ([4_decisions.md](./4_decisions.md#humans-browse-scribe-in-a-tmux-popup-over-fzf)).
- Scanning or cataloging sessions on scribe's own initiative. Backfill
  catalogs only sessions Daniel names.

  Amended 2026-11-21 ([ORB-26240]), proposed and not built, for the
  memory's **personal base** only: every interactive session he types in
  feeds `profile`, `now`, `milestones` and `people` unless he types
  `/scribe private`; the work layer still reads only sessions he switched
  on. Daniel, chief's chat, 2026-11-21: "work related sessions should be
  selectively chosen. also, I am fine with using personal as the base".
  Whether such a session also leaves a light catalog entry is his open
  question ([2_design.md §13.15](./2_design.md#1315-open-question-for-daniel-what-an-off-session-leaves)).
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
- [ORB-26240] — proposed the personal base and amended §4 and question 9 for it.

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
