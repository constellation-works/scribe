# Action tags

An action tag's entries are work for a consumer outside scribe (for example,
an intake that turns a `standard` entry into a standards change). `scribe tag
list` shows them in its ACTION column: `free` or `ask-first`, plus `quote`
when an entry needs the human's own words. `standard` ships; config can
mark any other tag (an `idea` tag, for example).

Leave action entries out of the
session brief and of preference reads, and out of answers unless asked
about them.

- **Only in an interactive session the human is in.** There, when the work
  surfaces an idea or a correction to a standard, write the
  entry yourself, unasked, under a `free` tag:
  `scribe entry add --tag <tag> --author <name> --related session:<id> '<words>'`.
  A statement an active entry of the same tag and project already covers is
  refused with code `already_covered` and names that entry; nothing is written.
- **Canceling your entry.** Tell the human in one visible line, for example
  `scribe: wrote <id> (cancel: #scribe retire <id>)`, naming the entry id
  and how to cancel it. Nothing picks an agent's entry up for five minutes,
  so their `#scribe retire <id>` cancels it in time.
- **Retiring your entry.** When the human says in plain words to drop it, retire it:
  `scribe entry retire <id> --author <name> --reason '<their words>'`.
- **When the human floats an idea** and the store has an `idea` tag, first
  run `scribe search '<the idea’s words>' --json` once. Keep only hits
  whose title ends in `(idea)` and show at most three.
- **When an earlier idea matches.** Give its `updated` date and its outcome
  from the pass-captured snippet. For a typed idea, say `open`.
  Example: `scribe: an earlier idea, <id> (2026-11-02, parked): <statement>`.
  Leave the existing idea as the record.
- **Pass-captured ideas.** In an on session a `write` catcher (such
  as `idea`) has the session pass write entries itself, under author
  `scribe:session-pass`, with their outcome (`open`, `parked` in their
  words); those are a turn's words captured by the pass, not entries anyone
  typed, and a catch from an assistant turn is the assistant's words. List them with `scribe
  entry list --tag idea --author-kind scribe`.
- **Their entry, on their instruction in this session.** Follow the
  settlement procedure linked from the skill. List first, name the id in your reply, and
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
- **Their preferences are recorded only when they ask.** Unprompted capture
  stays with the session pass and its proposals (in an on session). Never
  write a preference or memory entry unprompted; record one with
  `--spoken`, `--quote` and `--related session:<id>` only when the human
  explicitly tells you to, and do not ask them to retype with `#scribe`.
