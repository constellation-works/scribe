---
type: design
summary: "Spec: catchers — the [tags.<tag>.capture] fields (from, route, triggers, criteria, not, max_open), how the session pass applies them, the shipped preferences and standard catchers, and example catchers (orbit, todo, issue, idea) that are config only"
last_validated: 2026-11-21
---

# Spec: Catchers

A tag is a **catcher** when its `[tags.<tag>.capture]` table in
`config.toml` says what the session pass catches from a session and what a
catch becomes ([2_design.md §15.2](../2_design.md#152-tags-are-catchers)).
A tag without that table still holds entries, typed with `#scribe` or
written by an agent, but the pass catches nothing under it. A new catcher
is config only: no code change, no format step.

`scribe tag show <tag>` prints each field's effective value and its origin
(`shipped`, `config` with the file and line, or `none`).

## Why This Exists

Phase 2 of §15.9 made the shipped set two catchers, `preferences` and
`standard`, and moved `idea` and the issue record kind to config examples
(Daniel: "it should be a tag"; "No, example only"). This page is where those
examples live and where the fields are defined for anyone writing their
own.

## Fields

```toml
[tags.preferences]                # a tag's own settings, as before
aliases = ["preference", "pref"]
description = "A standing rule {user} wants agents to follow."
stale_after_days = 180

[tags.preferences.capture]
from = "user"                     # user | assistant | both
route = "propose"                 # propose | write
triggers = ["always", "never", "should not", "from now on", "I prefer", "by default"]
criteria = "A standing rule from {user} that applies beyond the work at hand."
not = "A choice about one page or task; an acknowledgement; a one-off command."
max_open = 10
```

| Field | Values | Default | Meaning |
|---|---|---|---|
| `from` | `user`, `assistant`, `both` | the shipped value; none for a new tag | Whose turns the catcher reads. `daniel` still reads as `user`, with a warning, for the 30-day compatibility window. |
| `route` | `propose`, `write` | the shipped value; none for a new tag | `propose` opens a claim the user settles with `#scribe approve` or `#scribe reject`. `write` has the pass write the entry itself, authored by `scribe:session-pass`, with no claim: a **collector**. |
| `triggers` | up to 32 phrases, each at most 120 characters, words separated by spaces; `*` stands for one to four words between two words | the shipped list; none for a new tag | A turn reaches the model under the tag only when one occurs in it, matched on word boundaries, case-insensitive. The list replaces the shipped one whole. |
| `criteria` | at most 300 characters | the shipped text; none | What qualifies, given to the model as the tag's definition. |
| `not` | at most 300 characters | the shipped text; none | What does not qualify. |
| `max_open` | 1 to 20 | 10 | The most open claims the tag may hold. |

Each field overrides its shipped default alone. `{user}` in `description`,
`criteria` and `not` stands for `[user] name`, else "the user".

A tag catches only when it has `from`, `route` and `triggers`: a table with
triggers but no `from` or `route` is refused as `config_invalid` ("needs
from, route"). Other values outside the table above are refused as
`config_invalid` and name the key and line.

## How the pass applies them

1. **The gate.** For each turn after `scribed_through`, the pass keeps a tag
   only when the turn's speaker is one the tag's `from` reads and one of its
   triggers occurs in the turn's own words: the user's typed text without
   pasted data or `#scribe` lines, or the assistant's reply text. The
   request lists each kept turn with its
   speaker, tag, route and the trigger it fired, and the model may return a
   claim only for a listed turn under a listed tag.
2. **The check.** Each claim's quote must lie verbatim in that turn, and the
   claim records the speaker (`candidate.role`). Under the store lock the
   pass checks the trigger again against the config of that moment, so a
   catcher edited while the model ran refuses what it no longer catches.
   `ask` and the brief never present an assistant quote as the user's
   words.
3. **The bounds.**
   - `propose`: at most 3 claims per pass across tags, `max_open` open
     claims per tag, and 20 open claims store-wide.
   - `write`: at most 3 entries per tag per run (`MAX_COLLECTED`), after the
     shared duplicate checks; the tag's existing entries go to the model as
     exclusions. A write-route claim carries an `outcome` (`open`,
     `parked` with the user's words, or `task`); one that became a task in
     the session writes nothing.

A typed `#scribe` marker is the user's own consent to one entry, so it
opens no claim under any route, and may name any tag, catcher or not.

## The shipped catchers

| Tag | From | Route | Triggers | Qualifies | Not |
|---|---|---|---|---|---|
| `preferences` (aliases `preference`, `pref`; "A standing rule {user} wants agents to follow."; stale after 180 days) | user | propose | "always", "never", "should not", "from now on", "I prefer", "by default" | a standing rule from the user that applies beyond the work at hand | a choice about one page or task; an acknowledgement; a one-off command |
| `standard` (`action = true`, `require_quote = true`; "A rule for how code or docs are written across {user}'s projects, in {user}'s words."; never stale) | user | propose | "as a standard", "our standard", "every repo", "all repos", "across repos", "house style", "convention" | a rule for how code or docs are written across the user's projects | a rule for one repository or one change; a preference about how agents talk or work |

## Example catchers

These are documentation, not shipped config. Paste one into `config.toml`
to use it. `crates/scribe/tests/session_pass.rs` reads each block below
from this page and runs it through a pass, so the examples stay working.

```toml
# Every mention of a project, collected without approval.
[tags.orbit]
description = "Anything {user} says about Orbit."
[tags.orbit.capture]
from = "user"
route = "write"
triggers = ["orbit"]
criteria = "{User} says something about Orbit: a plan, a problem, a judgement."
not = "Orbit named only as a path or a command."
```

```toml
# Work an agent left for the user to do by hand.
[tags.todo]
description = "Something an agent said {user} must do."
[tags.todo.capture]
from = "assistant"
route = "write"
triggers = ["you need to", "you'll need to", "you will need to", "left for you", "you must", "manual step", "todo"]
criteria = "The assistant names a step only {user} can take (a login, an approval, a purchase, a decision)."
not = "A step the assistant will take itself; an option {user} may take."
```

```toml
# Incidents: what went wrong, its cause and its fix, from either side.
[tags.issue]
description = "Something that went wrong, with its cause and fix when known."
[tags.issue.capture]
from = "both"
route = "write"
triggers = ["root cause", "rca", "look into why", "can you investigate", "the cause was", "caused by", "fixed by"]
criteria = "A failure in {user}'s tools or work, with what caused it or what fixed it, or {user} asking for that cause."
not = "How something works; a feature request; a failure the turn only names in passing."
```

```toml
# Ideas: the rules `idea` shipped with until phase 2.
[tags.idea]
action = true
[tags.idea.capture]
from = "user"
route = "write"
triggers = ["have * an idea", "had an idea", "here's an idea", "what do you think about * idea", "what if we"]
criteria = "{User} puts forward something to consider, not an instruction to do it now."
not = "A direction to act now; a preference; a question of fact."
```

## Removed in phase 2

- `route = "agent"` and the prompt hook's root-cause line. A table that
  still says `agent` (or a `[tags.issue.capture]` table with no route)
  loads with a warning and catches nothing; set `route = "write"` as in
  the `issue` example. It is refused after the 30-day window.
- The issue record kind and the `scribe issue` noun (`add`, `occur`,
  `update`, `move`, `search`, `show`, `list`, `stats`), `#scribe forget
  <issue-id>[#<n>]`, `scribe search --kind issue` and `ask`'s issue
  evidence. `issue` is no longer a reserved tag name.
- `idea` and its action settings no longer ship. Stored `idea-<n>` entries
  stay listed, searched, cited and settleable by id, as `decisions` and
  `task` entries do.

## Stored issues become `issue` entries

The first write that takes the store lock converts every
`issues/issue-<n>.md` to one entry under `issue`, with no model call
(§15.11):

| Entry field | From the issue |
|---|---|
| id | `issue-<n>`, the issue's own number, when free; else the next free `issue-<m>` |
| content | the title, then `Cause: …` and `Fix: …`: the header's when known, else the newest occurrence's that found one, else `cause not found` / `fix not found` |
| quote | the first occurrence's verified request, word for word |
| related | the issue's links, then each occurrence's `session:<id>` |
| author, created, updated | the agent that wrote it, and its times |
| project, host | the first occurrence's |
| cite | none: an issue kept no prompt hash, so the request's session is in `related` instead |

A forgotten issue has no text left and writes nothing. Each file then moves
to `pre-issue-<YYYY-MM-DD>/` in the data root. A file that does not parse
stays in `issues/` with a warning and is tried on the next write; while any
entry is unreadable, nothing converts. A run that finds the entry already
written (same tag, content and quote) only moves the file, so running it
again changes nothing. Stored `issue` entries read like any other tag's
entries with or without `[tags.issue]` in config.
