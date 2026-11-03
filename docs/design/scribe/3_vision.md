---
title: scribe — Vision
owner: claude
last_updated: 2026-11-02
last_validated: 2026-11-02
status: Draft
feature: scribe
doc_role: vision
type: design
summary: Where the scribe plugin may go after its first six slices — more providers, cross-host stores, better retrieval, an opt-in import of orbit-office decisions — and what it deliberately leaves out.
tags: [scribe]
paths: ["crates/**", "plugin/**"]
related_features: [scribe]
related_artifacts: [ORB-21747, ORB-21744]
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

The write path has no agent in it. Agents can ask, but only Daniel's typed
turns add, settle or forget a record, and every answer carries a quote that
code, not the model, has checked against the record.

## 4. Deliberately Out of Scope

- Capturing anything Daniel did not mark: no inference of preferences from
  behaviour, no tag entries from session passes.
- A UI. The CLI lists and the SessionStart line are the only views.
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

> Resolve any task above with `orbit task show <ID>` or `git log --grep=<ID>`.
