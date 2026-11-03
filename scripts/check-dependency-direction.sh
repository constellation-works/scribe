#!/usr/bin/env bash
# scripts/check-dependency-direction.sh — STD-02 R1–R6, R16, R17 for this workspace.
#
# Two layers of checks, both from source files alone (no build, no network,
# no jq), so the gate runs in `make ci-fast`:
#
#   1. Crate edges. Every workspace crate needs a policy below. A crate may
#      depend only on the internal crates in its allowlist, and never on the
#      external crates in its banlist. Dev-dependencies are exempt. A crate
#      without a policy fails the check, so a new member forces a decision.
#   2. Module edges inside a crate, as grep bans over non-test sources.
#
# The layer order is written down in ARCHITECTURE.md; change it and this
# script in the same commit (STD-02 R6).
set -euo pipefail
cd "$(dirname "$0")/.."

PREFIX="scribe"
fail=0
err() { echo "dependency-direction: $*" >&2; fail=1; }

# --- 1. crate policies -------------------------------------------------------
# policy <crate>  -> sets ALLOWED (internal deps) and BANNED (external deps)
policy() {
  case "$1" in
    scribe-core)
      ALLOWED=""
      # The domain never parses arguments, draws on a terminal or installs a
      # log subscriber: those belong to surfaces.
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored"
      ;;
    scribe)
      ALLOWED="scribe-core"
      BANNED=""
      ;;
    *) return 1 ;;
  esac
}

# Print the dependency names declared in the non-dev dependency tables of a
# Cargo.toml: [dependencies], [build-dependencies] and target-specific ones.
declared_deps() {
  awk '
    /^\[/ {
      section = $0
      in_deps = (section ~ /^\[(target\..*\.)?(build-)?dependencies\]$/)
      next
    }
    in_deps && /^[A-Za-z0-9_-]+[[:space:]]*(\.workspace)?[[:space:]]*=/ {
      name = $1; sub(/\.workspace$/, "", name); sub(/=.*/, "", name)
      gsub(/[[:space:]]/, "", name); print name
    }
  ' "$1"
}

contains() { # contains <word> <space-separated list>
  local word="$1" item
  for item in $2; do [[ "$item" == "$word" ]] && return 0; done
  return 1
}

for manifest in crates/*/Cargo.toml; do
  crate=$(awk -F'"' '/^name[[:space:]]*=/ { print $2; exit }' "$manifest")
  if ! policy "$crate"; then
    err "crate '$crate' ($manifest) has no policy; add one here and a row in ARCHITECTURE.md"
    continue
  fi
  while IFS= read -r dep; do
    if [[ "$dep" == "$PREFIX" || "$dep" == "$PREFIX"-* ]]; then
      contains "$dep" "$ALLOWED" || err "$crate must not depend on internal crate $dep"
    elif contains "$dep" "$BANNED"; then
      err "$crate must not depend on $dep (surface crate in the domain)"
    fi
  done < <(declared_deps "$manifest")
done

# --- 2. module bans ----------------------------------------------------------
# ban <path> <extended-regex> <message>: fail if the pattern appears in
# non-test, non-comment lines of the Rust sources under <path> (a file or a
# directory).
ban() {
  local hits
  hits=$(grep -rnHE --include='*.rs' --exclude-dir=tests "$2" "$1" 2>/dev/null \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true)
  if [[ -n "$hits" ]]; then
    echo "$hits" >&2
    err "$3"
  fi
}

CORE=crates/scribe-core/src
CLI=crates/scribe/src

# Core is business logic plus one persistence module (R5). Every core module
# but store/ is I/O-free and knows nothing of how records are stored; a new
# business module is covered as soon as it exists.
for path in "$CORE"/*; do
  name=$(basename "$path")
  case "$name" in store | tests) continue ;; esac
  ban "$path" 'std::(fs|net|process)|tempfile::|rustix::' "core/$name is business logic: file and process access belong in store/"
  ban "$path" 'crate::store'                              "core/$name is business logic: it must not depend on persistence"
done
# Inside store/: fsio is mechanism with no knowledge of records, and the
# format, redaction and audit record shapes do no I/O of their own (R4, R5).
ban "$CORE/store/fsio.rs"   'crate::(entry|tag|session|proposal|record|config|verbatim|store)|super::' \
                                                                 "store/fsio.rs is mechanism only; it must not know records or the store"
ban "$CORE/store/format.rs" 'std::fs|super::fsio'                "store/format.rs converts bytes; it must not do I/O"
ban "$CORE/store/redact.rs" 'std::fs|super::fsio'                "store/redact.rs masks text it is handed; it must not do I/O"
ban "$CORE/store/audit.rs"  'std::fs|super::fsio'                "store/audit.rs is a record shape; it must not do I/O"
# The domain receives resolved roots and configuration (R3).
ban "$CORE"          'std::env::|home_dir|current_dir'           "the core crate must not read the environment or discover paths"
# Composition does not know about parsing, dispatch or rendering (R2).
ban "$CLI/app.rs"    'crate::(cli|commands|output|audit_middleware)' "app.rs (composition) must not import surface modules"
# Rendering is derived from payloads only; it never reaches back up (R1).
ban "$CLI/output"    'crate::(app|cli|commands|audit_middleware)'    "output must not import app, cli, commands or audit_middleware"
# Dispatch depends on the middleware, never the reverse (R1).
ban "$CLI/audit_middleware.rs" 'crate::(cli|commands)'           "audit_middleware must not import cli or commands"

# Commands may read files the host owns (a hook's transcript, git config), but
# every write goes through the store's atomic writer under its lock (R4).
ban "$CLI/commands"  'std::fs::(write|remove|rename|create|copy|set_permissions)|OpenOptions' \
                                                                 "commands must not write files: store writes go through scribe-core's store"

# Persisted shapes never fabricate a timestamp on read (R16).
ban crates 'serde\(default *= *"[A-Za-z_:]*(now|now_utc)"'    "a persisted timestamp must not default to the current time"

# Retired paths stay retired (R17). List a path here when you delete a module.
for retired in \
  crates/scribe/src/command.rs \
  crates/scribe/src/tests/command.rs \
  crates/scribe-core/src/fsio.rs \
  crates/scribe-core/src/store.rs \
  crates/scribe-core/src/tests/fsio.rs \
  crates/scribe-core/src/tests/store.rs \
  crates/scribe-core/src/note.rs \
  crates/scribe-core/src/query.rs \
  crates/scribe-core/src/tests/note.rs \
  crates/scribe-core/src/tests/query.rs \
  crates/scribe/src/commands/note.rs \
  crates/scribe/src/commands/tests/note.rs; do
  [[ -e "$retired" ]] && err "retired path exists again: $retired"
done

if [[ "$fail" -eq 0 ]]; then
  echo "dependency-direction: ok"
fi
exit "$fail"
