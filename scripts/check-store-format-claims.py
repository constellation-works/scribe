#!/usr/bin/env python3
"""scripts/check-store-format-claims.py [root] — every marked current-format
claim in the store specification names the FORMAT the code declares.

The current document format is the `FORMAT` constant in
crates/scribe-core/src/store/format.rs. docs/design/scribe/specs/store.md also
states formats historically ("Format 28 adds ...", "the format-27 release
wrote ..."); those are left alone. A claim about the build you are reading
about is marked with an HTML comment placed directly before its number, so it
is invisible when rendered and the prose stays readable:

    The code writes format <!-- current-format -->30 and reads formats 1–<!-- current-format -->30.
    ... stamps `control.json` at <!-- current-stamp -->30.

`current-format` marks the format the build reads or writes; `current-stamp`
marks the format of the stamp it leaves in `control.json` and the other stamp
files. Both must equal FORMAT. The guard fails when FORMAT cannot be read, when
a marker is not directly followed by a number or has an unknown kind, when a
marked number is not a whole number (`30.1` is a fraction, not format 30) or
differs from FORMAT, and when the spec carries no marker of one kind (so
removing the markers cannot turn the guard into a no-op).

`root` defaults to the repository; scripts/tests/check-gates.sh points it at
fixture trees.
"""

from pathlib import Path
import re
import sys

FORMAT_RS = "crates/scribe-core/src/store/format.rs"
SPEC = "docs/design/scribe/specs/store.md"
KINDS = ("format", "stamp")

DECLARATION = re.compile(
    r"^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?const[ \t]+FORMAT[ \t]*:[ \t]*u32[ \t]*=[ \t]*(\d+)[ \t]*;",
    re.MULTILINE,
)
# Any comment that starts like a marker, so a misspelt one is an error and not silence.
MARKER_START = re.compile(r"<!--\s*current-")
# The number takes its fraction whole, so `30.1` is seen as a decimal and not as
# `30`. A dot counts only with a digit after it: `30.` and `30-build` stay `30`.
MARKER = re.compile(r"<!--[ \t]*current-(\w+)[ \t]*-->(\d+(?:\.\d+)?)?")


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent
    errors: list[str] = []

    try:
        declared = DECLARATION.findall((root / FORMAT_RS).read_text(encoding="utf-8"))
    except OSError as err:
        print(f"store-format-claims: cannot read {FORMAT_RS}: {err}", file=sys.stderr)
        return 1
    if len(declared) != 1:
        print(
            f"store-format-claims: expected one `const FORMAT: u32 = N;` in {FORMAT_RS}, found {len(declared)}",
            file=sys.stderr,
        )
        return 1
    current = int(declared[0])

    try:
        spec = (root / SPEC).read_text(encoding="utf-8")
    except OSError as err:
        print(f"store-format-claims: cannot read {SPEC}: {err}", file=sys.stderr)
        return 1

    seen = dict.fromkeys(KINDS, 0)
    for number, line in enumerate(spec.splitlines(), start=1):
        well_formed = 0
        for marker in MARKER.finditer(line):
            well_formed += 1
            kind, claimed = marker.group(1), marker.group(2)
            if kind not in KINDS:
                errors.append(f"{SPEC}:{number}: unknown marker `current-{kind}` (use current-format or current-stamp)")
            elif claimed is None:
                errors.append(f"{SPEC}:{number}: `current-{kind}` marker is not directly followed by a number")
            elif not claimed.isdecimal():
                errors.append(
                    f"{SPEC}:{number}: current {kind} claim says {claimed}, which is not a whole number; FORMAT is {current} in {FORMAT_RS}"
                )
            elif int(claimed) != current:
                errors.append(
                    f"{SPEC}:{number}: current {kind} claim says {claimed}, but FORMAT is {current} in {FORMAT_RS}"
                )
            else:
                seen[kind] += 1
        if len(MARKER_START.findall(line)) != well_formed:
            errors.append(f"{SPEC}:{number}: malformed `current-` marker")

    for kind in KINDS:
        if seen[kind] == 0 and not errors:
            errors.append(f"{SPEC}: no `<!-- current-{kind} -->` claim marked; the guard would check nothing")

    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(f"store-format-claims: {len(errors)} stale or malformed claim(s); FORMAT is {current}", file=sys.stderr)
        return 1
    print(f"store-format-claims: ok ({sum(seen.values())} claims, FORMAT = {current})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
