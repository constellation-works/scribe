# Command guide

The shipped skill tells agents how to use these commands. When a change here alters what an agent may or must do (a refusal, fallback, flag, limit, or an id or transcript rule), the change is not finished until the skill states the new rule.

Run these from the repository root, in order:

1. `rg -n '<old rule>' skills/scribe/` and find every statement of the old rule.
2. Edit the canonical skill under `skills/scribe/`, including its references.
3. Run `python3 scripts/sync-skills.py` to mirror it into `plugin/claude-code/skills/scribe/` and `plugin/codex/skills/scribe/`. Do not edit those copies by hand.

`make ci-fast` fails when a copy drifts, but it cannot tell whether a prose rule is still accurate, so run step 1 yourself.

This is the failure the rule prevents. In 30 days, five command changes left the skill stating the old rule: ORB-26525, ORB-26900, ORB-27212, ORB-27275 and ORB-27336.
