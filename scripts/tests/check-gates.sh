#!/usr/bin/env bash
# scripts/tests/check-gates.sh — cases for the source-only gates
# (STD-04 R2: each gate fix comes with a case that fails without it). Each
# case builds a small fixture tree in a temporary directory and runs a gate
# on it with the tree as its root: a broken tree must fail and its clean twin
# must pass, so a case cannot pass on a gate that refuses everything.
#
# GATES (default: scripts/) names the directory holding the gates under
# test, so the cases can be run against an older copy to show they fail.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
GATES="${GATES:-$here}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fail=0
n=0
# expect <pass|fail> <gate> <tree> <what>
expect() {
  n=$((n + 1))
  local got=pass
  "$GATES/$2" "$3" >"$work/out" 2>&1 || got=fail
  if [[ "$got" != "$1" ]]; then
    echo "check-gates: FAIL: $2 on $4: expected $1, got $got" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
}

# tree <name>: a fresh fixture root holding a minimal two-crate workspace.
tree() {
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src" "$root/crates/scribe/src"
  printf '[package]\nname = "scribe-core"\n' >"$root/crates/scribe-core/Cargo.toml"
  printf '[package]\nname = "scribe"\n\n[dependencies]\nscribe-core = { path = "../scribe-core" }\n' \
    >"$root/crates/scribe/Cargo.toml"
  printf '//! core\npub mod entry;\n' >"$root/crates/scribe-core/src/lib.rs"
  printf '//! entry\nuse std::fmt;\n' >"$root/crates/scribe-core/src/entry.rs"
  printf 'fn main() {}\n' >"$root/crates/scribe/src/main.rs"
  echo "$root"
}

# --- check-test-modules.sh: a tests/ directory its parent never declares ----
clean=$(tree modules-clean)
mkdir -p "$clean/crates/scribe-core/src/tests"
printf 'mod entry;\n' >"$clean/crates/scribe-core/src/tests/mod.rs"
printf '#[test]\nfn t() {}\n' >"$clean/crates/scribe-core/src/tests/entry.rs"
printf '#[cfg(test)]\nmod tests;\n' >>"$clean/crates/scribe-core/src/lib.rs"
expect pass check-test-modules.sh "$clean" "a declared tests/ directory"

orphan=$(tree modules-orphan)
mkdir -p "$orphan/crates/scribe-core/src/tests"
printf 'mod entry;\n' >"$orphan/crates/scribe-core/src/tests/mod.rs"
printf '#[test]\nfn t() {}\n' >"$orphan/crates/scribe-core/src/tests/entry.rs"
expect fail check-test-modules.sh "$orphan" "src/tests/ with no \`mod tests;\` in lib.rs"

nested=$(tree modules-nested)
mkdir -p "$nested/crates/scribe-core/src/store/tests"
printf 'mod fsio;\n' >"$nested/crates/scribe-core/src/store/tests/mod.rs"
printf '#[test]\nfn t() {}\n' >"$nested/crates/scribe-core/src/store/tests/fsio.rs"
printf '//! store\n' >"$nested/crates/scribe-core/src/store/mod.rs"
expect fail check-test-modules.sh "$nested" "store/tests/ with no \`mod tests;\` in store/mod.rs"
printf '#[cfg(test)]\nmod tests;\n' >>"$nested/crates/scribe-core/src/store/mod.rs"
expect pass check-test-modules.sh "$nested" "store/tests/ declared in store/mod.rs"

# --- check-dependency-direction.sh: std::fs reached by a grouped import ------
deps=$(tree deps-clean)
expect pass check-dependency-direction.sh "$deps" "a core module with use std::fmt"

for import in 'use std::{fmt, fs};' 'use std::{io, fs::File};' $'use std::{\n    fmt,\n    process::Command,\n};'; do
  bad=$(tree "deps-grouped-$n")
  printf '//! entry\n%s\n' "$import" >"$bad/crates/scribe-core/src/entry.rs"
  expect fail check-dependency-direction.sh "$bad" "a core module with $(echo "$import" | tr -s '\n ' ' ')"
done

near=$(tree deps-near)
printf '//! entry\nuse std::{fmt, ffi::OsStr};\n' >"$near/crates/scribe-core/src/entry.rs"
expect pass check-dependency-direction.sh "$near" "a grouped import of names that only start like fs"

if [[ "$fail" -eq 0 ]]; then
  echo "check-gates: ok ($n cases)"
fi
exit "$fail"
