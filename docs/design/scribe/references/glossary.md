---
type: design
summary: "Glossary: scribe"
last_validated: 2026-11-26
---

# Glossary: scribe

Terms with a specific meaning in this project. General CLI vocabulary (TTY,
stdout, exit code) is excluded. Terms marked *(template)* describe code that
exists today and is reshaped by slice S1.

| Term | Meaning |
|------|---------|
| **Tag** | A named record category supplied by scribe or configured with its aliases. Shipped tags include `decision`, `handoff`, `open-item`, `preferences` and `standard`. See [specs/store.md](../specs/store.md). |
| **Entry** | One stored record under a tag, with content, author, status and provenance. Some entries also carry a checked quote or structured decision, open-item or handoff fields. See [specs/output-contract.md](../specs/output-contract.md). |
| **Instruction** | A typed `#scribe` marker or settlement line such as `#scribe approve <id>`. Stop hands unresolved capture markers to the agent; settlement lines approve or reject proposals, or retire or forget entries. See [4_decisions.md](../4_decisions.md#the-consistency-scan-and-stop-call-no-model). |
| **Referent** | A reference such as "this" or "it" in a `#scribe` marker. The former form-3 referent call and the later annotation model call are retired; at Stop, eligible unsettled markers are handed to the agent for recording. `models.referent` and `models.annotation` are unused. See [2_design.md §14.6](../2_design.md#146-what-happens-to-forms-1-2-and-3). |
| **Session link** | A typed link (`spawned_from` or `continues`) from one session record to another, typed by Daniel with `/scribe on --from <id>` or `/scribe link <id>` and stored on the child; children are derived. Distinct from `related`, the references a session mentions. See [2_design.md §5.4](../2_design.md#54-session-links). |
| **Session record** | The catalog record of a session Daniel switched on with `/scribe on` or named with `session catalog`: a header over a log of segments. See [2_design.md §5](../2_design.md#5-scribe-on). |
| **Header** | The part of a format-4 session record every pass rewrites: title, summary (a table of contents), goals and next. See [2_design.md §3](../2_design.md#3-storage). |
| **Log**, **segment** | The append-only part of a session record: numbered segments, each a message range with a heading and text targeting 700 characters, up to 900. Only the last may be open; sealed segments never change. Cited `session:<id>#<n>`. See [2_design.md §5.5](../2_design.md#55-the-log-segments-and-sealing). |
| **Active**, **inactive** | Whether a session's content still describes something current, separate from `status`. An inactive session is hidden, not removed. **Pinned** (`keep_active`) sessions are exempt from automatic deactivation. See [2_design.md §5.7](../2_design.md#57-active-and-inactive-sessions). |
| **Author** | Who wrote an entry or proposal: `{kind, name}`, with `kind` `user` (a typed `#scribe` turn; older stores used `daniel`), `agent` (a CLI write verb) or `scribe` (scribe's routine). See [2_design.md §1.1](../2_design.md#11-the-cli-agent-surface). |
| **Candidate** | An uncataloged Claude Code or Codex transcript `transcript list` reports for Daniel to pick for cataloging. See [2_design.md §5.8](../2_design.md#58-cataloging-a-past-session). |
| **Cataloged by name** | A past session switched on with `scribe session catalog <id>` rather than `/scribe on`; its record carries `cataloged_by`. |
| **Pass** | One refresh of a session record by the session-pass routine. |
| **Proposal** | A candidate entry or a consistency finding waiting for Daniel's approval or rejection. See [2_design.md §7](../2_design.md#7-consistency-routine). |
| **Hidden** | An entry whose status is `superseded` or `retired`, or an inactive session. Search leaves these out by default; `--include-hidden` includes them. |
| **Evidence** | The entries, session headers, segments or stored messages returned as ranked hits by `scribe search`. Search does not produce a model answer. |
| **Verbatim check** | The quote test: NFC-normalizes both strings, changes CRLF to LF, collapses whitespace, then checks for a case-sensitive substring at least 8 characters long. See [2_design.md §6.4](../2_design.md#64-the-verbatim-check). |
| **Data directory** | Where the store lives: `--root`, else `SCRIBE_ROOT`, else `~/.scribe`. |
| **Store format** | The integer `FORMAT` written in exports and the `control.json` format stamp; a newer one is refused. Phase 8 removed record-file decoders: records live in `scribe.db`, and persisted-shape changes use database migrations. See [specs/store.md](../specs/store.md). |
| **Audit log** | The `audit` table in `scribe.db` (`<root>/audit.jsonl` before phase 7): records audited mutating-command outcomes with record ids and error codes, not arguments or record content. |
| **Payload** *(template)* | The typed record every output mode renders from; its JSON form is the contract. See [specs/output-contract.md](../specs/output-contract.md). |
| **Sink** *(template)* | The per-invocation resolution of output mode, color and width. |
| **List envelope** | The `--json` shape of every list: a record array plus `total`, `truncated` and `warnings`; keys include `entries`, `sessions`, `proposals`, `tags`, `hits`, `transcripts`, `marks`, and `items` for groups. A default `scribe search` (no `--kind` or `--tag`) that finds no visible entry, session or segment, while stored messages match, also reports `message_matches` (the count of those messages). Other searches omit it, even when stored messages match. |
