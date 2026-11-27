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
#       Makefile does not define as a rule (a line inside a define/endef
#       variable body is the variable's value, not a rule);
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
    BEGIN { wanted = ENVIRON["footprint_target"]; depth = 0 }
    # Judge one logical line: physical lines joined across trailing
    # backslashes, as GNU make reads them.
    function judge(line,    rest) {
      # A tab-indented line is a recipe, and a line whose first non-blank
      # character is # is a comment, whatever it continues onto.
      if (line ~ /^\t/ || line ~ /^ *#/) {
        return 0
      }
      sub(/^ +/, "", line)
      if (index(line, wanted) != 1) {
        return 0
      }
      # Whitespace may separate the target from its colon.
      rest = substr(line, length(wanted) + 1)
      sub(/^[ \t]+/, "", rest)
      if (substr(rest, 1, 1) != ":") {
        return 0
      }
      # These are variable assignments, not target rules. In particular,
      # :::= is the immediate-expansion assignment operator used by GNU make.
      if (rest ~ /^(:::=|::=|:=|\?=|\+=|!=|=)/) {
        return 0
      }
      # A target-specific assignment has whitespace after the target colon
      # and then a variable assignment rather than a prerequisite list.
      if (rest ~ /^:[ \t]+((export|unexport|override|private)[ \t]+)*[^ \t]+[ \t]*(:::=|::=|:=|\?=|\+=|!=|=)/) {
        return 0
      }
      return 1
    }
    # A define line opens a variable body. Its first word follows any export,
    # override or private modifiers, and a tab-led line is a recipe, never one.
    function opens_define(line,    word) {
      if (line ~ /^\t/) {
        return 0
      }
      word = line
      sub(/^[ \t]+/, "", word)
      while (word ~ /^(export|override|private)([ \t]|$)/) {
        sub(/^[a-z]+[ \t]*/, "", word)
      }
      return word ~ /^define([ \t]|$)/
    }
    # Inside a body, GNU make reads only define and endef lines, and a tab-led
    # line is never either. A nested define adds a level; endef closes one.
    function track_body(line,    word) {
      if (line ~ /^\t/) {
        return
      }
      word = line
      sub(/^[ \t]+/, "", word)
      if (word ~ /^define([ \t]|$)/) {
        depth++
      } else if (word ~ /^endef([ \t]|$)/) {
        depth--
      }
    }
    {
      physical = $0
      # A CRLF line ends in a carriage return that GNU Make reads as line
      # ending, so drop it: a backslash before it still continues the line.
      sub(/\r$/, "", physical)
      # An odd run of trailing backslashes continues onto the next line.
      continued = (physical ~ /(^|[^\\])(\\\\)*\\$/)
      if (continued) {
        sub(/\\$/, " ", physical)
      }
      logical = logical physical
      if (continued) {
        next
      }
      if (depth > 0) {
        track_body(logical)
      } else if (opens_define(logical)) {
        depth = 1
      } else if (judge(logical)) {
        found = 1
        exit
      }
      logical = ""
    }
    END {
      if (!found && logical != "" && depth == 0 && judge(logical)) {
        found = 1
      }
      exit !found
    }
  ' < "$makefile"; then
    err "$MAP names make target \`$t\`, which is not a Makefile target"
  fi
done

if [[ "$fail" -eq 0 ]]; then
  echo "footprint-companions: ok (${#paths[@]} path(s), ${#targets[@]} target(s))"
fi
exit "$fail"
