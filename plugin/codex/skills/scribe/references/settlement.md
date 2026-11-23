# Settling and deleting on the human’s instruction

- [Procedure](#procedure)
- [Commands](#commands)

## Procedure

The human can ask you to approve or reject a claim, retire or forget an entry,
or deactivate a session. Only a session the human is typing in does this:
routines, scheduled or background jobs, Orbit runs and subagents never settle
or delete. Every act follows the same order.

1. **List before acting.** Find the targets with `scribe proposal list --state
   open --json` (claims), `scribe entry list --status active --json` (narrow
   it with `--tag` or `--project`), `scribe search "<words>" --json`
   or `scribe session list --json`. Act only on ids the output shows.
2. **Name every id in your reply, then wait.** Before the human answers, your
   reply names each id you may act on, one per line, with the act you would
   run: `p-<n>: approve`, `e-<n>: retire`. The check reads this reply against
   their answer, so keep it plain. Put `approve` or `accept` on the line of
   each approval's id. Give each act's reversal in the same reply (step 7).

   - Omit these words throughout the reply, including the reversals, even
     when the act is intended. The check refuses not, no, never, don't,
     dont, can't, cant, cannot, won't, wont, shouldn't, shouldnt, doesn't,
     doesnt, didn't, didnt, isn't, isnt, except, unless, without, only, avoid,
     but, stop, cancel and other than. Curly apostrophes count too. A
     snake_case code such as `instruction_not_found` is exempt.
3. **Act only on an explicit instruction in this session.** It names the act
   and the ids, or answers your reply that names them, such as `approve p-<n>`
   or `yes` after your reply lists `p-<n>: approve`.

   - A bare `yes`, `ok`, `go ahead` or `do it` backs only an approval.
     Reject, retire and forget need the act's own word in the human's turn
     (`reject p-<n>`, `retire it`, `forget e-<n>`). After `yes` to one of
     those, ask them to say the act.
   - When a request names no id, such as "clean up the queue" or "the stale
     ones", list the candidates, name them, and wait.
4. **Pass their words verbatim, with your own session id.** `--instruction` is
   a span of one of the human's typed turns in this session, one of their last
   three, written within the last 30 minutes. Never paraphrase it or join
   turns. `--session` is the id of the session you are running in, never an id
   they name or one from a list, because the check reads the transcript of
   whichever id it is given.
5. **Run on the store's host only.** The check reads the transcript from this
   host's transcripts root, so run these commands on the host that holds the
   store and this session's transcript. Elsewhere the check refuses with
   `instruction_not_found`. Do not copy a transcript there or change the id to
   get past it.
6. **One command per target.** Run the command for the act below with one id
   each. Never batch ids, match by description or run a verb the human did not
   ask for. A consistency proposal about their entry is theirs alone: the CLI
   refuses it, and you never change their entry's content.
7. **Say each act's reversal** in the reply that names it:
   - **Approve**: the entry it wrote is retired if the approval was wrong. An
     entry from an assistant's claim retires under your own `--author`; the
     human's entry retires on their instruction. Its text is kept.
   - **Retire**: repaired by adding a new entry on their explicit instruction,
     using the write path for its tag. `scribe entry show <e-id>` still
     shows the text.

     | Tag | Write path for a new entry |
     | --- | --- |
     | Non-action memory tag (`preferences` or a configured memory tag) | Use `scribe entry add --spoken` with the human's quote. |
     | Action tag (such as `standard`) | Use the action-tag route with the human's words as `--quote` where required. |
     | `handoff` | Write a new handoff. |
     | `require_evidence` tag (such as `finding`) | Use `scribe entry add --evidence <pointer>` to record your finding. |

     Reserve `--spoken` for non-action memory tags. An action, `handoff` or
     `require_evidence` tag refuses it with `not_memory_tag`.
   - **Reject**: irreversible. The claim closes and stays out of future
     proposals.
   - **Forget**: irreversible. The entry's text and the proposal quotes of it
     are cleared for good; its id, tag and dates stay.
   - **Deactivate a session**: reversed by `scribe session activate <id>
     --author <name>`.
8. **Echo each result in one line.** Use `p-<n> approved as e-<n>`,
   `p-<n> rejected`, `e-<n> retired`, `e-<n> forgotten` or
   `<session id> deactivated`.

   - On refusal, stop that target and echo the exact code printed
     (`instruction_not_found`, `instruction_stale`, `instruction_mismatch`,
     `not_interactive` or another code). The check skips snake_case codes,
     so echoing one leaves their next answer valid. Keep the rest of the
     reply free of step 2's words.
   - Do not reword their words, switch verbs or retry to get past the check.
     Ask them to name the id and say the act again in a new message.
9. **Forget only on their words.** Run `entry forget` only when their words say
   forget, erase or for good. If they say purge or permanently alone, ask them
   to say forget, erase or for good first.

## Commands

Use your own author name for `<name>`.

```sh
scribe proposal settle <p-id> --approve --author <name> --session <id> --instruction '<their words>'
scribe proposal settle <p-id> --reject --author <name> --session <id> --instruction '<their words>'
scribe entry retire <e-id> --author <name> --reason '<why>' --session <id> --instruction '<their words>'
scribe entry forget <e-id> --author <name> --session <id> --instruction '<their words>' --confirm
scribe session deactivate <id> --author <name> --reason '<why>'
```

`--session` and `--instruction` come together or not at all. An entry an agent
wrote, not the human's, retires with `--author` and `--reason` alone. Session
deactivation takes no instruction flags; run it on the human's explicit yes for
that session. Name the approval in the reason.
