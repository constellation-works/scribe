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
| **Tag** | A named list Daniel writes to (`decisions`, `preferences`), declared with its aliases in `config.toml`. See [2_design.md §3](../2_design.md#3-storage). |
| **Entry** | One record in a tag: `said` (Daniel's `#scribe` line verbatim), `content` (what it records, verbatim from that line or from the message it points at), `cite` and `status`. See [2_design.md §3](../2_design.md#3-storage). |
| **Instruction** | One line of a user-typed turn starting with `#scribe`. Each yields exactly one entry or one refusal. See [2_design.md §4](../2_design.md#4-scribe). |
| **Referent** | The earlier message "this"/"it" in an instruction points at, chosen by the referent model and verbatim-checked. See [2_design.md §4.3](../2_design.md#43-resolving-this-and-it). |
| **Session link** | A typed link (`spawned_from` or `continues`) from one session record to another, typed by Daniel with `/scribe on --from <id>` or `/scribe link <id>` and stored on the child; children are derived. Distinct from `related`, the references a session mentions. See [2_design.md §5.4](../2_design.md#54-session-links). |
| **Session record** | The one model-written catalog record (summary, goals, outcome, next) of a session Daniel switched on with `/scribe on`. See [2_design.md §5](../2_design.md#5-scribe-on). |
| **Pass** | One refresh of a session record by the session-pass routine. |
| **Proposal** | A consistency finding (`supersedes` or `stale`) waiting for `#scribe approve` or `reject`. See [2_design.md §7](../2_design.md#7-consistency-routine). |
| **Hidden** | An entry whose status is `superseded` or `retired`: kept on disk, left out of `ask` evidence by default. |
| **Evidence** | The ranked records `ask` hands to the decision model; an answer may only quote evidence. See [2_design.md §6](../2_design.md#6-ask). |
| **Verbatim check** | The one quote test: whitespace-collapsed, case-sensitive substring of the source. See [2_design.md §6.4](../2_design.md#64-the-verbatim-check). |
| **Decision model** | The model `ask` answers with (`models.ask`, Sonnet by default). |
| **Data directory** | Where the store lives: `--root`, else `SCRIBE_ROOT`, else `~/.scribe`. |
| **Store format** | The integer `format` in each persisted document; a newer one is refused, an older one is upgraded through `UPGRADES`. See [specs/store.md](../specs/store.md). |
| **Audit log** | `<root>/audit.jsonl`: one line per mutating command (what ran, on which record, how it ended), identifiers only. |
| **Payload** *(template)* | The typed record every output mode renders from; its JSON form is the contract. See [specs/output-contract.md](../specs/output-contract.md). |
| **Sink** *(template)* | The per-invocation resolution of output mode, color and width. |
| **List envelope** | The `--json` shape of every list: `{"<records>", "total", "truncated", "warnings"}`, with `<records>` one of `entries`, `sessions`, `proposals`, `tags`. |
