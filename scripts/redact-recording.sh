#!/usr/bin/env bash
# scripts/redact-recording.sh — redact a session rebuild replay recording so
# it holds no readable text, in one step (docs/design/scribe/specs/replay-fixtures.md).
#
#   scripts/redact-recording.sh <recording-dir> <output-dir> <key>
#
# Maps letters and digits of every free text in the transcript, seed store
# and model outputs with a substitution fixed by <key>, rewrites each
# call-NNN.in from a replay of the redacted recording, checks each new prompt
# is the recorded one with only its data mapped, and replays the result
# strictly against fixture.json's expectations. <output-dir> must not exist.
# Keep <key> private and out of the repository: anyone with it can invert the
# substitution. Runs the session_replay integration test's redaction entry;
# it never reads or writes ~/.scribe.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ $# -ne 3 || -z "$3" ]]; then
  echo "usage: $0 <recording-dir> <output-dir> <key>" >&2
  exit 2
fi
from=$(cd "$1" && pwd -P)
to_parent=$(cd "$(dirname "$2")" && pwd -P)
to="$to_parent/$(basename "$2")"
if [[ -e "$to" ]]; then
  echo "error: $to already exists" >&2
  exit 1
fi

SCRIBE_REDACT_FROM="$from" SCRIBE_REDACT_TO="$to" SCRIBE_REDACT_KEY="$3" \
  cargo test --locked -p scribe --test session_replay -- --exact redact_a_recording
echo "redacted recording: $to" >&2
