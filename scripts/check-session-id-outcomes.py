#!/usr/bin/env python3
"""Verify the bounded session-ID table against the shared CLI test matrix."""
import json
import sys
from pathlib import Path

root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent
try:
    rows = json.loads((root / "scripts/session-id-outcomes.json").read_text())
    if not rows or len({row["id"] for row in rows}) != len(rows):
        raise ValueError("matrix must have unique rows")
    lines = ["| Case | Outcome | Exit | Error code |", "| --- | --- | --- | --- |"]
    for row in rows:
        code = f'`{row["code"]}`' if row["code"] else "—"
        lines.append(f'| {row["case"]} | {row["outcome"]} | {row["exit"]} | {code} |')
    expected = "\n".join(lines)
    for name in ("docs/guide.md", "docs/design/scribe/2_design.md"):
        text = (root / name).read_text()
        start = "<!-- session-id-outcomes:start -->"
        end = "<!-- session-id-outcomes:end -->"
        if text.count(start) != 1 or text.count(end) != 1 or text.split(start)[1].split(end)[0].strip() != expected:
            raise ValueError(f"{name}: session-ID outcome table differs from matrix")
except (OSError, ValueError, KeyError, TypeError, IndexError) as error:
    print(f"check-session-id-outcomes: {error}", file=sys.stderr)
    sys.exit(1)
print("check-session-id-outcomes: ok")
