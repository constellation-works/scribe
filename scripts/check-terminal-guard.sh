#!/usr/bin/env bash
# scripts/check-terminal-guard.sh [root] — STD-01 R12/R17 and STD-02 R15, R3.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees. Over the non-test Rust sources, two rules:
#
#   1. Terminal state and the std streams belong to the CLI's output module
#      only. Anything else that asks whether a stream is a terminal, decides
#      the terminal, or writes to a stream would make a second, disagreeing
#      decision.
#   2. The environment names the terminal decision and the log filter read
#      are declared and captured in composition only, and passed down as
#      values. A module that names one reads the environment itself. Composition
#      may capture them; it never decides the color, the width or the format.
#
# Environment reads as such (`std::env::var` and its kin) are clippy's
# (clippy.toml), not this script's.
set -euo pipefail
root="${1:-$(dirname "$0")/..}"
if ! cd "$root"; then
  echo "terminal-guard: cannot scan: root '$root' is missing or not a directory" >&2
  exit 1
fi

OUTPUT_DIR=crates/scribe/src/output
COMPOSITION=crates/scribe-core/src/composition.rs
TERMINAL='is_terminal|IsTerminal|Terminal::(decide|detect)|Probe::detect|io::stdout|io::stderr|println!|eprintln!|print!|eprint!'
ENV_NAMES='"NO_COLOR"|"CLICOLOR_FORCE"|"TERM"|"COLUMNS"|SCRIBE_FORMAT|SCRIBE_LOG'

# Matches outside tests, in code rather than a comment. A doc comment naming
# the variable it documents is not a read.
#
# grep exit 1 (no matches) is a clean result; only an exit above 1 is a scan
# failure. The trailing `|| true` covers only the comment filter's own
# no-match, never the scan itself.
scan() {
  local pattern="$1"
  local out err rc=0
  out=$(mktemp)
  err=$(mktemp)
  grep -rnE --include='*.rs' --exclude-dir=tests "$pattern" crates >"$out" 2>"$err" || rc=$?
  if [[ "$rc" -gt 1 ]]; then
    cat "$err" >&2
    echo "terminal-guard: scan failed (grep exit $rc)" >&2
    rm -f "$out" "$err"
    return 2
  fi
  rm -f "$err"
  grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' "$out" || true
  rm -f "$out"
}

# Fail closed when there is nothing to scan (STD-04 R10): an unusable root
# must not report ok.
if [[ ! -d crates ]]; then
  echo "terminal-guard: cannot scan: crates/ is missing or not a directory" >&2
  exit 1
fi
if ! find crates -name '*.rs' -print -quit 2>/dev/null | grep -q .; then
  echo "terminal-guard: cannot scan: no Rust sources under crates/" >&2
  exit 1
fi

status=0

if terminal_raw=$(scan "$TERMINAL"); then
  :
else
  scan_rc=$?
  echo "terminal-guard: cannot scan the Rust sources under crates/" >&2
  exit "$scan_rc"
fi
if [[ -n "$terminal_raw" ]]; then
  terminal_hits=$(printf '%s\n' "$terminal_raw" | grep -v "^$OUTPUT_DIR/" || true)
else
  terminal_hits=""
fi
if [[ -n "$terminal_hits" ]]; then
  echo "$terminal_hits" >&2
  echo "terminal-guard: terminal state and std streams belong to $OUTPUT_DIR only" >&2
  status=1
fi

if env_raw=$(scan "$ENV_NAMES"); then
  :
else
  scan_rc=$?
  echo "terminal-guard: cannot scan the Rust sources under crates/" >&2
  exit "$scan_rc"
fi
if [[ -n "$env_raw" ]]; then
  env_hits=$(printf '%s\n' "$env_raw" | grep -v "^$COMPOSITION:" || true)
else
  env_hits=""
fi
if [[ -n "$env_hits" ]]; then
  echo "$env_hits" >&2
  echo "terminal-guard: terminal and log environment names are captured by $COMPOSITION only" >&2
  status=1
fi

if [[ "$status" -ne 0 ]]; then
  exit 1
fi
echo "terminal-guard: ok"
