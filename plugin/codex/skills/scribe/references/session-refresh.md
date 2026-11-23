# Refreshing a session

A refresh replaces this session's context with an empty one in the same
session. Run one when the human asks ("refresh continue", "refresh new"),
when the session that started you asks, or when instructions you follow
tell you to start one. The mode decides what the next context reads:

- `continue`: the next context starts with your handoff and carries on.
- `new`: the next work has no bearing on this; the handoff is this
  context's account, and the next context gets the ordinary brief.

With no mode named, ask which. Then, as the last acts of your turn:

1. **Write the handoff** under your own author and project with the selected mode.
   Omit `--quote` when `CLAUDE_CODE_SESSION_ID` matches the related session
   and a typed user turn is in its transcript or its verified clear ancestry
   (at most 16 predecessors). Peer and scheduled prompts do not count as typed
   turns, so a session a peer restarted after a clear is checked through its
   clear ancestry. Elsewhere pass `--quote '<their request>'` with a verbatim
   span of at least 8 characters from a typed turn in this session.

   ```sh
   scribe entry add --tag handoff --author <name> --project <project> \
     --related session:<this session id> --mode <continue-or-new> \
     '## State
   ...
   ## In flight
   ...
   ## Next
   ...
   ## Waiting on you
   ...
   ## Do not touch
   ...
   ## Pointers
   ...'
   ```

   Keep the six headings in order and the body within 3,000 characters.
   Put `(none)` under empty headings. For `continue`, write for a reader
   with no context. Give measured **State** with ids, start **Next** with the
   first act, and name every brief, record and session id to read in **Pointers**.

   If the write is refused, stop and report the refusal before clearing anything.
2. **Report** the handoff id (`scribe: wrote <id> …`) to whoever asked,
   with anything the next context must not miss.
3. **Clear.** Once the request and mode are explicit, call the Claude desktop session tool
   `clear_session` on this session (`self`). It clears when the turn ends,
   so run nothing after it and say nothing that must survive. If the call
   is refused or denied, say so and ask the human to type `/clear`.
   Elsewhere ask for `/clear` (Claude Code) or a new chat (Codex).

A cleared session waits for its next message; whoever asked sends it.

**Starting after a refresh.** After a desktop clear with a `continue`
handoff, scribe's `SessionStart` hook puts the handoff body (from
`## State`) before `# scribe brief`, carries the recording switch, and
links the sessions with `continues`. Treat it as the previous context's
account: read its Pointers, check each **In flight** item's current state,
then do **Next**. Don't type `/scribe on` again.

When nothing was restored
(the hook said `could not continue this session` or listed possible
predecessors, or it said nothing after a clear), the human types
`/scribe continue <handoff-id>`, which supplies the body with that prompt.
After `new`, or in Codex, nothing is added: the brief lists the previous
session with `[handoff: <id>]`, and step 4 of the session start reads it.
