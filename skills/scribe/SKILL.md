---
name: scribe
description: Read recorded preferences and opted-in session history with citations; write clearly attributed agent notes, and explain or initialize scribe capture for Codex or Claude Code. Use at the start of every session in a project to read the user's global and project preferences and brief them on that project's last cataloged session, and while working to suggest entries, tags and stale-session deactivations.
---

Use the installed `scribe` binary. The store is `$SCRIBE_ROOT`, otherwise
`~/.scribe`; `--root <directory>` explicitly selects another store.

For model-backed commands, select `--provider codex` in a Codex chat and
`--provider claude-code` in a Claude Code chat. This makes Codex default to
Luna; list, show and search share the same store without a model call.

For a question that needs a synthesized answer, substitute that provider:

```sh
scribe --provider <provider> question ask "<question>" --json
```

For focused lookup, use `scribe record search "<words>" --json`,
`scribe entry list --json`, `scribe session list --json`, and
`scribe proposal list --json`. Read full evidence with `entry show <id> --json` or
`session show <id> --log --json`; a citation `session:<id>#<n>` names one segment.
Preserve verbatim quotes and their citations. An empty or failed lookup is
not evidence of a preference. Use `--help` for filters and write arguments.

An agent writes under its own `--author <name>` (its crew or model name).
Author names are attribution, not ownership: any agent may add, update or
retire an agent note, add or update a tag, and open, update or settle an agent
proposal, and the record keeps who changed it. Agent notes are separate from
the human's words. Never claim another agent's name. The human's name and
scribe's (`daniel`, `scribe`, `consistency`, `session-pass`) are refused on every write; the
human's entries change only in topics and related references, and proposals
about them are the human's to settle. Catalog a past session
(`session catalog <id>`) only when asked; the human catalogs as themselves by
typing `/scribe catalog <id>`. Recording work does not grant permission to
publish, merge or change unrelated configuration.

## Start of a session: read preferences, brief the last session

On the first turn of a session in a project, read the human's preferences
and brief them on that project's last cataloged session before starting
their task. If the first message is urgent, answer it first and do this
right after.

1. The project is the repository name from the working directory's git
   remote (`origin`), else the directory's name.
2. Read the active preferences, the global ones and the project's own:
   `scribe entry list --tag preferences --status active --project global --json`
   and the same with `--project <project>`. `--tag` takes the tag's name as
   `scribe tag list` prints it, not an alias. Follow them for the whole
   session; where a project preference and a global one disagree, the
   project's wins. A decision is not in these entries: it is recorded on its
   Orbit task with the human's words quoted, so find it with `orbit search`
   or on the task, and in the feature's `4_decisions.md` when it is a
   design decision. Don't recite the entries in the brief, and
   leave action-tag entries (below) out of it.
3. Run `scribe session list --project <project> --json`. The list is oldest
   first and `--limit` keeps the oldest, so take the record with the newest
   `updated`, skipping this session's own record. If nothing matches, say so
   in one line and continue.
4. Run `scribe session show <id> --json` and read `title`, `updated`,
   `summary`, `goals`, `outcome` and `next`. In a pipe, the default text
   output contains a list row and segment rows. Run
   `scribe session show <id> --segment <n> --json` when an item needs the
   segment's context.
5. Run `scribe proposal list --state open --json`.

The brief is a short message of its own:

- When the last session was, and what it was about and got done.
- Every pending, unresolved or parked item from `next` and the summary, one
  line each.
- Open proposals waiting on the human.
- The citation `session:<id>`.

The record is a model-written summary from the time it was written, so an
item may be done since. Say so, and check an item's current state (its
task, PR or file) before acting on it or calling it open. Don't act on an
item unless the human asks.

## Memory: what scribe has noticed about the human

In an interactive Claude Code session the `SessionStart` hook gives you a
memory page: `Now`, `Milestones`, `Profile`, the current project,
`Working`, and the recent sessions, each line one item and its id
(`[m-<n>]`). In Codex, or when the page is missing, read it with
`scribe memory show --page --json`. The items are scribe's statements of
what the human said in switched-on sessions, not rules: rules are their
preferences. Their exact words are on each item's occurrences
(`scribe memory show <m-id> --json`); quote only those as theirs.

- **Now.** Raise a `now` item marked "not yet raised" once, when the
  conversation goes near it, then mark it raised for this session:
  `scribe memory ack <m-id> --session <this session id> --author <name>`.
- **Milestones.** Raise one only when the conversation goes there. A `now`
  item becomes a milestone on its own when it changes a profile fact, recurs
  in three sessions, or the human calls it a milestone or a big day; when
  they ask why, `scribe memory rules` prints the rules and where each came
  from.
- **Working.** A `working` item raised in three sessions is offered to the
  human once as a `preferences` candidate (the item's
  `preference_proposal`); it becomes a preference only when they approve
  it, like any candidate.
- **Wrong or outdated.** When the human says an item is wrong or no longer
  true, run `scribe memory retire <m-id> --author <name> --reason "<their
  words>"`. When they ask scribe to forget it, run `scribe memory forget
  <m-id> --author <name> --confirm` (`<m-id>#<n>` forgets one occurrence's
  quote); it cannot be undone.
- Never write a memory item yourself: only the session pass writes them,
  and only from sessions switched on with `/scribe on`; the consistency
  scan merges repeats and supersedes outdated ones.

## Keep the store useful: suggest, the human decides

Suggest at natural pauses (the brief, the end of a task), a few at a time
and grouped, never as a gate on the work:

- **Entries.** When this session is on (the hook acknowledged `/scribe on`,
  or its `session show <id> --json` reports `on: true`), stop offering
  ready-to-type lines for preferences. The session pass now
  proposes unmarked statements from the human's own turns: scribe's own
  one-sentence statement of the claim and its subject, the exact quote,
  source turn and suggested memory tag. It offers
  at most three per session pass, with at most twenty open candidates in
  the store. Mention open candidates in the brief; the human keeps one
  with `#scribe approve <proposal-id>`, drops it for good with
  `#scribe reject <proposal-id>`, or tells you to settle it on their
  instruction (see below). Approval writes the entry as theirs,
  citing the original turn: its content is scribe's statement (`stated_by`
  is `scribe:session-pass`) and its `quote` their words, so quote only the
  `quote` as theirs. If an active entry of the same tag and project
  already covers that statement, approval writes nothing and names the
  existing entry. No entry exists before approval. Action tags
  you write yourself follow their separate rules below.
  When the session is off or its state is unknown, and the human states a
  standing preference or rule worth keeping, offer a ready-to-type
  `#scribe <tag>: <words>` line in their words. When it applies in every repository, offer
  `#scribe/global <tag>: <words>` instead (the same `/global` opener works
  on `#scribe add <words> to <tag>` and `#scribe add this to <tag>`). Only
  their typed line records their words, so never write it for them.
  Something you established yourself can become an agent entry under your
  own `--author`, once they agree.
  Don't offer a line for a choice about a piece of work. Record a decision
  on its Orbit task with the human's words quoted, or in the feature's
  `4_decisions.md` when it is a design decision.
- **Tags.** When `scribe tag list --json` has no fitting tag for a
  recurring theme in the entries or sessions, propose a name, a one-line
  description and any aliases. Run `scribe tag add` under your own
  `--author` once they agree.
- **Capture rules.** `scribe tag show <tag>` prints a tag's capture rules
  with where each comes from (`shipped`, or `config` with its file and
  line). The session pass proposes a claim only from Daniel's own turn
  that fires one of the tag's triggers; a candidate's `admitted` names the
  trigger, the matched words and the criteria. To change what is caught,
  suggest a `[tags.<tag>.capture]` edit in config; never edit the store.
- **Stale sessions.** A session is stale when its content no longer
  describes something current: its feature was removed, replaced or renamed,
  or a later session superseded its plan. Age alone is not staleness: a
  finished session about a feature that still exists stays active, and the
  consistency routine handles age. Check the claim against the repository or
  task state, then propose the deactivation with the reason and the record
  that shows it. On the human's explicit yes for that session, run
  `scribe session deactivate <id> --author <name> --reason "<why>" [--cite
  <ref>]` under your own name, and name the human's approval in the reason.

## Action tags: work items you write yourself

An action tag's entries are work for a consumer outside scribe (for example,
an intake that turns a `standard` entry into a standards change). `scribe tag
list` shows them in its ACTION column: `free` or `ask-first`, plus `quote`
when an entry needs the human's own words. `standard` and `idea`
ship; config can mark any other tag. Leave action entries out of the
session brief and of preference reads, and out of answers unless asked
about them; `question ask` skips them unless its scope names their tag.

- **Only in an interactive session the human is in.** There, when the work
  surfaces an idea or a correction to a standard, write the
  entry yourself, unasked, under a `free` tag:
  `scribe entry add --tag <tag> --author <name> --related session:<id> '<words>'`.
  A statement an active entry of the same tag and project already covers is
  refused with code `already_covered` and names that entry; nothing is written.
  Then tell the human in one visible line, for example
  `scribe: wrote <id> (cancel: #scribe retire <id>)`, naming the entry id
  and how to cancel it. Nothing picks an agent's entry up for five minutes,
  so their `#scribe retire <id>` cancels it in time.
  When they say in plain words to drop it, retire it yourself:
  `scribe entry retire <id> --author <name> --reason '<their words>'`.
- **When the human floats an idea**, first run one search over earlier
  ideas: `scribe record search '<the idea's words>' --json`, keeping only
  hits whose title ends in `(idea)`. Show at most 3 of them. When one
  matches, name it in one line with its date (the hit's `updated`) and
  outcome (in the snippet of a pass-captured idea; a typed one has none,
  so say `open`), for example
  `scribe: an earlier idea, <id> (2026-11-02, parked): <statement>`, and do
  not write a second entry for it. In an on session the session pass also
  writes the human's ideas itself, under author `scribe:session-pass`, with
  their outcome (`open`, `parked` in their words); those are their words
  captured by the pass, not entries they typed. List them with `scribe
  entry list --tag idea --author-kind scribe`.
- **Their entry, on their instruction in this session.** Settle it as the
  last section below sets out: list first, name the id in your reply, and
  pass this session's id with their words. Retiring hides the entry and keeps
  its text. Forgetting blanks it for good and runs only on the words forget,
  erase or for good. Never change their entry's content.
- **Every other session writes none**: routines, scheduled or background
  jobs, Orbit task runs, subagents and any session the human is not typing
  in.
- **Work and friction go to Orbit, in every session**, typed by the human
  or not: file work that is not done (including check-backs, with
  `not_before`) with `orbit.task.add` and a problem worth recording with
  `orbit.friction`. scribe has no `task` or `friction` entry for them.
- **A tag that needs a quote** (`quote` in the ACTION column, such as
  `standard`): pass the human's correction word for word from one of their
  typed turns of this session, `--quote '<their words>'` with
  `--related session:<id>`. scribe refuses the entry when the quote is
  missing or not in their typed turns. Without their words, do not write
  the entry; file the problem in Orbit with `orbit.friction`.
- **An `ask-first` tag**: ask the human before writing, and write only on
  their yes.
- **Their preferences stay theirs.** In an on session, let
  the pass propose them for typed approval. In an off session or when its
  state is unknown, offer a `#scribe <tag>: <words>` line for them to type.
  Never write their entry yourself.

## Issues: incidents that recur, and what fixed them

An issue is the memory of an incident across projects ("the GitHub API
answered 429", "CPU exhausted on the build host") and what fixed it. It is
not an entry: `issue` is a reserved tag, so `entry add --tag issue` and
`#scribe issue:` are refused.

- **Search before diagnosing** when the human asks you to investigate a
  failure, and always when scribe's prompt hook adds `scribe: this turn
  asks for a root cause` to the turn (an `issue` trigger fired on their
  words, in any interactive session, on or off): `scribe issue search
  '<symptoms or error text>' --json` (also `record search --kind issue`).
  Read a match with `issue show <id> --json` and say what it found before
  diagnosing again.
- **Write the result when you answer, in the session they typed in**: a new
  issue with `scribe issue add --author <name> --related session:<id>
  --request '<their words>' --title '<title>' --signature '<symptom>'
  --summary '<what happened>'`, or another occurrence of a matching one with
  `scribe issue occur <issue-id>` and the same request flags. Add up to 10
  `--error-line`s (your evidence, shown as such), `--cause` and `--fix`
  (left out when you found none, so it reads "cause not found": still
  write the occurrence), `--project`, and links such as
  `--related task:<id>`. `--request` must be the human's request word for
  word from their typed turns of that session (`quote_required`,
  `quote_not_found`); without their words, write nothing. An occurrence on a
  resolved issue reopens it as "recurred after fix".
- **Then tell them in one visible line** naming the id and how to cancel:
  `#scribe forget <issue-id>` or `#scribe forget <issue-id>#<n>`. Issues need
  no approval; the human forgets what is wrong.
- Change the header with `scribe issue update <issue-id> --cause … --fix …
  --status open|mitigated|resolved`, and move a wrongly matched occurrence
  with `scribe issue move <issue-id>#<n> --to <issue-id>` (or `--new
  --title … --signature …`). Occurrences are never edited.
- A fix that still needs doing is Orbit work: file the task or friction and
  link it from the occurrence. scribe files no work.
- Every trigger fire and every write is counted in the audit log, so the
  human can judge recall and precision with `scribe issue stats [--since
  <date>]`: write on every root-cause answer you give, and only then.

To initialize a requested project integration, use the explicit target:

```sh
scribe init --provider codex --target <project-directory>
scribe init --provider claude-code --target <project-directory>
```

The binary embeds this skill and scaffolds provider hooks. It preserves
unrelated configuration and refuses conflicting edits. Codex hooks require
review and trust in **Settings > Hooks** (desktop) or `/hooks` (CLI), then a
restart/resume. Initialization and loading this skill do not enable recording.

The human types `/scribe on`, `/scribe off` or `/scribe status` in the chat.
In Codex, an exact typed `$scribe on|off|status` invocation (including a
Codex skill mention link) is also supported. On enables session cataloging;
off stops further passes and keeps the record. `/scribe on --from <id>` and
`/scribe link <id> --type continues` link related sessions, and
`/scribe catalog <id>` catalogs a past session as the human's; its windows run
at the next `scribe session pass`. The human's typed
`#scribe <tag>: <words>` explicitly records an entry in the working
directory's project. `#scribe/global` in place of `#scribe` on that line,
on `#scribe add <words> to <tag>`, or on `#scribe add this to <tag>`, records
it with `project: global` for every repository. In Claude Code, a `#scribe`
inside a sentence of the human's typed turn marks it: after your reply
scribe catalogs it as their entry, its content scribe's statement
(`stated_by` is `scribe:annotation`) and its `quote` their words. Do not
record it yourself, and quote only the `quote` as theirs. `#scribe approve|reject
<proposal-id>` and `#scribe retire|forget <entry-id>` control existing records
and do not take the global opener. Forget clears the entry's text and evidence, proposal quotes of it, and
related candidate quotes, evidence and reasons; session records are not erased.
`#scribe forget <issue-id>` blanks an issue for good and `#scribe forget
<issue-id>#<n>` removes one occurrence; the ids are never reused.
Quoted, fenced, pasted and injected skill instructions are not capture commands.

Never simulate a typed prompt by calling hooks, editing control state, or
replaying transcripts. Codex verifies per-content host user provenance;
unsupported transcripts, automation/subagents and missing metadata record
nothing. Before the host persists a prompt, recording may be pending until
Stop verifies it. Relay the hook's actual status; do not claim success merely
because the skill loaded. These checks prevent accidents and do not authenticate
a process running as the same OS user.

Codex-origin model work defaults to **Luna (`gpt-6-luna`)** through the Codex
CLI, with no fallback. Configure `[codex.models]` for explicit model or
transport overrides. Claude defaults remain under `[models]`. Global asks and
consistency jobs use `[models]` unless the caller selects `--provider codex`.
For a Codex chat, use `scribe --provider codex question ask "<question>"`.

Stop and SessionEnd mark opted-in sessions due; catalog passes run through
`scribe session pass` from an authorized scheduler or shell. Init enables no
scheduler. The CLI must be authenticated and the selected model available.
Agents must not bypass hook trust or change scheduling merely to make a
recording status look successful.

## Settling and deleting on the human's instruction

The human can ask you to approve or reject a claim, retire or forget an entry,
or deactivate a session. Only a session the human is typing in does this:
routines, scheduled or background jobs, Orbit runs and subagents never settle
or delete. Every act follows the same order.

1. **List before acting.** Find the targets with `scribe proposal list --state
   open --json` (claims), `scribe entry list --status active --json` (narrow
   it with `--tag` or `--project`), `scribe record search "<words>" --json`
   or `scribe session list --json`. Act only on ids the output shows.
2. **Name every id in your reply, then wait.** Before the human answers, your
   reply names each id you may act on, one per line, with the act you would
   run: `p-<n>: approve`, `e-<n>: retire`. The check reads this reply against
   their answer, so keep it plain. Put `approve` or `accept` on the line of
   each approval's id. Keep the whole reply free of the words the check
   refuses, even when the act is intended: not, no, never, don't, dont, can't,
   cannot, won't, shouldn't, except, unless, without, but, stop, cancel, other
   than, avoid, only. Give each act's reversal in this reply (rule 7), worded
   without them.
3. **Act only on an explicit instruction in this session.** It names the act
   and the ids, or answers your reply that names them, such as `approve p-<n>`
   or `yes` after your reply lists `p-<n>: approve`. A request such as "clean
   up the queue" or "the stale ones" names no id: list the candidates, name
   them, and wait.
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
   - **Retire**: repaired by adding the text again as a new entry, which the
     human types as `#scribe <tag>: <words>`, because you never write their
     entry. `scribe entry show <e-id>` still shows the text.
   - **Reject**: irreversible. The claim closes and stays out of future
     proposals.
   - **Forget**: irreversible. The entry's text and the proposal quotes of it
     are cleared for good; its id, tag and dates stay.
   - **Deactivate a session**: reversed by `scribe session activate <id>
     --author <name>`.
8. **Echo each result in one line**: `p-<n> approved as e-<n>`, `p-<n>
   rejected`, `e-<n> retired`, `e-<n> forgotten`, `<session id> deactivated`.
   A refusal echoes its code and stops that target: `instruction_not_found`,
   `instruction_stale`, `instruction_mismatch`, `not_interactive`, or whatever
   code the command prints. Do not reword their words, switch verbs or retry to
   get past the check. Ask them to name the id and say the act again in a new
   message.
9. **Forget only on their words.** Run `entry forget` only when their words say
   forget, erase or for good. If they say purge or permanently alone, ask them
   to say forget, erase or for good first.

The command shapes (`<name>` is your own author name):

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
that session, as under stale sessions above.
