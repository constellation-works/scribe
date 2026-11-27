#!/usr/bin/env bash
# scripts/check-terminal-guard.sh [root] — STD-01 R12/R17 and STD-02 R15, R3.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees. Over the non-test Rust sources, two rules:
#
#   1. Terminal state (`cw_cli::Terminal` / `cw_cli::Probe`) belongs to the
#      CLI's output module only. Anything else that decides or probes the
#      terminal would make a second, disagreeing decision.
#   2. The environment names the terminal decision and the log filter read
#      are declared and captured in composition only, and passed down as
#      values. A module that names one reads the environment itself. Composition
#      may capture them; it never decides the color, the width or the format.
#
# Environment reads (`std::env::var` and its kin), ordinary standard stream
# calls (`std::io::stdout`, `std::io::stderr`,
# `std::io::IsTerminal::is_terminal`), and `print!`/`eprint!`/`println!`/
# `eprintln!` are Clippy's (`clippy.toml` and `Cargo.toml`, STD-02 R6, R15).
# Public reexports of `std::io::stdout` and `std::io::stderr` are checked here:
# Clippy's method-path lint does not see an exported use declaration.
set -euo pipefail
root="${1:-$(dirname "$0")/..}"
if ! cd "$root"; then
  echo "terminal-guard: cannot scan: root '$root' is missing or not a directory" >&2
  exit 1
fi

OUTPUT_DIR=crates/scribe/src/output
COMPOSITION=crates/scribe-core/src/composition.rs
TERMINAL='Terminal::(decide|detect)|Probe::detect'
ENV_NAMES='"NO_COLOR"|"CLICOLOR_FORCE"|"TERM"|"COLUMNS"|SCRIBE_FORMAT|SCRIBE_LOG'
STREAM_REEXPORT='pub[[:space:]]+use[[:space:]]+(::)?std::io::(stdout|stderr)([[:space:]]+as[[:space:]]+[[:alpha:]_][[:alnum:]_]*)?[[:space:]]*;|pub[[:space:]]+use[[:space:]]+(::)?std::io::[{]([[:space:]]*|[^}]*[[:space:],])(stdout|stderr)([[:space:]]+as[[:space:]]+[[:alpha:]_][[:alnum:]_]*)?([[:space:],}]|$)'

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

# Rust use trees can span several lines. Keep the source check focused on
# complete public use declarations so rustfmt's line wrapping cannot hide one.
scan_stream_reexports() {
  local raw
  if ! raw=$(find crates -type d -name tests -prune -o -type f -name '*.rs' \
    -exec awk -v pattern="$STREAM_REEXPORT" '
      function emit() {
        if (pending && statement ~ pattern) {
          printf "%s:%d:%s\n", statement_file, statement_line, statement
        }
        pending = 0
        statement = ""
      }
      FNR == 1 && NR > 1 { emit() }
      {
        line = $0
        sub(/[[:space:]]*\/\/.*/, "", line)
        if (line ~ /^[[:space:]]*$/) next
        if (!pending) {
          if (line !~ /pub[[:space:]]+use[[:space:]]+/) next
          pending = 1
          statement = line
          statement_file = FILENAME
          statement_line = FNR
        } else {
          statement = statement " " line
        }
        if (statement ~ /;/) emit()
      }
      END { emit() }
    ' {} +); then
    echo "terminal-guard: cannot scan the Rust sources under crates/" >&2
    return 2
  fi
  printf '%s' "$raw"
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
  echo "terminal-guard: terminal state and probes belong to $OUTPUT_DIR only" >&2
  status=1
fi

if stream_raw=$(scan_stream_reexports); then
  :
else
  scan_rc=$?
  exit "$scan_rc"
fi
filter_rc=0
stream_hits=$(filter_allowlist "$stream_raw" "$OUTPUT_DIR/") || filter_rc=$?
if [[ "$filter_rc" -ne 0 ]]; then
  exit "$filter_rc"
fi
if [[ -n "$stream_hits" ]]; then
  echo "$stream_hits" >&2
  echo "terminal-guard: public standard stream reexports belong to $OUTPUT_DIR only" >&2
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
