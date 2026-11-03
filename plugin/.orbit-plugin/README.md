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
| `scribe.session-pass` | mutating, not agent-callable | none | `not_implemented` until S6 |
| `scribe.consistency-scan` | mutating, not agent-callable | none | `not_implemented` until S6 |

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

Then set the store root in the workspace's Orbit config and check one call:

```toml
[plugins.scribe]
root = "/home/<user>/.scribe"     # absolute; ask answers not_configured until set
# task_workspaces = ["ws_constellation"]   # optional: where orchestration tasks live
```

```sh
orbit tool run scribe.ask --input '{"question":"can you fetch me last session summary?"}'
```

The grants are: `fs` (read the store root), `network` (the `claude` CLI),
`env_pass` (`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`), and
`orbit_tools` (`orbit.search`, `orbit.task.show`, read-only).

## Tests

```sh
cargo test -p scribe --test plugin        # envelope behaviour and the manifest contract
PATH="$PWD/target/debug:$PATH" orbit plugin test plugin/.orbit-plugin \
  --grant fs,network,env_pass,orbit_tools  # the YAML goldens
```
