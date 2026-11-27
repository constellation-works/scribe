#!/usr/bin/env bash
# scripts/check-source-style.sh [root] — STD-12 R26: paths may use `super`
# for one parent only; refer to more distant items from `crate::`.
#
# This text-match gate also checks comments and rustdoc links in Rust files.
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

if [[ ! -d crates ]]; then
  echo "source-style: cannot scan crates/ (directory is missing)" >&2
  exit 1
fi

fail=0
while IFS= read -r file; do
  if grep -nH -F 'super::super' "$file"; then
    echo "source-style: STD-12 R26 forbids multi-level super paths in Rust files" >&2
    fail=1
  fi
done < <(find crates -type f -name '*.rs' | sort)

if [[ "$fail" -eq 0 ]]; then
  echo "source-style: ok"
fi
exit "$fail"
