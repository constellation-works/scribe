#!/usr/bin/env bash
# scripts/redact-recording.sh — redact a session rebuild replay recording so
# it holds no readable text, in one step (docs/design/scribe/specs/replay-fixtures.md).
#
#   scripts/redact-recording.sh <recording-dir> <output-dir> < key-file
#   printf '%s\n' "$KEY" | scripts/redact-recording.sh <recording-dir> <output-dir>
#
# Maps letters and digits of every free text in the transcript, seed store
# and model outputs with a substitution fixed by the key, rewrites each
# call-NNN.in from a replay of the redacted recording, checks each new prompt
# is the recorded one with only its data mapped, and replays the result
# strictly against fixture.json's expectations. <output-dir> must not exist.
# The key is one line on stdin, read silently at a terminal. It is never in
# argv, where `ps` shows it to every user, and never exported: it reaches the
# session_replay test's redaction entry on that test's stdin. Keep it private
# and out of the repository: anyone with it can invert the substitution.
# Nothing reads or writes ~/.scribe.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <recording-dir> <output-dir>   (the key is one line on stdin)" >&2
  exit 2
fi

key=""
if [[ -t 0 ]]; then
  printf 'redaction key: ' >&2
  IFS= read -rs key || true
  printf '\n' >&2
else
  IFS= read -r key || true
fi
if [[ -z "$key" ]]; then
  echo "usage: the redaction key is one non-empty line on stdin" >&2
  exit 2
fi

from=$(cd "$1" && pwd -P)
to_parent=$(cd "$(dirname "$2")" && pwd -P)
to="$to_parent/$(basename "$2")"
if [[ -e "$to" ]]; then
  echo "error: $to already exists" >&2
  exit 1
fi

printf '%s\n' "$key" | SCRIBE_REDACT_FROM="$from" SCRIBE_REDACT_TO="$to" \
  cargo test --locked -p scribe --test session_replay -- --exact redact_a_recording
echo "redacted recording: $to" >&2
