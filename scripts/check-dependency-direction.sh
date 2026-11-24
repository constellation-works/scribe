#!/usr/bin/env bash
# scripts/check-dependency-direction.sh [root] — STD-02 R1–R6, R16, R17 for this workspace.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
#
# Two layers of checks, both from source files alone (no build, no network,
# no jq), so the gate runs in `make ci-fast`:
#
#   1. Crate edges. Every workspace crate needs a policy below. A crate may
#      depend only on the internal crates in its allowlist, and never on the
#      external crates in its banlist. Dev-dependencies are exempt. A crate
#      without a policy fails the check, so a new member forces a decision.
#   2. Module edges inside a crate: scribe-domain's ring order, and grep
#      bans over non-test sources for what a crate edge cannot express
#      (std I/O, the environment, one module's mechanism inside a crate).
#
# The layer order is written down in ARCHITECTURE.md; change it and this
# script in the same commit (STD-02 R6).
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

PREFIX="scribe"
fail=0
err() { echo "dependency-direction: $*" >&2; fail=1; }

# --- 1. crate policies -------------------------------------------------------
# policy <crate>  -> sets ALLOWED (internal deps) and BANNED (external deps)
policy() {
  case "$1" in
    scribe-domain)
      ALLOWED=""
      # The domain never parses arguments, draws on a terminal or installs a
      # log subscriber: those belong to surfaces. Nor does it touch files,
      # processes or scribe.db: those belong to the store.
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite tempfile rustix"
      ;;
    scribe-store)
      # The only crate that touches the data root and scribe.db.
      ALLOWED="scribe-domain"
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored"
      ;;
    scribe-tools)
      # Functions of host files, bytes and text. It depends on the store for
      # the capture position a window resumes from and, until the domain is
      # dissolved (ORB-27921 step 5), on scribe-domain.
      ALLOWED="scribe-domain scribe-store"
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite tempfile"
      ;;
    scribe-core)
      ALLOWED="scribe-domain scribe-store scribe-tools"
      # The use cases reach files and scribe.db only through the store. Its
      # tests may stage database states with rusqlite: dev-dependencies are
      # exempt (declared_deps reads normal dependencies only). rustix and
      # tempfile are allowed for providers/, whose model adapters supervise
      # process groups and hand the codex child its schema in a private
      # temporary directory.
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite"
      ;;
    scribe)
      # The CLI reaches scribe.db through scribe_core::Store; its tests may
      # stage database states with rusqlite as a dev-dependency. Composition,
      # the tools and the audit guard live in the core (ORB-27921 step 3).
      ALLOWED="scribe-core"
      BANNED="rusqlite"
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

# ban_std <path> <modules> <message>: fail if a non-test Rust source under
# <path> names std::<module> for one of the |-separated <modules>, either as a
# path (`std::fs::write`, `use std::fs;`) or inside a grouped import that may
# span lines (`use std::{fmt, fs};`, `use std::{io, fs::File}`), which a
# line grep for `std::fs` misses.
ban_std() {
  ban "$1" "std::($2)" "$3"
  local hits
  hits=$(find "$1" -name '*.rs' -not -path '*/tests/*' -print0 2>/dev/null \
    | xargs -0 perl -0777 -ne '
        while (/^[ \t]*(?:pub(?:\([a-z]+\))?[ \t]+)?use[ \t]+(?:::)?std::\{([^;]*)\};/mg) {
          my ($stmt, $group, $at) = ($&, $1, $-[0]);
          next unless $group =~ /(?<![A-Za-z0-9_:])(?:'"$2"')(?![A-Za-z0-9_])/;
          my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
          $stmt =~ s/\s+/ /g;
          print "$ARGV:$line:$stmt\n";
        }' 2>/dev/null || true)
  if [[ -n "$hits" ]]; then
    echo "$hits" >&2
    err "$3"
  fi
}

DOMAIN=crates/scribe-domain/src
STORE=crates/scribe-store/src
CORE=crates/scribe-core/src
CLI=crates/scribe/src

# The crate edges keep the use cases out of scribe-domain and scribe-store,
# and rusqlite out of every crate but the store. What they
# cannot keep out is std: scribe-domain and scribe-core do no file, network
# or process I/O of their own (R4, R5), with two exceptions in the core:
# composition.rs resolves the host (it may name std::fs), and providers/
# spawns the model processes (std::fs and std::process).
for path in "$DOMAIN"/* "$CORE"/*; do
  case "$(basename "$path")" in tests) continue ;; providers)
    ban_std "$path" 'net' "${path#crates/}: the model adapters open no socket"
    continue ;;
  composition.rs)
    ban_std "$path" 'net|process' "${path#crates/}: composition resolves the host; providers/ spawns the model processes"
    continue ;;
  esac
  ban_std "$path" 'fs|net|process' "${path#crates/}: file and process access belong in scribe-store"
done

# scribe-domain's rings (ORB-27707 step 4; ARCHITECTURE.md): every module is
# in one ring and imports only modules of its own ring or an earlier one,
# whether by path (`crate::session::…`), in a grouped import
# (`crate::{session, …}`) or through a name the crate root re-exports
# (`crate::SessionId`). A new module fails until it is given a ring here.
DOMAIN_RINGS='
base:       record provider
records:    session entry handoff
'
ring_hits=$(DOMAIN_RINGS="$DOMAIN_RINGS" perl -e '
  use strict; use warnings;
  my $root = shift;
  my (%ring, %rank); my $n = 0;
  for my $line (split /\n/, $ENV{DOMAIN_RINGS}) {
    next unless $line =~ /^(\w+):\s*(.*)$/;
    $rank{$1} = $n++;
    $ring{$_} = $1 for split " ", $2;
  }
  local $/;
  open my $lib, "<", "$root/lib.rs" or die "$root/lib.rs: $!";
  my $l = <$lib>;
  my (%mods, %name);
  $mods{$1} = 1 while $l =~ /^pub mod (\w+);/mg;
  while ($l =~ /^pub use (\w+)::\{([^}]*)\};/mg) {
    my ($m, $g) = ($1, $2);
    $name{(split /\s+as\s+/)[-1]} = $m for grep { length } map { s/\s//gr } split /,/, $g;
  }
  $name{$2} = $1 while $l =~ /^pub use (\w+)::(\w+);/mg;
  for my $m (sort keys %mods) {
    next if $m eq "fixtures";    # test support, compiled for tests only
    print "$root/lib.rs: module $m has no ring\n" unless $ring{$m};
  }
  my @files = split /\0/, `find "$root" -name "*.rs" -not -path "*/tests/*" -print0`;
  for my $file (@files) {
    my ($rel) = $file =~ m{^\Q$root\E/(.*)$};
    next if $rel eq "lib.rs" || $rel eq "fixtures.rs";
    my ($src) = $rel =~ m{^(\w+)};
    next unless $ring{$src};
    open my $fh, "<", $file or die "$file: $!";
    my $s = <$fh>;
    $s =~ s{^\s*//.*$}{}mg;
    my @refs;
    push @refs, [$1, $-[0]] while $s =~ /crate::(\w+)/g;
    while ($s =~ /crate::\{/g) {
      my ($at, $i, $d) = ($-[0], pos($s), 1);
      my $j = $i;
      while ($d && $j < length $s) { my $c = substr($s, $j++, 1); $d++ if $c eq "{"; $d-- if $c eq "}"; }
      my $g = substr($s, $i, $j - $i - 1);
      1 while $g =~ s/\{[^{}]*\}//g;
      push @refs, [$_, $at] for map { /^\s*(\w+)/ ? $1 : () } split /,/, $g;
      pos($s) = $j;
    }
    for my $r (@refs) {
      my ($t, $at) = @$r;
      my $m = $mods{$t} ? $t : $name{$t};
      next unless $m && $ring{$m} && $m ne $src;
      next if $rank{$ring{$m}} <= $rank{$ring{$src}};
      my $line = 1 + (substr($s, 0, $at) =~ tr/\n//);
      print "$file:$line: $src ($ring{$src}) imports $t from $m ($ring{$m})\n";
    }
  }
' "$DOMAIN" 2>&1 || true)
if [[ -n "$ring_hits" ]]; then
  echo "$ring_hits" >&2
  err "scribe-domain's ring order: a module imports only its own ring or an earlier one (ARCHITECTURE.md)"
fi

# scribe-core's apps (ORB-27921 step 2; ARCHITECTURE.md, the architecture
# spec §Apps): every directory under apps/ is in the order below, and an app
# imports only apps earlier in it, and only through what that app's mod.rs
# exports (`crate::apps::entry::Item`), never a flow file
# (`crate::apps::entry::flow::…`). A new app fails until it is given a place.
APP_ORDER='tag mark memory entry proposal session group search brief consistency hook admin'
for shared in common shared util; do
  [[ -e "$CORE/apps/$shared" ]] && err "apps/$shared: code two apps need goes to the lower one, to runtime or to the tools"
done
app_hits=$(APP_ORDER="$APP_ORDER" perl -e '
  use strict; use warnings;
  my $root = shift;
  my $n = 0; my %rank = map { $_ => $n++ } split " ", $ENV{APP_ORDER};
  exit 0 unless -d $root;
  opendir my $dh, $root or die "$root: $!";
  for my $d (sort grep { !/^\./ && -d "$root/$_" } readdir $dh) {
    print "$root/$d: app $d has no place in the app order\n" unless exists $rank{$d};
  }
  local $/;
  my @files = split /\0/, `find "$root" -name "*.rs" -not -path "*/tests/*" -print0`;
  for my $file (@files) {
    my ($src) = $file =~ m{^\Q$root\E/(\w+)/};
    next unless defined $src && exists $rank{$src};
    open my $fh, "<", $file or die "$file: $!";
    my $s = <$fh>;
    $s =~ s{^\s*//.*$}{}mg;
    my @refs;
    push @refs, [$1, $2, $-[0]] while $s =~ /crate::apps::(\w+)((?:::\w+)*(?:::\{)?)/g;
    while ($s =~ /crate::apps::\{/g) {
      my ($at, $i, $d) = ($-[0], pos($s), 1);
      my $j = $i;
      while ($d && $j < length $s) { my $c = substr($s, $j++, 1); $d++ if $c eq "{"; $d-- if $c eq "}"; }
      my $g = substr($s, $i, $j - $i - 1);
      1 while $g =~ s/\{[^{}]*\}/X/g;
      for (split /,/, $g) { push @refs, [$1, $2, $at] if /^\s*(\w+)((?:::\w+)*)/ }
      pos($s) = $j;
    }
    for my $r (@refs) {
      my ($t, $rest, $at) = @$r;
      next if $t eq $src;
      my $line = 1 + (substr($s, 0, $at) =~ tr/\n//);
      if (!exists $rank{$t}) { print "$file:$line: $src imports unknown app $t\n"; next }
      print "$file:$line: $src imports $t, which is not lower in the app order\n" if $rank{$t} >= $rank{$src};
      print "$file:$line: $src reaches into $t$rest: import what its mod.rs exports\n" if $rest =~ /^::[a-z_]\w*::/;
    }
  }
' "$CORE/apps" 2>&1 || true)
if [[ -n "$app_hits" ]]; then
  echo "$app_hits" >&2
  err "scribe-core's app order: an app imports only lower apps, through their mod.rs (ARCHITECTURE.md)"
fi
# ORB-27921 step 2 moved every use case into apps/ and deleted cases/
# (listed with the retired paths below). Nothing names it again: not the
# core, and not the binary, which reaches the core through each app's
# exec, Command, Outcome and Error.
ban "$CORE" 'crate::cases(::|[^[:alnum:]_]|$)' "cases/ is retired: a use case lives in its app under apps/"
ban "$CLI" 'scribe_core::cases(::|[^[:alnum:]_]|$)' "cases/ is retired: call the app's exec through scribe_core::apps::<app>"
# The model callers are five flows: session pass, catalog and rebuild,
# memory refresh and group close. Only those flows, and the mod.rs of their
# apps (which carries the adapter in its Command and the model failure in its
# Error), name providers, the Models port or ModelError; so does
# runtime/errors.rs, whose CommandError carries the model failure as the
# binary's error did (ORB-27921 step 3, PR 4). The request and answer shapes
# under crate::models stay free to use.
models_port='crate::providers(::|[^[:alnum:]_]|$)|(^|[^[:alnum:]_])(Models|ModelError|ModelsOnce|ModelsEach)([^[:alnum:]_]|$)'
while IFS= read -r file; do
  case "${file#"$CORE"/}" in
    apps/session/mod.rs | apps/session/catalog.rs | apps/session/rebuild.rs | apps/session/pass/* | \
      apps/memory/mod.rs | apps/memory/refresh.rs | apps/group/mod.rs | apps/group/close.rs | \
      runtime/errors.rs) continue ;;
  esac
  ban "$file" "$models_port" \
    "${file#crates/} names the Models port: only session pass, catalog, rebuild, memory refresh and group close call a model"
done < <(find "$CORE/apps" "$CORE/runtime" -name '*.rs' -not -path '*/tests/*' | sort)
# The providers know nothing of the apps or the runtime that call them.
ban "$CORE/providers" 'crate::(apps|runtime)(::|[^[:alnum:]_]|$)' "providers must not import the apps or the runtime"

# scribe-tools: only fs.rs names std::fs; nothing reads the environment,
# discovers a path, spawns a process, names the core or writes the store.
TOOLS=crates/scribe-tools/src
for path in "$TOOLS"/*; do
  case "$(basename "$path")" in tests | fs.rs) continue ;; esac
  ban_std "$path" 'fs|net|process' "${path#crates/}: only scribe-tools/src/fs.rs names std::fs, and the tools spawn nothing"
done
ban_std "$TOOLS/fs.rs" 'net|process' "scribe-tools/src/fs.rs: the tools spawn nothing and open no socket"
ban "$TOOLS" 'std::env::|home_dir|current_dir\(\)' "scribe-tools must not read the environment or discover paths"
ban "$TOOLS" 'scribe_core(::|[^[:alnum:]_]|$)' "scribe-tools must not name scribe_core: the core calls the tools"
ban "$TOOLS" 'Store::write|\.write\([[:space:]]*\|' "scribe-tools must not write the store"

# Pass is an orchestrator; its lower-level callers in the rules ring cannot
# depend on it.
for path in config.rs; do
  ban "$DOMAIN/$path" 'crate::pass(::|[^[:alnum:]_]|$)' "domain/$path must not depend on pass"
done
# The model shapes must not import their callers.
ban "$CORE/providers/models.rs" 'crate::(memory|group)(::|[^[:alnum:]_]|$)' "the model shapes must not import their callers"
# A grouped crate import can name a caller without spelling crate::caller.
port_grouped=$(perl -0777 -ne '
  while (/^[ \t]*(?:pub(?:\([a-z]+\))?[ \t]+)?use[ \t]+(?:::)?crate::\{([^;]*)\};/mg) {
    my ($stmt, $group, $at) = ($&, $1, $-[0]);
    1 while $group =~ s/\{[^{}]*\}//g;
    next unless $group =~ /(?<![A-Za-z0-9_:])(?:memory|group)(?![A-Za-z0-9_])/;
    my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
    $stmt =~ s/\s+/ /g;
    print "$ARGV:$line:$stmt\n";
  }' "$CORE/providers/models.rs" 2>/dev/null || true)
if [[ -n "$port_grouped" ]]; then
  echo "$port_grouped" >&2
  err "the model shapes must not import their callers"
fi

# Inside scribe-store: fsio is mechanism with no knowledge of records, and
# the codec and audit record shapes do no I/O of their own (R4, R5).
ban "$STORE/fsio.rs"        'crate::(entry|tag|session|proposal|record|config|verbatim)|super::' \
                                                                 "store/fsio.rs is mechanism only; it must not know records or the store"
for shape in codec:"converts bytes" audit:"is a record shape"; do
  file="$STORE/${shape%%:*}.rs"
  ban_std "$file" 'fs' "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
  ban "$file" 'super::fsio' "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
done
# SQLite is one store module's mechanism (2_design.md §15.7): every other
# store module reaches scribe.db through it, and the crate policies keep
# rusqlite out of every other crate. testing.rs is test support: it hands
# the tests a connection.
for path in "$STORE"/*; do
  case "$(basename "$path")" in tests | db.rs | db | testing.rs) continue ;; esac
  ban "$path" 'rusqlite' "scribe-store/${path#"$STORE"/} must not use rusqlite: scribe.db belongs to db.rs and db/"
done
# The domain receives resolved roots and configuration (R3).
# A child's working directory (`Command::current_dir(path)`, providers/) is
# set, not discovered: only the no-argument `current_dir()` is a discovery.
# In the core only composition.rs reads the environment or discovers a path.
for path in "$CORE"/*; do
  case "$(basename "$path")" in tests | composition.rs) continue ;; esac
  ban "$path" 'std::env::|home_dir|current_dir\(\)' "${path#crates/}: only composition.rs reads the environment or discovers paths"
done
# rustix and tempfile serve the model processes: only providers/ names them.
for path in "$CORE"/*; do
  case "$(basename "$path")" in tests | providers) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])(rustix|tempfile)::' "${path#crates/}: only providers/ uses rustix and tempfile"
done
# Composition builds the context the apps run against; it imports no app.
ban "$CORE/composition.rs" 'crate::apps(::|[^[:alnum:]_]|$)' "composition.rs must not import the apps"
ban "$DOMAIN"        'std::env::|home_dir|current_dir'           "the domain crate must not read the environment or discover paths"
ban "$STORE"         'std::env::|home_dir|current_dir'           "the store crate must not read the environment or discover paths"
# ORB-27921 step 4: db/ holds the storage mechanism. Only db/ names
# rusqlite (testing.rs, the test-support shim, hands tests a connection),
# and db/fsio.rs, mechanism only, names nothing else of the store but its
# error type.
while IFS= read -r -d '' path; do
  case "$path" in "$STORE"/db/* | "$STORE"/testing.rs) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])rusqlite::' "${path#crates/}: only scribe-store's db/ uses rusqlite"
done < <(find "$STORE" -name '*.rs' -not -path '*/tests/*' -print0)
fsio_hits=$(grep -nE '(crate|super)::' "$STORE/db/fsio.rs" | grep -vE '^[0-9]+:[[:space:]]*//' | grep -v 'crate::error::' || true)
if [[ -n "$fsio_hits" ]]; then
  echo "$fsio_hits" >&2
  err "scribe-store/src/db/fsio.rs: fsio imports nothing of the store but its error"
fi
# contracts/ holds record shapes only: it imports nothing from db/ or
# repository/, as a path or inside a grouped import, and decides nothing
# (no `check_*` function).
ban "$STORE/contracts" '(crate|super)::(db|repository)([^[:alnum:]_]|$)' \
  "scribe-store/src/contracts: contracts import nothing from db/ or repository/"
contracts_hits=$(find "$STORE/contracts" -name '*.rs' -print0 2>/dev/null \
  | xargs -0 perl -0777 -ne '
      while (/^[ \t]*(?:pub(?:\([a-z]+\))?[ \t]+)?use[ \t]+(?:crate|super)::\{([^;]*)\};/mg) {
        my ($stmt, $group, $at) = ($&, $1, $-[0]);
        next unless $group =~ /(?<![A-Za-z0-9_:])(?:db|repository)(?![A-Za-z0-9_])/;
        my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
        $stmt =~ s/\s+/ /g;
        print "$ARGV:$line:$stmt\n";
      }' 2>/dev/null || true)
if [[ -n "$contracts_hits" ]]; then
  echo "$contracts_hits" >&2
  err "scribe-store/src/contracts: contracts import nothing from db/ or repository/"
fi
ban "$STORE/contracts" '(^|[^[:alnum:]_])fn[[:space:]]+check_' \
  "scribe-store/src/contracts: a contract decides nothing (no check_* function)"
# Composition does not know about parsing, dispatch or rendering (R2).
# The model adapters spawn supervised processes through providers/cli.rs's
# supervisor in scribe-core, so process-group supervision, the cleared
# environment and the budget live in one place (STD-03 R11-R15, STD-05 R10).
ban "$CLI" 'process::(Command|Child)|Command::new' "the binary spawns no process: scribe-core's providers/ reaches a model"

# Rendering is derived from payloads only; it never reaches back up (R1).
ban "$CLI/output"    'crate::(app|cli|commands)'    "output must not import app, cli or commands"
# The binary reaches the core through its public surface only: the
# composition context, the apps and the re-exported types. It names no
# lower crate and no runtime path; the audit guard runs inside each app's
# exec (ORB-27921 step 3).
ban "$CLI" '(^|[^[:alnum:]_])scribe_(domain|store|tools)::' "the binary must not name scribe_domain, scribe_store or scribe_tools: it reaches them through scribe_core"
ban "$CLI" 'scribe_core::runtime(::|[^[:alnum:]_]|$)' "the binary must not name scribe_core::runtime: the runtime is internal to the core"

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
  crates/scribe/src/commands/tests/note.rs \
  crates/scribe/src/models.rs \
  crates/scribe/src/models \
  crates/scribe/src/tests/models.rs \
  crates/scribe/src/tests/codex_models.rs \
  crates/scribe/src/app.rs \
  crates/scribe/src/tests/app.rs \
  crates/scribe/src/commands/ports.rs \
  crates/scribe/src/commands/hook/tests/payload.rs \
  crates/scribe/src/commands/hook/project.rs \
  crates/scribe/src/commands/hook/group_turns \
  crates/scribe/src/audit_middleware.rs \
  crates/scribe/src/tests/audit_middleware.rs \
  crates/scribe/src/commands/hook/session_start.rs \
  crates/scribe/src/commands/hook/stop.rs \
  crates/scribe/src/commands/hook/session_end.rs \
  crates/scribe/src/commands/hook/codex.rs \
  crates/scribe-store/src/init.rs \
  crates/scribe-store/src/db.rs \
  crates/scribe-store/src/fsio.rs \
  crates/scribe-store/src/write.rs \
  crates/scribe-store/src/codec.rs \
  crates/scribe-store/src/archive.rs \
  crates/scribe-store/src/rebuild.rs \
  crates/scribe-store/src/repo.rs \
  crates/scribe-store/src/documents.rs \
  crates/scribe-store/src/ids.rs \
  crates/scribe-store/src/sessions.rs \
  crates/scribe-store/src/memory.rs \
  crates/scribe-store/src/tags.rs \
  crates/scribe-store/src/marks.rs \
  crates/scribe-store/src/pickups.rs \
  crates/scribe-store/src/candidates.rs \
  crates/scribe-store/src/audit.rs \
  crates/scribe-store/src/db/groups/close.rs \
  crates/scribe-core/src/cases \
  crates/scribe-domain/src/brief.rs \
  crates/scribe-domain/src/search.rs \
  crates/scribe-domain/src/consistency.rs \
  crates/scribe-domain/src/action.rs \
  crates/scribe-domain/src/checks \
  crates/scribe-domain/src/activity.rs \
  crates/scribe-domain/src/pass.rs \
  crates/scribe-domain/src/pass \
  crates/scribe-domain/src/redact.rs \
  crates/scribe-domain/src/annotation.rs \
  crates/scribe-domain/src/group.rs \
  crates/scribe-domain/src/capture.rs \
  crates/scribe-domain/src/instruction.rs \
  crates/scribe-domain/src/error.rs \
  crates/scribe-domain/src/message.rs \
  crates/scribe-domain/src/memory \
  crates/scribe-domain/src/transcript.rs \
  crates/scribe-domain/src/transcript \
  crates/scribe-domain/src/origin.rs \
  crates/scribe-domain/src/control.rs \
  crates/scribe-domain/src/config.rs \
  crates/scribe-domain/src/models.rs \
  crates/scribe-domain/src/text.rs \
  crates/scribe-domain/src/verbatim.rs \
  crates/scribe-domain/src/claim.rs \
  crates/scribe-domain/src/trigger.rs \
  crates/scribe-domain/src/proposal.rs \
  crates/scribe-domain/src/voice.rs \
  crates/scribe-domain/src/tag.rs; do
  [[ -e "$retired" ]] && err "retired path exists again: $retired"
done
# ORB-27707 step 4: scribe-core's business modules became scribe-domain
# and its store/ module scribe-store; scribe-core keeps apps/, runtime/,
# tests/ and lib.rs. A module file under scribe-core with one of these names is a copy
# that should be in the crate below.
for module in action activity annotation ask brief capture checks claim consistency \
  control entry error group handoff instruction memory message models origin pass proposal \
  provider record redact search session tag text transcript trigger verbatim voice \
  store testing; do
  for retired in "crates/scribe-core/src/$module.rs" "crates/scribe-core/src/$module"; do
    [[ -e "$retired" ]] && err "retired path exists again: $retired (now in scribe-domain or scribe-store)"
  done
done

if [[ "$fail" -eq 0 ]]; then
  echo "dependency-direction: ok"
fi
exit "$fail"
