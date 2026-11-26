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
cd "${1:-$(dirname "$0")/..}"

OUTPUT_DIR=crates/scribe/src/output
COMPOSITION=crates/scribe-core/src/composition.rs
TERMINAL='is_terminal|IsTerminal|Terminal::(decide|detect)|Probe::detect|io::stdout|io::stderr|println!|eprintln!|print!|eprint!'
ENV_NAMES='"NO_COLOR"|"CLICOLOR_FORCE"|"TERM"|"COLUMNS"|SCRIBE_FORMAT|SCRIBE_LOG'

# Matches outside tests, in code rather than a comment. A doc comment naming
# the variable it documents is not a read.
scan() {
  grep -rnE --include='*.rs' --exclude-dir=tests "$1" crates 2>/dev/null \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true
}

status=0

terminal_hits=$(scan "$TERMINAL" | grep -v "^$OUTPUT_DIR/" || true)
if [[ -n "$terminal_hits" ]]; then
  echo "$terminal_hits" >&2
  echo "terminal-guard: terminal state and std streams belong to $OUTPUT_DIR only" >&2
  status=1
fi

env_hits=$(scan "$ENV_NAMES" | grep -v "^$COMPOSITION:" || true)
if [[ -n "$env_hits" ]]; then
  echo "$env_hits" >&2
  echo "terminal-guard: terminal and log environment names are captured by $COMPOSITION only" >&2
  status=1
fi

if [[ "$status" -ne 0 ]]; then
  exit 1
fi
echo "terminal-guard: ok"
