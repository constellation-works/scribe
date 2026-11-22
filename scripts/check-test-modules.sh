#!/usr/bin/env bash
# scripts/check-test-modules.sh [root] — STD-02 R19: every unit-test file
# under a `src/**/tests/` directory is declared in that directory's `mod.rs`,
# and every such directory is itself declared (`mod tests;`) by its parent
# module. An undeclared file or directory compiles to nothing, so its tests
# silently never run.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

fail=0
declares() { # declares <file> <module>: the file has `mod <module>;`
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
