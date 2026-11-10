# scribe — Orbit plugin

The Orbit side of scribe ([design](../../docs/design/scribe/2_design.md#1-boundary-and-surfaces)):
one MCP tool, `scribe.ask`, answering from Daniel's `#scribe` entries, his
session records and Orbit tasks, with every quote checked verbatim. The
backend is the installed `scribe` binary (`bin/scribe-plugin` execs
`scribe plugin call`); this tree holds only the manifest, schemas and tests.
The Claude Code hooks that write entries are a separate package,
`plugin/claude-code/`.

| Tool | Kind | MCP | State |
|---|---|---|---|
| `scribe.ask` | read-only | `global` | live |
| `scribe.session-pass` | mutating, not agent-callable | none | live; routine ships disabled, runs every 5 minutes once enabled; also runs by hand |
| `scribe.consistency-scan` | mutating, not agent-callable | none | live; the `scribe-consistency` routine runs it |

`routines/` and `jobs/` hold the two routines, seeded by `orbit plugin
enable` with `enabled: false`: `scribe-session-pass` (`*/5 * * * *`, the
records of sessions whose pass is due, as `scribe session pass` writes them;
enabled on each host after review; Daniel can also run it by hand where
`claude` is on `PATH`) and `scribe-consistency` (`0 6 * * 1`, proposals,
as `scribe consistency scan` opens them). Each runs one job step that
calls its tool. A call starts no later session or window after 50 s and no
batch after 170 s, to end inside Orbit's 300 s limit; the rest waits for the next run. No tool settles a
proposal: that is `#scribe approve|reject` in a typed prompt.

## Install (operator)

From a checkout of this repository, on the host whose `~/.scribe` holds the
store:

```sh
make install                      # puts scribe on PATH
scribe question ask "can you fetch me last session summary?"   # the CLI works first
orbit plugin validate plugin/.orbit-plugin --render
orbit plugin add plugin/.orbit-plugin
orbit plugin enable scribe --grant fs,network,env_pass,orbit_tools
```

An install after a plugin change runs:

```sh
claude plugin marketplace update scribe-local
claude plugin update scribe@scribe-local
orbit plugin upgrade scribe plugin/.orbit-plugin --grant fs,network,env_pass,orbit_tools
```

Then set the store root and the transcripts directory in the workspace's
Orbit config and check one call:

```toml
[plugins.scribe]
root = "/home/<user>/.scribe"     # absolute; every tool answers not_configured until set
transcripts = "/home/<user>/.claude/projects"   # absolute; session-pass needs it
# task_workspaces = ["ws_constellation"]   # optional: where orchestration tasks live
```

```sh
orbit tool run scribe.ask --input '{"question":"can you fetch me last session summary?"}'
```

(`orbit config set plugins.scribe.root <path>` sets a key the same way.)
The routines ship disabled so the operator can review each host's setup,
following Orbit's convention. `orbit routine list` shows them; enable the
session routine with `orbit routine enable scribe-session-pass` once the
store and transcripts are configured and `claude` is reachable from the
Orbit clock's `PATH`. Once enabled it runs every 5 minutes. Daniel,
2026-11-10: "perhaps routine was not a bad idea then? let's add that back".
The Mac enabled it that day; the shipped default remains disabled.

Also run `scribe session pass` by hand (due sessions, five per run) or
`scribe session pass --session <id>` (a named session, due or not, ignoring
retry backoff). New messages run in consecutive windows, default 40
messages / 15000 characters, configurable via `[catalog] window_messages`
and `window_chars`. Every valid window checkpoints the record; a failure
keeps that progress and the next run resumes at its failed window.
Automatic retries wait 5, 10, 20, 40, then at most 60 minutes while the
session stays due; a completed pass resets backoff. JSON adds `windows`
per session (persisted this run), and `remaining` includes sessions waiting
for backoff or the routine's time budget. On macOS, verify `claude` is
reachable from the clock's environment before enabling the routine.
After a plugin change, refresh the package and upgrade the plugin:

```sh
claude plugin marketplace update scribe-local
claude plugin update scribe@scribe-local
orbit plugin upgrade scribe plugin/.orbit-plugin --grant fs,network,env_pass,orbit_tools
```

(or `claude plugin install scribe --marketplace ./plugin` on a fresh install) so its
`SessionStart` hook is registered.

The grants are: `fs` (read and write the store root, read the transcripts
directory), `network` (the `claude` CLI), `env_pass`
(`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`), and `orbit_tools`
(`orbit.search`, `orbit.task.show`, read-only). The `root` default,
`/var/tmp`, means unset: Orbit grants writes only to a directory that
exists, and scribe never opens a store there. If the store's
`config.toml` names extra variables under `[redact] env`, add those
names to `env_pass` in `plugin.yaml` as well: a routine that cannot see
a configured redact name's value refuses (`redact_env_unset`) rather
than write text it was told to mask.

## Tests

```sh
cargo test -p scribe --test plugin        # envelope behaviour and the manifest contract
PATH="$PWD/target/debug:$PATH" orbit plugin test plugin/.orbit-plugin \
  --grant fs,network,env_pass,orbit_tools  # the YAML goldens
```
