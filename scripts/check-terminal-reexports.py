#!/usr/bin/env python3
"""Report public standard-stream use trees outside the output boundary."""

import os
from pathlib import Path
import sys

import direction_lex as lex


def use_paths(stmt):
    """Flatten `stmt` with a leading `::` kept.

    Stored `paths` drop that marker, so `::elsewhere` would match a local
    module named `elsewhere`.
    """
    return lex.flatten_use_tokens(stmt.tree_toks, keep_absolute=True)


def scan(path):
    source = path.read_text(encoding="utf-8")
    statements = tuple(
        stmt
        for stmt in lex.scan_file(source).use_stmts
        if stmt.brace_depth == len(stmt.mod_path) and not stmt.is_test_only
    )
    crate_root = is_crate_root(path)
    # Rust imports are in scope throughout their module, including before their
    # declaration. Resolve chains to a fixed point, without crossing modules.
    aliases = {}
    changed = True
    while changed:
        changed = False
        for stmt in statements:
            for segments, alias in use_paths(stmt):
                if not segments or segments[-1] == "*":
                    continue
                resolved = expand(segments, stmt.mod_path, aliases, crate_root)
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
            expand(segments, stmt.mod_path, aliases, crate_root)
            in (("std", "io", "stdout"), ("std", "io", "stderr"), ("std", "io", "*"))
            for segments, _ in use_paths(stmt)
        ):
            print(f"{path}:{stmt.line}: public standard stream reexport")


def is_crate_root(path):
    """Whether path is a crate root: src/lib.rs, src/main.rs or a src/bin/*.rs target."""
    if path.parent.name == "bin":
        return path.parent.parent.name == "src"
    return path.name in ("lib.rs", "main.rs") and path.parent.name == "src"


def expand(segments, module, aliases, crate_root=False):
    """Resolve a use path to the std path of the alias it names in this file.

    Module paths are relative to the file, whose top level is `()`. `self` is the
    current module, `super` its parent and `crate` the crate root. A parent of
    the file's top level, or the crate root of a file that is not a crate root,
    lies outside the file: the path then stays as written and matches no alias.

    A leading `::` segment is an absolute extern-prelude path. Rust 2024
    resolves it to an external crate, never a same-named local module, so this
    file's aliases are not consulted. Stripping the marker leaves `::std::...`
    as a `std` path.
    """
    if segments[:1] == (lex.ABSOLUTE_PATH_SEGMENT,):
        return segments[1:]
    if not segments or segments[0] == "std":
        return segments
    scope, rest = module, segments
    while rest and rest[0] in ("self", "super", "crate"):
        head, rest = rest[0], rest[1:]
        if head == "super":
            if not scope:
                return segments
            scope = scope[:-1]
        elif head == "crate":
            if not crate_root:
                return segments
            scope = ()
    if not rest:
        return scope
    # A path may pass through child modules before the alias it names.
    for i in range(len(rest)):
        key = (scope + rest[:i], rest[i])
        if key in aliases:
            return aliases[key] + rest[i + 1 :]
    return segments


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
