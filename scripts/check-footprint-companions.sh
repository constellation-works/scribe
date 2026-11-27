#!/usr/bin/env bash
# scripts/check-footprint-companions.sh [root] — the companions map
# (docs/footprint-companions.md) lists every coupled set, and this gate
# checks the map matches the tree.
#
# Fails, naming the entry, when:
#   (a) the map itself is missing under the root;
#   (b) a `- path: `...`` line in the map's Checked entries block names
#       a path that does not exist under the root;
#   (c) a `- make-target: `...`` line in the map names a target the root
#       Makefile does not define;
#   (d) the map names no checked entries at all (fail closed, STD-04 R10).
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it
# at fixture trees. Needs bash, grep, sed, and awk, so the gate runs in
# `make ci-fast`.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${1:-$SCRIPT_DIR/..}"
MAP="docs/footprint-companions.md"

fail=0
err() { echo "footprint-companions: $*" >&2; fail=1; }

map="$ROOT/$MAP"
if [[ ! -f "$map" ]]; then
  echo "footprint-companions: $MAP is missing; it lists every coupled set" >&2
  exit 1
fi

paths=()
while IFS= read -r line; do
  [[ -n "$line" ]] && paths+=("$line")
done < <(grep -E '^- path: `[^`]+`$' "$map" | sed -E 's/^- path: `([^`]+)`$/\1/' || true)
targets=()
while IFS= read -r line; do
  [[ -n "$line" ]] && targets+=("$line")
done < <(grep -E '^- make-target: `[^`]+`$' "$map" | sed -E 's/^- make-target: `([^`]+)`$/\1/' || true)

if [[ "${#paths[@]}" -eq 0 && "${#targets[@]}" -eq 0 ]]; then
  err "$MAP names no checked entries; list each companion path and regeneration target"
fi

makefile="$ROOT/Makefile"
for p in ${paths[@]+"${paths[@]}"}; do
  if [[ ! -e "$ROOT/$p" ]]; then
    err "$MAP names \`$p\`, which does not exist"
  fi
done
for t in ${targets[@]+"${targets[@]}"}; do
  if [[ ! -f "$makefile" ]] || ! footprint_target="$t" awk '
    BEGIN { wanted = ENVIRON["footprint_target"] }
    index($0, wanted ":") == 1 {
      rest = substr($0, length(wanted) + 1)
      # These are variable assignments, not target rules. In particular,
      # :::= is the immediate-expansion assignment operator used by GNU make.
      if (rest ~ /^(:::=|::=|:=|\?=|\+=|!=|=)/) {
        next
      }
      # A target-specific assignment has whitespace after the target colon
      # and then a variable assignment rather than a prerequisite list.
      if (rest ~ /^:[ \t]+((export|unexport|override|private)[ \t]+)*[^ \t]+[ \t]*(:::=|::=|:=|\?=|\+=|!=|=)/) {
        next
      }
      found = 1
      exit
    }
    END { exit !found }
  ' < "$makefile"; then
    err "$MAP names make target \`$t\`, which is not a Makefile target"
  fi
done

if [[ "$fail" -eq 0 ]]; then
  echo "footprint-companions: ok (${#paths[@]} path(s), ${#targets[@]} target(s))"
fi
exit "$fail"
