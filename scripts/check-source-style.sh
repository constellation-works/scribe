#!/usr/bin/env bash
# scripts/check-source-style.sh [root] — STD-12 R26: paths may use `super`
# for one parent only; refer to more distant items from `crate::`.
#
# This text-match gate also checks comments and rustdoc links in Rust files.
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
set -euo pipefail
root="${1:-$(dirname "$0")/..}"
if ! cd "$root"; then
  echo "source-style: cannot scan: root '$root' is missing or not a directory" >&2
  exit 1
fi

if [[ ! -d crates ]]; then
  echo "source-style: cannot scan crates/ (directory is missing)" >&2
  exit 1
fi

# Fail closed when enumeration itself fails (STD-04 R10): with `pipefail`
# the pipeline status carries a `find` (or `sort`) failure out of the
# command substitution instead of masking it behind `sort`'s exit 0.
if ! file_list=$(find crates -type f -name '*.rs' | sort); then
  echo "source-style: cannot scan: failed to enumerate Rust sources under crates/" >&2
  exit 1
fi

if [[ -z "$file_list" ]]; then
  echo "source-style: cannot scan: no Rust sources under crates/" >&2
  exit 1
fi

# The plugin conformance test compiles into `crates/scribe/tests/plugin.rs`
# via `#[path = ...]`, so it is a compiled Rust source outside `crates/`
# and is scanned with the same rule when present.
scan_list="$file_list"
if [[ -f plugin/.orbit-plugin/tests/conformance.rs ]]; then
  scan_list+=$'\nplugin/.orbit-plugin/tests/conformance.rs'
fi

fail=0
while IFS= read -r file; do
  [[ -n "$file" ]] || continue
  if grep -nH -F 'super::super' "$file"; then
    echo "source-style: STD-12 R26 forbids multi-level super paths in Rust files" >&2
    fail=1
  else
    rc=$?
    if [[ "$rc" -gt 1 ]]; then
      echo "source-style: cannot scan $file (grep exit $rc)" >&2
      exit 1
    fi
  fi
done <<<"$scan_list"

if [[ "$fail" -eq 0 ]]; then
  echo "source-style: ok"
fi
exit "$fail"
