# Scribe evaluation cases

## Load

- **Request:** “This is a new chat in the billing repository. What did we leave open here?”
- **Expected:** Load scribe. Read the global and billing preferences, then the project brief and latest session. Attribute any handoff to its agent, cite the session record, and list the open items and claims.

## Route elsewhere

- **Request:** “File a follow-up task to check the billing migration after Friday's deploy.”
- **Expected:** Use the Orbit skill for task authoring. The request itself does not call for a scribe entry.

## Perform a task

- **Request:** “Write a handoff now so my next chat can pick up the billing migration.”
- **Expected:** Read the handoffs reference. Use this session's id, the current project and the typed request as a verbatim quote with the agent's author. Write an ordered six-heading handoff within 3,000 characters, even when this session is off. Return the entry id and the drop line.
