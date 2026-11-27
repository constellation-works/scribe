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

# --- check-source-style.sh: STD-12 R26 ----------------------------------
style_one_level="$work/style-one-level"
mkdir -p "$style_one_level/crates/scribe-core/src/apps/session"
printf 'use super::entry::Entry;\n' >"$style_one_level/crates/scribe-core/src/apps/session/mod.rs"
expect pass check-source-style.sh "$style_one_level" "a one-level super path"

style_multi_level="$work/style-multi-level"
mkdir -p "$style_multi_level/crates/scribe-core/src/apps/session/tests"
printf '//! See [`super::super::catalog`].\n' \
  >"$style_multi_level/crates/scribe-core/src/apps/session/tests/mod.rs"
expect fail check-source-style.sh "$style_multi_level" "a multi-level path in a rustdoc link"
if ! grep -Fq "STD-12 R26 forbids multi-level super paths" "$work/out"; then
  echo "check-gates: FAIL: source-style R26 fixture lacks its diagnostic" >&2
  fail=1
fi

# Fail closed when there is nothing to scan or enumeration/read fails
# (STD-04 R10): an empty crates/ tree, an unreadable Rust source (grep
# exits > 1), and an unreadable directory (find exits non-zero) must fail
# with a cannot-scan diagnostic and never print ok. The one-level clean
# tree above is the passing twin, so a gate that refuses everything cannot
# pass these cases.
style_empty="$work/style-empty"
mkdir -p "$style_empty/crates"
expect fail check-source-style.sh "$style_empty" "a crates tree with no Rust sources"
if ! grep -Fq "source-style: cannot scan" "$work/out" || grep -Fq "source-style: ok" "$work/out"; then
  echo "check-gates: FAIL: empty crates/ did not report cannot-scan without ok" >&2
  fail=1
fi

style_unreadable_file="$work/style-unreadable-file"
mkdir -p "$style_unreadable_file/crates/scribe/src"
printf 'use super::super::foo;\n' >"$style_unreadable_file/crates/scribe/src/lib.rs"
chmod 000 "$style_unreadable_file/crates/scribe/src/lib.rs"
expect fail check-source-style.sh "$style_unreadable_file" "an unreadable Rust source"
if ! grep -Fq "source-style: cannot scan" "$work/out" || grep -Fq "source-style: ok" "$work/out"; then
  echo "check-gates: FAIL: unreadable file did not report cannot-scan without ok" >&2
  fail=1
fi
chmod 644 "$style_unreadable_file/crates/scribe/src/lib.rs"

style_unreadable_dir="$work/style-unreadable-dir"
mkdir -p "$style_unreadable_dir/crates/scribe/src"
printf 'use super::super::foo;\n' >"$style_unreadable_dir/crates/scribe/src/lib.rs"
chmod 000 "$style_unreadable_dir/crates/scribe/src"
expect fail check-source-style.sh "$style_unreadable_dir" "an unreadable source directory"
if ! grep -Fq "source-style: cannot scan" "$work/out" || grep -Fq "source-style: ok" "$work/out"; then
  echo "check-gates: FAIL: unreadable directory did not report cannot-scan without ok" >&2
  fail=1
fi
chmod 755 "$style_unreadable_dir/crates/scribe/src"

# The plugin conformance test compiles into crates/scribe/tests/plugin.rs
# via #[path], so a multi-level path there fails the same R26 rule while
# its clean twin passes.
style_plugin_bad="$work/style-plugin-bad"
mkdir -p "$style_plugin_bad/crates/scribe/src" "$style_plugin_bad/plugin/.orbit-plugin/tests"
printf 'fn main() {}\n' >"$style_plugin_bad/crates/scribe/src/main.rs"
printf 'use super::super::x;\n' >"$style_plugin_bad/plugin/.orbit-plugin/tests/conformance.rs"
expect fail check-source-style.sh "$style_plugin_bad" "a multi-level path in plugin conformance.rs"
if ! grep -Fq "STD-12 R26 forbids multi-level super paths" "$work/out"; then
  echo "check-gates: FAIL: plugin conformance fixture lacks its R26 diagnostic" >&2
  fail=1
fi
if ! grep -Fq "plugin/.orbit-plugin/tests/conformance.rs" "$work/out"; then
  echo "check-gates: FAIL: plugin conformance fixture does not name the offending file" >&2
  fail=1
fi

style_plugin_clean="$work/style-plugin-clean"
mkdir -p "$style_plugin_clean/crates/scribe/src" "$style_plugin_clean/plugin/.orbit-plugin/tests"
printf 'fn main() {}\n' >"$style_plugin_clean/crates/scribe/src/main.rs"
printf 'use crate::x;\n' >"$style_plugin_clean/plugin/.orbit-plugin/tests/conformance.rs"
expect pass check-source-style.sh "$style_plugin_clean" "a clean plugin conformance.rs beside a clean crates/"

# tree <name>: a fresh fixture root holding a minimal two-crate workspace.
tree() {
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src" "$root/crates/scribe/src"
  printf '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n' >"$root/Cargo.toml"
  printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n' >"$root/crates/scribe-core/Cargo.toml"
  printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nscribe-core = { path = "../scribe-core" }\n' \
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

for decl in 'mod extra;' 'pub mod extra;' 'mod r#extra;' 'pub mod r#extra;'; do
  bad=$(tree "root-$n")
  printf '//! core\npub mod apps;\n%s\n' "$decl" >"$bad/crates/scribe-core/src/lib.rs"
  printf 'fn extra() {}\n' >"$bad/crates/scribe-core/src/extra.rs"
  expect fail check-dependency-direction.sh "$bad" "a root module declared \`$decl\` with no place"
done

# A raw identifier names the same root module as its plain spelling (ORB-28902):
# an unlisted raw root module fails with the canonical name in its diagnostic,
# and a raw spelling of a listed place still passes.
raw_root=$(tree root-raw-diagnostic)
printf '//! core\npub mod apps;\npub mod r#extra;\n' >"$raw_root/crates/scribe-core/src/lib.rs"
printf 'fn extra() {}\n' >"$raw_root/crates/scribe-core/src/extra.rs"
expect fail check-dependency-direction.sh "$raw_root" "a root module declared \`pub mod r#extra;\` with no place"
if ! grep -Fq "lib.rs declares the root module extra, which is not one of the places" "$work/out"; then
  echo "check-gates: FAIL: raw root module lacks the canonical diagnostic" >&2
  fail=1
fi
raw_place=$(tree root-raw-place)
printf '//! core\npub mod apps;\npub mod r#config;\n' >"$raw_place/crates/scribe-core/src/lib.rs"
expect pass check-dependency-direction.sh "$raw_place" "a raw spelling of a listed root place \`pub mod r#config;\`"

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
  printf '[package]\nname = "scribe-store"\nversion = "0.1.0"\n' >"$root/crates/scribe-store/Cargo.toml"
  printf '//! store\n' >"$root/crates/scribe-store/src/lib.rs"
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
  $'[dependencies]\nrenamed-core.workspace = true\n'; do
  for core_dep in \
    $'[dependencies]\nback = { path = "../scribe", package = "scribe" }\n' \
    $'[dependencies]\nback.path = "../scribe"\nback.package = "scribe"\n' \
    $'[dependencies.back]\npath = "../scribe"\npackage = "scribe"\n' \
    $'[target.\'cfg(unix)\'.dependencies.back]\npackage = "scribe"\npath = "../scribe"\n' \
    $'[build-dependencies]\nscribe = { path = "../scribe" }\n'; do
    n_scribe=$((n_scribe + 1))
    mixed=$(tree "cycle-crate-mixed-$n_scribe")
    printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n\n[workspace.dependencies]\nrenamed-core = { path = "crates/scribe-core", package = "scribe-core" }\n' >"$mixed/Cargo.toml"
    printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n%s' "$scribe_dep" >"$mixed/crates/scribe/Cargo.toml"
    printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n%s' "$core_dep" >"$mixed/crates/scribe-core/Cargo.toml"
    expect fail check-dependency-direction.sh "$mixed" "a crate cycle through mixed dependency forms ($n_scribe)"
    if ! grep -Fq "crate import cycle:" "$work/out"; then
      echo "check-gates: FAIL: missing crate import cycle diagnostic for mixed forms ($n_scribe)" >&2
      cat "$work/out" >&2
      fail=1
    fi
  done
done

# Quoted structural keys, whitespace in target headers and root workspace
# dotted declarations resolve to the same package as their conventional forms.
for form in quoted-inline quoted-dotted quoted-top-dotted quoted-subtable quoted-attr-subkey spaced-target workspace-dotted workspace-top-dotted workspace-table; do
  for package in rusqlite serde; do
    fixture=$(tree "dependency-spelling-$form-$package")
    case "$form" in
      quoted-inline) decl="[dependencies]
sql = { version = '1', 'package' = '$package' }" ;;
      quoted-dotted) decl="[dependencies]
sql.\"package\" = '$package'
sql.version = '1'" ;;
      quoted-top-dotted) decl="'dependencies' . sql = { version = '1', package = '$package' }" ;;
      quoted-subtable) decl="[\"dependencies\" . sql]
'package' = '$package'
version = '1'" ;;
      quoted-attr-subkey) decl="[dependencies]
$package.\"version\" = '1'" ;;
      spaced-target) decl="[target . 'cfg(unix)' . \"dependencies\"]
$package = '1'" ;;
      workspace-top-dotted)
        printf 'workspace.members = ["crates/scribe", "crates/scribe-core"]\nworkspace.dependencies.sql = { version = "1", package = "%s" }\n' "$package" >"$fixture/Cargo.toml"
        decl="[dependencies]
sql.'workspace' = true" ;;
      workspace-dotted|workspace-table)
        if [[ "$form" == workspace-dotted ]]; then
          printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\ndependencies.sql = { version = "1", package = "%s" }\n' "$package" >"$fixture/Cargo.toml"
        else
          printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n[workspace.dependencies]\nsql = { version = "1", package = "%s" }\n' "$package" >"$fixture/Cargo.toml"
        fi
        decl="[dependencies]
sql.'workspace' = true" ;;
    esac
    if [[ "$form" == quoted-top-dotted ]]; then
      printf '%s\n\n[package]\nname = "scribe"\nversion = "0.1.0"\n' "$decl" >"$fixture/crates/scribe/Cargo.toml"
    else
      printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n%s\n' "$decl" >"$fixture/crates/scribe/Cargo.toml"
    fi
    if [[ "$package" == rusqlite ]]; then
      expect fail check-dependency-direction.sh "$fixture" "$form resolves banned rusqlite"
      if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
        echo "check-gates: FAIL: missing rusqlite diagnostic for $form" >&2
        fail=1
      fi
    else
      expect pass check-dependency-direction.sh "$fixture" "$form resolves allowed serde"
    fi
  done
done

# A quoted dotted subkey declares its dependency as the bare spelling does: the
# `scribe."path"` back edge closes the crate cycle, and its acyclic twin passes.
# A root `workspace.dependencies.<alias>` written as top-level dotted keys
# (no [workspace] header) names the package a member inherits (ORB-29129).
cyc_quoted=$(tree cycle-crate-quoted-subkey)
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nscribe."path" = "../scribe"\n' \
  >"$cyc_quoted/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$cyc_quoted" "a crate cycle closed by a quoted dotted subkey"
if ! grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing crate import cycle diagnostic for a quoted dotted subkey" >&2
  fail=1
fi
cyc_quoted_ok=$(tree cycle-crate-quoted-subkey-clean)
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nserde."version" = "1"\n' \
  >"$cyc_quoted_ok/crates/scribe-core/Cargo.toml"
expect pass check-dependency-direction.sh "$cyc_quoted_ok" "an acyclic quoted dotted subkey"

cyc_ws_top=$(tree cycle-workspace-top-dotted)
printf 'workspace.members = ["crates/scribe", "crates/scribe-core"]\nworkspace.dependencies.back = { path = "crates/scribe", package = "scribe" }\n' \
  >"$cyc_ws_top/Cargo.toml"
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nback.workspace = true\n' \
  >"$cyc_ws_top/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$cyc_ws_top" "a crate cycle through a top-level workspace.dependencies alias"
if ! grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing crate import cycle diagnostic for a top-level workspace alias" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

# A dotted `workspace.dependencies.*` line under [workspace.metadata] or
# [package.metadata] is metadata, not a workspace dependency (ORB-29133): it
# must neither mask a banned crate / cycle edge nor taint a clean alias.
meta_masked=$(tree cycle-workspace-metadata-masked)
printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n\n[workspace.dependencies]\nback = { path = "crates/scribe", package = "scribe" }\n\n[workspace.metadata]\nworkspace.dependencies.back = { package = "serde" }\n' \
  >"$meta_masked/Cargo.toml"
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nback.workspace = true\n' \
  >"$meta_masked/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$meta_masked" "a crate cycle masked by [workspace.metadata] still fails"
if ! grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing crate import cycle diagnostic for a [workspace.metadata] mask" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

meta_clean=$(tree workspace-metadata-clean)
printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n\n[workspace.dependencies]\nmydep = { path = "crates/scribe-core", package = "scribe-core" }\n\n[workspace.metadata]\nworkspace.dependencies.mydep = { package = "rusqlite" }\n' \
  >"$meta_clean/Cargo.toml"
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nmydep.workspace = true\n' \
  >"$meta_clean/crates/scribe/Cargo.toml"
expect pass check-dependency-direction.sh "$meta_clean" "a clean alias with a [workspace.metadata] rusqlite mask"

pkgmeta_masked=$(tree cycle-package-metadata-masked)
rm -rf "$pkgmeta_masked/crates/scribe"
mkdir -p "$pkgmeta_masked/src"
printf 'fn main() {}\n' >"$pkgmeta_masked/src/main.rs"
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nscribe-core = { path = "crates/scribe-core" }\n\n[workspace]\nmembers = ["crates/scribe-core"]\n\n[workspace.dependencies]\nback = { path = ".", package = "scribe" }\n\n[package.metadata]\nworkspace.dependencies.back = { package = "serde" }\n' \
  >"$pkgmeta_masked/Cargo.toml"
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nback.workspace = true\n' \
  >"$pkgmeta_masked/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$pkgmeta_masked" "a crate cycle masked by [package.metadata] still fails"
if ! grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing crate import cycle diagnostic for a [package.metadata] mask" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

pkgmeta_clean=$(tree package-metadata-clean)
rm -rf "$pkgmeta_clean/crates/scribe"
mkdir -p "$pkgmeta_clean/src"
printf 'fn main() {}\n' >"$pkgmeta_clean/src/main.rs"
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nmydep.workspace = true\n\n[workspace]\nmembers = ["crates/scribe-core"]\n\n[workspace.dependencies]\nmydep = { path = "crates/scribe-core", package = "scribe-core" }\n\n[package.metadata]\nworkspace.dependencies.mydep = { package = "rusqlite" }\n' \
  >"$pkgmeta_clean/Cargo.toml"
expect pass check-dependency-direction.sh "$pkgmeta_clean" "a clean alias with a [package.metadata] rusqlite mask"

# Top-level multiline arrays and multiline strings whose continuation lines
# begin with `[` must not clear top-level state (`top`) before subsequent
# `workspace.dependencies.*` declarations (ORB-29207).
for ml_kind in array basic-str literal-str; do
  case "$ml_kind" in
    array)
      ml_val=$'workspace.metadata.grid = [\n  ["a"],\n]' ;;
    basic-str)
      ml_val=$'workspace.metadata.note = """\n[note]\nworkspace.dependencies.clean = { package = "rusqlite" }\n"""' ;;
    literal-str)
      ml_val=$'workspace.metadata.note = \x27\x27\x27\n[note]\nworkspace.dependencies.clean = { package = "rusqlite" }\n\x27\x27\x27' ;;
  esac

  ml_bad=$(tree "multiline-$ml_kind-bad")
  printf 'workspace.members = ["crates/scribe", "crates/scribe-core"]\n%s\nworkspace.dependencies.bad = { path = "crates/scribe", package = "scribe" }\n' \
    "$ml_val" >"$ml_bad/Cargo.toml"
  printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nbad.workspace = true\n' \
    >"$ml_bad/crates/scribe-core/Cargo.toml"
  expect fail check-dependency-direction.sh "$ml_bad" "a crate cycle after a top-level multiline $ml_kind starting with ["
  if ! grep -Fq "crate import cycle:" "$work/out"; then
    echo "check-gates: FAIL: missing crate import cycle diagnostic after multiline $ml_kind" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi

  ml_banned=$(tree "multiline-$ml_kind-banned")
  printf 'workspace.members = ["crates/scribe", "crates/scribe-core"]\n%s\nworkspace.dependencies.sql = { version = "0.43", package = "rusqlite" }\n' \
    "$ml_val" >"$ml_banned/Cargo.toml"
  printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nscribe-core = { path = "../scribe-core" }\nsql.workspace = true\n' \
    >"$ml_banned/crates/scribe/Cargo.toml"
  expect fail check-dependency-direction.sh "$ml_banned" "a banned crate after a top-level multiline $ml_kind starting with ["
  if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
    echo "check-gates: FAIL: missing rusqlite diagnostic after multiline $ml_kind" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi

  ml_clean=$(tree "multiline-$ml_kind-clean")
  printf 'workspace.members = ["crates/scribe", "crates/scribe-core"]\n%s\nworkspace.dependencies.clean = { path = "crates/scribe-core", package = "scribe-core" }\n' \
    "$ml_val" >"$ml_clean/Cargo.toml"
  printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nclean.workspace = true\n' \
    >"$ml_clean/crates/scribe/Cargo.toml"
  expect pass check-dependency-direction.sh "$ml_clean" "a clean alias after a top-level multiline $ml_kind starting with ["
done

# A cycle closed by a top-level dotted dependency after a multiline array with
# a bare nested array (no trailing comma) cites the declaration line (ORB-29298).
ml_cycle_no_comma=$(tree multiline-cycle-no-comma)
printf 'package.name = "scribe-core"\npackage.version = "0.1.0"\npackage.edition = "2024"\npackage.metadata.grid = [\n["row"]\n]\ndependencies.back = { path = "../scribe", package = "scribe" }\n' \
  >"$ml_cycle_no_comma/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$ml_cycle_no_comma" "a crate cycle closed by a dotted dependency after a bare nested array"
if ! grep -Fq "crate import cycle:" "$work/out" || ! grep -Fq "crates/scribe-core/Cargo.toml:7" "$work/out"; then
  echo "check-gates: FAIL: the cycle diagnostic does not cite crates/scribe-core/Cargo.toml:7 after a bare nested array" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

# A multiline string can close on the same line as its enclosing array. Keep
# the trailing `]` visible to array-depth tracking so the next dependency is
# still located (and the cycle diagnostic cites its declaration line).
for ml_kind in basic-str literal-str; do
  case "$ml_kind" in
    basic-str) ml_val=$'package.metadata.rows = ["""row\n"""]' ;;
    literal-str) ml_val=$(printf "package.metadata.rows = ['''row\n''']") ;;
  esac

  ml_cycle_string=$(tree "multiline-cycle-string-$ml_kind")
  printf 'package.name = "scribe-core"\npackage.version = "0.1.0"\npackage.edition = "2024"\n%s\ndependencies.back = { path = "../scribe", package = "scribe" }\n' \
    "$ml_val" >"$ml_cycle_string/crates/scribe-core/Cargo.toml"
  expect fail check-dependency-direction.sh "$ml_cycle_string" "a crate cycle after a multiline $ml_kind closed with its array"
  if ! grep -Fq "crate import cycle:" "$work/out" || ! grep -Fq "crates/scribe-core/Cargo.toml:6" "$work/out"; then
    echo "check-gates: FAIL: the cycle diagnostic does not cite crates/scribe-core/Cargo.toml:6 after a multiline $ml_kind closed with its array" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done

# Member top-level dotted `dependencies.*` after a multiline array starting with
# `[` stays enforced, while `[package.metadata]` dotted keys stay ignored.
member_ml_bad=$(tree member-multiline-bad)
printf 'package.name = "scribe"\npackage.version = "0.1.0"\npackage.metadata.grid = [\n  ["a"],\n]\ndependencies.sql = { version = "0.43", package = "rusqlite" }\n' \
  >"$member_ml_bad/crates/scribe/Cargo.toml"
expect fail check-dependency-direction.sh "$member_ml_bad" "member top-level dotted dependency after multiline array starting with ["
if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
  echo "check-gates: FAIL: missing rusqlite diagnostic for member top-level multiline array" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

member_meta_clean=$(tree member-metadata-clean)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nscribe-core = { path = "../scribe-core" }\n\n[package.metadata]\ndependencies.rusqlite = "0.43"\n' \
  >"$member_meta_clean/crates/scribe/Cargo.toml"
expect pass check-dependency-direction.sh "$member_meta_clean" "member [package.metadata] dotted dependencies.rusqlite is ignored"

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

# A TOML comment is not part of a declaration: a `package = ...` after a `#`
# names no package. The legal serde dependency passes with a banned name in its
# comment; the banned rusqlite dependency fails with a legal name in its comment.
for comment_legal in \
  $'[dependencies]\nserde = "1" # package = "rusqlite"\n' \
  $'[dependencies]\nserde = { version = "1" } # package = "rusqlite"\n' \
  $'[dependencies.serde]\nversion = "1" # package = "rusqlite"\n'; do
  n_scribe=$((n_scribe + 1))
  legal=$(tree "comment-legal-$n_scribe")
  printf '[package]\nname = "scribe"\n\n%s' "$comment_legal" >"$legal/crates/scribe/Cargo.toml"
  expect pass check-dependency-direction.sh "$legal" "a legal dependency with a banned name in its comment ($n_scribe)"
  if grep -Fq "rusqlite" "$work/out"; then
    echo "check-gates: FAIL: a banned name in a trailing comment was reported ($n_scribe)" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done
for comment_banned in \
  $'[dependencies]\nrusqlite = "0.43" # package = "serde"\n' \
  $'[dependencies]\nrusqlite = { version = "0.43" } # package = "serde"\n' \
  $'[dependencies.rusqlite]\nversion = "0.43" # package = "serde"\n'; do
  n_scribe=$((n_scribe + 1))
  commented=$(tree "comment-banned-$n_scribe")
  printf '[package]\nname = "scribe"\n\n%s' "$comment_banned" >"$commented/crates/scribe/Cargo.toml"
  expect fail check-dependency-direction.sh "$commented" "a banned dependency with a legal name in its comment ($n_scribe)"
  if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
    echo "check-gates: FAIL: missing banned-crate diagnostic for a commented dependency ($n_scribe)" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done

# Cargo keys each declaration by its table as well as by its name: the same key
# in [dependencies], [build-dependencies] or two target tables is two dependencies
# (ORB-28908). A banned package fails wherever its declaration sits, in either
# order; legal reuse passes; a crate cycle closed only by a reused key is cited
# at the declaration that closes it.
n_reuse=0
for banned_reuse in \
  $'[dependencies]\nrusqlite = "0.43"\n\n[build-dependencies]\nrusqlite = { version = "1", package = "serde" }\n' \
  $'[build-dependencies]\nrusqlite = { version = "1", package = "serde" }\n\n[dependencies]\nrusqlite = "0.43"\n' \
  $'[target.\'cfg(unix)\'.dependencies]\nrusqlite = "0.43"\n\n[target.\'cfg(windows)\'.dependencies]\nrusqlite = { version = "1", package = "serde" }\n' \
  $'[target.\'cfg(windows)\'.dependencies]\nrusqlite = { version = "1", package = "serde" }\n\n[target.\'cfg(unix)\'.dependencies]\nrusqlite = "0.43"\n' \
  $'[dependencies.rusqlite]\nversion = "0.43"\n\n[build-dependencies.rusqlite]\nversion = "1"\npackage = "serde"\n' \
  $'[build-dependencies]\nrusqlite = { version = "1", package = "serde" }\n\n[dependencies.rusqlite]\nversion = "0.43"\n' \
  $'[dependencies]\nrusqlite = { version = "1", package = "serde" }\n\n[target.\'cfg(unix)\'.dependencies]\nrusqlite = "0.43"\n'; do
  n_reuse=$((n_reuse + 1))
  reused=$(tree "reuse-banned-$n_reuse")
  printf '[package]\nname = "scribe"\n\n%s' "$banned_reuse" >"$reused/crates/scribe/Cargo.toml"
  expect fail check-dependency-direction.sh "$reused" "a banned package under a reused key ($n_reuse)"
  if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
    echo "check-gates: FAIL: missing banned-crate diagnostic for a reused key ($n_reuse)" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done
for legal_reuse in \
  $'[dependencies]\nserde = "1"\n\n[build-dependencies]\nserde = { version = "1", package = "serde_json" }\n' \
  $'[dependencies.serde]\nversion = "1"\n\n[build-dependencies.serde]\nversion = "1"\npackage = "serde_json"\n' \
  $'[target.\'cfg(unix)\'.dependencies]\nserde = "1"\n\n[target.\'cfg(windows)\'.dependencies]\nserde = { version = "1", package = "serde_json" }\n' \
  $'[dependencies]\nserde = { version = "1", package = "serde_json" }\n\n[target.\'cfg(unix)\'.dependencies]\nserde = "1"\n' \
  $'[dependencies]\nserde = "1"\n\n[dev-dependencies]\nrusqlite = "0.43"\n'; do
  n_reuse=$((n_reuse + 1))
  legal=$(tree "reuse-legal-$n_reuse")
  printf '[package]\nname = "scribe"\n\n%s' "$legal_reuse" >"$legal/crates/scribe/Cargo.toml"
  expect pass check-dependency-direction.sh "$legal" "a reused key with legal packages, or a dev-only banned one ($n_reuse)"
done
for core_reuse in \
  $'[dev-dependencies]\nback = { path = "../scribe", package = "scribe" }\n\n[build-dependencies]\nback = { path = "../scribe", package = "scribe" }\n' \
  $'[target.\'cfg(windows)\'.dev-dependencies]\nback = { path = "../scribe", package = "scribe" }\n\n[target.\'cfg(unix)\'.build-dependencies]\nback = { path = "../scribe", package = "scribe" }\n'; do
  n_reuse=$((n_reuse + 1))
  cyc_reuse=$(tree "cycle-reused-$n_reuse")
  printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n%s' "$core_reuse" >"$cyc_reuse/crates/scribe-core/Cargo.toml"
  expect fail check-dependency-direction.sh "$cyc_reuse" "a crate cycle closed by a reused key ($n_reuse)"
  edge_line=$(grep -n 'back = { path = "../scribe"' "$cyc_reuse/crates/scribe-core/Cargo.toml" | tail -n 1 | cut -d: -f1)
  if ! grep -Fq "crate import cycle:" "$work/out" || ! grep -Fq "crates/scribe/Cargo.toml:6" "$work/out" || ! grep -Fq "crates/scribe-core/Cargo.toml:$edge_line" "$work/out"; then
    echo "check-gates: FAIL: the cycle diagnostic does not cite both manifest locations (crates/scribe/Cargo.toml:6 and crates/scribe-core/Cargo.toml:$edge_line) ($n_reuse)" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done

# Nonmember packages that happen to share a workspace member's name (registry
# dependencies or excluded path packages) do not close a crate cycle, even when
# the internal-name rule independently rejects the dependency. A real back edge
# to the workspace member still reports the cycle with both manifest locations.
for nonmember_reg in \
  $'[dependencies]\nscribe = { version = "1", package = "serde" }\n\n[build-dependencies]\nscribe = { version = "0.1.0" }\n' \
  $'[target.\'cfg(windows)\'.dependencies]\nscribe = { version = "1", package = "serde" }\n\n[target.\'cfg(unix)\'.dependencies]\nscribe = { version = "0.1.0" }\n'; do
  n_reuse=$((n_reuse + 1))
  reg_same_name=$(tree "nonmember-reg-$n_reuse")
  printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n%s' "$nonmember_reg" >"$reg_same_name/crates/scribe-core/Cargo.toml"
  expect fail check-dependency-direction.sh "$reg_same_name" "a registry dependency sharing a member name ($n_reuse)"
  if ! grep -Fq "scribe-core must not depend on internal crate scribe" "$work/out" || grep -Fq "crate import cycle:" "$work/out"; then
    echo "check-gates: FAIL: registry dependency sharing a member name reported wrong diagnostic ($n_reuse)" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done

excluded_same_name=$(tree "excluded-same-name")
mkdir -p "$excluded_same_name/external/src"
printf '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\nexclude = ["external"]\n' >"$excluded_same_name/Cargo.toml"
printf '[package]\nname = "scribe"\nversion = "0.2.0"\n' >"$excluded_same_name/external/Cargo.toml"
printf 'pub fn ext() {}\n' >"$excluded_same_name/external/src/lib.rs"
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nback = { path = "../../external", package = "scribe" }\n' \
  >"$excluded_same_name/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$excluded_same_name" "an excluded path package sharing a member name"
if ! grep -Fq "scribe-core must not depend on internal crate scribe" "$work/out" || grep -Fq "crate import cycle:" "$work/out"; then
  echo "check-gates: FAIL: excluded same-name package reported wrong diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

real_back_edge=$(tree "real-back-edge")
printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\n\n[dependencies]\nback = { path = "../scribe", package = "scribe" }\n' \
  >"$real_back_edge/crates/scribe-core/Cargo.toml"
expect fail check-dependency-direction.sh "$real_back_edge" "a real aliased back edge to the workspace member"
if ! grep -Fq "crate import cycle: scribe -> scribe-core -> scribe" "$work/out" || ! grep -Fq "crates/scribe/Cargo.toml:6" "$work/out" || ! grep -Fq "crates/scribe-core/Cargo.toml:6" "$work/out"; then
  echo "check-gates: FAIL: real aliased back edge did not cite both manifest locations" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

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

# A root facade is no unit, but a path through it still reaches what it
# re-exports: a consumer calling crate::facade::item() depends on backing, and
# backing calling the consumer back is a cycle (ORB-28903).
for reexport in 'pub use crate::backing::item;' 'pub use crate::backing::*;' \
  'pub use crate::backing::item as item;' 'pub use crate::{backing::item};'; do
  fac=$(tree "cycle-facade-$n")
  printf '//! app\nmod backing;\nmod consumer;\npub mod facade { %s }\nfn main() {}\n' "$reexport" \
    >"$fac/crates/scribe/src/main.rs"
  printf '//! consumer\npub fn helper() { crate::facade::item(); }\n' >"$fac/crates/scribe/src/consumer.rs"
  printf '//! backing\npub fn item() { crate::consumer::helper(); }\n' >"$fac/crates/scribe/src/backing.rs"
  expect fail check-dependency-direction.sh "$fac" "a cycle through the facade re-export $reexport"
  if ! grep -Fq "scribe import cycle: backing -> consumer -> backing" "$work/out"; then
    echo "check-gates: FAIL: missing the facade cycle diagnostic for $reexport" >&2
    fail=1
  fi
  printf '//! consumer\nuse crate::facade::item;\npub fn helper() { item(); }\n' >"$fac/crates/scribe/src/consumer.rs"
  expect fail check-dependency-direction.sh "$fac" "a cycle through a use of the facade re-export $reexport"
  printf '//! backing\npub fn item() {}\n' >"$fac/crates/scribe/src/backing.rs"
  expect pass check-dependency-direction.sh "$fac" "an acyclic control for the facade re-export $reexport"
done

# A facade with several globs sends a name to the glob that defines it, a name
# another crate defines carries no edge, and a facade that goes through
# another re-export is followed to the module that defines the item.
fac_globs=$(tree cycle-facade-globs)
printf '//! app\nmod backing;\nmod consumer;\nmod other;\npub mod facade { pub use crate::backing::*; pub use crate::other::*; }\nfn main() {}\n' \
  >"$fac_globs/crates/scribe/src/main.rs"
printf '//! consumer\npub fn helper() { crate::facade::item(); }\n' >"$fac_globs/crates/scribe/src/consumer.rs"
printf '//! backing\npub fn item() {}\n' >"$fac_globs/crates/scribe/src/backing.rs"
printf '//! other\npub fn unrelated() { crate::consumer::helper(); }\n' >"$fac_globs/crates/scribe/src/other.rs"
expect pass check-dependency-direction.sh "$fac_globs" "a facade glob that does not define the name"
printf '//! other\npub fn item() { crate::consumer::helper(); }\n' >"$fac_globs/crates/scribe/src/other.rs"
expect fail check-dependency-direction.sh "$fac_globs" "a facade glob that does define the name"

fac_ext=$(tree cycle-facade-external)
printf '//! app\nmod backing;\nmod consumer;\npub mod facade { pub use other_crate::item; pub use crate::backing::*; }\nfn main() {}\n' \
  >"$fac_ext/crates/scribe/src/main.rs"
printf '//! consumer\npub fn helper() { crate::facade::item(); }\n' >"$fac_ext/crates/scribe/src/consumer.rs"
printf '//! backing\npub fn unrelated() { crate::consumer::helper(); }\n' >"$fac_ext/crates/scribe/src/backing.rs"
expect pass check-dependency-direction.sh "$fac_ext" "a facade name another crate defines"

fac_chain=$(tree cycle-facade-chain)
printf '//! app\nmod backing;\nmod consumer;\nmod real;\npub mod facade { pub use crate::backing::*; }\nfn main() {}\n' \
  >"$fac_chain/crates/scribe/src/main.rs"
printf '//! consumer\npub fn helper() { crate::facade::item(); }\n' >"$fac_chain/crates/scribe/src/consumer.rs"
printf '//! backing\npub use crate::real::item;\n' >"$fac_chain/crates/scribe/src/backing.rs"
printf '//! real\npub fn item() { crate::consumer::helper(); }\n' >"$fac_chain/crates/scribe/src/real.rs"
expect fail check-dependency-direction.sh "$fac_chain" "a cycle through a facade and a further re-export"
if ! grep -Fq "scribe import cycle: consumer -> real -> consumer" "$work/out"; then
  echo "check-gates: FAIL: missing the diagnostic for a facade and a further re-export" >&2
  fail=1
fi

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

# 3. Module cycles inside a layer without a declared order (scribe::output::common)
#    and inside declared ordered layers (scribe::output, scribe-store::db).
cyc_out_ok=$(tree cycle-output-clean)
mkdir -p "$cyc_out_ok/crates/scribe/src/output/common"
printf '//! color\n' >"$cyc_out_ok/crates/scribe/src/output/common/color.rs"
printf '//! table\nuse super::color;\n' >"$cyc_out_ok/crates/scribe/src/output/common/table.rs"
expect pass check-dependency-direction.sh "$cyc_out_ok" "acyclic scribe::output::common modules"

cyc_out_bad=$(tree cycle-output-bad)
mkdir -p "$cyc_out_bad/crates/scribe/src/output/common"
printf '//! color\nuse super::table;\n' >"$cyc_out_bad/crates/scribe/src/output/common/color.rs"
printf '//! table\nuse super::color;\n' >"$cyc_out_bad/crates/scribe/src/output/common/table.rs"
expect fail check-dependency-direction.sh "$cyc_out_bad" "in-layer module cycle in scribe::output::common (color <-> table)"
if ! grep -Fq "scribe::output::common import cycle:" "$work/out"; then
  echo "check-gates: FAIL: missing scribe::output::common cycle diagnostic" >&2
  fail=1
fi

ord_out_ok=$(tree order-output-clean)
mkdir -p "$ord_out_ok/crates/scribe/src/output/common"
printf '//! common\n' >"$ord_out_ok/crates/scribe/src/output/common/mod.rs"
printf '//! entry\nuse super::common;\n' >"$ord_out_ok/crates/scribe/src/output/entry.rs"
expect pass check-dependency-direction.sh "$ord_out_ok" "acyclic scribe::output downward edge (entry -> common)"

ord_out_bad=$(tree order-output-upward)
mkdir -p "$ord_out_bad/crates/scribe/src/output/common"
printf '//! common\nuse super::entry;\n' >"$ord_out_bad/crates/scribe/src/output/common/mod.rs"
printf '//! entry\n' >"$ord_out_bad/crates/scribe/src/output/entry.rs"
expect fail check-dependency-direction.sh "$ord_out_bad" "upward edge in scribe::output (common -> entry)"
if ! grep -Fq "scribe::output: common imports entry, which is not lower" "$work/out"; then
  echo "check-gates: FAIL: missing scribe::output declared order diagnostic" >&2
  fail=1
fi

ord_out_unlisted=$(tree order-output-unlisted)
mkdir -p "$ord_out_unlisted/crates/scribe/src/output"
printf '//! extra\n' >"$ord_out_unlisted/crates/scribe/src/output/extra.rs"
expect fail check-dependency-direction.sh "$ord_out_unlisted" "unlisted module in scribe::output (extra)"
if ! grep -Fq "scribe::output: module 'extra' has no place in the declared order" "$work/out"; then
  echo "check-gates: FAIL: missing scribe::output unlisted-module diagnostic" >&2
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

# Raw identifiers (r#name) name the same inline module as the plain spelling
# (ORB-28902). A permitted edge stays permitted in every spelling; an upward edge
# fails in every spelling, and its diagnostic names the canonical module.
raw_ok=$(store_tree order-db-raw-permitted)
cat >"$raw_ok/crates/scribe-store/src/db/mod.rs" <<'EOF'
mod r#groups {}
mod r#sessions { use super::r#groups; }
EOF
expect pass check-dependency-direction.sh "$raw_ok" "raw inline siblings with a downward edge"

mixed_ok=$(store_tree order-db-mixed-permitted)
cat >"$mixed_ok/crates/scribe-store/src/db/mod.rs" <<'EOF'
mod groups {}
mod r#sessions { use super::groups; fn f() { super::r#groups::g(); } }
EOF
expect pass check-dependency-direction.sh "$mixed_ok" "downward edges in plain and raw spellings, use and path"

for body in \
  $'mod r#log { use super::codec; }\nmod codec {}' \
  $'mod log { use super::r#codec; }\nmod r#codec {}' \
  $'mod r#log { use super::r#codec; }\nmod r#codec {}' \
  $'mod log { fn f() { super::r#codec::g(); } }\nmod r#codec {}'; do
  bad=$(store_tree "order-db-raw-upward-$n")
  printf '%s\n' "$body" >"$bad/crates/scribe-store/src/db/mod.rs"
  expect fail check-dependency-direction.sh "$bad" "raw upward edge: $(echo "$body" | tr -s '\n ' ' ')"
  if ! grep -Fq "scribe-store::db: log imports codec, which is not lower" "$work/out"; then
    echo "check-gates: FAIL: raw upward edge lacks the canonical diagnostic" >&2
    fail=1
  fi
done

unlisted_raw=$(store_tree order-db-raw-unlisted)
printf 'pub(crate) mod r#inline_extra {}\n' >"$unlisted_raw/crates/scribe-store/src/db/mod.rs"
expect fail check-dependency-direction.sh "$unlisted_raw" "raw-spelled unlisted inline module in scribe-store::db"
if ! grep -Fq "scribe-store::db: module 'inline_extra' has no place in the declared order" "$work/out"; then
  echo "check-gates: FAIL: raw unlisted module lacks the canonical diagnostic" >&2
  fail=1
fi

# A root re-export names its target through a raw path; the alias resolves to the
# canonical module, so an upward edge through it fails and a downward one passes.
alias_up=$(store_tree order-db-raw-alias-upward)
printf 'pub use crate::db::r#codec as enc;\n' >"$alias_up/crates/scribe-store/src/lib.rs"
printf '//! log\nuse crate::enc::Thing;\n' >"$alias_up/crates/scribe-store/src/db/log.rs"
expect fail check-dependency-direction.sh "$alias_up" "upward edge through a root alias of a raw path"
if ! grep -Fq "scribe-store::db: log imports codec, which is not lower" "$work/out"; then
  echo "check-gates: FAIL: root alias of a raw path lacks the canonical diagnostic" >&2
  fail=1
fi
alias_down=$(store_tree order-db-raw-alias-downward)
printf 'pub use crate::db::r#log as lg;\n' >"$alias_down/crates/scribe-store/src/lib.rs"
printf '//! log\n' >"$alias_down/crates/scribe-store/src/db/log.rs"
printf '//! codec\nuse crate::lg::Thing;\n' >"$alias_down/crates/scribe-store/src/db/codec.rs"
expect pass check-dependency-direction.sh "$alias_down" "downward edge through a root alias of a raw path"

# --- test cfg attributes are read as attributes (ORB-28901) ------------------
# A test-only cfg is read from the attribute itself. Text that only looks like
# one (a doc string, a comment) is not a cfg, a comment inside a cfg is ignored,
# and any(test, feature = "test-support") counts in either operand order.
# commands_tree <name>: a scribe::commands fixture whose brief.rs store imports.
commands_tree() {
  local root
  root=$(tree "$1")
  mkdir -p "$root/crates/scribe/src/commands"
  printf '//! brief\n' >"$root/crates/scribe/src/commands/brief.rs"
  echo "$root"
}

# Production: store imports the later sibling brief, and no cfg excuses it.
doc_bad=$(commands_tree cfg-doc-upward)
printf '//! store\n#[doc = "#[cfg(test)]"]\nuse super::brief::Brief;\n' >"$doc_bad/crates/scribe/src/commands/store.rs"
expect fail check-dependency-direction.sh "$doc_bad" "upward import under a doc attribute that says #[cfg(test)]"
if ! grep -Fq "scribe::commands: store imports brief, which is not lower" "$work/out"; then
  echo "check-gates: FAIL: missing upward-import diagnostic under a doc attribute" >&2
  fail=1
fi
comment_bad=$(commands_tree cfg-comment-upward)
printf '//! store\n#[doc = "x"]\n// #[cfg(test)]\nuse super::brief::Brief;\n' >"$comment_bad/crates/scribe/src/commands/store.rs"
expect fail check-dependency-direction.sh "$comment_bad" "upward import after a line comment that says #[cfg(test)]"

# Test-only controls: a comment inside the cfg is ignored, so the import is test-only.
comment_ok=$(commands_tree cfg-comment-test-only)
printf '//! store\n#[cfg(test /* fixture */)]\nuse super::brief::Brief;\n' >"$comment_ok/crates/scribe/src/commands/store.rs"
expect pass check-dependency-direction.sh "$comment_ok" "test-only upward import with a comment inside its cfg"

# A string with a line continuation keeps the raw offsets, so the cfg after it
# is still read at its own place (the attribute ends at the use, no space).
cont_ok=$(commands_tree cfg-string-continuation)
printf '//! store\nconst S: &str = "a\\\nb";\n#[cfg(test)]use super::brief::Brief;\n' >"$cont_ok/crates/scribe/src/commands/store.rs"
expect pass check-dependency-direction.sh "$cont_ok" "test-only import after a string with a line continuation"

# Inline test-support modules are test-only in either operand order. Without
# the cfg, the same module is a production unit with no place in the order.
support_fwd=$(store_tree cfg-support-forward)
printf '#[cfg(any(test, feature = "test-support"))]\nmod support { use super::codec; }\n' >"$support_fwd/crates/scribe-store/src/db/mod.rs"
expect pass check-dependency-direction.sh "$support_fwd" "test-support module under any(test, feature = ...)"
support_rev=$(store_tree cfg-support-reversed)
printf '#[cfg(any(feature = "test-support", test))]\nmod support { use super::codec; }\n' >"$support_rev/crates/scribe-store/src/db/mod.rs"
expect pass check-dependency-direction.sh "$support_rev" "test-support module under any(feature = ..., test)"
support_plain=$(store_tree cfg-support-plain)
printf 'mod support { use super::codec; }\n' >"$support_plain/crates/scribe-store/src/db/mod.rs"
expect fail check-dependency-direction.sh "$support_plain" "the same module without a test cfg is a production unit"

# lib.rs reads its root modules' cfgs the same way: a comment inside the cfg is
# ignored, and a doc string or a comment that says #[cfg(test)] is not one.
root_test=$(tree cfg-root-comment-test-only)
printf '//! core\npub mod apps;\n#[cfg(test /* fixture */)]\nmod extra;\n' >"$root_test/crates/scribe-core/src/lib.rs"
printf 'fn extra() {}\n' >"$root_test/crates/scribe-core/src/extra.rs"
expect pass check-dependency-direction.sh "$root_test" "root test module with a comment inside its cfg"
root_any=$(tree cfg-root-any)
printf '//! core\npub mod apps;\n#[cfg(any(feature = "test-support", test))]\nmod extra;\n' >"$root_any/crates/scribe-core/src/lib.rs"
printf 'fn extra() {}\n' >"$root_any/crates/scribe-core/src/extra.rs"
expect pass check-dependency-direction.sh "$root_any" "root test-support module under any(feature = ..., test)"
root_doc=$(tree cfg-root-doc)
printf '//! core\npub mod apps;\n#[doc = "#[cfg(test)]"]\nmod extra;\n' >"$root_doc/crates/scribe-core/src/lib.rs"
printf 'fn extra() {}\n' >"$root_doc/crates/scribe-core/src/extra.rs"
expect fail check-dependency-direction.sh "$root_doc" "root module under a doc attribute that says #[cfg(test)]"
if ! grep -Fq "lib.rs declares the root module extra" "$work/out"; then
  echo "check-gates: FAIL: missing root-module diagnostic under a doc attribute" >&2
  fail=1
fi
root_comment=$(tree cfg-root-comment)
printf '//! core\npub mod apps;\n// #[cfg(test)]\nmod extra;\n' >"$root_comment/crates/scribe-core/src/lib.rs"
printf 'fn extra() {}\n' >"$root_comment/crates/scribe-core/src/extra.rs"
expect fail check-dependency-direction.sh "$root_comment" "root module after a line comment that says #[cfg(test)]"

# --- ORB-29106: cargo metadata oracle agreement & shared direction_lex.py ----
# Verify the gate agrees directly with `cargo metadata --offline --no-deps
# --format-version 1` across TOML comments, reused keys in [dependencies]/
# [build-dependencies] and two cfg target tables, quoted/single-quoted keys
# including 'package', workspace-inherited aliases in dotted and subtable form,
# and target-scoped dev-dependencies (exempt) vs build-dependencies (checked).
assert_agrees_with_metadata() {
  local root="$1" what="$2" want_banned="$3"
  local meta_has_rusqlite
  meta_has_rusqlite=$(
    cd "$root" && cargo metadata --offline --no-deps --format-version 1 \
      | python3 -I -c '
import json, sys
meta = json.load(sys.stdin)
found = any(
    dep["name"] == "rusqlite" and dep.get("kind") != "dev"
    for pkg in meta["packages"] if pkg["name"] == "scribe"
    for dep in pkg.get("dependencies", [])
)
print("yes" if found else "no")
'
  )
  if [[ "$meta_has_rusqlite" != "$want_banned" ]]; then
    echo "check-gates: FAIL: cargo metadata oracle disagreed with fixture $what (expected $want_banned, got $meta_has_rusqlite)" >&2
    fail=1
  fi
  if [[ "$want_banned" == "yes" ]]; then
    expect fail check-dependency-direction.sh "$root" "cargo metadata agreement ($what)"
    if ! grep -Fq "must not depend on rusqlite" "$work/out"; then
      echo "check-gates: FAIL: missing rusqlite diagnostic for metadata agreement ($what)" >&2
      sed 's/^/  | /' "$work/out" >&2
      fail=1
    fi
  else
    expect pass check-dependency-direction.sh "$root" "cargo metadata agreement ($what)"
  fi
}

# Workspace-inherited aliases in subtable form ([workspace.dependencies.sql])
# and dotted form (workspace.dependencies.sql / [workspace] dependencies.sql),
# consumed via both `[dependencies.sql] workspace = true` and `sql.workspace = true`.
for form in ws-subtable-dotted-use ws-subtable-subtable-use ws-dotted-subtable-use; do
  for pkg in rusqlite serde; do
    f=$(tree "meta-$form-$pkg")
    case "$form" in
      ws-subtable-dotted-use)
        printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n\n[workspace.dependencies.sql]\nversion = "0.43"\npackage = "%s"\n' "$pkg" >"$f/Cargo.toml"
        printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nsql.workspace = true\n' >"$f/crates/scribe/Cargo.toml"
        ;;
      ws-subtable-subtable-use)
        printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\n\n[workspace.dependencies.sql]\nversion = "0.43"\n'\''package'\'' = "%s"\n' "$pkg" >"$f/Cargo.toml"
        printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies.sql]\nworkspace = true\n' >"$f/crates/scribe/Cargo.toml"
        ;;
      ws-dotted-subtable-use)
        printf '[workspace]\nmembers = ["crates/scribe", "crates/scribe-core"]\ndependencies.sql.version = "0.43"\ndependencies.sql."package" = "%s"\n' "$pkg" >"$f/Cargo.toml"
        printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies.sql]\nworkspace = true\n' >"$f/crates/scribe/Cargo.toml"
        ;;
    esac
    if [[ "$pkg" == rusqlite ]]; then
      assert_agrees_with_metadata "$f" "$form ($pkg)" yes
    else
      assert_agrees_with_metadata "$f" "$form ($pkg)" no
    fi
  done
done

# Oracle agreement for TOML comments, key reuse across normal/build and two cfg
# tables, quoted/single-quoted 'package', and target dev vs build dependencies.
meta_comment_ok=$(tree meta-comment-ok)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1" # package = "rusqlite"\n' >"$meta_comment_ok/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_comment_ok" "TOML comment no false refusal" no

meta_comment_bad=$(tree meta-comment-bad)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nrusqlite = "0.43" # package = "serde"\n' >"$meta_comment_bad/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_comment_bad" "TOML comment no missed dependency" yes

meta_reuse_build=$(tree meta-reuse-build)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[dependencies]\nsql = { version = "1", package = "serde" }\n\n[build-dependencies]\nsql = { version = "0.43", '\''package'\'' = '\''rusqlite'\'' }\n' >"$meta_reuse_build/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_reuse_build" "same key in dependencies and build-dependencies" yes

meta_reuse_cfg=$(tree meta-reuse-cfg)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[target.'\''cfg(unix)'\''.dependencies]\nsql = { version = "1", "package" = "serde" }\n\n[target.'\''cfg(windows)'\''.dependencies]\nsql = { version = "0.43", '\''package'\'' = '\''rusqlite'\'' }\n' >"$meta_reuse_cfg/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_reuse_cfg" "same key in two cfg target tables" yes

meta_target_dev=$(tree meta-target-dev-exempt)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[target.'\''cfg(unix)'\''.dev-dependencies]\nrusqlite = "0.43"\n' >"$meta_target_dev/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_target_dev" "target dev-dependency stays exempt" no

meta_target_build=$(tree meta-target-build-banned)
printf '[package]\nname = "scribe"\nversion = "0.1.0"\n\n[target.'\''cfg(unix)'\''.build-dependencies]\nrusqlite = "0.43"\n' >"$meta_target_build/crates/scribe/Cargo.toml"
assert_agrees_with_metadata "$meta_target_build" "target build-dependency is enforced" yes

# --- ORB-29295: a warning from a successful cargo metadata stays out of the JSON
# A virtual workspace on edition 2024 with no `resolver` makes cargo print a
# resolver advisory on stderr while `cargo metadata` succeeds. The clean one
# must pass, the banned one must still fail with its own diagnostic, and a
# member cargo cannot load must fail with cargo's diagnostic.
advisory_tree() { # advisory_tree <name>: two-member virtual workspace, no resolver
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src" "$root/crates/scribe-tools/src"
  printf '[workspace]\nmembers = ["crates/scribe-core", "crates/scribe-tools"]\n' >"$root/Cargo.toml"
  printf '[package]\nname = "scribe-core"\nversion = "0.1.0"\nedition = "2024"\n' >"$root/crates/scribe-core/Cargo.toml"
  printf '[package]\nname = "scribe-tools"\nversion = "0.1.0"\nedition = "2024"\n' >"$root/crates/scribe-tools/Cargo.toml"
  : >"$root/crates/scribe-core/src/lib.rs"
  : >"$root/crates/scribe-tools/src/lib.rs"
  echo "$root"
}

advisory_clean=$(advisory_tree advisory-clean)
if ! (cd "$advisory_clean" && cargo metadata --offline --no-deps --format-version 1 >/dev/null 2>"$work/advisory.err") \
  || ! grep -Fq "virtual workspace defaulting to" "$work/advisory.err"; then
  echo "check-gates: FAIL: the advisory fixture no longer makes cargo metadata print its advisory" >&2
  fail=1
fi
expect pass check-dependency-direction.sh "$advisory_clean" "a clean virtual workspace whose cargo metadata prints a resolver advisory"

advisory_banned=$(advisory_tree advisory-banned)
printf '[package]\nname = "scribe-tools"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\nrusqlite = "0.43"\n' \
  >"$advisory_banned/crates/scribe-tools/Cargo.toml"
expect fail check-dependency-direction.sh "$advisory_banned" "a banned dependency beside a resolver advisory"
if ! grep -Fq "scribe-tools must not depend on rusqlite" "$work/out"; then
  echo "check-gates: FAIL: banned dependency beside a resolver advisory lacks its diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

advisory_broken=$(advisory_tree advisory-broken)
printf '[workspace]\nmembers = ["crates/scribe-core", "crates/scribe-tools", "crates/missing"]\n' >"$advisory_broken/Cargo.toml"
expect fail check-dependency-direction.sh "$advisory_broken" "a workspace member cargo cannot load"
if ! grep -Fq "cannot run: cargo metadata failed" "$work/out" || ! grep -Fq "failed to load manifest for workspace member" "$work/out"; then
  echo "check-gates: FAIL: unloadable workspace member lacks the cargo diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

# Shared Rust lexer (`scripts/direction_lex.py`): `mod r#store {`, `mod r#name;`,
# `#[doc = "#[cfg(test)]"]`, `#[cfg(test /* fixture */)]`, and
# `#[cfg(any(feature = "test-support", test))]` behave as rustc reads them.
mod_rstore_bad=$(commands_tree lex-mod-rstore-upward)
cat >"$mod_rstore_bad/crates/scribe/src/commands/mod.rs" <<'EOF'
#[doc = "#[cfg(test)]"]
mod r#store {
    use super::brief::Brief;
}
EOF
expect fail check-dependency-direction.sh "$mod_rstore_bad" "mod r#store { under #[doc = \"#[cfg(test)]\"] with upward import"
if ! grep -Fq "scribe::commands: store imports brief, which is not lower" "$work/out"; then
  echo "check-gates: FAIL: mod r#store { lacks canonical upward-order diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

mod_rstore_ok=$(commands_tree lex-mod-rstore-clean)
cat >"$mod_rstore_ok/crates/scribe/src/commands/mod.rs" <<'EOF'
mod instruction {}
mod r#store {
    use super::instruction;
}
#[cfg(test /* fixture */)]
mod r#test_helper {
    use super::brief::Brief;
}
#[cfg(any(feature = "test-support", test))]
mod r#support_helper {
    use super::brief::Brief;
}
EOF
expect pass check-dependency-direction.sh "$mod_rstore_ok" "mod r#store { downward edge and test-cfg raw inline modules"

mod_rname_doc=$(tree lex-mod-rname-doc)
printf '//! core\npub mod apps;\n#[doc = "#[cfg(test)]"]\nmod r#name;\n' >"$mod_rname_doc/crates/scribe-core/src/lib.rs"
printf 'fn name() {}\n' >"$mod_rname_doc/crates/scribe-core/src/name.rs"
expect fail check-dependency-direction.sh "$mod_rname_doc" "mod r#name; under #[doc = \"#[cfg(test)]\"]"
if ! grep -Fq "lib.rs declares the root module name, which is not one of the places" "$work/out"; then
  echo "check-gates: FAIL: mod r#name; lacks canonical root-module diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

mod_rname_cfg_comment=$(tree lex-mod-rname-cfg-comment)
printf '//! core\npub mod apps;\n#[cfg(test /* fixture */)]\nmod r#name;\n' >"$mod_rname_cfg_comment/crates/scribe-core/src/lib.rs"
printf 'fn name() {}\n' >"$mod_rname_cfg_comment/crates/scribe-core/src/name.rs"
expect pass check-dependency-direction.sh "$mod_rname_cfg_comment" "mod r#name; under #[cfg(test /* fixture */)]"

mod_rname_cfg_any=$(tree lex-mod-rname-cfg-any)
printf '//! core\npub mod apps;\n#[cfg(any(feature = "test-support", test))]\nmod r#name;\n' >"$mod_rname_cfg_any/crates/scribe-core/src/lib.rs"
printf 'fn name() {}\n' >"$mod_rname_cfg_any/crates/scribe-core/src/name.rs"
expect pass check-dependency-direction.sh "$mod_rname_cfg_any" "mod r#name; under #[cfg(any(feature = \"test-support\", test))]"

# Section 2 guards also scan through direction_lex.py: block comments, raw
# strings, test-only cfgs, and raw identifiers are handled uniformly.
lex_sec2_clean=$(store_tree lex-sec2-clean)
mkdir -p "$lex_sec2_clean/crates/scribe-core/src/apps/entry" \
  "$lex_sec2_clean/crates/scribe-core/src/providers" \
  "$lex_sec2_clean/crates/scribe-store/src/contracts"
cat >"$lex_sec2_clean/crates/scribe-core/src/config.rs" <<'EOF'
//! config
/* use std::{fmt, net}; */
const S: &str = r#"use std::{fmt, net};"#;
#[cfg(test /* fixture */)]
use std::net::TcpStream;
EOF
cat >"$lex_sec2_clean/crates/scribe-core/src/apps/entry/mod.rs" <<'EOF'
/* use super::session::Entry; */
const S: &str = "use crate::apps::session::Entry;";
#[cfg(any(feature = "test-support", test))]
use super::session::Entry;
EOF
cat >"$lex_sec2_clean/crates/scribe-core/src/providers/models.rs" <<'EOF'
/* use crate::{memory}; */
const S: &str = "use crate::group::Item;";
EOF
cat >"$lex_sec2_clean/crates/scribe-store/src/contracts/entry.rs" <<'EOF'
/* use crate::{db, repository}; */
EOF
cat >"$lex_sec2_clean/crates/scribe-store/src/db/mod.rs" <<'EOF'
//! db
/* PRAGMA journal_mode = WAL */
EOF
cat >"$lex_sec2_clean/crates/scribe-store/src/db/files.rs" <<'EOF'
use crate::error::Error;
/* use crate::db::log; */
EOF
expect pass check-dependency-direction.sh "$lex_sec2_clean" "section 2 guards ignore comments, strings and test-only cfgs via shared lexer"

socket_feature=$(tree socket-feature)
cat >"$socket_feature/crates/scribe-core/src/config.rs" <<'EOF'
//! config
#[cfg(feature = "test-support")]
pub fn production() -> Option<std::net::TcpStream> { None }
EOF
expect fail check-dependency-direction.sh "$socket_feature" "socket under production-enabled test-support feature"
if ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
  echo "check-gates: FAIL: feature socket fixture lacks socket diagnostic" >&2
  fail=1
fi

socket_any=$(tree socket-feature-any)
printf '#[cfg(any(test, feature = "test-support"))]\nuse std::net::TcpStream;\n' >"$socket_any/crates/scribe-core/src/config.rs"
expect fail check-dependency-direction.sh "$socket_any" "socket under any(test, test-support)"
if ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
  echo "check-gates: FAIL: any feature socket fixture lacks socket diagnostic" >&2
  fail=1
fi

socket_test=$(tree socket-test-only)
printf '#[cfg(all(test, feature = "test-support"))]\nmod tests { use std::net::TcpStream; }\n' >"$socket_test/crates/scribe-core/src/config.rs"
expect pass check-dependency-direction.sh "$socket_test" "socket exclusively under cfg(test)"

lex_sec2_std=$(tree lex-sec2-std-comment)
printf '//! config\n#[doc = "#[cfg(test)]"]\nuse std::{ /* note */ net };\n' >"$lex_sec2_std/crates/scribe-core/src/config.rs"
expect fail check-dependency-direction.sh "$lex_sec2_std" "ban_std catches grouped net with inner comment and doc attr"

lex_absolute=$(tree lex-absolute-return)
cat >"$lex_absolute/crates/scribe-core/src/config.rs" <<'EOF'
//! config
pub fn result() -> [u8; 4] {
    return ::std::net::Ipv4Addr::LOCALHOST.octets();
}
EOF
expect fail check-dependency-direction.sh "$lex_absolute" "absolute std::net after return"
if ! grep -Fq 'config.rs:3:    return ::std::net::Ipv4Addr::LOCALHOST.octets();' "$work/out" || \
   ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
  echo "check-gates: FAIL: absolute std::net lacks location or socket diagnostic" >&2
  fail=1
fi

lex_absolute_forms=(
  'pub fn addr() -> ::std::net::Ipv4Addr { todo!() }|return type arrow'
  'pub trait T {}\nimpl T for ::std::net::Ipv4Addr {}|for keyword'
  'pub fn f() where ::std::net::Ipv4Addr: Clone {}|where keyword'
  'pub fn f() -> impl ::std::net::ToSocketAddrs { ("127.0.0.1", 80) }|impl keyword'
  'pub fn f(_: &dyn ::std::net::ToSocketAddrs<Iter = It>) {}|dyn keyword'
  'pub fn f(x: u8) { match x { _ => ::std::net::Ipv4Addr::LOCALHOST }; }|match arm arrow'
)
for form in "${lex_absolute_forms[@]}"; do
  lex_form=$(tree lex-absolute-form)
  printf '//! config\n%b\n' "${form%|*}" >"$lex_form/crates/scribe-core/src/config.rs"
  expect fail check-dependency-direction.sh "$lex_form" "absolute std::net after ${form##*|}"
  if ! grep -Fq 'config.rs:' "$work/out" || ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
    echo "check-gates: FAIL: absolute std::net after ${form##*|} lacks location or socket diagnostic" >&2
    fail=1
  fi
done

lex_nested=$(tree lex-nested-std)
cat >"$lex_nested/crates/scribe-core/src/config.rs" <<'EOF'
//! config
fn nested() {
    foo::std::net::item();
    r#return::std::net::item();
    r#for::std::net::item();
    Vec::<u8>::std::net::item();
    Option::<Vec<u8>>::std::net::item();
    Box::<dyn Fn(u32) -> bool>::std::net::item();
    <Vec<u8> as Tr>::std::net::item();
    Vec::<&&str>::std::net::item();
    Result::<u8, u16>::std::net::item();
    Foo::<{ 1 + 2 }>::std::net::item();
}
fn ty_nested(_: (Result<u8, u16>::std::net::Item, u8)) {}
struct FieldType { field: [Result<u8, u16>::std::net::Item; 2] }
enum VariantType { V { field: [Result<u8, u16>::std::net::Item; 2] } }
struct AttrField { #[allow(unused)] field: [Result<u8, u16>::std::net::Item; 2] }
enum AttrVariant { V { a: u8, #[allow(unused)] #[doc = "x"] field: [Result<u8, u16>::std::net::Item; 2] } }
EOF
expect pass check-dependency-direction.sh "$lex_nested" "std inside ordinary, raw-identifier and generic-qualified paths"

# A braced enum sibling must not hide declaration type context from a later
# variant's field. The macro parses the item without resolving its field type.
lex_enum_sibling=$(tree lex-enum-braced-sibling)
cat >"$lex_enum_sibling/crates/scribe-core/src/config.rs" <<'EOF'
macro_rules! accept { ($item:item) => {}; }
accept! {
    enum Example { A {}, B { x: [Result<u8, u16>::std::net::Item; 2] } }
}
EOF
if ! rustc --edition=2024 --crate-type lib --emit=metadata -o "$work/lex-enum-sibling.rmeta" \
  "$lex_enum_sibling/crates/scribe-core/src/config.rs" >"$work/rustc.out" 2>&1; then
  echo "check-gates: FAIL: braced enum sibling fixture did not compile under Rust 2024" >&2
  sed 's/^/  | /' "$work/rustc.out" >&2
  fail=1
fi
expect pass check-dependency-direction.sh "$lex_enum_sibling" "type context after an earlier braced enum variant"

# Stringify preserves tokens without resolving their generic-qualified paths.
lex_stringify=$(tree lex-stringify-generic)
cat >"$lex_stringify/crates/scribe-core/src/config.rs" <<'EOF'
pub const EXAMPLE: &str = stringify!((Result<u8, u16>::std::net::Item, u8));
pub const BRACKETS: &str = stringify!([Result<u8, u16>::std::net::Item]);
EOF
if ! rustc --edition=2024 --crate-type lib --emit=metadata -o "$work/lex-stringify.rmeta" \
  "$lex_stringify/crates/scribe-core/src/config.rs" >"$work/rustc.out" 2>&1; then
  echo "check-gates: FAIL: stringify fixture did not compile under Rust 2024" >&2
  sed 's/^/  | /' "$work/rustc.out" >&2
  fail=1
fi
expect pass check-dependency-direction.sh "$lex_stringify" "generic-qualified paths in stringify tokens"

# Qualified and raw builtin stringify calls preserve tokens without
# resolving their generic-qualified paths.
lex_stringify_qualified=$(tree lex-stringify-qualified)
cat >"$lex_stringify_qualified/crates/scribe-core/src/config.rs" <<'EOF'
pub const EXAMPLE: &str = std::stringify!([Result<u8, u16>::std::net::Item]);
pub const RAW_EXAMPLE: &str = r#stringify!([Result<u8, u16>::std::net::Item]);
EOF
if ! rustc --edition=2024 --crate-type lib --emit=metadata -o "$work/lex-stringify-qualified.rmeta" \
  "$lex_stringify_qualified/crates/scribe-core/src/config.rs" >"$work/rustc.out" 2>&1; then
  echo "check-gates: FAIL: qualified stringify fixture did not compile under Rust 2024" >&2
  sed 's/^/  | /' "$work/rustc.out" >&2
  fail=1
fi
expect pass check-dependency-direction.sh "$lex_stringify_qualified" "generic-qualified paths in qualified and raw stringify tokens"

# A local macro can shadow stringify and evaluate its arguments. Its
# comparison must not be mistaken for a generic-qualified path.
lex_shadow_stringify=$(tree lex-shadow-stringify)
cat >"$lex_shadow_stringify/crates/scribe-core/src/config.rs" <<'EOF'
macro_rules! stringify {
    ($left:expr, $right:expr) => { ($left, $right) };
}
pub fn f(a: u32, b: u32, c: u32) -> (bool, bool) {
    stringify!(a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits())
}
EOF
if ! rustc --edition=2024 --crate-type lib --emit=metadata -o "$work/lex-shadow-stringify.rmeta" \
  "$lex_shadow_stringify/crates/scribe-core/src/config.rs" >"$work/rustc.out" 2>&1; then
  echo "check-gates: FAIL: shadowed stringify fixture did not compile under Rust 2024" >&2
  sed 's/^/  | /' "$work/rustc.out" >&2
  fail=1
fi
expect fail check-dependency-direction.sh "$lex_shadow_stringify" "evaluated comparisons in shadowed stringify"
if ! grep -Fq 'config.rs:5:    stringify!(a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits())' "$work/out" || \
   ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
  echo "check-gates: FAIL: shadowed stringify lacks line 5 or socket diagnostic" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

# Compiled Rust 2024 fixtures using `::std::net` on the right side of `>` or
# `>>` comparisons/shifts must produce the socket guard diagnostic with their
# source line, even when `<` comparisons or `<<` shifts appear nearby (ORB-29337).
lex_cmp_forms=(
  'pub fn f(a: u32) -> bool { a > ::std::net::Ipv4Addr::LOCALHOST.to_bits() }|greater-than comparison'
  'pub fn f(a: u32, b: u32) -> bool { a < b && b > ::std::net::Ipv4Addr::LOCALHOST.to_bits() }|comparison after < &&'
  'pub fn f(a: u32) -> u32 { a >> ::std::net::Ipv4Addr::LOCALHOST.to_bits() }|right-shift operator'
  'pub fn f(a: u32) -> bool { a << 1 > ::std::net::Ipv4Addr::LOCALHOST.to_bits() }|greater-than after left-shift'
  'pub fn f(a: u32) -> u32 { a << 1 >> ::std::net::Ipv4Addr::LOCALHOST.to_bits() }|right-shift after left-shift'
  $'pub fn f(a: u32, b: u32, c: u32) -> bool {\n    if a < b {}\n    c > ::std::net::Ipv4Addr::LOCALHOST.to_bits()\n}|comparison after if < block'
  $'fn take(_: bool, _: bool) -> bool { true }\npub fn f(a: u32, b: u32, c: u32) -> bool { take(a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits()) }|comma-separated comparison in call'
  $'pub struct Flags { pub flags: [bool; 2] }\npub fn f(a: u32, b: u32, c: u32) -> Flags {\n    Flags { flags: [a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits()] }\n}|comma-separated comparison in struct field'
  $'pub struct Flags { pub flags: [bool; 2] }\npub fn f(a: u32, b: u32, c: u32) -> Flags {\n    Flags { #[allow(unused)] flags: [a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits()] }\n}|comma-separated comparison in attributed struct field'
  $'pub struct Flags(pub [bool; 2]);\npub fn f(a: u32, b: u32, c: u32) -> Flags {\n    Flags { 0: [a < b, c > ::std::net::Ipv4Addr::LOCALHOST.to_bits()] }\n}|comma-separated comparison in numeric struct field'
)
for form in "${lex_cmp_forms[@]}"; do
  lex_cmp=$(tree "lex-cmp-$n")
  printf '//! config\n%s\n' "${form%|*}" >"$lex_cmp/crates/scribe-core/src/config.rs"
  if ! rustc --edition=2024 --crate-type lib --emit=metadata -o "$work/lex-cmp.rmeta" \
    "$lex_cmp/crates/scribe-core/src/config.rs" >"$work/rustc.out" 2>&1; then
    echo "check-gates: FAIL: comparison fixture (${form##*|}) did not compile under Rust 2024" >&2
    sed 's/^/  | /' "$work/rustc.out" >&2
    fail=1
  fi
  expect fail check-dependency-direction.sh "$lex_cmp" "absolute std::net after ${form##*|}"
  want_line=$(grep -n '::std::net' "$lex_cmp/crates/scribe-core/src/config.rs" | cut -d: -f1)
  want_text=$(sed -n "${want_line}p" "$lex_cmp/crates/scribe-core/src/config.rs")
  if ! grep -Fq "config.rs:${want_line}:${want_text}" "$work/out" || \
     ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
    echo "check-gates: FAIL: comparison fixture (${form##*|}) lacks line ${want_line} or socket diagnostic" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
done

# A keyword or `std` spelled after `$` is a macro metavariable (ORB-29336),
# so `$for::std::net` qualifies a local module and must pass. A real `for`
# keyword beside it still makes `::std::net` absolute and must report.
lex_metavar=$(tree lex-metavar-keyword)
cat >"$lex_metavar/crates/scribe-core/src/config.rs" <<'EOF'
//! config
macro_rules! m { ($for:ident) => { $for::std::net::X }; }
pub fn f() { let _ = m!(bar); }
mod bar { pub mod std { pub mod net { pub const X: u8 = 0; } } }
EOF
expect pass check-dependency-direction.sh "$lex_metavar" "keyword-named metavariable qualifying a local std::net module"

lex_metavar_std=$(tree lex-metavar-std)
cat >"$lex_metavar_std/crates/scribe-core/src/config.rs" <<'EOF'
//! config
macro_rules! m { ($std:ident) => { $std::net::X }; }
pub fn f() { let _ = m!(bar); }
mod bar { pub mod net { pub const X: u8 = 0; } }
EOF
expect pass check-dependency-direction.sh "$lex_metavar_std" "std-named metavariable qualifying a local net module"

lex_metavar_abs=$(tree lex-metavar-abs)
cat >"$lex_metavar_abs/crates/scribe-core/src/config.rs" <<'EOF'
//! config
macro_rules! m { ($for:ident) => { $for::std::net::X }; }
pub trait T {}
impl T for ::std::net::Ipv4Addr {}
EOF
expect fail check-dependency-direction.sh "$lex_metavar_abs" "absolute std::net after a real for beside a metavariable"
if ! grep -Fq 'config.rs:4:' "$work/out" || grep -Fq 'config.rs:2:' "$work/out" || \
   ! grep -Fq 'scribe-core opens no socket' "$work/out"; then
  echo "check-gates: FAIL: metavariable fixture reports the wrong line or lacks the socket diagnostic" >&2
  fail=1
fi

lex_sec2_app=$(tree lex-sec2-app-raw)
mkdir -p "$lex_sec2_app/crates/scribe-core/src/apps/entry"
printf '#[doc = "#[cfg(test)]"]\nuse super::r#session::Entry;\n' >"$lex_sec2_app/crates/scribe-core/src/apps/entry/mod.rs"
expect fail check-dependency-direction.sh "$lex_sec2_app" "app order catches raw-ident upward import under doc attr"

lex_sec2_contracts=$(store_tree lex-sec2-contracts)
mkdir -p "$lex_sec2_contracts/crates/scribe-store/src/contracts"
printf 'use crate::{ /* note */ db };\n' >"$lex_sec2_contracts/crates/scribe-store/src/contracts/entry.rs"
expect fail check-dependency-direction.sh "$lex_sec2_contracts" "contracts grouped import catches db after inner comment"

lex_sec2_files=$(store_tree lex-sec2-files)
printf 'use crate::{error::Error, db::log};\n' >"$lex_sec2_files/crates/scribe-store/src/db/files.rs"
expect fail check-dependency-direction.sh "$lex_sec2_files" "db/files.rs catches non-error import in same grouped use"

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
# the terminal; any other module that decides, probes or names one fails
# (STD-01 R17, STD-02 R3, R15). Standard streams and print macros are enforced
# by Clippy (`clippy.toml` and `Cargo.toml`). A doc comment or a test names one
# harmlessly.
guard_tree() {
  local root="$work/$1"
  mkdir -p "$root/crates/scribe-core/src" "$root/crates/scribe/src/output/tests" \
    "$root/crates/scribe/src/commands"
  printf '//! composition\npub const LOG_ENV: &str = "SCRIBE_LOG";\n' \
    >"$root/crates/scribe-core/src/composition.rs"
  printf '//! output\npub(crate) fn decide() -> bool {\n    cw_cli::Probe::detect().is_some() && cw_cli::Terminal::detect(&env, "scribe", None).is_some()\n}\n' \
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
printf 'pub(crate) fn f() -> bool {\n    cw_cli::Terminal::detect(&env, "scribe", None).is_some()\n}\n' \
  >>"$decision/crates/scribe/src/commands/entry.rs"
expect fail check-terminal-guard.sh "$decision" "a second terminal decision in a command module"

probe=$(guard_tree guard-probe)
printf 'pub(crate) fn g() {\n    let _ = cw_cli::Probe::detect();\n}\n' \
  >>"$probe/crates/scribe/src/cli.rs"
expect fail check-terminal-guard.sh "$probe" "a terminal probe in the CLI outside output"

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
printf 'pub(crate) fn h() {\n    let _ = cw_cli::Probe::detect();\n}\n' >>"$guard_violation/crates/scribe/src/commands/entry.rs"
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
printf '#[test]\nfn example() { let _ = cw_cli::Probe::detect(); }\n' \
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
printf '#[test]\nfn render() { let _ = cw_cli::Probe::detect(); }\n' \
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

expect_clippy fail "$(clippy_tree clippy-stdout-qualified <<'EOF'
//! Core config fixture.

/// Reaches stdout directly via qualified std::io::stdout.
pub fn out() -> std::io::Stdout {
    std::io::stdout()
}
EOF
)" "clippy::disallowed-methods" "scribe-core calling std::io::stdout()"

expect_clippy fail "$(clippy_tree clippy-stdout-named <<'EOF'
//! Core config fixture.

use std::io::stdout;

/// Reaches stdout directly via named stdout import.
pub fn out() -> std::io::Stdout {
    stdout()
}
EOF
)" "clippy::disallowed-methods" "scribe-core importing std::io::stdout by name"

expect_clippy fail "$(clippy_tree clippy-stderr-qualified <<'EOF'
//! Core config fixture.

/// Reaches stderr directly via qualified std::io::stderr.
pub fn err() -> std::io::Stderr {
    std::io::stderr()
}
EOF
)" "clippy::disallowed-methods" "scribe-core calling std::io::stderr()"

expect_clippy fail "$(clippy_tree clippy-stderr-named <<'EOF'
//! Core config fixture.

use std::io::stderr;

/// Reaches stderr directly via named stderr import.
pub fn err() -> std::io::Stderr {
    stderr()
}
EOF
)" "clippy::disallowed-methods" "scribe-core importing std::io::stderr by name"

expect_clippy fail "$(clippy_tree clippy-is-terminal <<'EOF'
//! Core config fixture.

use std::io::IsTerminal;

/// Probes terminal state directly via std::io::IsTerminal::is_terminal.
pub fn tty(stream: &impl IsTerminal) -> bool {
    stream.is_terminal()
}
EOF
)" "clippy::disallowed-methods" "scribe-core calling std::io::IsTerminal::is_terminal"

expect_clippy fail "$(clippy_tree clippy-print-stdout <<'EOF'
//! Core config fixture.

/// Prints to stdout directly via println!.
pub fn speak() {
    println!("hi");
}
EOF
)" "clippy::print-stdout" "scribe-core calling println!"

expect_clippy fail "$(clippy_tree clippy-print-stderr <<'EOF'
//! Core config fixture.

/// Prints to stderr directly via eprintln!.
pub fn warn() {
    eprintln!("hi");
}
EOF
)" "clippy::print-stderr" "scribe-core calling eprintln!"

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
# The host adapters' expectations are scoped to their approved operations, so
# the same disallowed paths stay banned in the rest of each adapter module.
# Each fixture compiles the production code under the real lint configuration:
# fs.rs whole, and the expectation-carrying helpers of composition.rs and
# console.rs (the rest of those modules needs their crates). A forbidden call
# appended to the module must fail, as must a direct exit, which the
# independent `clippy::exit` lint bans in every exempt module.
adapter_clippy() {
  local name="$1" root="$work/clippy-adapter-$1"
  mkdir -p "$root/src"
  cp "$REPO_ROOT/clippy.toml" "$root/clippy.toml"
  cp "$REPO_ROOT/Cargo.lock" "$root/Cargo.lock"
  {
    printf '[workspace]\nresolver = "3"\n\n[package]\nname = "adapter-%s-fixture"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n' "$name"
    grep -E '^cw-app = ' "$REPO_ROOT/Cargo.toml"
    printf "\n[target.'cfg(unix)'.dependencies]\n"
    printf 'rustix = { version = "1", default-features = false, features = ["std", "process", "fs"] }\n'
    printf '\n[lints]\nworkspace = true\n\n%s\n' "$clippy_lints"
  } >"$root/Cargo.toml"
  echo "$root"
}

# The top-level items of $1 that carry an `#[expect(` attribute.
expect_items() {
  awk '/^#\[expect\(/ { on = 1 } on { print } on && /^}/ { on = 0 }' "$1"
}

fs_clippy=$(adapter_clippy fs)
cp "$REPO_ROOT/crates/scribe-tools/src/fs.rs" "$fs_clippy/src/lib.rs"
expect_clippy pass "$fs_clippy" "" "the production file-system adapter with item-scoped expectations"
fs_good=$(cat "$fs_clippy/src/lib.rs")
printf '%s\n' "$fs_good" >"$fs_clippy/src/lib.rs"
cat >>"$fs_clippy/src/lib.rs" <<'EOF'

/// A stream handle the adapter has no business taking.
pub fn leak() -> std::io::Stderr { std::io::stderr() }
EOF
expect_clippy fail "$fs_clippy" "clippy::disallowed-methods" "std::io::stderr in the file-system adapter"
printf '%s\n' "$fs_good" >"$fs_clippy/src/lib.rs"
cat >>"$fs_clippy/src/lib.rs" <<'EOF'

/// A file read outside the approved operations.
pub fn raw(path: &str) -> std::io::Result<Vec<u8>> { std::fs::read(path) }
EOF
expect_clippy fail "$fs_clippy" "clippy::disallowed-methods" "an unapproved std::fs::read in the file-system adapter"
printf '%s\n' "$fs_good" >"$fs_clippy/src/lib.rs"
cat >>"$fs_clippy/src/lib.rs" <<'EOF'

/// A direct exit.
pub fn bail() -> ! { std::process::exit(1) }
EOF
expect_clippy fail "$fs_clippy" "clippy::exit" "direct exit in the file-system adapter"

composition_clippy=$(adapter_clippy composition)
{
  printf '//! Composition adapter fixture.\n#![allow(dead_code)]\n'
  printf 'use std::ffi::OsString;\nuse std::path::{Path, PathBuf};\n\n'
  expect_items "$REPO_ROOT/crates/scribe-core/src/composition.rs"
} >"$composition_clippy/src/lib.rs"
if ! grep -Fq 'std::env::var(' "$composition_clippy/src/lib.rs"; then
  echo "check-gates: FAIL: the composition adapter fixture holds no environment read" >&2
  fail=1
fi
expect_clippy pass "$composition_clippy" "" "the production composition adapter's scoped expectations"
composition_good=$(cat "$composition_clippy/src/lib.rs")
printf '%s\n' "$composition_good" >"$composition_clippy/src/lib.rs"
cat >>"$composition_clippy/src/lib.rs" <<'EOF'

/// A stream handle composition has no business taking.
pub fn leak() -> std::io::Stdout { std::io::stdout() }
EOF
expect_clippy fail "$composition_clippy" "clippy::disallowed-methods" "std::io::stdout in the composition adapter"
printf '%s\n' "$composition_good" >"$composition_clippy/src/lib.rs"
cat >>"$composition_clippy/src/lib.rs" <<'EOF'

/// An unbounded channel.
pub fn queue() { let _ = std::sync::mpsc::channel::<u8>(); }
EOF
expect_clippy fail "$composition_clippy" "clippy::disallowed-methods" "an unbounded channel in the composition adapter"
printf '%s\n' "$composition_good" >"$composition_clippy/src/lib.rs"
cat >>"$composition_clippy/src/lib.rs" <<'EOF'

/// A direct exit.
pub fn bail() -> ! { std::process::exit(1) }
EOF
expect_clippy fail "$composition_clippy" "clippy::exit" "direct exit in the composition adapter"

console_clippy=$(adapter_clippy console)
{
  printf '//! Console adapter fixture.\n#![allow(dead_code)]\nuse std::io;\n\n'
  expect_items "$REPO_ROOT/crates/scribe/src/output/common/console.rs"
} >"$console_clippy/src/lib.rs"
if ! grep -Fq 'io::stdout()' "$console_clippy/src/lib.rs"; then
  echo "check-gates: FAIL: the console adapter fixture holds no stream handle" >&2
  fail=1
fi
expect_clippy pass "$console_clippy" "" "the production console adapter's scoped expectations"
console_good=$(cat "$console_clippy/src/lib.rs")
printf '%s\n' "$console_good" >"$console_clippy/src/lib.rs"
cat >>"$console_clippy/src/lib.rs" <<'EOF'

/// An unbounded channel, unrelated to the stream boundary.
pub fn queue() { let _ = std::sync::mpsc::channel::<u8>(); }
EOF
expect_clippy fail "$console_clippy" "clippy::disallowed-methods" "an unbounded channel in the console module"
printf '%s\n' "$console_good" >"$console_clippy/src/lib.rs"
cat >>"$console_clippy/src/lib.rs" <<'EOF'

/// An environment read, unrelated to the stream boundary.
pub fn home() -> Option<String> { std::env::var("HOME").ok() }
EOF
expect_clippy fail "$console_clippy" "clippy::disallowed-methods" "an environment read in the console module"
printf '%s\n' "$console_good" >"$console_clippy/src/lib.rs"
cat >>"$console_clippy/src/lib.rs" <<'EOF'

/// A direct exit.
pub fn bail() -> ! { std::process::exit(1) }
EOF
expect_clippy fail "$console_clippy" "clippy::exit" "direct exit in the console module"

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

# Compile the actual provider CLI adapter with the pinned dependencies. The
# clean twin passes with only supervise's spawn statement exempted; adding a
# direct exit, swapping drain's sync_channel(1) for an unbounded channel, or
# calling mpsc::channel in the codex submodule each fail under the same lint
# config.
cli_clippy="$work/clippy-provider-cli"
mkdir -p "$cli_clippy/src/cli"
cp "$REPO_ROOT/clippy.toml" "$cli_clippy/clippy.toml"
cp "$REPO_ROOT/crates/scribe-core/src/providers/cli.rs" "$cli_clippy/src/cli.rs"
cp "$REPO_ROOT/crates/scribe-core/src/providers/cli/codex.rs" "$cli_clippy/src/cli/codex.rs"
{
  printf '[workspace]\nresolver = "3"\n\n[package]\nname = "provider-cli-fixture"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\n'
  grep -E '^(cw-app|serde|serde_json|tempfile) = ' "$REPO_ROOT/Cargo.toml"
  printf "\n[target.'cfg(unix)'.dependencies]\n"
  grep -E '^rustix = ' "$REPO_ROOT/Cargo.toml"
  printf '\n[lints]\nworkspace = true\n\n%s\n' "$clippy_lints"
} >"$cli_clippy/Cargo.toml"
cp "$REPO_ROOT/Cargo.lock" "$cli_clippy/Cargo.lock"
cat >"$cli_clippy/src/lib.rs" <<'EOF'
//! Provider CLI adapter fixture.
#![allow(dead_code)]

mod cli;

/// Model port stubs.
pub mod models {
    use serde::Deserialize;
    use std::time::Duration;

    /// Group summary budget stub.
    pub const ASK_BUDGET: Duration = Duration::from_secs(90);
    /// Profile schema stub.
    pub const PROFILE_SCHEMA: &str = "{}";
    /// Session header schema stub.
    pub const SESSION_HEADER_SCHEMA: &str = "{}";

    /// Model error stub.
    #[derive(Debug)]
    pub enum ModelError {
        /// Timed out.
        Timeout {
            /// Elapsed budget.
            budget: Duration,
        },
        /// Unavailable binary.
        Unavailable(String),
        /// Non-zero exit or error output.
        Failed(String),
        /// Malformed answer.
        Malformed(String),
    }

    /// Group close evidence stub.
    pub struct CloseEvidence;
    impl CloseEvidence {
        /// Prompt stub.
        pub fn prompt(&self) -> String {
            String::new()
        }
    }

    /// Session request stub.
    pub struct SessionRequest {
        /// Model id.
        pub model: String,
        /// Call budget.
        pub budget: Duration,
    }
    impl SessionRequest {
        /// Schema stub.
        pub fn schema(&self) -> String {
            String::new()
        }
        /// Prompt stub.
        pub fn prompt(&self) -> String {
            String::new()
        }
    }

    /// Session header request stub.
    pub struct SessionHeaderRequest {
        /// Model id.
        pub model: String,
        /// Call budget.
        pub budget: Duration,
    }
    impl SessionHeaderRequest {
        /// Prompt stub.
        pub fn prompt(&self) -> String {
            String::new()
        }
    }

    /// Profile request stub.
    pub struct ProfileRequest {
        /// Model id.
        pub model: String,
        /// Call budget.
        pub budget: Duration,
    }
    impl ProfileRequest {
        /// Prompt stub.
        pub fn prompt(&self) -> String {
            String::new()
        }
    }

    /// Session answer stub.
    #[derive(Deserialize)]
    pub struct SessionAnswer;
    /// Session header answer stub.
    #[derive(Deserialize)]
    pub struct SessionHeader;
    /// Profile answer stub.
    #[derive(Deserialize)]
    pub struct ProfileAnswer;
}

/// Provider transport selection stub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Claude CLI.
    ClaudeCode,
    /// Codex CLI.
    Codex,
}

/// Redactor stub.
#[derive(Clone, Debug)]
pub struct Redactor;

impl Redactor {
    /// Construct a stub redactor.
    pub fn new<I, K, V>(_: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
    {
        Self
    }

    /// Return text unchanged in the compile-only lint fixture.
    pub fn mask(&self, text: &str) -> String {
        text.to_owned()
    }
}

/// Models trait stub.
pub trait Models {
    /// Session distillation stub.
    fn session(&self, request: &models::SessionRequest) -> Result<models::SessionAnswer, models::ModelError>;
    /// Session header stub.
    fn session_header(&self, request: &models::SessionHeaderRequest) -> Result<models::SessionHeader, models::ModelError>;
    /// Group summary stub.
    fn group_summary(&self, model: &str, evidence: &models::CloseEvidence) -> Result<String, models::ModelError>;
    /// Profile rebuild stub.
    fn profile(&self, request: &models::ProfileRequest) -> Result<models::ProfileAnswer, models::ModelError>;
}

EOF
expect_clippy pass "$cli_clippy" "" "the production provider CLI adapter with scoped spawn expectation"
cat >>"$cli_clippy/src/cli.rs" <<'EOF'

fn forbidden_exit() -> ! { std::process::exit(1) }
EOF
expect_clippy fail "$cli_clippy" "clippy::exit" "direct exit in the production provider CLI adapter"
cp "$REPO_ROOT/crates/scribe-core/src/providers/cli.rs" "$cli_clippy/src/cli.rs"
sed 's/mpsc::sync_channel(1)/mpsc::channel()/' "$REPO_ROOT/crates/scribe-core/src/providers/cli.rs" >"$cli_clippy/src/cli.rs"
if cmp -s "$REPO_ROOT/crates/scribe-core/src/providers/cli.rs" "$cli_clippy/src/cli.rs"; then
  echo "check-gates: FAIL: did not rewrite drain sync_channel(1) for negative fixture" >&2
  fail=1
fi
expect_clippy fail "$cli_clippy" "clippy::disallowed-methods" \
  "an unbounded mpsc::channel() in the provider drain"
if ! grep -Fq "use a bounded channel (std::sync::mpsc::sync_channel)" "$work/out"; then
  echo "check-gates: FAIL: missing bounded-channel diagnostic for drain" >&2
  fail=1
fi
cp "$REPO_ROOT/crates/scribe-core/src/providers/cli.rs" "$cli_clippy/src/cli.rs"
cat >>"$cli_clippy/src/cli/codex.rs" <<'EOF'

fn forbidden_channel() {
    let _ = std::sync::mpsc::channel::<()>();
}
EOF
expect_clippy fail "$cli_clippy" "clippy::disallowed-methods" \
  "an unbounded mpsc::channel() in the provider codex submodule"
if ! grep -Fq "use a bounded channel (std::sync::mpsc::sync_channel)" "$work/out"; then
  echo "check-gates: FAIL: missing bounded-channel diagnostic for codex submodule" >&2
  fail=1
fi

# --- check-footprint-companions.sh: the companions map matches the tree ---
fp_clean="$work/fp-clean"
mkdir -p "$fp_clean/docs"
printf -- '- path: `ARCHITECTURE.md`\n- make-target: `goldens`\n' >"$fp_clean/docs/footprint-companions.md"
printf 'map\n' >"$fp_clean/ARCHITECTURE.md"
printf 'goldens:\n\ttrue\n' >"$fp_clean/Makefile"
expect pass check-footprint-companions.sh "$fp_clean" "a companions map whose entries all exist"

fp_double_colon="$work/fp-double-colon"
mkdir -p "$fp_double_colon/docs"
printf -- '- make-target: `goldens`\n' >"$fp_double_colon/docs/footprint-companions.md"
printf 'goldens::\n\ttrue\n' >"$fp_double_colon/Makefile"
expect pass check-footprint-companions.sh "$fp_double_colon" \
  "a companions map whose target has a genuine double-colon rule"

fp_missing="$work/fp-missing"
mkdir -p "$fp_missing/docs"
printf -- '- path: `ARCHITECTURE.md`\n- make-target: `goldens`\n' >"$fp_missing/docs/footprint-companions.md"
printf 'goldens:\n\ttrue\n' >"$fp_missing/Makefile"
expect fail check-footprint-companions.sh "$fp_missing" "a companions map naming a path that does not exist"
if ! grep -Fq 'names `ARCHITECTURE.md`, which does not exist' "$work/out"; then
  echo "check-gates: FAIL: footprint fixture lacks its missing-path diagnostic" >&2
  fail=1
fi

fp_target="$work/fp-target"
mkdir -p "$fp_target/docs"
printf -- '- path: `ARCHITECTURE.md`\n- make-target: `nosuch`\n' >"$fp_target/docs/footprint-companions.md"
printf 'map\n' >"$fp_target/ARCHITECTURE.md"
printf 'goldens:\n\ttrue\n' >"$fp_target/Makefile"
expect fail check-footprint-companions.sh "$fp_target" "a companions map naming a target the Makefile does not define"
if ! grep -Fq 'names make target `nosuch`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: footprint fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_assign="${work}/fp-target-assign"
mkdir -p "$fp_target_assign/docs"
printf -- '- make-target: `LOCKED`\n' >"$fp_target_assign/docs/footprint-companions.md"
printf 'LOCKED:=--locked\n' >"$fp_target_assign/Makefile"
expect fail check-footprint-companions.sh "$fp_target_assign" \
  "a companions map naming a target that appears only as a := variable assignment"
if ! grep -Fq 'names make target `LOCKED`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: := assignment fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_double_colon_assign="${work}/fp-target-double-colon-assign"
mkdir -p "$fp_target_double_colon_assign/docs"
printf -- '- make-target: `LOCKED`\n' >"$fp_target_double_colon_assign/docs/footprint-companions.md"
printf 'LOCKED::=--locked\n' >"$fp_target_double_colon_assign/Makefile"
expect fail check-footprint-companions.sh "$fp_target_double_colon_assign" \
  "a companions map naming a target that appears only as a ::= variable assignment"
if ! grep -Fq 'names make target `LOCKED`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: ::= assignment fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_immediate_assign="${work}/fp-target-immediate-assign"
mkdir -p "$fp_target_immediate_assign/docs"
printf -- '- make-target: `LOCKED`\n' >"$fp_target_immediate_assign/docs/footprint-companions.md"
printf 'LOCKED:::=--locked\n' >"$fp_target_immediate_assign/Makefile"
expect fail check-footprint-companions.sh "$fp_target_immediate_assign" \
  "a companions map naming a target that appears only as a :::= variable assignment"
if ! grep -Fq 'names make target `LOCKED`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: :::= assignment fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_specific_assign="${work}/fp-target-specific-assign"
mkdir -p "$fp_target_specific_assign/docs"
printf -- '- make-target: `LOCKED`\n' >"$fp_target_specific_assign/docs/footprint-companions.md"
printf 'LOCKED: VALUE = --locked\n' >"$fp_target_specific_assign/Makefile"
expect fail check-footprint-companions.sh "$fp_target_specific_assign" \
  "a companions map naming a target that appears only in a target-specific assignment"
if ! grep -Fq 'names make target `LOCKED`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: target-specific assignment fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_regex="${work}/fp-target-regex"
mkdir -p "$fp_target_regex/docs"
printf -- '- make-target: `g.ldens`\n' >"$fp_target_regex/docs/footprint-companions.md"
printf 'goldens:\n\ttrue\n' >"$fp_target_regex/Makefile"
expect fail check-footprint-companions.sh "$fp_target_regex" \
  "a companions map whose regex metacharacter target name does not match goldens"
if ! grep -Fq 'names make target `g.ldens`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: literal target fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_comment_cont="${work}/fp-target-comment-cont"
mkdir -p "$fp_target_comment_cont/docs"
printf -- '- make-target: `ghost`\n' >"$fp_target_comment_cont/docs/footprint-companions.md"
printf '# disabled \\\nghost:\n\ttrue\n' >"$fp_target_comment_cont/Makefile"
expect fail check-footprint-companions.sh "$fp_target_comment_cont" \
  "a companions map naming a target that appears only on a comment continuation"
if ! grep -Fq 'names make target `ghost`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: comment continuation fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_target_header_space="${work}/fp-target-header-space"
mkdir -p "$fp_target_header_space/docs"
printf -- '- make-target: `ghost`\n' >"$fp_target_header_space/docs/footprint-companions.md"
printf 'ghost :\n\t@true\n' >"$fp_target_header_space/Makefile"
expect pass check-footprint-companions.sh "$fp_target_header_space" \
  "a companions map whose target has whitespace before the rule colon"

fp_target_header_space_assign="${work}/fp-target-header-space-assign"
mkdir -p "$fp_target_header_space_assign/docs"
printf -- '- make-target: `LOCKED`\n' >"$fp_target_header_space_assign/docs/footprint-companions.md"
printf 'LOCKED :=--locked\n' >"$fp_target_header_space_assign/Makefile"
expect fail check-footprint-companions.sh "$fp_target_header_space_assign" \
  "a companions map naming a target that appears only as a spaced := assignment"

# A line inside a define/endef variable body is the variable's value, not a
# rule, so it never satisfies the map. Each body below leaves ghost without a
# rule, which GNU Make reports as "No rule to make target 'ghost'" (ORB-29451).
n_define=0
for body in \
  $'define VARIABLE\n  ghost :\nendef\n' \
  $'define VARIABLE\nghost :\nendef\n' \
  $'override define VARIABLE\n  ghost :\nendef\n' \
  $'export define VARIABLE\n  ghost :\nendef\n' \
  $'define OUTER\ndefine INNER\nendef\n  ghost :\nendef\n' \
  $'define VARIABLE\n\tendef\n  ghost :\nendef\n'; do
  n_define=$((n_define + 1))
  fp_define="$work/fp-define-body-$n_define"
  mkdir -p "$fp_define/docs"
  printf -- '- make-target: `ghost`\n' >"$fp_define/docs/footprint-companions.md"
  printf '%s' "$body" >"$fp_define/Makefile"
  expect fail check-footprint-companions.sh "$fp_define" \
    "a companions map naming a target that appears only in a define body ($n_define)"
  if ! grep -Fq 'names make target `ghost`, which is not a Makefile target' "$work/out"; then
    echo "check-gates: FAIL: define body fixture ($n_define) lacks its unknown-target diagnostic" >&2
    fail=1
  fi
done

fp_target_indented_rule="${work}/fp-target-indented-rule"
mkdir -p "$fp_target_indented_rule/docs"
printf -- '- make-target: `ghost`\n' >"$fp_target_indented_rule/docs/footprint-companions.md"
printf '  ghost :\n\t@true\n' >"$fp_target_indented_rule/Makefile"
expect pass check-footprint-companions.sh "$fp_target_indented_rule" \
  "a companions map whose target is an indented real rule"

fp_target_after_define="${work}/fp-target-after-define"
mkdir -p "$fp_target_after_define/docs"
printf -- '- make-target: `ghost`\n' >"$fp_target_after_define/docs/footprint-companions.md"
printf 'define VARIABLE\n  other :\nendef # done\n  ghost :\n\t@true\n' >"$fp_target_after_define/Makefile"
expect pass check-footprint-companions.sh "$fp_target_after_define" \
  "a companions map whose target is a real rule after a define closed with a comment"

# A CRLF Makefile reads as its LF twin does (ORB-29452): GNU Make joins a
# backslash-CRLF continuation, so a comment or recipe that continues onto a
# target name does not define it, and a CRLF endef closes its define body.
fp_crlf_comment="$work/fp-crlf-comment"
mkdir -p "$fp_crlf_comment/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_comment/docs/footprint-companions.md"
printf '# disabled \\\r\nghost:\r\n\ttrue\r\n' >"$fp_crlf_comment/Makefile"
expect fail check-footprint-companions.sh "$fp_crlf_comment" \
  "a companions map naming a target that appears only on a CRLF comment continuation"
if ! grep -Fq 'names make target `ghost`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: CRLF comment continuation fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_crlf_recipe="$work/fp-crlf-recipe"
mkdir -p "$fp_crlf_recipe/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_recipe/docs/footprint-companions.md"
printf 'goldens:\r\n\t@echo hi \\\r\nghost:\r\n\ttrue\r\n' >"$fp_crlf_recipe/Makefile"
expect fail check-footprint-companions.sh "$fp_crlf_recipe" \
  "a companions map naming a target that appears only on a CRLF recipe continuation"
if ! grep -Fq 'names make target `ghost`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: CRLF recipe continuation fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_crlf_define_body="$work/fp-crlf-define-body"
mkdir -p "$fp_crlf_define_body/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_define_body/docs/footprint-companions.md"
printf 'define VARIABLE\r\n  ghost :\r\nendef\r\n' >"$fp_crlf_define_body/Makefile"
expect fail check-footprint-companions.sh "$fp_crlf_define_body" \
  "a companions map naming a target that appears only in a CRLF define body"
if ! grep -Fq 'names make target `ghost`, which is not a Makefile target' "$work/out"; then
  echo "check-gates: FAIL: CRLF define body fixture lacks its unknown-target diagnostic" >&2
  fail=1
fi

fp_crlf_define_after="$work/fp-crlf-define-after"
mkdir -p "$fp_crlf_define_after/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_define_after/docs/footprint-companions.md"
printf 'define VARIABLE\r\n  other :\r\nendef\r\n  ghost :\r\n\t@true\r\n' >"$fp_crlf_define_after/Makefile"
expect pass check-footprint-companions.sh "$fp_crlf_define_after" \
  "a companions map whose target is a real CRLF rule after a CRLF define"

fp_crlf_target="$work/fp-crlf-target"
mkdir -p "$fp_crlf_target/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_target/docs/footprint-companions.md"
printf 'ghost:\r\n\t@true\r\n' >"$fp_crlf_target/Makefile"
expect pass check-footprint-companions.sh "$fp_crlf_target" \
  "a companions map whose target is a genuine CRLF rule"

fp_crlf_continued_comment="$work/fp-crlf-continued-comment"
mkdir -p "$fp_crlf_continued_comment/docs"
printf -- '- make-target: `ghost`\n' >"$fp_crlf_continued_comment/docs/footprint-companions.md"
printf '# disabled \\\r\n  more\r\nghost:\r\n\t@true\r\n' >"$fp_crlf_continued_comment/Makefile"
expect pass check-footprint-companions.sh "$fp_crlf_continued_comment" \
  "a companions map whose target is a real CRLF rule after a continued comment ends"

fp_relative_parent="$work/fp-relative-parent"
fp_relative="$fp_relative_parent/review=tree"
mkdir -p "$fp_relative/docs"
printf -- '- make-target: `goldens`\n' >"$fp_relative/docs/footprint-companions.md"
printf 'goldens:\n\ttrue\n' >"$fp_relative/Makefile"
if ! (cd "$fp_relative_parent" && \
  "$GATES/check-footprint-companions.sh" review=tree </dev/null) \
  >"$work/out" 2>&1; then
  echo "check-gates: FAIL: check-footprint-companions.sh rejected a relative root containing '='" >&2
  sed 's/^/  | /' "$work/out" >&2
  fail=1
fi

# --- generate-feature-board.py: the entry template parses once scaffolded (STD-06 R5, R6) ---
# The board generator skips _-prefixed folders, so no gate reads the template
# itself. Scaffold a feature from it the way CONVENTIONS.md tells authors to,
# with the stage set, and render the board: the generator raises on any key it
# requires that the template lacks, before it writes anything.
board_template="$REPO_ROOT/docs/design/_templates/1_overview.md"
# board_scaffold <tree> <template>: the generator plus one feature folder built from <template>
board_scaffold() {
  mkdir -p "$1/scripts" "$1/docs/design/feature-x"
  cp "$GATES/generate-feature-board.py" "$1/scripts/"
  sed 's/^stage: .*/stage: idea/' "$2" >"$1/docs/design/feature-x/1_overview.md"
}
# expect_board <pass|fail> <tree> <what>: render the board in <tree>
expect_board() {
  n=$((n + 1))
  local got=pass
  python3 "$2/scripts/generate-feature-board.py" >"$work/out" 2>&1 || got=fail
  if [[ "$got" != "$1" ]]; then
    echo "check-gates: FAIL: generate-feature-board.py on $3: expected $1, got $got" >&2
    sed 's/^/  | /' "$work/out" >&2
    fail=1
  fi
}

board_clean="$work/board-clean"
board_scaffold "$board_clean" "$board_template"
expect_board pass "$board_clean" "a feature scaffolded from the entry template"

board_no_next="$work/board-no-next"
sed '/^next:/d' "$board_template" >"$work/template-no-next.md"
board_scaffold "$board_no_next" "$work/template-no-next.md"
expect_board fail "$board_no_next" "an entry template without a next line"
if ! grep -Fq "missing next" "$work/out"; then
  echo "check-gates: FAIL: board fixture lacks its missing-next diagnostic" >&2
  fail=1
fi

if [[ "$fail" -eq 0 ]]; then
  echo "check-gates: ok ($n cases)"
fi
exit "$fail"
