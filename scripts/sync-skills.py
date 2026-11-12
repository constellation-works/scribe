#!/usr/bin/env python3
"""Generate packaged generic skill copies; --check is the source-only parity gate."""
import argparse
import pathlib
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
canonical = (root / "skills/scribe/SKILL.md").read_bytes()
failed = False
for name in ("plugin/claude-code/skills/scribe/SKILL.md", "plugin/codex/skills/scribe/SKILL.md"):
    target = root / name
    if args.check:
        if not target.exists() or target.read_bytes() != canonical:
            print(f"skill copy differs: {name}; run python3 scripts/sync-skills.py", file=sys.stderr)
            failed = True
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(canonical)
sys.exit(1 if failed else 0)
