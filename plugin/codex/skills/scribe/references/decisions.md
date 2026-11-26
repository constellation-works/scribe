# Decisions and open items

- [Ask Daniel](#ask-daniel)
- [Answer and close an item](#answer-and-close-an-item)
- [Record a lasting decision](#record-a-lasting-decision)
- [Boundaries](#boundaries)

Use these records for different jobs: a **decision** is a lasting reference; an
**open item** is something waiting for Daniel. Keep a choice about one piece of
work on its Orbit task, with Daniel's words quoted. Keep a repository design
decision in that repository's `4_decisions.md`; Scribe may hold a short pointer
to it so shared search can find it. Use the `decision` tag (aliases `decisions`,
`dec`) or the `open-item` tag (aliases `open-items`, `item`).

Do not copy an Orbit decision request into an open item. It is already in
Daniel's queue there. Do not mine chat or session transcripts for missed
decisions: the session pass, routines and Orbit jobs do not write either
record.

## Ask Daniel

The chief or a captain writes an item when a question or action is waiting on
Daniel. A choice has two to six full options, a recommended option and why it
is preferred. An action for Daniel has no options. Say what is blocked while
waiting; use `nothing` when nothing is blocked.

```sh
scribe entry add --tag open-item --author <model> --project <project> \
  --asked-by <session-name> --blocked '<what waits meanwhile>' \
  [--option '<option in full>' ... --recommended <n> --why '<why>'] \
  [--answer-in any|session:<id>|hand] \
  --related session:<id> --related task:<record> \
  '<the question, or the action that is his>'
```

Use `--answer-in any` (the default) unless only one session can carry the
answer in its auto mode (`session:<id>`), or Daniel must take the action
himself (`hand`), such as changing permissions or providing credentials. An
answer recorded in Scribe does not authorize another session's auto mode to do
what it refused. For `session:<id>`, Daniel must type his answer in that
session before it can carry out the action.

Tell Daniel the outcome and the one-line `open-item-<n>` id; do not repeat the
options in the message. He can check items with:

```sh
scribe entry list --tag open-item --state open --state answered
```

## Answer and close an item

The session where Daniel answers records his exact words and the session where
he typed them. Scribe checks the quote against that host's transcript.

```sh
scribe entry answer open-item-<n> --author <model> \
  --quote '<his words>' --related session:<where he typed>
```

A quote shorter than eight characters is accepted only when it is a whole
typed turn. The latest typed turn containing the quote supplies the answer's
time. Until the item closes, a later answer can replace an earlier one. The
session named by `--related` is where Scribe searches for the quote; it does
not override the item's `answer_in` instruction.

The asker carries out the answer, then closes the item. A choice must be
answered before it can close. An action item can close without an answer once
the asker confirms it is done.

```sh
scribe entry close open-item-<n> --author <model> \
  [--decision decision-<m>] [--note '<why no decision, or what action was done>']
```

Supply `--decision` or `--note` (or both). If the answer yields a lasting
decision, write that decision with `--settles open-item-<n>` and close the
item with `--decision decision-<m>`. The decision must be active and name this
item in `--settles`, so the link is two-way. If no lasting decision results,
use `--note` to say why; for a completed action item, record what was done.
With only `--decision`, the stored note says `settled by decision-<m>`.
`--note` is limited to 300 characters.

`answered` is not `closed`: the item remains visible until the asker acts and
closes it. Closing retires it with its answer and closure record. If the
question goes away or was wrong, retire it instead; that
produces the separate `withdrawn` state.

## Record a lasting decision

Write a decision in the same turn as carrying it out, including when no open
item prompted it. A standing rule, grant of authority, policy choice or design
decision with no other home belongs here. For a repository design, keep the
full reasoning and alternatives in `4_decisions.md` and record only a short
Scribe pointer with `--related doc:<repo>/<path>#<anchor>`.

When Daniel decided, record his exact words and only a reason he actually
gave:

```sh
scribe entry add --tag decision --author <model> --project <project> \
  --decided-by user --quote '<his words>' \
  --related session:<where he typed> --related task:<record> \
  [--option '<option>' ... --recommended <n>] [--reason-quote '<his reason>'] \
  [--supersedes decision-<n>] [--settles open-item-<n>] \
  '<what was decided, and its scope>'
```

When an agent decided, record its session name and rationale:

```sh
scribe entry add --tag decision --author <model> --project <project> \
  --decided-by <session-name> --rationale '<why>' \
  --related session:<id> --related task:<record> \
  [--option '<option>' ... --recommended <n>] \
  [--supersedes decision-<n>] [--settles open-item-<n>] \
  '<what was decided, and its scope>'
```

Use `--decided-by user` only for Daniel's decision. His quote must be a verbatim
span of one of his typed turns in a related session on this host; a quote under
eight characters must be the whole turn. Include at least one `task:`, `pr:` or
`repo:` reference for the record.

For a user decision, also include the `session:` where Daniel typed it. If
Scribe cannot find his quote, record the decision on its task instead. Do not
supply `--rationale` for a user decision. Add `--reason-quote` only when Daniel
stated a reason, using his exact words.

An agent decision requires a rationale. Options are optional; when shown,
record each option in full and give the 1-based recommended option. Decisions
are limited to 3,000 characters across the content, options, rationale and
quotes.

To reverse a decision, write a new one with `--supersedes decision-<n>` for
the active decision it replaces. Reversing a decision Daniel made, or a trial
decision with no typed decider, requires `--decided-by user` and his quote.

The old entry stays readable with `entry show`, but is marked superseded and
is no longer current. `entry update` can correct content, options and an
agent's rationale; it cannot change the decider, quotes or `--supersedes` link.
Retire a record with one of those wrong fixed fields and write it again.

## Boundaries

- Keep Orbit decision requests in Orbit; do not copy them into open items. An
  answer in Scribe does not approve an action in another session.
- Write decisions and open items intentionally. Do not infer or mine them from
  earlier conversations or session-pass suggestions.
- Keep a task-specific choice on its Orbit task. Keep a repository design
  decision in its `4_decisions.md`; use Scribe only for a searchable pointer.
