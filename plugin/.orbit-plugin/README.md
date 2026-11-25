# scribe — Orbit plugin

The Orbit plugin keeps opted-in session records, proposals and memory current.
It uses `scribe plugin call` as its exec backend. Every tool has
`agent_callable: false` and `mcp_scope: none`.

| Job | Deterministic tools | Seeded routine |
|---|---|---|
| `scribe-session-pass` | `scribe.session-pass-prepare`, `scribe.session-pass-apply`, `scribe.session-pass-fail` | Every five minutes, disabled |
| `scribe-consistency` | `scribe.consistency-scan` | Mondays at 06:00, disabled |
| `scribe-memory-refresh` | `scribe.memory-refresh-prepare`, `scribe.memory-refresh-apply` | Sundays at 05:00, disabled |
| `scribe-group-close` | `scribe.group-close-prepare`, `scribe.group-close-apply` | None |

Settle proposals through typed `#scribe approve|reject` prompts.
[Plugin design](../../docs/design/scribe/2_design.md#1-boundary-and-surfaces)

## Install (operator)

From a checkout of this repository, on the host whose `~/.scribe` holds the
store. Orbit refuses a plugin source inside the repository its runtime resolves
(this checkout, or the workspace repository that contains it), so the add and
upgrade commands below install from a copy made outside it. The copy keeps
the `.orbit-plugin` directory name: Orbit reads a plugin only from a directory
named `.orbit-plugin`, and refuses a copy under any other name. Orbit copies
the installed tree into its global root, so the temporary copy can be removed
after the command.

```sh
make install                      # puts scribe on PATH
scribe brief                      # the CLI works first
orbit plugin validate plugin/.orbit-plugin --render
PLUGIN_SRC="$(mktemp -d)/.orbit-plugin"   # outside the repository, same name
cp -R plugin/.orbit-plugin "$PLUGIN_SRC"
orbit plugin add "$PLUGIN_SRC"
orbit plugin enable scribe --grant fs,network,env_pass
```

`orbit plugin enable` also seeds the routines into the workspace repository's
`.orbit/routines/`, even when `HOME` is a scratch directory.

An install after a plugin change runs:

```sh
claude plugin marketplace update scribe-local
claude plugin update scribe@scribe-local
PLUGIN_SRC="$(mktemp -d)/.orbit-plugin"   # a fresh copy of the current tree
cp -R plugin/.orbit-plugin "$PLUGIN_SRC"
orbit plugin upgrade scribe "$PLUGIN_SRC" --grant fs,network,env_pass
```

`claude plugin update` applies on the next Claude Code restart.

Then set the store root and the transcripts directory in the workspace's
Orbit config and check one call:

```toml
[plugins.scribe]
root = "/home/<user>/.scribe"     # absolute; every tool answers not_configured until set
transcripts = "/home/<user>/.claude/projects"   # optional when codex_home is set
codex_home = "/home/<user>/.codex"             # optional Codex active and archived transcripts
session_distill_crew = "haiku"
session_header_crew = "haiku"
```

```sh
orbit job run scribe-session-pass
orbit job run scribe-session-pass --input 'sessions=["<id>"]'
```

Orbit keeps one active session-pass run at a time. Enable the five-minute
routine with `orbit routine enable scribe-session-pass` once the store,
transcript roots and activity crews are configured.

The job admits up to five due sessions by default and runs at most 25
iterations. It stops starting windows after 90 seconds to leave time for
the in-flight window and its retry. Passes use consecutive windows of up to 40 messages and 15,000
characters by default. Set `[catalog] window_messages` and `window_chars`
to change those limits. Each valid window checkpoints the record. A failed
window preserves those checkpoints and keeps the session due. Automatic
retries wait 5, 10, 20, 40, then at most 60 minutes. An explicitly named
session bypasses backoff. `[instructions] session` and `session_header`
override activity guidance before the fixed answer contract.

`scribe session pass` settles eligible sessions locally and prints the Orbit
job command when new windows need distillation. `scribe session pass-prepare`
and `scribe session pass-apply` accept the same JSON as the plugin tools on
stdin and emit their results on stdout.

The grants are: `fs` (read and write the store root, read the transcripts
directory), `network` (the `claude` CLI) and `env_pass`
(`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`). The `root` default,
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
  --grant fs,network,env_pass  # the YAML goldens
```

The default package reads opted-in Codex transcripts when `codex_home` is
configured. The [generated Codex package](../orbit-codex/README.md) remains
available with its existing grants. Session-pass activities use Orbit's
crew settings for both transcript providers.
