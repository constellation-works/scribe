#!/usr/bin/env python3
"""Report public standard-stream use trees outside the output boundary."""

import os
from pathlib import Path
import sys

import direction_lex as lex


def scan(path):
    source = path.read_text(encoding="utf-8")
    statements = lex.scan_file(source).use_stmts
    # Rust imports are in scope throughout their module, including before their
    # declaration. Resolve chains to a fixed point, without crossing modules.
    aliases = {}
    changed = True
    while changed:
        changed = False
        for stmt in statements:
            for segments, alias in stmt.paths:
                if not segments or segments[-1] == "*":
                    continue
                resolved = expand(segments, stmt.mod_path, aliases)
                if resolved[:1] != ("std",):
                    continue
                key = (stmt.mod_path, alias or segments[-1])
                if key not in aliases:
                    aliases[key] = resolved
                    changed = True

    for stmt in statements:
        if stmt.vis != "pub":
            continue
        if any(
            expand(segments, stmt.mod_path, aliases)
            in (("std", "io", "stdout"), ("std", "io", "stderr"), ("std", "io", "*"))
            for segments, _ in stmt.paths
        ):
            print(f"{path}:{stmt.line}: public standard stream reexport")


def expand(segments, module, aliases):
    if not segments or segments[0] == "std":
        return segments
    return aliases.get((module, segments[0]), (segments[0],)) + segments[1:]


def walk_error(error):
    raise error


def main():
    try:
        for directory, dirs, files in os.walk("crates", onerror=walk_error):
            dirs[:] = sorted(d for d in dirs if d != "tests")
            for name in sorted(files):
                if name.endswith(".rs"):
                    scan(Path(directory) / name)
    except (OSError, UnicodeError) as error:
        print(f"terminal-guard: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
