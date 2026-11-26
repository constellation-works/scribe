# Command guide

When changing a command or its app (`crates/scribe-core/src/apps/<app>/`), update the shipped skill in the same change if the change alters what an agent may or must do (a refusal, fallback, flag, limit, or an id or transcript rule).

## Mirror and update procedure

Run these from the repository root, in order:

1. `rg -n '<old rule>' skills/scribe/` and find every statement of the old rule.
2. Edit the canonical skill under `skills/scribe/`, including its references.
3. Run `python3 scripts/sync-skills.py` to mirror it into `plugin/claude-code/skills/scribe/` and `plugin/codex/skills/scribe/`.

   After a canonical removal or rename, the generator reports each obsolete mirror file as `extra skill file` and exits unsuccessfully. Remove only the files it names from the corresponding provider mirrors (both mirrors when both are reported).

   The generator copies current canonical files but leaves stale files in place. Do not edit or hand-copy current files.

   Rerun `python3 scripts/sync-skills.py` until it exits successfully, then run `python3 scripts/sync-skills.py --check` to verify byte parity for both providers.

Run step 1 yourself: `make ci-fast` detects copy drift but cannot tell whether a prose rule is still accurate.
