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
    scribe-core)
      ALLOWED="scribe-domain scribe-store"
      # The use cases reach files and scribe.db only through the store. Its
      # tests may stage database states with rusqlite: dev-dependencies are
      # exempt (declared_deps reads normal dependencies only).
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite tempfile rustix"
      ;;
    scribe)
      # The CLI reaches scribe.db through scribe_core::Store; its tests may
      # stage database states with rusqlite as a dev-dependency.
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
# and rusqlite, tempfile and rustix out of every crate but the store. What
# they cannot keep out is std: scribe-domain and scribe-core do no file,
# network or process I/O of their own (R4, R5).
for path in "$DOMAIN"/* "$CORE"/*; do
  [[ "$(basename "$path")" == tests ]] && continue
  ban_std "$path" 'fs|net|process' "${path#crates/}: file and process access belong in scribe-store"
done

# scribe-domain's rings (ORB-27707 step 4; ARCHITECTURE.md): every module is
# in one ring and imports only modules of its own ring or an earlier one,
# whether by path (`crate::session::…`), in a grouped import
# (`crate::{session, …}`) or through a name the crate root re-exports
# (`crate::SessionId`). A new module fails until it is given a ring here.
DOMAIN_RINGS='
base:       record text verbatim provider origin
records:    session activity control entry handoff tag proposal
transcript: transcript
rules:      trigger voice claim brief capture consistency pass instruction models config error checks search group memory message action annotation redact
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
ban "$CORE/apps" 'crate::cases(::|[^[:alnum:]_]|$)' "an app must not import cases/: move what it needs first"

# Pass is an orchestrator; its lower-level callers in the rules ring cannot
# depend on it.
for path in config.rs annotation.rs consistency.rs instruction.rs memory message.rs; do
  ban "$DOMAIN/$path" 'crate::pass(::|[^[:alnum:]_]|$)' "domain/$path must not depend on pass"
done
# The Models port must not import its callers.
ban "$DOMAIN/models.rs" 'crate::(memory|group)(::|[^[:alnum:]_]|$)' "the Models port must not import its callers"
# A grouped crate import can name a caller without spelling crate::caller.
port_grouped=$(perl -0777 -ne '
  while (/^[ \t]*(?:pub(?:\([a-z]+\))?[ \t]+)?use[ \t]+(?:::)?crate::\{([^;]*)\};/mg) {
    my ($stmt, $group, $at) = ($&, $1, $-[0]);
    1 while $group =~ s/\{[^{}]*\}//g;
    next unless $group =~ /(?<![A-Za-z0-9_:])(?:memory|group)(?![A-Za-z0-9_])/;
    my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
    $stmt =~ s/\s+/ /g;
    print "$ARGV:$line:$stmt\n";
  }' "$DOMAIN/models.rs" 2>/dev/null || true)
if [[ -n "$port_grouped" ]]; then
  echo "$port_grouped" >&2
  err "the Models port must not import its callers"
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
ban "$CORE"          'std::env::|home_dir|current_dir'           "the core crate must not read the environment or discover paths"
ban "$DOMAIN"        'std::env::|home_dir|current_dir'           "the domain crate must not read the environment or discover paths"
ban "$STORE"         'std::env::|home_dir|current_dir'           "the store crate must not read the environment or discover paths"
# Composition does not know about parsing, dispatch or rendering (R2).
ban "$CLI/app.rs"    'crate::(cli|commands|output|audit_middleware)' "app.rs (composition) must not import surface modules"
# The model adapter is composed by app.rs and knows nothing of the surface (R2).
ban "$CLI/models.rs" 'crate::(cli|commands|output|audit_middleware)' "models.rs (the model adapter) must not import surface modules"
ban "$CLI/models" 'crate::(cli|commands|output|audit_middleware)' "model transport modules must not import surface modules"
# The model adapter spawns supervised processes through models.rs's
# supervisor, so process-group supervision, the cleared environment and the
# budget live in one place (STD-03 R11-R15, STD-05 R10).
for path in "$CLI"/*; do
  case "$(basename "$path")" in models.rs | models | tests) continue ;; esac
  ban "$path" 'process::(Command|Child)|Command::new' "only models.rs spawns processes: reach a model through the Models port"
done

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
  crates/scribe/src/commands/tests/note.rs \
  crates/scribe-core/src/cases/ctx.rs \
  crates/scribe-core/src/cases/resolve.rs \
  crates/scribe-core/src/cases/read_error.rs \
  crates/scribe-core/src/cases/write_error.rs \
  crates/scribe-core/src/cases/audit.rs \
  crates/scribe-core/src/cases/tag.rs \
  crates/scribe-core/src/cases/tag_cmd.rs \
  crates/scribe-core/src/cases/mark.rs \
  crates/scribe-core/src/cases/memory.rs \
  crates/scribe-core/src/cases/memory_block.rs \
  crates/scribe-core/src/cases/memory_cmd.rs \
  crates/scribe-core/src/cases/entry.rs \
  crates/scribe-core/src/cases/entry_cmd.rs \
  crates/scribe-core/src/cases/entry_verify.rs \
  crates/scribe-core/src/cases/pickup.rs \
  crates/scribe-core/src/cases/instruction_check.rs \
  crates/scribe-core/src/cases/proposal.rs \
  crates/scribe-core/src/cases/proposal_cmd.rs \
  crates/scribe-core/src/cases/session.rs \
  crates/scribe-core/src/cases/session_activity.rs \
  crates/scribe-core/src/cases/catalog.rs \
  crates/scribe-core/src/cases/rebuild.rs \
  crates/scribe-core/src/cases/pass.rs \
  crates/scribe-core/src/cases/pass/run.rs \
  crates/scribe-core/src/cases/transcript.rs; do
  [[ -e "$retired" ]] && err "retired path exists again: $retired"
done
# ORB-27707 step 4: scribe-core's business modules became scribe-domain
# and its store/ module scribe-store; scribe-core keeps cases/, tests/ and
# lib.rs. A module file under scribe-core with one of these names is a copy
# that should be in the crate below.
for module in action activity annotation ask brief capture checks claim config consistency \
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
