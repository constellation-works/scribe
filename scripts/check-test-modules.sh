#!/usr/bin/env bash
# scripts/check-test-modules.sh [root] — STD-02 R19: every unit-test file
# under a `src/**/tests/` directory is declared in that directory's `mod.rs`,
# and every such directory is itself declared (`mod tests;`) by its parent
# module. An undeclared file or directory compiles to nothing, so its tests
# silently never run. Each `tests/mod.rs` holds only `mod` declarations, comments
# and module-level `#![expect(...)]` lint expectations: its own tests, helpers
# and re-exports belong in a named file beside it.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

fail=0
declares() {
  [[ -f "$1" ]] && grep -qE "^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]+$2[[:space:]]*;" "$1"
}

while IFS= read -r file; do
  dir=$(dirname "$file")
  name=$(basename "$file" .rs)
  [[ "$name" == "mod" ]] && continue
  if ! declares "$dir/mod.rs" "$name"; then
    echo "test-modules: $file is not declared in $dir/mod.rs (add \`mod $name;\`)" >&2
    fail=1
  fi
done < <(find crates -path '*/src/*' -path '*/tests/*' -name '*.rs' | sort)

# `tests/mod.rs` is wiring only: print each line that is not a comment, a blank,
# a `mod name;` declaration or part of an inner `#![expect(...)]` attribute.
wiring_only() {
  awk '
    # The attribute ends at the first `)]`; only a comment may follow it on that
    # line, so an item cannot hide behind the closing bracket.
    in_attr {
      if (match($0, /\)\]/)) {
        in_attr = 0
        if (substr($0, RSTART + RLENGTH) !~ /^[[:space:]]*(\/\/.*)?$/) printf "%d: %s\n", NR, $0
      }
      next
    }
    /^[[:space:]]*$/ || /^[[:space:]]*\/\// { next }
    /^[[:space:]]*#!\[expect\(/ {
      if (!match($0, /\)\]/)) { in_attr = 1; next }
      if (substr($0, RSTART + RLENGTH) ~ /^[[:space:]]*(\/\/.*)?$/) next
      printf "%d: %s\n", NR, $0
      next
    }
    /^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*;[[:space:]]*(\/\/.*)?$/ { next }
    { printf "%d: %s\n", NR, $0 }
  ' "$1"
}

while IFS= read -r file; do
  if ! offending=$(wiring_only "$file"); then
    echo "test-modules: cannot read $file" >&2
    fail=1
    continue
  fi
  if [[ -n "$offending" ]]; then
    echo "test-modules: $file holds more than \`mod\` declarations (STD-02 R19; move tests, helpers and re-exports into a named file beside it):" >&2
    sed 's/^/  | /' <<<"$offending" >&2
    fail=1
  fi
done < <(find crates -path '*/src/*' -path '*/tests/mod.rs' | sort)

# The parent of `<p>/tests/` is `<p>/mod.rs` or `<p>.rs`, or for `src/tests/`
# the crate root `src/lib.rs` or `src/main.rs`.
while IFS= read -r dir; do
  parent=$(dirname "$dir")
  candidates=("$parent/mod.rs" "$parent.rs")
  if [[ "$(basename "$parent")" == "src" ]]; then
    candidates=("$parent/lib.rs" "$parent/main.rs")
  fi
  declared=0
  for candidate in "${candidates[@]}"; do
    if declares "$candidate" tests; then declared=1; fi
  done
  if [[ "$declared" -eq 0 ]]; then
    echo "test-modules: $dir/ is not declared by its parent module (add \`#[cfg(test)] mod tests;\` to ${candidates[0]} or ${candidates[1]})" >&2
    fail=1
  fi
done < <(find crates -path '*/src/*' -type d -name tests | sort)

if [[ "$fail" -eq 0 ]]; then
  echo "test-modules: ok"
fi
exit "$fail"
