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

# The Models port may not depend on its callers.
port=$(tree port-clean)
printf 'use crate::voice::Voice;\n' >"$port/crates/scribe-core/src/models.rs"
expect pass check-dependency-direction.sh "$port" "a port using a lower-level module"
printf 'use crate::{voice::{group, Voice}};\n' >"$port/crates/scribe-core/src/models.rs"
expect pass check-dependency-direction.sh "$port" "a nested name that is not a caller module"
for module in ask memory group; do
  printf 'use crate::%s::Item;\n' "$module" >"$port/crates/scribe-core/src/models.rs"
  expect fail check-dependency-direction.sh "$port" "a port importing $module"
  printf 'use crate::{voice::Voice,\n    %s::Item};\n' "$module" >"$port/crates/scribe-core/src/models.rs"
  expect fail check-dependency-direction.sh "$port" "a port importing $module in a group"
done

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
# format_tree <name> <FORMAT>: a fixture root whose codec.rs declares FORMAT
# and whose store spec is read from stdin.
format_tree() {
  local root
  root=$(tree "$1")
  mkdir -p "$root/crates/scribe-core/src/store" "$root/docs/design/scribe/specs"
  printf 'pub(super) const FORMAT: u32 = %s;\n' "$2" >"$root/crates/scribe-core/src/store/codec.rs"
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

if [[ "$fail" -eq 0 ]]; then
  echo "check-gates: ok ($n cases)"
fi
exit "$fail"
