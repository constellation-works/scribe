#!/usr/bin/env python3
"""Mirror the canonical skill directory in both provider packages."""
import argparse
import pathlib
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--check", action="store_true")
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent


def files(directory):
    return {
        path.relative_to(directory)
        for path in directory.rglob("*")
        if path.is_file() or path.is_symlink()
    }


canonical = root / "skills/scribe"
if not (canonical / "SKILL.md").is_file():
    sys.exit("canonical skill is missing: skills/scribe/SKILL.md")
source = {path: (canonical / path).read_bytes() for path in sorted(files(canonical))}
failed = False
for provider in ("claude-code", "codex"):
    mirror = root / "plugin" / provider / "skills/scribe"
    present = files(mirror)
    for relative in sorted(present - source.keys()):
        name = (mirror / relative).relative_to(root)
        print(f"extra skill file: {name}; remove the stale mirror file", file=sys.stderr)
        failed = True
    for relative, contents in source.items():
        target = mirror / relative
        name = target.relative_to(root)
        if any((root / parent).is_symlink() for parent in (name, *name.parents)):
            print(f"skill copy contains a symlink: {name}", file=sys.stderr)
            failed = True
        elif args.check:
            if relative not in present or target.read_bytes() != contents:
                print(f"skill copy differs: {name}; run python3 scripts/sync-skills.py", file=sys.stderr)
                failed = True
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(contents)
sys.exit(1 if failed else 0)
