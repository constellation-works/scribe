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
# Each scan file must exist before grep runs. A file that cannot be created
# leaves nothing to redirect into, and the failed redirection exits 1, the
# status of a clean no-match. So a scan that cannot create its files fails
# here (status 2) and is never read as no matches.
#
# The scratch directory is checked before mktemp runs, and the files use an
# explicit template inside it. A bare mktemp falls back to another directory
# on some platforms (macOS does when TMPDIR is absent), so it cannot be the
# check that decides.
#
# grep exit 1 (no matches) is a clean result; only an exit above 1 is a scan
# failure. The comment filter's exit 1 is likewise its no-match, and only an
# exit above 1 from it fails the scan.
scan() {
  local pattern="$1"
  local scratch="${TMPDIR:-/tmp}"
  local out err rc=0 filter_rc=0
  if [[ ! -d "$scratch" || ! -w "$scratch" ]]; then
    echo "terminal-guard: cannot scan: cannot create a scan file in $scratch" >&2
    return 2
  fi
  if ! out=$(mktemp "$scratch/terminal-guard.XXXXXX"); then
    echo "terminal-guard: cannot scan: cannot create a scan file in $scratch" >&2
    return 2
  fi
  if ! err=$(mktemp "$scratch/terminal-guard.XXXXXX"); then
    rm -f "$out"
    echo "terminal-guard: cannot scan: cannot create a scan file in $scratch" >&2
    return 2
  fi
  grep -rnE --include='*.rs' --exclude-dir=tests "$pattern" crates >"$out" 2>"$err" || rc=$?
  if [[ "$rc" -gt 1 ]]; then
    cat "$err" >&2
    echo "terminal-guard: scan failed (grep exit $rc)" >&2
    rm -f "$out" "$err"
    return 2
  fi
  rm -f "$err"
  grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' "$out" || filter_rc=$?
  rm -f "$out"
  if [[ "$filter_rc" -gt 1 ]]; then
    echo "terminal-guard: scan failed (comment filter exit $filter_rc)" >&2
    return 2
  fi
  return 0
}

# Filters allowlisted hits out of a scan result. grep exit 1 is ordinary
# no-match (every hit was allowlisted); any exit above 1 is a scan failure.
filter_allowlist() {
  local raw="$1"
  local allow_prefix="$2"
  local rc=0
  if [[ -z "$raw" ]]; then
    return 0
  fi
  printf '%s\n' "$raw" | grep -v "^$allow_prefix" || rc=$?
  if [[ "$rc" -gt 1 ]]; then
    echo "terminal-guard: scan failed (allowlist filter exit $rc)" >&2
    return 2
  fi
  return 0
}

# Fail closed when there is nothing eligible to scan (STD-04 R10): an unusable
# root or a root whose only Rust files live under excluded `tests/` directories
# must not report ok.
if [[ ! -d crates ]]; then
  echo "terminal-guard: cannot scan: crates/ is missing or not a directory" >&2
  exit 1
fi
if ! eligible_rs=$(find crates -type d -name tests -prune -o -type f -name '*.rs' -print -quit); then
  echo "terminal-guard: cannot scan: failed to enumerate Rust sources under crates/" >&2
  exit 1
fi
if [[ -z "$eligible_rs" ]]; then
  echo "terminal-guard: cannot scan: no non-test Rust sources under crates/ (tests/ directories are excluded)" >&2
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
filter_rc=0
terminal_hits=$(filter_allowlist "$terminal_raw" "$OUTPUT_DIR/") || filter_rc=$?
if [[ "$filter_rc" -ne 0 ]]; then
  exit "$filter_rc"
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
filter_rc=0
env_hits=$(filter_allowlist "$env_raw" "$COMPOSITION:") || filter_rc=$?
if [[ "$filter_rc" -ne 0 ]]; then
  exit "$filter_rc"
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
