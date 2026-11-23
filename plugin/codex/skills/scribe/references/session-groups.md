# Session groups

Contents:
- [Group commands](#group-commands)
- [Project groups](#project-groups)
- [Corrections](#corrections)

A group is a named piece of shared work across the sessions on this host. Its
id is `g-<n>`. Use its id, a unique prefix, or a project group slug wherever
a command takes a group selector.
For open, join, leave and close in Codex, put `--provider codex` before
`group` so scribe can find the calling session.

## Group commands

- **Open.** `scribe group open --title '<title>' --kind <kind> [--project <project>]
  [--related <ref>] --quote '<their words>'`, with `--related` repeated for each
  task, PR or repository. Open a group only when the human asks: the quote must
  be a verbatim span of one of the human's typed turns in this session.
- **After opening.** Opening warns about any other open group in the project.
  The opener joins separately to take a seat.
- **Join.** Run `scribe group join <id> --as <seat> --author <name> [--role <role>]`
  in the session that joins, under its own `--author`. Join when the human names
  the group and seat, or when the brief that started the session names them. A
  new seat takes `--role`, which defaults to the seat name and then stays fixed.
- **Project group seat.** For a project group, omit `--as` and `--role` to take
  the automatic seat:
  the 8-character short session id, then the full session id, then the full
  session id with a numeric suffix. It never displaces a seat already held
  in the group. If this session already holds a seat, `group join` without
  `--as` keeps that seat. The role is `member`.
- **Seat changes.** A session holds one seat at a time; joining the seat it already holds changes
  nothing. A seat held by another session is refused (`seat_occupied`) unless
  its holder has been idle for a day or, in Claude Code, this session was cleared
  from that holder. Pass `--replace`, which takes the seat from its holder, only
  when the human says so. A closed group takes no members.
- **Leave.** `scribe group leave` takes the calling session off its seat and
  drops the seat when nobody else holds it. Run it when the human asks. It also
  deletes this session's turns held only on group consent (turns it also
  catalogues stay) and clears the group's closing summary if it has one.
- **Seats across a refresh.** In an interactive Claude Code session, a start
  after a clear takes over the seat of the session it was cleared from, under
  that session's author, when that session was the seat's newest transcript and
  the group is still open. The new session holds the seat from then on, and the
  old one holds nothing. Its start context says which seat it holds. The start
  does not switch recording on.
- **Missing seat after a refresh.** If the predecessor is unknown, ambiguous or
  holds no open seat, nothing is inherited and the context says to re-run
  `scribe group join`. Codex inherits nothing. A new chat joins again, and
  until the old transcript is a day idle that join needs `--replace`, which
  the human must ask for.
- **Stored turns.** While the group is open, a member's typed turns are stored
  from the time they join. Explicitly adding a member with `/scribe group add`
  also backfills eligible older typed turns. Stored turns contain the human's
  words only and are group-only: no memory pass reads them and they make no
  catalog entry, since a session pass reads the transcript, not these rows.
- **Capture exclusions.** Scribe excludes assistant turns, harness and
  `/scribe` control lines, pasted blocks, fenced code, and turns with a
  finance or sensitive-event word. It also excludes peer, agent and
  scheduled messages, including `<teammate-message>`, `<agent-message>`,
  `<scheduled-prompt>`, task notifications and any turn with an agent origin.
- **Capture mechanism.** Scribe redacts stored text. Storing takes no model call
  and creates no session record or switch.
- **Reading.** `scribe group show <id> --json` gives one group: its title, kind,
  project, times, related references, closing summary if closed, and each seat's
  role, its transcripts in join order and its newest handoff id.
- **Roster.** `scribe group list --json` lists groups newest first with their rosters. The
  brief's `## Last group` names the project's newest group. Read a handoff
  with `scribe entry show <id>`.
- **Scoped reads.** Limit reads to member sessions with `--group <id>` on
  `search`, `ask` (also `question ask`), `entry list` and `session list`.
  Group-scoped asks use matching stored turns and skip Orbit tasks.

  ```sh
  scribe search "<words>" --kind message --group <id> --json
  scribe --provider <provider> ask "<question>" --group <id> --json
  ```
- **Citing.** Cite a group as `g-<n>`. Its closing summary is an account the
  model wrote at close, not the human's words. Quote the human only from a
  stored-turn hit titled `(user)`, citing its `session:<id>`.
- **Close.** Run `scribe group close <id>` from the session that opened the
  group or from the session that now holds the opener's seat (its newest
  transcript, after a clear or a `--replace`), and only when the human asks. This runs a model call.
- **Close with a quote.** From any other session, and only when the human asks
  in their own typed words, run `scribe group close <id> --quote '<span>'`. The
  span must be a verbatim turn the human typed in this session and must contain
  the group id (`g-<n>`), not a slug. The call closes the group without taking a
  seat. Otherwise it is refused.
- **Closing summary.** Scribe sends up to 16 of the members' newest active
  handoffs and 32 newest stored user turns to the ask model in one call. It then
  writes the summary (up to 4,000 characters, redacted), naming the group's
  related references as work missing from the handoffs, and the closed time. A
  closed group takes no members and stores no more turns. Closing a closed group
  returns its record without another model call.

## Project groups

- **Typed join.** Have the human type `/scribe group <slug>` to join the open
  project group, or create and join it when absent. In Codex they may type
  `$scribe group <slug>`, a skill mention link
  `[$scribe](/<path>/scribe/SKILL.md) group <slug>`, or `/scribe group <slug>`.
- **Slug.** A slug is 1–64 bytes of lowercase ASCII letters, digits and
  interior hyphens (not a `g-<n>` id). The group uses the slug as its title,
  `project` as its kind and the working directory's project. Each typed join
  records under author `user`.
- **Automatic seat.** When the session needs a new seat, Scribe tries its
  8-character short id, then its full session id, then its full session id with
  a numeric suffix, using the first name not already held in the group. It never
  displaces a held seat; the role is `member`.
- **Agent join.** An agent joining a project group with
  `scribe group join <slug> --author <name>` (prefix `--provider codex` before
  `group` in Codex) can omit `--as` and `--role` to take those defaults.

To add past sessions, give the human `/scribe group add <slug> <session-id>...`
to type, naming the sessions to add. It creates the project group when absent
and adds only the named sessions; the session that typed the line is not
enrolled unless it is named. A named session's typed turns are stored only
when its transcript is on disk; otherwise only membership is added.

If any named session already belongs to another group, the whole line is
refused and nothing changes. This is a human-typed prompt line with no terminal
command or agent route, so the agent gives the human the line to type.

A second session joins itself to the existing open group by typing
`/scribe group <slug>` there. A `group add` line joins only the sessions it
names. A session in a group must leave before joining another. Peer, scheduled
and harness prompts cannot give consent. Clear-time seat inheritance also
applies to project groups in Claude Code. The group stays open until
`scribe group close <slug>` from its opening session or the current holder of
its seat.

A quoted close, described in the Close with a quote bullet above, names the
group's `g-<n>` id, not the slug.

## Corrections

For corrections, have the human type `/scribe group move <session-id> <slug>` to move a session (stored turns follow), or `/scribe group remove <session-id>` to remove it (group-only turns are erased). To relabel a recorded session, have the human type `/scribe session update <session-id> project <slug>` or `/scribe session update <session-id> title <text>`. These are typed lines only; agents cannot act for the human.
