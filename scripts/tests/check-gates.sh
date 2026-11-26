#!/usr/bin/env bash
# scripts/tests/check-gates.sh — cases for the repository gates and clippy boundaries
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
REPO_ROOT="${REPO_ROOT:-$(cd "$GATES/.." && pwd)}"
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
  printf '//! core\npub mod apps;\n' >"$root/crates/scribe-core/src/lib.rs"
  printf '//! config\nuse std::fmt;\n' >"$root/crates/scribe-core/src/config.rs"
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

# --- check-dependency-direction.sh: std::net reached by a grouped import -----
deps=$(tree deps-clean)
expect pass check-dependency-direction.sh "$deps" "a core module with use std::fmt"

for import in 'use std::{fmt, net};' 'use std::{io, net::TcpStream};' $'use std::{\n    fmt,\n    net::UdpSocket,\n};'; do
  bad=$(tree "deps-grouped-$n")
  printf '//! config\n%s\n' "$import" >"$bad/crates/scribe-core/src/config.rs"
  expect fail check-dependency-direction.sh "$bad" "a core module with $(echo "$import" | tr -s '\n ' ' ')"
done

near=$(tree deps-near)
printf '//! config\nuse std::{fmt, net_like::Item};\n' >"$near/crates/scribe-core/src/config.rs"
expect pass check-dependency-direction.sh "$near" "a grouped import of names that only start like net"

# A relative import (`super::`) is the crate path it names: it meets the app
# order and the port's rules as the `crate::` spelling does, and lib.rs may
# declare a file module only for a root place (ORB-28114). The clean twins
# are legal relative imports within an app and the root places.
rel=$(tree deps-rel-clean)
mkdir -p "$rel/crates/scribe-core/src/apps/entry/rules" "$rel/crates/scribe-core/src/apps/session"
printf 'use super::entry::Entry;\n' >"$rel/crates/scribe-core/src/apps/session/mod.rs"
printf 'use super::rules::Rule;\n' >"$rel/crates/scribe-core/src/apps/entry/flow.rs"
printf 'use super::super::Command;\n' >"$rel/crates/scribe-core/src/apps/entry/rules/check.rs"
printf '//! core\npub mod apps;\npub mod composition;\npub mod providers;\npub(crate) mod config;\npub(crate) mod runtime;\n#[cfg(test)]\nmod extra;\n' \
  >"$rel/crates/scribe-core/src/lib.rs"
expect pass check-dependency-direction.sh "$rel" "legal relative imports within an app, and the root places"

for spec in \
  'apps/entry/mod.rs|use super::session::Entry;' \
  'apps/entry/flow.rs|use super::super::session::Entry;' \
  'apps/entry/mod.rs|use super::{session::Entry};' \
  $'apps/entry/mod.rs|use super::{\n    session::Entry,\n};' \
  'apps/tag/mod.rs|use super::entry::Command;' \
  'apps/tag/flow.rs|use super::super::entry::Command;' \
  'apps/tag/mod.rs|use crate::{apps::entry::Command};' \
  'apps/tag/mod.rs|use crate::{config, apps::entry::Command};' \
  'apps/tag/mod.rs|use crate::{apps::{entry::Command}};' \
  'apps/tag/mod.rs|use crate::{apps::{mark::Mark, entry::{Command, Outcome}}};' \
  'apps/tag/mod.rs|use crate::apps::{mark::Mark, entry::{Command, Outcome}};' \
  'apps/tag/mod.rs|use super::super::{apps::entry::Command};' \
  $'apps/tag/mod.rs|use crate::{\n    config,\n    apps::entry::Command,\n};' \
  'apps/mark/mod.rs|use crate::{apps::entry::flow::Step};' \
  'apps/mark/mod.rs|use crate::apps::{entry::{flow::Step}};' \
  'apps/entry/mod.rs|use super::tag::rules::helper;' \
  'apps/entry/mod.rs|use super::tag::{rules::helper};' \
  'apps/entry/flow.rs|use super::super::tag::{rules::helper};' \
  'apps/entry/mod.rs|use super::{tag::{rules::helper}};' \
  'apps/entry/mod.rs|use super::tag::{helper, rules::{self, sub::helper}};' \
  'apps/entry/mod.rs|use crate::apps::tag::{rules::helper};' \
  'apps/entry/mod.rs|use crate::apps::tag::rules::{helper};' \
  'apps/entry/mod.rs|use super::tag::rules::*;' \
  'apps/entry/mod.rs|use super::tag::{rules::*};' \
  $'apps/entry/mod.rs|use super::tag::{\n    helper, // ok\n    rules::helper,\n};' \
  'apps/tag/mod.rs|use crate::{apps::nosuch::Command};' \
  'providers/models.rs|use super::super::apps::session::Entry;'; do
  bad=$(tree "rel-up-$n")
  mkdir -p "$bad/crates/scribe-core/src/providers" "$bad/crates/scribe-core/src/$(dirname "${spec%%|*}")"
  printf '%s\n' "${spec#*|}" >"$bad/crates/scribe-core/src/${spec%%|*}"
  expect fail check-dependency-direction.sh "$bad" "an upward or reaching import in ${spec%%|*}: $(echo "${spec#*|}" | tr -s '\n ' ' ')"
done

# The downward twins of the grouped and relative spellings above are legal.
for spec in \
  'apps/entry/mod.rs|use super::tag::Command;' \
  'apps/entry/mod.rs|use super::tag::helper;' \
  'apps/entry/mod.rs|use super::tag::{helper, Other};' \
  'apps/entry/flow.rs|use super::super::tag::Command;' \
  'apps/entry/flow.rs|use super::super::tag::{self, helper};' \
  'apps/entry/mod.rs|use super::{tag::{helper, Other}};' \
  'apps/entry/mod.rs|use crate::{apps::tag::Command};' \
  'apps/entry/mod.rs|use crate::{apps::tag::{helper, Other}};' \
  'apps/entry/mod.rs|use crate::apps::tag::{helper, Other};' \
  'apps/entry/mod.rs|use crate::{config, apps::{tag::Command, mark::{Mark, Outcome}}};' \
  'apps/entry/mod.rs|use crate::apps::{tag::Command, mark::{Mark, Outcome}};' \
  $'apps/entry/mod.rs|use crate::{\n    config,\n    apps::tag::Command,\n};' \
  'apps/entry/mod.rs|use super::tag::*;' \
  'apps/entry/mod.rs|use crate::{config, runtime::Ctx};'; do
  ok=$(tree "rel-down-$n")
  mkdir -p "$ok/crates/scribe-core/src/$(dirname "${spec%%|*}")"
  printf '%s\n' "${spec#*|}" >"$ok/crates/scribe-core/src/${spec%%|*}"
  expect pass check-dependency-direction.sh "$ok" "a downward import in ${spec%%|*}: $(echo "${spec#*|}" | tr -s '\n ' ' ')"
done

for decl in 'mod extra;' 'pub mod extra;'; do
  bad=$(tree "root-$n")
  printf '//! core\npub mod apps;\n%s\n' "$decl" >"$bad/crates/scribe-core/src/lib.rs"
  printf 'fn extra() {}\n' >"$bad/crates/scribe-core/src/extra.rs"
  expect fail check-dependency-direction.sh "$bad" "a root module declared \`$decl\` with no place"
done

# The Models port may not depend on its callers.
port=$(tree port-clean)
mkdir -p "$port/crates/scribe-core/src/providers"
printf 'use crate::voice::Voice;\n' >"$port/crates/scribe-core/src/providers/models.rs"
expect pass check-dependency-direction.sh "$port" "a port using a lower-level module"
printf 'use crate::{voice::{group, Voice}};\n' >"$port/crates/scribe-core/src/providers/models.rs"
expect pass check-dependency-direction.sh "$port" "a nested name that is not a caller module"
for module in memory group; do
  printf 'use crate::%s::Item;\n' "$module" >"$port/crates/scribe-core/src/providers/models.rs"
  expect fail check-dependency-direction.sh "$port" "a port importing $module"
  printf 'use crate::{voice::Voice,\n    %s::Item};\n' "$module" >"$port/crates/scribe-core/src/providers/models.rs"
  expect fail check-dependency-direction.sh "$port" "a port importing $module in a group"
done

# Inside scribe-store, db/codec.rs and repository/audit.rs do no file I/O:
# neither names db/files.rs, cw-fs or cw-sqlite (std::fs is enforced by
# clippy.toml, and the fsio module they once named is retired).
store_tree() {
  local root
  root=$(tree "$1")
  mkdir -p "$root/crates/scribe-store/src/db" "$root/crates/scribe-store/src/repository"
  printf '[package]\nname = "scribe-store"\n' >"$root/crates/scribe-store/Cargo.toml"
  printf '//! codec\n' >"$root/crates/scribe-store/src/db/codec.rs"
  printf '//! audit\n' >"$root/crates/scribe-store/src/repository/audit.rs"
  echo "$root"
}
store=$(store_tree store-clean)
expect pass check-dependency-direction.sh "$store" "a store whose codec and audit do no file I/O"
for shape in db/codec repository/audit; do
  for import in 'use super::files;' 'use crate::db::files;' 'use cw_fs::Cap;' 'use cw_sqlite::Db;'; do
    bad=$(store_tree "store-io-$n")
    printf '//! shape\n%s\n' "$import" >"$bad/crates/scribe-store/src/${shape}.rs"
    expect fail check-dependency-direction.sh "$bad" "a store $shape with $import"
  done
done

# scribe-store opens, configures and migrates scribe.db only through cw-sqlite.
for pragma in 'PRAGMA journal_mode = WAL' 'PRAGMA synchronous = FULL' 'PRAGMA user_version = 1' 'conn.busy_timeout(wait)'; do
  bad=$(store_tree "store-pragma-$n")
  printf '//! db\nlet _ = "%s";\n' "$pragma" >"$bad/crates/scribe-store/src/db/mod.rs"
  expect fail check-dependency-direction.sh "$bad" "scribe-store db/mod.rs setting $pragma directly"
done

# Exercise direct, relative and grouped paths in both guarded shapes, plus
# nearby names and unrelated identifiers that must remain legal.
for shape in db/codec repository/audit; do
  for import in 'db::files::read();' 'use super::super::db::files;' \
    'use super::{log, files};' 'use crate::db::{self, docs, files};' \
    'use crate::{db::files};' 'use crate::{db::{docs, files::read}};' \
    $'use super::{\n    log,\n    files,\n};' \
    $'use crate::db::{\n    docs,\n    files,\n};' \
    $'use crate::{\n    db::files,\n};' \
    'use super::super::db::{files, DB_FILE};' \
    'use crate::db::{files, DB_FILE};' \
    $'/*\nuse crate::db::{files, DB_FILE};\n*/\nuse crate::db::{files, DB_FILE};' \
    $'use crate::db::{\n    docs, // done; next\n    files,\n};' \
    $'use crate::db::{\n    docs, /* a; b */\n    files,\n};' \
    'pub(in crate::db) use crate::db::{files as private_files};' \
    'pub (crate) use crate::db::{files};'; do
    bad=$(store_tree "store-path-$n")
    printf '%s\n' "$import" >"$bad/crates/scribe-store/src/${shape}.rs"
    expect fail check-dependency-direction.sh "$bad" "a store $shape referencing files: $import"
  done
  for clean_import in 'use super::{log, files_extra};' 'use crate::db::{docs, files_extra};' \
    'use crate::{db::docs, other::files};' $'use crate::db::{\n    docs,\n    files_extra,\n};' \
    'use super::super::db::{docs, DB_FILE};' 'pub(in crate::db) use crate::db::{docs as d};' \
    $'use crate::db::{\n    docs, // files; not an import\n};' \
    $'/*\nuse crate::db::{files, DB_FILE};\n*/' \
    '/* use crate::db::{files, DB_FILE}; */' \
    $'/*\n/* nested */\nuse crate::db::{files, DB_FILE};\n*/' \
    '// use crate::db::{files, DB_FILE};' \
    'struct Example { files: Vec<String> }' 'fn f(files: usize) { let files = files + 1; }'; do
    ok=$(store_tree "store-near-$n")
    printf '%s\n' "$clean_import" >"$ok/crates/scribe-store/src/${shape}.rs"
    expect pass check-dependency-direction.sh "$ok" "a store $shape with unrelated files: $clean_import"
  done
done

# Delimiters in line comments and literals cannot hide a later live import.
# Check the diagnostic as well as the exit status: blanking must keep lines.
for shape in db/codec repository/audit; do
  for prefix in '// glob dir/*' 'const P: &str = "/*";' \
    'const P: &str = r#"/*"#;' 'const P: &str = "escaped \" /*";'; do
    bad=$(store_tree "store-lex-$n")
    printf '%s\nuse crate::db::{docs, files};\n/* trailing note */\n' "$prefix" \
      >"$bad/crates/scribe-store/src/${shape}.rs"
    expect fail check-dependency-direction.sh "$bad" "a live import after $prefix"
    if ! grep -Fq "crates/scribe-store/src/${shape}.rs:2: grouped import reaches db/files.rs" "$work/out"; then
      echo "check-gates: FAIL: missing line 2 diagnostic for $shape after $prefix" >&2
      fail=1
    fi
  done
  ok=$(store_tree "store-nested-lex-$n")
  printf '%s\n' '/* outer /* inner */' 'use crate::db::{docs, files};' '*/' \
    >"$ok/crates/scribe-store/src/${shape}.rs"
  expect pass check-dependency-direction.sh "$ok" "a grouped import inside nested block comments"
  printf '%s\n' 'const TEXT: &str = r#"' 'use crate::db::{docs, files};' '"#;' \
    >"$ok/crates/scribe-store/src/${shape}.rs"
  expect pass check-dependency-direction.sh "$ok" "a grouped import inside a raw string"
done

# --- check-dependency-direction.sh: unit import cycles & in-layer order (STD-02 R6, R39) ---
# 1. Crate-level cycle (two crates mutually depending in [dependencies]).
cyc_crate=$(tree cycle-crate-bad)
printf '[package]\nname = "scribe-core"\n\n[dependencies]\nscribe = { path = "../scribe" }\n' \
  >"$cyc_crate/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$cyc_crate" "a crate-level import cycle (scribe <-> scribe-core)"
if ! grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing crate import cycle diagnostic" >&2
  fail=1
fi

# Cargo resolves a dependency by its `package`, not its table key, and accepts
# the same dependency as an inline entry, a dotted key or a subtable. Mix the
# forms on both sides of the cycle: each pair must still report it.
n_scribe=0
for scribe_dep in \
  $'[dependencies]\nrenamed-core = { path = "../scribe-core", package = "scribe-core" }\n' \
  $'[dependencies]\nrenamed-core.path = "../scribe-core"\nrenamed-core.package = "scribe-core"\n' \
  $'[dependencies.renamed-core]\npath = "../scribe-core"\npackage = "scribe-core"\n' \
  $'[dependencies]\nscribe-core.workspace = true\n'; do
  for core_dep in \
    $'[dependencies]\nback = { path = "../scribe", package = "scribe" }\n' \
    $'[dependencies]\nback.path = "../scribe"\nback.package = "scribe"\n' \
    $'[dependencies.back]\npath = "../scribe"\npackage = "scribe"\n' \
    $'[target.\'cfg(unix)\'.dependencies.back]\npackage = "scribe"\npath = "../scribe"\n' \
    $'[build-dependencies]\nscribe = { path = "../scribe" }\n'; do
    n_scribe=$((n_scribe + 1))
    mixed=$(tree "cycle-crate-mixed-$n_scribe")
    printf '[package]\nname = "scribe"\n\n%s' "$scribe_dep" >"$mixed/crates/scribe/Cargo.toml"
    printf '[package]\nname = "scribe-core"\n\n%s' "$core_dep" >"$mixed/crates/scribe-core/Cargo.toml"
    expect fail check-dependency-direction.sh "$mixed" "a crate cycle through mixed dependency forms ($n_scribe)"
    if ! grep -Fq "crate import cycle:" "$work/out"; then
      echo "check-gates: FAIL: missing crate import cycle diagnostic for mixed forms ($n_scribe)" >&2
      cat "$work/out" >&2
      fail=1
    fi
  done
done

# The same forms without a back edge are legal: an aliased scribe -> scribe-core
# edge, and an alias that merely reuses an internal crate's name for an
# external package, are not cycles.
for scribe_dep in \
  $'[dependencies]\nrenamed-core = { path = "../scribe-core", package = "scribe-core" }\n' \
  $'[dependencies.renamed-core]\npath = "../scribe-core"\npackage = "scribe-core"\n'; do
  for core_dep in \
    $'[dependencies]\nscribe = { version = "1", package = "serde" }\n' \
    $'[dependencies.scribe]\nversion = "1"\npackage = "serde"\n' \
    $'[dev-dependencies]\nback = { path = "../scribe", package = "scribe" }\n' \
    $'[dev-dependencies.back]\npath = "../scribe"\npackage = "scribe"\n'; do
    n_scribe=$((n_scribe + 1))
    legal=$(tree "cycle-crate-aliased-clean-$n_scribe")
    printf '[package]\nname = "scribe"\n\n%s' "$scribe_dep" >"$legal/crates/scribe/Cargo.toml"
    printf '[package]\nname = "scribe-core"\n\n%s' "$core_dep" >"$legal/crates/scribe-core/Cargo.toml"
    expect pass check-dependency-direction.sh "$legal" "an acyclic aliased dependency ($n_scribe)"
  done
done

# The allowlist reads the package too: an alias cannot hide a banned crate.
for banned_dep in \
  $'sql = { version = "0.43", package = "rusqlite" }\n' \
  $'\n[dependencies.sql]\nversion = "0.43"\npackage = "rusqlite"\n'; do
  n_scribe=$((n_scribe + 1))
  hidden=$(tree "banned-aliased-$n_scribe")
  printf '[package]\nname = "scribe"\n\n[dependencies]\nscribe-core = { path = "../scribe-core" }\n%s' \
    "$banned_dep" >"$hidden/crates/scribe/Cargo.toml"
  expect fail check-dependency-direction.sh "$hidden" "a banned crate behind an alias ($n_scribe)"
  if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
    echo "check-gates: FAIL: missing banned-crate diagnostic for the alias ($n_scribe)" >&2
    fail=1
  fi
done

# 2. Top-level module cycles inside a crate (direct, relative super::, grouped, 3-node)
#    plus clean acyclic twins and comment/string non-cycles.
cyc_top_ok=$(tree cycle-top-clean)
mkdir -p "$cyc_top_ok/crates/scribe-core/src/providers"
printf '//! config\n' >"$cyc_top_ok/crates/scribe-core/src/config.rs"
printf '//! providers\nuse crate::config;\n' >"$cyc_top_ok/crates/scribe-core/src/providers/mod.rs"
expect pass check-dependency-direction.sh "$cyc_top_ok" "acyclic top-level modules (providers -> config)"

for import in 'use crate::providers;' 'use super::providers;' 'use crate::{providers};'; do
  cyc_top_bad=$(tree "cycle-top-$n")
  mkdir -p "$cyc_top_bad/crates/scribe-core/src/providers"
  printf '//! config\n%s\n' "$import" >"$cyc_top_bad/crates/scribe-core/src/config.rs"
  printf '//! providers\nuse crate::config;\n' >"$cyc_top_bad/crates/scribe-core/src/providers/mod.rs"
  expect fail check-dependency-direction.sh "$cyc_top_bad" "top-level module cycle config <-> providers via $import"
  if ! grep -Fq "scribe-core import cycle:" "$work/out"; then
    echo "check-gates: FAIL: missing scribe-core import cycle diagnostic for $import" >&2
    fail=1
  fi
done

# Re-exports retain their public binding, and adjacent use statements must each
# be scanned. Check the full cycle and both source locations.
for case in alias grouped; do
  cyc=$(tree "cycle-regression-$case")
  mkdir -p "$cyc/crates/scribe-core/src/providers"
  printf 'use crate::config::Config;\n' >"$cyc/crates/scribe-core/src/providers/mod.rs"
  if [ "$case" = alias ]; then
    printf 'pub use providers::ProviderValue as RuntimeValue;\n' >"$cyc/crates/scribe-core/src/lib.rs"
    printf 'use crate::RuntimeValue;\n' >"$cyc/crates/scribe-core/src/config.rs"
  else
    printf 'use std::fmt; use crate::{providers::ProviderValue};\n' >"$cyc/crates/scribe-core/src/config.rs"
  fi
  expect fail check-dependency-direction.sh "$cyc" "cycle through $case import"
  for evidence in 'scribe-core import cycle: config -> providers -> config' \
    'crates/scribe-core/src/config.rs:1' 'crates/scribe-core/src/providers/mod.rs:1'; do
    if ! grep -Fq "$evidence" "$work/out"; then
      echo "check-gates: FAIL: missing $evidence for $case cycle" >&2
      fail=1
    fi
  done
  printf 'struct ProviderValue;\n' >"$cyc/crates/scribe-core/src/providers/mod.rs"
  expect pass check-dependency-direction.sh "$cyc" "acyclic control for $case import"
done

cyc_three_ok=$(tree cycle-three-clean)
printf '//! error\n' >"$cyc_three_ok/crates/scribe/src/error.rs"
printf '//! help\nuse crate::error;\n' >"$cyc_three_ok/crates/scribe/src/help.rs"
printf '//! cli\nuse crate::help;\n' >"$cyc_three_ok/crates/scribe/src/cli.rs"
expect pass check-dependency-direction.sh "$cyc_three_ok" "acyclic 3-module chain (cli -> help -> error)"

cyc_three_bad=$(tree cycle-three-bad)
printf '//! error\nuse crate::cli;\n' >"$cyc_three_bad/crates/scribe/src/error.rs"
printf '//! help\nuse crate::error;\n' >"$cyc_three_bad/crates/scribe/src/help.rs"
printf '//! cli\nuse crate::help;\n' >"$cyc_three_bad/crates/scribe/src/cli.rs"
expect fail check-dependency-direction.sh "$cyc_three_bad" "3-node top-level cycle cli -> help -> error -> cli"
if ! grep -Fq "scribe import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing 3-node cycle diagnostic" >&2
  fail=1
fi

cyc_comment=$(tree cycle-comment-clean)
mkdir -p "$cyc_comment/crates/scribe-core/src/providers"
printf '// use crate::providers;\n/* use crate::providers; */\nconst S: &str = r#"use crate::providers;"#;\n' \
  >"$cyc_comment/crates/scribe-core/src/config.rs"
printf '//! providers\nuse crate::config;\n' >"$cyc_comment/crates/scribe-core/src/providers/mod.rs"
expect pass check-dependency-direction.sh "$cyc_comment" "commented and raw-string cycle-like imports"

# 3. Module cycles inside a layer without a declared order (scribe::output)
#    and inside a declared ordered layer (scribe-store::db).
cyc_out_ok=$(tree cycle-output-clean)
mkdir -p "$cyc_out_ok/crates/scribe/src/output"
printf '//! color\n' >"$cyc_out_ok/crates/scribe/src/output/color.rs"
printf '//! render\nuse super::color;\n' >"$cyc_out_ok/crates/scribe/src/output/render.rs"
expect pass check-dependency-direction.sh "$cyc_out_ok" "acyclic scribe::output modules"

cyc_out_bad=$(tree cycle-output-bad)
mkdir -p "$cyc_out_bad/crates/scribe/src/output"
printf '//! color\nuse super::render;\n' >"$cyc_out_bad/crates/scribe/src/output/color.rs"
printf '//! render\nuse super::color;\n' >"$cyc_out_bad/crates/scribe/src/output/render.rs"
expect fail check-dependency-direction.sh "$cyc_out_bad" "in-layer module cycle in scribe::output (color <-> render)"
if ! grep -Fq "scribe::output import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing scribe::output cycle diagnostic" >&2
  fail=1
fi

cyc_db_ok=$(store_tree cycle-db-clean)
printf '//! groups\n' >"$cyc_db_ok/crates/scribe-store/src/db/groups.rs"
printf '//! sessions\nuse super::groups;\n' >"$cyc_db_ok/crates/scribe-store/src/db/sessions.rs"
expect pass check-dependency-direction.sh "$cyc_db_ok" "acyclic scribe-store::db downward edge (sessions -> groups)"

cyc_db_bad=$(store_tree cycle-db-bad)
printf '//! groups\nuse super::sessions;\n' >"$cyc_db_bad/crates/scribe-store/src/db/groups.rs"
printf '//! sessions\nuse super::groups;\n' >"$cyc_db_bad/crates/scribe-store/src/db/sessions.rs"
expect fail check-dependency-direction.sh "$cyc_db_bad" "in-layer module cycle in scribe-store::db (groups <-> sessions)"
if ! grep -Fq "scribe-store::db import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing scribe-store::db cycle diagnostic" >&2
  fail=1
fi

# 4. In-layer order enforcement without a cycle (upward edge and unlisted module).
ord_db_bad=$(store_tree order-db-upward)
printf '//! log\nuse super::codec;\n' >"$ord_db_bad/crates/scribe-store/src/db/log.rs"
expect fail check-dependency-direction.sh "$ord_db_bad" "upward edge in scribe-store::db (log -> codec)"
if ! grep -Fq "not lower in the declared order" "$work/out"; then
  echo "check-gates: FAIL: missing declared order diagnostic" >&2
  fail=1
fi

ord_db_unlisted=$(store_tree order-db-unlisted)
printf '//! extra\n' >"$ord_db_unlisted/crates/scribe-store/src/db/extra.rs"
expect fail check-dependency-direction.sh "$ord_db_unlisted" "unlisted module in scribe-store::db (extra)"
if ! grep -Fq "has no place in the declared order" "$work/out"; then
  echo "check-gates: FAIL: missing unlisted-module order diagnostic" >&2
  fail=1
fi

# Inline siblings have the same order and edge rules as file modules.
ord_inline=$(store_tree order-db-inline)
cat >"$ord_inline/crates/scribe-store/src/db/mod.rs" <<'EOF'
mod log {}
mod codec { use super::log; }
EOF
expect pass check-dependency-direction.sh "$ord_inline" "listed inline siblings with a downward edge"
cat >"$ord_inline/crates/scribe-store/src/db/mod.rs" <<'EOF'
mod log { use super::codec; }
mod codec {}
EOF
expect fail check-dependency-direction.sh "$ord_inline" "inline sibling upward edge (log -> codec)"
if ! grep -Fq "scribe-store::db: log imports codec, which is not lower" "$work/out"; then
  echo "check-gates: FAIL: missing inline upward-edge diagnostic" >&2
  fail=1
fi
cat >"$ord_inline/crates/scribe-store/src/db/mod.rs" <<'EOF'
// mod decoy {}
const TEXT: &str = "mod decoy {}";
#[cfg(test)]
mod tests { mod decoy {} }
#[cfg(any(test, feature = "test-support"))]
mod support { use super::inline_extra; }
pub(crate) mod inline_extra {}
EOF
expect fail check-dependency-direction.sh "$ord_inline" "unlisted inline module in scribe-store::db"
if ! grep -Fq "scribe-store::db: module 'inline_extra' has no place in the declared order" "$work/out"; then
  echo "check-gates: FAIL: missing inline unlisted-module diagnostic" >&2
  fail=1
fi
if grep -Eq "module '(decoy|tests|support)' has no place" "$work/out"; then
  echo "check-gates: FAIL: comment, string, or test module counted as a unit" >&2
  fail=1
fi

# --- check-readme-links.py: README links, anchors and paths (STD-09 R12) -----
# readme <name>: a fixture root whose README.md is read from stdin, with the
# files the clean README below names. A broken case appends one reference to it.
readme() {
  local root
  root=$(tree "$1")
  mkdir -p "$root/docs" "$root/img"
  printf '## Install\n' >"$root/docs/guide.md"
  printf 'png\n' >"$root/img/logo.png"
  printf 'MIT\n' >"$root/LICENSE"
  cat >"$root/README.md"
  echo "$root"
}
readme_clean=$(cat <<'EOF'
# scribe

<a href="LICENSE">Licence</a> and <a href="https://example.com/">site</a>.

<h1 align="center"><img src="img/logo.png" alt="logo" /></h1>

## Install

Read the [guide](docs/guide.md#install), [the top](#scribe) or [the licence](LICENSE).

<a id="setup"></a>
Set up with [setup](#setup) and ![logo](img/logo.png).

<img src="img/logo.png" srcset="img/logo.png 1x, img/logo.png 2x" alt="logo">

<img alt="a src=img/missing.png inside a value" src="img/logo.png">

[external](https://example.com/missing.md) and [guide by reference][g].

[g]: docs/guide.md

```markdown
[never checked](docs/missing.md)
```
EOF
)
# readme_with <name> <text>: the clean README with <text> appended.
readme_with() {
  local root
  root=$(readme "$1" <<<"$readme_clean")
  printf '%s\n' "$2" >>"$root/README.md"
  echo "$root"
}
printf 'outside\n' >"$work/outside-target.md"

clean=$(readme readme-clean <<<"$readme_clean")
expect pass check-readme-links.py "$clean" "a README whose links, anchors, images and HTML attributes resolve"

expect fail check-readme-links.py "$(readme_with readme-link-missing '[x](docs/missing.md)')" \
  "a Markdown link to a missing file"
expect fail check-readme-links.py "$(readme_with readme-anchor-own '[x](#no-such-heading)')" \
  "a heading anchor that is not in the README"
expect fail check-readme-links.py "$(readme_with readme-anchor-other '[x](docs/guide.md#no-such-heading)')" \
  "a heading anchor that is not in the file it names"
expect fail check-readme-links.py "$(readme_with readme-image-missing '![x](img/missing.png)')" \
  "a Markdown image path that does not resolve"
expect fail check-readme-links.py "$(readme_with readme-html-src '<img src="img/missing.png" alt="x">')" \
  "an HTML img src that does not resolve"
expect fail check-readme-links.py \
  "$(readme_with readme-html-srcset '<img src="img/logo.png" srcset="img/logo.png 1x, img/missing.png 2x" alt="x">')" \
  "an HTML srcset candidate that does not resolve"
expect fail check-readme-links.py "$(readme_with readme-html-href '<a href="docs/missing.md">x</a>')" \
  "an HTML href that does not resolve"
expect fail check-readme-links.py "$(readme_with readme-html-quoted-gt '<a title="a > b" href="docs/missing.md">x</a>')" \
  "an HTML href after a > inside a quoted attribute value"
expect fail check-readme-links.py "$(readme_with readme-reference-missing $'[x][bad]\n\n[bad]: docs/missing.md')" \
  "a reference definition whose target does not resolve"
expect fail check-readme-links.py "$(readme_with readme-case-differs '[x](docs/Guide.md)')" \
  "a link whose case differs from the file on disk"
expect fail check-readme-links.py "$(readme_with readme-escapes-root '[x](../outside-target.md)')" \
  "a link that leaves the repository, even to a file that exists beside it"
expect fail check-readme-links.py "$(readme_with readme-live-after-code-span '```x``` [y](docs/missing.md)')" \
  "a live link after a triple-backtick code span on the same line"
expect fail check-readme-links.py "$(tree readme-absent)" "a repository with no README.md"

# --- check-store-format-claims.py: marked current-format claims in the store spec ---
# format_tree <name> <FORMAT>: a fixture root whose db/codec.rs declares FORMAT
# and whose store spec is read from stdin.
format_tree() {
  local root
  root=$(tree "$1")
  mkdir -p "$root/crates/scribe-store/src/db" "$root/docs/design/scribe/specs"
  printf 'pub(super) const FORMAT: u32 = %s;\n' "$2" >"$root/crates/scribe-store/src/db/codec.rs"
  cat >"$root/docs/design/scribe/specs/store.md"
  echo "$root"
}
claims_clean='The code writes format <!-- current-format -->30 and reads formats 1–<!-- current-format -->30.
Format 29 adds groups. The format-28 release stamped `control.json` at 28.
The current format-<!-- current-format -->30 build stamps `control.json` at <!-- current-stamp -->30.'
expect pass check-store-format-claims.py "$(format_tree claims-clean 30 <<<"$claims_clean")" \
  "marked claims that match FORMAT beside historical format statements"
expect fail check-store-format-claims.py "$(format_tree claims-bumped 31 <<<"$claims_clean")" \
  "marked claims left at 30 after FORMAT moved to 31"
expect fail check-store-format-claims.py \
  "$(format_tree claims-stale-format 30 <<<"${claims_clean/writes format <!-- current-format -->30/writes format <!-- current-format -->29}")" \
  "a marked current-format claim that differs from FORMAT"
expect fail check-store-format-claims.py \
  "$(format_tree claims-stale-stamp 30 <<<"${claims_clean/at <!-- current-stamp -->30/at <!-- current-stamp -->28}")" \
  "a marked current control-stamp claim that differs from FORMAT"
expect fail check-store-format-claims.py \
  "$(format_tree claims-no-number 30 <<<"${claims_clean/at <!-- current-stamp -->30/at <!-- current-stamp -->thirty}")" \
  "a marker not followed by a number"
expect fail check-store-format-claims.py \
  "$(format_tree claims-bad-kind 30 <<<"${claims_clean/at <!-- current-stamp -->30/at <!-- current-stmap -->30}")" \
  "a marker of an unknown kind"
expect fail check-store-format-claims.py \
  "$(format_tree claims-fraction-format 30 <<<"${claims_clean/writes format <!-- current-format -->30/writes format <!-- current-format -->30.1}")" \
  "a marked current-format claim that is a fraction (30.1) of FORMAT"
expect fail check-store-format-claims.py \
  "$(format_tree claims-fraction-stamp 30 <<<"${claims_clean/at <!-- current-stamp -->30/at <!-- current-stamp -->30.1}")" \
  "a marked current-stamp claim that is a fraction (30.1) of FORMAT"
expect pass check-store-format-claims.py "$(format_tree claims-punctuated 30 <<<'The code writes format <!-- current-format -->30. Reads formats 1–<!-- current-format -->30.
Stamps `control.json` at <!-- current-stamp -->30-build.')" \
  "marked claims followed by sentence punctuation or a build hyphen"
expect fail check-store-format-claims.py "$(format_tree claims-unmarked 30 <<<'The code writes format 30.')" \
  "a spec with no marked claim"
expect fail check-store-format-claims.py "$(tree claims-absent)" "a tree with no codec.rs or store spec"

# The published session-ID tables must match the same matrix as CLI tests.
outcomes=$(tree outcomes)
mkdir -p "$outcomes/scripts" "$outcomes/docs/design/scribe"
cp "$here/session-id-outcomes.json" "$outcomes/scripts/"
cp "$here/../docs/guide.md" "$outcomes/docs/guide.md"
cp "$here/../docs/design/scribe/2_design.md" "$outcomes/docs/design/scribe/2_design.md"
expect pass check-session-id-outcomes.py "$outcomes" "matching session-ID outcome tables"
sed -i.bak 's/Refused as not switched on/Stale outcome/' "$outcomes/docs/guide.md"
expect fail check-session-id-outcomes.py "$outcomes" "a stale session-ID outcome table"

# --- check-terminal-guard.sh: one terminal decision, and its environment names --
# Composition captures the log and terminal names; the output module decides
# the terminal; any other module that decides, prints or names one fails
# (STD-01 R17, STD-02 R3). A doc comment or a test names one harmlessly.
guard_tree() {
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src" "$root/crates/scribe/src/output/tests" \
    "$root/crates/scribe/src/commands"
  printf '//! composition\npub const LOG_ENV: &str = "SCRIBE_LOG";\n' \
    >"$root/crates/scribe-core/src/composition.rs"
  printf '//! output\npub(crate) fn decide() -> bool {\n    std::io::stdout().is_terminal() && cw_cli::Terminal::detect(&env, "scribe", None).is_some()\n}\n' \
    >"$root/crates/scribe/src/output/mod.rs"
  printf '#[test]\nfn no_color() {\n    let _ = Env::from_vars([("NO_COLOR", "1")]);\n}\n' \
    >"$root/crates/scribe/src/output/tests/render.rs"
  printf '/// Output format [default: $SCRIBE_FORMAT, else auto]\npub(crate) fn run() {}\n' \
    >"$root/crates/scribe/src/commands/entry.rs"
  printf 'pub(crate) fn cli() {}\n' >"$root/crates/scribe/src/cli.rs"
  echo "$root"
}

clean=$(guard_tree guard-clean)
expect pass check-terminal-guard.sh "$clean" \
  "composition captures the log name, output decides, a doc comment and a test name a variable"

decision=$(guard_tree guard-second-decision)
printf 'pub(crate) fn f() -> bool {\n    std::io::stdout().is_terminal()\n}\n' \
  >>"$decision/crates/scribe/src/commands/entry.rs"
expect fail check-terminal-guard.sh "$decision" "a second terminal decision in a command module"

probe=$(guard_tree guard-probe)
printf 'pub(crate) fn g() {\n    let _ = cw_cli::Probe::detect();\n}\n' \
  >>"$probe/crates/scribe/src/cli.rs"
expect fail check-terminal-guard.sh "$probe" "a terminal probe in the CLI outside output"

printing=$(guard_tree guard-print)
printf 'pub(crate) fn h() {\n    println!("hi");\n}\n' >>"$printing/crates/scribe/src/commands/entry.rs"
expect fail check-terminal-guard.sh "$printing" "a print from a command module"

raw=$(guard_tree guard-raw-color)
printf 'pub(crate) fn k() -> bool {\n    std::env::var_os("NO_COLOR").is_some()\n}\n' \
  >>"$raw/crates/scribe/src/commands/entry.rs"
expect fail check-terminal-guard.sh "$raw" "a raw NO_COLOR read in a command module"

log=$(guard_tree guard-log-output)
printf 'pub(crate) fn log_name() -> &'\''static str {\n    "SCRIBE_LOG"\n}\n' \
  >>"$log/crates/scribe/src/output/mod.rs"
expect fail check-terminal-guard.sh "$log" "the output module naming SCRIBE_LOG itself"

core=$(guard_tree guard-core-format)
printf 'pub fn format_name() -> &'\''static str {\n    "SCRIBE_FORMAT"\n}\n' \
  >"$core/crates/scribe-core/src/format_name.rs"
expect fail check-terminal-guard.sh "$core" "a core module naming SCRIBE_FORMAT"

# An unusable root scans nothing, so the guard must fail closed (STD-04
# R10): no crates tree, a file named crates, or no Rust sources under
# crates/. A valid clean tree still passes (guard-clean above); grep's
# ordinary no-match stays a pass.
empty_root="$work/guard-no-crates"
mkdir -p "$empty_root"
expect fail check-terminal-guard.sh "$empty_root" "a root with no crates tree"
if ! grep -Fq "terminal-guard: cannot scan" "$work/out"; then
  echo "check-gates: FAIL: missing cannot-scan diagnostic for a root with no crates" >&2
  fail=1
fi

file_root="$work/guard-crates-file"
mkdir -p "$file_root"
printf 'not a directory\n' >"$file_root/crates"
expect fail check-terminal-guard.sh "$file_root" "a root where crates is a plain file"
if ! grep -Fq "terminal-guard: cannot scan" "$work/out"; then
  echo "check-gates: FAIL: missing cannot-scan diagnostic for a crates file" >&2
  fail=1
fi

norust_root="$work/guard-no-rust"
mkdir -p "$norust_root/crates/scribe/src"
printf '[package]\nname = "scribe"\n' >"$norust_root/crates/scribe/Cargo.toml"
expect fail check-terminal-guard.sh "$norust_root" "a crates tree with no Rust sources"
if ! grep -Fq "terminal-guard: cannot scan" "$work/out"; then
  echo "check-gates: FAIL: missing cannot-scan diagnostic for a crates tree with no Rust" >&2
  fail=1
fi

# Scan files come from mktemp. With TMPDIR absent none can be created, and a
# redirection into a missing file exits 1, the status of a clean no-match. So
# the guard must fail with its cannot-scan diagnostic and never print ok, for
# a tree with a violation and for a clean one (STD-04 R10). The usable-TMPDIR
# controls show the same trees decide on their content, so the absent
# directory is the only cause of the failure.
absent_tmp="$work/absent-tmpdir"
guard_violation=$(guard_tree guard-violation-tmp)
printf 'pub(crate) fn h() {\n println!("hi");\n}\n' >>"$guard_violation/crates/scribe/src/commands/entry.rs"
guard_clean=$(guard_tree guard-clean-tmp)

for tree in "$guard_violation" "$guard_clean"; do
  TMPDIR="$absent_tmp" expect fail check-terminal-guard.sh "$tree" \
    "a tree with TMPDIR absent, so no scan file can be created"
  if ! grep -Fq "terminal-guard: cannot scan" "$work/out" || grep -Fq "terminal-guard: ok" "$work/out"; then
    echo "check-gates: FAIL: $tree with TMPDIR absent did not report cannot-scan without ok" >&2
    fail=1
  fi
done
TMPDIR="$work" expect pass check-terminal-guard.sh "$guard_clean" \
  "a clean readable tree with a usable TMPDIR has no matches and passes"
TMPDIR="$work" expect fail check-terminal-guard.sh "$guard_violation" \
  "a terminal violation with a usable TMPDIR still fails"

# A root whose only Rust sources sit under excluded `tests/` directories scans
# zero eligible files, so it must fail closed with an actionable diagnostic
# (STD-04 R10) rather than reporting ok. Adding an eligible non-test source
# lets the same root pass.
tests_only="$work/guard-tests-only"
mkdir -p "$tests_only/crates/scribe/tests"
printf '#[test]\nfn example() { println!("ok in test"); }\n' \
  >"$tests_only/crates/scribe/tests/example.rs"
expect fail check-terminal-guard.sh "$tests_only" \
  "a root containing only crates/scribe/tests/*.rs (excluded from scan)"
if ! grep -Fq "terminal-guard: cannot scan" "$work/out" || \
  ! grep -Fq "tests" "$work/out" || \
  grep -Fq "terminal-guard: ok" "$work/out"; then
  echo "check-gates: FAIL: $tests_only did not report an actionable cannot-scan diagnostic mentioning tests/" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

nested_tests_only="$work/guard-nested-tests-only"
mkdir -p "$nested_tests_only/crates/scribe/src/output/tests"
printf '#[test]\nfn render() { println!("ok in test"); }\n' \
  >"$nested_tests_only/crates/scribe/src/output/tests/render.rs"
expect fail check-terminal-guard.sh "$nested_tests_only" \
  "a root containing only nested src/.../tests/*.rs (excluded from scan)"
if ! grep -Fq "terminal-guard: cannot scan" "$work/out" || grep -Fq "terminal-guard: ok" "$work/out"; then
  echo "check-gates: FAIL: $nested_tests_only did not report cannot-scan without ok" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

mkdir -p "$tests_only/crates/scribe/src"
printf 'pub fn eligible() {}\n' >"$tests_only/crates/scribe/src/lib.rs"
expect pass check-terminal-guard.sh "$tests_only" \
  "the same root passes once an eligible non-test Rust source exists"

# A deterministic grep wrapper returning 2 for any of the three filtering calls
# (comment filter in scan(), terminal allowlist filter, env allowlist filter)
# must fail nonzero rather than masking the error behind `|| true` and passing a
# tree with a violation or a clean eligible tree (STD-04 R10).
real_grep=$(command -v grep)
grep_wrap_dir="$work/grep-wrapper"
mkdir -p "$grep_wrap_dir"
cat >"$grep_wrap_dir/grep" <<EOF
#!/usr/bin/env bash
case "\${FAIL_FILTER_KIND:-}" in
  comment)
    for arg in "\$@"; do
      if [[ "\$arg" == "-vE" ]]; then
        echo "grep: simulated comment filter failure" >&2
        exit 2
      fi
    done
    ;;
  terminal-allow)
    for arg in "\$@"; do
      if [[ "\$arg" == "^crates/scribe/src/output/" ]]; then
        echo "grep: simulated terminal allowlist filter failure" >&2
        exit 2
      fi
    done
    ;;
  env-allow)
    for arg in "\$@"; do
      if [[ "\$arg" == "^crates/scribe-core/src/composition.rs:" ]]; then
        echo "grep: simulated env allowlist filter failure" >&2
        exit 2
      fi
    done
    ;;
esac
exec "$real_grep" "\$@"
EOF
chmod +x "$grep_wrap_dir/grep"

guard_env_violation=$(guard_tree guard-env-violation-filter)
printf 'pub(crate) fn f() -> &'\''static str {\n    "SCRIBE_FORMAT"\n}\n' \
  >>"$guard_env_violation/crates/scribe/src/commands/entry.rs"

for kind in comment terminal-allow env-allow; do
  case "$kind" in
    comment|terminal-allow) bad_tree="$guard_violation" ;;
    env-allow) bad_tree="$guard_env_violation" ;;
  esac
  for target_tree in "$bad_tree" "$guard_clean"; do
    FAIL_FILTER_KIND="$kind" PATH="$grep_wrap_dir:$PATH" \
      expect fail check-terminal-guard.sh "$target_tree" \
      "filter failure ($kind) on $target_tree must exit nonzero"
    if grep -Fq "terminal-guard: ok" "$work/out"; then
      echo "check-gates: FAIL: filter failure ($kind) printed terminal-guard: ok" >&2
      sed 's/^/  | /' "$work/out" >&2
      fail=1
    fi
    if ! grep -Fq "terminal-guard: scan failed" "$work/out"; then
      echo "check-gates: FAIL: filter failure ($kind) missing scan-failed diagnostic" >&2
      sed 's/^/  | /' "$work/out" >&2
      fail=1
    fi
  done
done

expect pass check-terminal-guard.sh "$guard_clean" \
  "ordinary filter no-match on a clean eligible tree still passes"
expect fail check-terminal-guard.sh "$guard_env_violation" \
  "an env violation with ordinary grep still fails"

# --- clippy.toml: disallowed host operations in scribe-core (STD-02 R6) ----
# Build a minimal workspace inheriting [workspace.lints] and clippy.toml so
# every host operation banned in scribe-core fails regardless of path spelling
# (qualified or imported by name), while a clean twin passes. These cases compile
# small fixture crates, so make ci-fast (through structure) needs clippy; a
# missing clippy fails here rather than skipping (STD-04 R10).
if ! command -v cargo >/dev/null 2>&1 || ! cargo clippy --version >/dev/null 2>&1; then
  echo "check-gates: FAIL: cargo clippy is required for clippy boundary cases" >&2
  exit 2
fi

clippy_lints=$(awk '
  /^\[workspace\.lints\.clippy\]$/ { on = 1 }
  on { print }
' "$REPO_ROOT/Cargo.toml")

clippy_tree() {
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src"
  cp "$REPO_ROOT/clippy.toml" "$root/clippy.toml"
  cat >"$root/Cargo.toml" <<EOF
[workspace]
resolver = "3"
members = ["crates/scribe-core"]

[workspace.package]
version = "0.3.0"
edition = "2024"
rust-version = "1.89"

$clippy_lints
EOF
  cat >"$root/crates/scribe-core/Cargo.toml" <<'EOF'
[package]
name = "scribe-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true

[lints]
workspace = true
EOF
  printf '//! Core fixture.\npub mod config;\n' >"$root/crates/scribe-core/src/lib.rs"
  cat >"$root/crates/scribe-core/src/config.rs"
  echo "$root"
}

expect_clippy() {
  local want="$1" root="$2" lint="$3" what="$4" got=pass
  n=$((n + 1))
  (
    cd "$root" &&
      CARGO_TARGET_DIR="$root/target" CLIPPY_CONF_DIR="$root" \
        cargo clippy --workspace --all-targets --offline -- -D warnings
  ) >"$work/out" 2>&1 || got=fail
  if [[ "$got" != "$want" ]]; then
    echo "check-gates: FAIL: clippy.toml on $what: expected $want, got $got" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  elif [[ -n "$lint" ]] && ! grep -Fq "$lint" "$work/out"; then
    echo "check-gates: FAIL: clippy.toml on $what: missing diagnostic $lint" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
}

clippy_clean=$(clippy_tree clippy-clean <<'EOF'
//! Clean core config fixture.

/// Returns a constant without touching host I/O, environment or process exit.
pub fn ok() -> usize {
    1
}
EOF
)
expect_clippy pass "$clippy_clean" "" "a clean scribe-core config module"

expect_clippy fail "$(clippy_tree clippy-dirbuilder-qualified <<'EOF'
//! Core config fixture.

/// Creates a directory directly via qualified std::fs::DirBuilder.
pub fn make_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().create(path)
}
EOF
)" "clippy::disallowed-types" "scribe-core calling std::fs::DirBuilder::new().create"

expect_clippy fail "$(clippy_tree clippy-dirbuilder-named <<'EOF'
//! Core config fixture.

use std::fs::DirBuilder;

/// Creates a directory directly via named DirBuilder import.
pub fn make_dir(path: &std::path::Path) -> std::io::Result<()> {
    DirBuilder::new().create(path)
}
EOF
)" "clippy::disallowed-types" "scribe-core importing std::fs::DirBuilder by name"

expect_clippy fail "$(clippy_tree clippy-dirbuilder-grouped <<'EOF'
//! Core config fixture.

use std::fs::{self, DirBuilder};

/// Creates a directory directly via grouped DirBuilder import.
pub fn make_dir(path: &std::path::Path) -> std::io::Result<()> {
    let _ = fs::DirBuilder::new();
    DirBuilder::new().create(path)
}
EOF
)" "clippy::disallowed-types" "scribe-core importing std::fs::DirBuilder in a group"

expect_clippy fail "$(clippy_tree clippy-temp-dir-qualified <<'EOF'
//! Core config fixture.

/// Discovers the host temp directory via qualified std::env::temp_dir.
pub fn tmp() -> std::path::PathBuf {
    std::env::temp_dir()
}
EOF
)" "clippy::disallowed-methods" "scribe-core calling std::env::temp_dir()"

expect_clippy fail "$(clippy_tree clippy-temp-dir-named <<'EOF'
//! Core config fixture.

use std::env::temp_dir;

/// Discovers the host temp directory via named temp_dir import.
pub fn tmp() -> std::path::PathBuf {
    temp_dir()
}
EOF
)" "clippy::disallowed-methods" "scribe-core importing std::env::temp_dir by name"

expect_clippy fail "$(clippy_tree clippy-exit-qualified <<'EOF'
//! Core config fixture.

/// Terminates the process directly via qualified std::process::exit.
pub fn bail() -> ! {
    std::process::exit(1)
}
EOF
)" "clippy::disallowed-methods" "scribe-core calling std::process::exit(1)"

expect_clippy fail "$(clippy_tree clippy-exit-named <<'EOF'
//! Core config fixture.

use std::process::exit;

/// Terminates the process directly via named exit import.
pub fn bail() -> ! {
    exit(1)
}
EOF
)" "clippy::disallowed-methods" "scribe-core importing std::process::exit by name"

# Copy each real module's inner expectation, then compile a representative
# allowed operation before adding an exit call. This exercises the independent
# lint under the same exemption that previously hid direct exit calls.
for adapter in composition cli fs; do
  case "$adapter" in
    composition) source="$REPO_ROOT/crates/scribe-core/src/composition.rs"
      operation='pub fn host() { let _ = std::env::var_os("PATH"); }' ;;
    cli) source="$REPO_ROOT/crates/scribe-core/src/providers/cli.rs"
      operation='pub fn host() { let _ = std::process::Command::new("unused").spawn(); }' ;;
    fs) source="$REPO_ROOT/crates/scribe-tools/src/fs.rs"
      operation='pub fn host() { let _ = std::fs::read("unused"); let _: Option<std::fs::File> = None; }' ;;
  esac
  fixture=$(clippy_tree "clippy-exit-$adapter" <<EOF
//! Host adapter fixture.
$(sed -n '/^#!\[expect(/,/^)]/p' "$source")
/// Exercises the approved host operation without running it.
$operation
EOF
  )
  # Place the twin at the production module's relative path.
  case "$adapter" in
    composition) module=composition ;;
    cli) module=providers/cli ;;
    fs) module=fs ;;
  esac
  fixture_src="$fixture/crates/scribe-core/src"
  mkdir -p "$(dirname "$fixture_src/$module.rs")"
  mv "$fixture_src/config.rs" "$fixture_src/$module.rs"
  if [[ "$adapter" == cli ]]; then
    printf '//! Core fixture.\n/// Provider fixtures.\npub mod providers { pub mod cli; }\n' >"$fixture_src/lib.rs"
  else
    printf '//! Core fixture.\npub mod %s;\n' "$module" >"$fixture_src/lib.rs"
  fi
  expect_clippy pass "$fixture" "" "approved $adapter host operation"
  cat >>"$fixture_src/$module.rs" <<'EOF'
/// Direct exit must remain forbidden under the adapter expectation.
pub fn bail() -> ! { std::process::exit(1) }
EOF
  expect_clippy fail "$fixture" "clippy::exit" "direct exit in the $adapter exemption"
done

# Compile the actual store file adapter with the pinned kit dependencies. The
# bad twin appends a raw open to that same module; both run the real lint
# configuration, without ever executing the open. The fixture resolves offline
# from the repository lock, so fetch that locked graph first: a cold cargo cache
# (CI runs this gate before any build) cannot check out the git kit offline.
if ! cargo fetch --locked --manifest-path "$REPO_ROOT/Cargo.toml" >"$work/fetch.out" 2>&1; then
  echo "check-gates: FAIL: cannot fetch the locked kit dependencies" >&2
  sed 's/^/  | /' "$work/fetch.out" >&2
  exit 1
fi
store_clippy="$work/clippy-store-files"
mkdir -p "$store_clippy/src/db"
cp "$REPO_ROOT/clippy.toml" "$store_clippy/clippy.toml"
cp "$REPO_ROOT/crates/scribe-store/src/db/files.rs" "$store_clippy/src/db/files.rs"
{
  printf '[workspace]\nresolver = "3"\n\n[package]\nname = "store-files-fixture"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n'
  grep -E '^cw-(fs|sqlite) = ' "$REPO_ROOT/Cargo.toml"
  printf '\n[lints]\nworkspace = true\n\n%s\n' "$clippy_lints"
} >"$store_clippy/Cargo.toml"
cp "$REPO_ROOT/Cargo.lock" "$store_clippy/Cargo.lock"
cat >"$store_clippy/src/lib.rs" <<'EOF'
//! Store file adapter fixture.
#![allow(dead_code)]
mod error {
    use std::path::{Path, PathBuf};
    pub enum Error { Symlink { path: PathBuf }, Other }
    pub type Result<T> = std::result::Result<T, Error>;
    impl Error {
        pub fn io(_: &str, _: &Path, _: std::io::Error) -> Self { Self::Other }
    }
    impl From<cw_fs::Error> for Error {
        fn from(_: cw_fs::Error) -> Self { Self::Other }
    }
}
mod db { mod files; }
EOF
expect_clippy pass "$store_clippy" "" "the production file adapter with scoped filesystem expectations"
cat >>"$store_clippy/src/db/files.rs" <<'EOF'

fn forbidden_exit() -> ! { std::process::exit(1) }
EOF
expect_clippy fail "$store_clippy" "clippy::exit" "direct exit in the production file adapter"
# Restore the adapter before testing the separate SQLite boundary.
cp "$REPO_ROOT/crates/scribe-store/src/db/files.rs" "$store_clippy/src/db/files.rs"
cat >>"$store_clippy/src/db/files.rs" <<'EOF'

fn forbidden_open(path: &std::path::Path) -> cw_sqlite::rusqlite::Result<cw_sqlite::rusqlite::Connection> {
    cw_sqlite::rusqlite::Connection::open(path)
}
EOF
expect_clippy fail "$store_clippy" "clippy::disallowed-methods" \
  "a raw SQLite open through the cw-sqlite re-export in the file adapter"
if ! grep -Fq "open scribe.db through cw-sqlite" "$work/out"; then
  echo "check-gates: FAIL: missing raw SQLite open diagnostic" >&2
  fail=1
fi

if [[ "$fail" -eq 0 ]]; then
  echo "check-gates: ok ($n cases)"
fi
exit "$fail"
