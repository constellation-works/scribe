---
type: design
summary: "Glossary: scribe"
last_validated: 2026-11-02
---

# Glossary: scribe

Terms with a specific meaning in this project. General CLI vocabulary (TTY,
stdout, exit code) is excluded. Terms marked *(template)* describe code that
exists today and is reshaped by slice S1.

| Term | Meaning |
|------|---------|
| **Tag** | A named list Daniel writes to (`preferences`), declared with its aliases in `config.toml`. See [2_design.md §3](../2_design.md#3-storage). |
| **Entry** | One record in a tag: `said` (Daniel's `#scribe` line verbatim), `content` (what it records, verbatim from that line or from the message it points at), `cite` and `status`. See [2_design.md §3](../2_design.md#3-storage). |
| **Instruction** | One line of a user-typed turn starting with `#scribe`. Each yields exactly one entry or one refusal. See [2_design.md §4](../2_design.md#4-scribe). |
| **Referent** | Retired with form 3 ([2_design.md §14.6](../2_design.md#146-what-happens-to-forms-1-2-and-3)): "this"/"it" after a `#scribe` marker is resolved by the annotation call, which quotes Daniel's own words. `models.referent` loads with a warning that it is unused. |
| **Session link** | A typed link (`spawned_from` or `continues`) from one session record to another, typed by Daniel with `/scribe on --from <id>` or `/scribe link <id>` and stored on the child; children are derived. Distinct from `related`, the references a session mentions. See [2_design.md §5.4](../2_design.md#54-session-links). |
| **Session record** | The one model-written catalog record of a session Daniel switched on with `/scribe on` or named with `session catalog`: up to format 3 summary, goals, outcome and next; from format 4 a header over a log. See [2_design.md §5](../2_design.md#5-scribe-on). |
| **Header** | The part of a format-4 session record every pass rewrites: title, summary (a table of contents), goals and next. See [2_design.md §3](../2_design.md#3-storage). |
| **Log**, **segment** | The append-only part of a format-4 session record: numbered segments, each a message range with a heading and about 800 characters of text. Only the last may be open; sealed segments never change. Cited `session:<id>#<n>`. See [2_design.md §5.5](../2_design.md#55-the-log-segments-and-sealing). |
| **Active**, **inactive** | Whether a session's content still describes something current, separate from `status`. An inactive session is hidden, not removed. **Pinned** (`keep_active`) sessions are exempt from automatic deactivation. See [2_design.md §5.7](../2_design.md#57-active-and-inactive-sessions). |
| **Author** | Who wrote an entry or proposal: `{kind, name}`, `kind` `daniel` (a typed `#scribe` turn), `agent` (a CLI write verb) or `scribe` (scribe's routine). See [2_design.md §1.1](../2_design.md#11-the-cli-agent-surface). |
| **Candidate** | An uncataloged Claude Code transcript `transcript list` reports for Daniel to pick for backfill. See [2_design.md §5.8](../2_design.md#58-cataloging-a-past-session). |
| **Cataloged by name** | A past session switched on with `scribe session catalog <id>` rather than `/scribe on`; its record carries `cataloged_by`. |
| **Pass** | One refresh of a session record by the session-pass routine. |
| **Proposal** | A consistency finding (`supersedes` or `stale`) waiting for `#scribe approve` or `reject`. See [2_design.md §7](../2_design.md#7-consistency-routine). |
| **Hidden** | An entry whose status is `superseded` or `retired`, or (from format 4) an inactive session: kept on disk, left out of `ask` evidence by default. |
| **Evidence** | The ranked records `ask` hands to the decision model; an answer may only quote evidence. See [2_design.md §6](../2_design.md#6-ask). |
| **Verbatim check** | The one quote test: whitespace-collapsed, case-sensitive substring of the source. See [2_design.md §6.4](../2_design.md#64-the-verbatim-check). |
| **Decision model** | The model `ask` answers with (`models.ask`, Sonnet by default). |
| **Data directory** | Where the store lives: `--root`, else `SCRIBE_ROOT`, else `~/.scribe`. |
| **Store format** | The integer `format` (`FORMAT` in `store/codec.rs`) in each exported document and format stamp; a newer one is refused. Phase 8 removed `UPGRADES` and the file decoders: no build reads an older record file, and persisted-shape changes are `scribe.db` migrations. See [specs/store.md](../specs/store.md). |
| **Audit log** | the `audit` table of `scribe.db` (`<root>/audit.jsonl` before phase 7): one line per mutating command (what ran, on which record, how it ended), identifiers only. |
| **Payload** *(template)* | The typed record every output mode renders from; its JSON form is the contract. See [specs/output-contract.md](../specs/output-contract.md). |
| **Sink** *(template)* | The per-invocation resolution of output mode, color and width. |
| **List envelope** | The `--json` shape of every list: `{"<records>", "total", "truncated", "warnings"}`, with `<records>` one of `entries`, `sessions`, `proposals`, `tags`, and from S11/S14 `hits` and `transcripts`. |
