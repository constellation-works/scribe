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
#   2. Module edges inside a crate: grep bans over non-test sources for
#      what a crate edge cannot express (std I/O, the environment, one
#      module's mechanism inside a crate, the app order, the core's root
#      places). A `super::` path counts as the `crate::` path it names, so a
#      relative import meets the same rules (resolve, ban_crate).
#
# The crates, the app order and these guards are written down in
# ARCHITECTURE.md, from the architecture spec's Guards section
# (docs/design/scribe/specs/architecture.md); change them and this script in
# the same commit (STD-02 R6).
set -euo pipefail
cd "${1:-$(dirname "$0")/..}"

PREFIX="scribe"
fail=0
err() { echo "dependency-direction: $*" >&2; fail=1; }

# --- 1. crate policies -------------------------------------------------------
# policy <crate>  -> sets ALLOWED (internal deps) and BANNED (external deps)
policy() {
  case "$1" in
    scribe-store)
      # The only crate that touches the data root and scribe.db.
      ALLOWED=""
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored"
      ;;
    scribe-tools)
      # Functions of host files, bytes and text. It depends on the store for
      # the capture position a window resumes from and the record shapes.
      ALLOWED="scribe-store"
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite tempfile"
      ;;
    scribe-core)
      ALLOWED="scribe-store scribe-tools"
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
      err "$crate must not depend on $dep (banned for this crate)"
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

# resolve <file>: the file's text with each super:: path rewritten to the
# crate:: path it names from the file's module (apps/entry/mod.rs is
# crate::apps::entry, so its super:: is crate::apps). Line numbers are kept,
# so a relative import meets the same rules as the absolute one.
RESOLVE_SUPER='
  my @m = split m{/}, $ENV{RESOLVE_REL} =~ s/\.rs$//r;
  pop @m if @m && $m[-1] eq "mod";
  @m = () if @m == 1 && $m[0] eq "lib";
  s{(?<![\w:])((?:super::)+)}{
    my $n = () = $1 =~ /super::/g;
    my $keep = @m - $n; $keep = 0 if $keep < 0;
    my $base = join "::", @m[0 .. $keep - 1];
    $base eq "" ? "crate::" : "crate::" . $base . "::";
  }ge;
'
resolve() {
  [[ -f "$1" ]] || return 0
  if grep -q 'super::' "$1"; then
    RESOLVE_REL="${1#"$CORE"/}" perl -0777 -pe "$RESOLVE_SUPER" "$1"
  else
    cat "$1"
  fi
}

# ban_crate <path> <regex> <message>: ban, over the resolved text, the
# non-test Rust sources under <path> (a file or a directory).
ban_crate() {
  local hits="" file found
  while IFS= read -r -d '' file; do
    found=$(resolve "$file" | grep -nE "$2" | grep -vE '^[0-9]+:[[:space:]]*//' | sed "s|^|$file:|") || true
    if [[ -n "$found" ]]; then
      hits+="$found"$'\n'
    fi
  done < <(find "$1" -name '*.rs' -not -path '*/tests/*' -print0 2>/dev/null)
  if [[ -n "$hits" ]]; then
    printf '%s' "$hits" >&2
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

STORE=crates/scribe-store/src
CORE=crates/scribe-core/src
CLI=crates/scribe/src
# Reading the clock: the contracts and the tools take a time as an argument.
clock='(^|[^[:alnum:]_])(now_utc|now_local)\(|(OffsetDateTime|SystemTime|Instant|Utc|Local)::now([^[:alnum:]_]|$)'

# The crate edges keep the use cases out of scribe-store, and rusqlite out
# of every crate but the store. What they cannot keep out is std:
# scribe-core does no file, network or process I/O of its own (R4, R5),
# with two exceptions:
# composition.rs resolves the host (it may name std::fs), and providers/
# spawns the model processes (std::fs and std::process).
for path in "$CORE"/*; do
  case "$(basename "$path")" in tests) continue ;; providers)
    ban_std "$path" 'net' "${path#crates/}: the model adapters open no socket"
    continue ;;
  composition.rs)
    ban_std "$path" 'net|process' "${path#crates/}: composition resolves the host; providers/ spawns the model processes"
    continue ;;
  esac
  ban_std "$path" 'fs|net|process' "${path#crates/}: file and process access belong in scribe-store"
done

# scribe-core's apps (ORB-27921 step 2; ARCHITECTURE.md, the architecture
# spec §Apps): every directory under apps/ is in the order below, and an app
# imports only apps earlier in it, and only through what that app's mod.rs
# exports (`crate::apps::entry::Item`), never a flow file
# (`crate::apps::entry::flow::…`). A new app fails until it is given a place.
APP_ORDER='tag mark memory entry proposal session group search brief consistency hook admin'
for shared in common shared util; do
  [[ -e "$CORE/apps/$shared" ]] && err "apps/$shared: code two apps need goes to the lower one, to runtime or to the tools"
done
# The app order reads each app file as resolve gives it, so a relative import
# (`use super::session::X;` in apps/entry/mod.rs) is held to the order an
# absolute one is, and a grouped import through the crate root
# (`use crate::{apps::session::X};`), the apps module, or an app/submodule
# (`use crate::apps::entry::{...};`) is flattened to the paths it names.
APP_CHECK='
  use strict; use warnings;
  my $n = 0; my %rank = map { $_ => $n++ } split " ", $ENV{APP_ORDER};
  my ($file, $src) = @ENV{qw(APP_FILE APP_SRC)};
  local $/;
  my $s = <STDIN>;
  $s =~ s{^[ \t]*//.*$}{}mg;
  # A group body split at its top-level commas; each item is a path or a
  # path with a nested group, flattened to the full paths it names.
  sub flat {
    my ($prefix, $g) = @_;
    $g =~ s{//[^\n]*}{}g;
    my (@items, @out);
    my ($d, $cur) = (0, "");
    for my $c (split //, $g) {
      $d++ if $c eq "{";
      $d-- if $c eq "}";
      if ($c eq "," && $d == 0) { push @items, $cur; $cur = ""; next }
      $cur .= $c;
    }
    push @items, $cur;
    for my $it (@items) {
      $it =~ s/^\s+|\s+$//g;
      next if $it eq "";
      if ($it =~ /^(.*?)\{(.*)\}$/s) {
        (my $p = $1) =~ s/\s+//g;
        push @out, flat($prefix . $p, $2);
      } else {
        $it =~ s/\s+as\s+\w+$//;
        $it =~ s/\s+//g;
        push @out, $prefix . $it;
      }
    }
    return @out;
  }
  my @refs;
  while ($s =~ /crate::apps::(\w+)((?:::(?:\w+|\*))*)(::\s*\{)?/g) {
    push @refs, [$1, $2, $-[0]] unless defined $3;
  }
  # A grouped import names apps through the crate root (`crate::{apps::entry::X}`),
  # through the apps module (`crate::apps::{entry::X}`), or through an app or
  # submodule (`crate::apps::entry::{flow::X}`); nested groups are flattened, so
  # `crate::apps::entry::{X, flow::{Y}}` is read as every path it names.
  while ($s =~ /crate::((?:apps::(?:\w+::)*)?)\s*\{/g) {
    my ($at, $i, $prefix, $d) = ($-[0], pos($s), $1, 1);
    my $j = $i;
    while ($d && $j < length $s) { my $c = substr($s, $j++, 1); $d++ if $c eq "{"; $d-- if $c eq "}"; }
    for (flat($prefix, substr($s, $i, $j - $i - 1))) {
      push @refs, [$1, $2, $at] if /^apps::(\w+)((?:::(?:\w+|\*))*)/;
    }
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
'
app_hits=""
for dir in "$CORE"/apps/*/; do
  [[ -d "$dir" ]] || continue
  name=$(basename "$dir")
  if [[ " $APP_ORDER " != *" $name "* ]]; then
    app_hits+="$CORE/apps/$name: app $name has no place in the app order"$'\n'
  fi
done
while IFS= read -r -d '' file; do
  rel="${file#"$CORE/apps/"}"
  src="${rel%%/*}"
  [[ "$rel" == */* && " $APP_ORDER " == *" $src "* ]] || continue
  found=$(resolve "$file" | APP_FILE="$file" APP_SRC="$src" APP_ORDER="$APP_ORDER" perl -e "$APP_CHECK" 2>&1) || true
  if [[ -n "$found" ]]; then
    app_hits+="$found"$'\n'
  fi
done < <(find "$CORE/apps" -name '*.rs' -not -path '*/tests/*' -print0 2>/dev/null)
if [[ -n "$app_hits" ]]; then
  printf '%s' "$app_hits" >&2
  err "scribe-core's app order: an app imports only lower apps, through their mod.rs (ARCHITECTURE.md)"
fi
# The core's root modules are its places (ARCHITECTURE.md). lib.rs declares a
# file module (`mod x;`, any visibility, not test-only) only for one of them,
# so a new root module fails until it is given a place here and in the docs.
# The inline re-export modules lib.rs declares name no file and are not read.
ROOT_PLACES="apps composition config providers runtime"
undeclared=$(ROOT_PLACES="$ROOT_PLACES" perl -0777 -ne '
  my %place = map { $_ => 1 } split " ", $ENV{ROOT_PLACES};
  while (/^[ \t]*((?:#\[cfg\(test\)\][ \t]*\n[ \t]*)?)(?:pub(?:\([a-z]+\))?[ \t]+)?mod[ \t]+(\w+)[ \t]*;/mg) {
    next if $1 ne "" || $place{$2};
    my $line = 1 + (substr($_, 0, $-[0]) =~ tr/\n//);
    print "$ARGV:$line: lib.rs declares the root module $2, which is not one of the places\n";
  }' "$CORE/lib.rs" 2>/dev/null || true)
if [[ -n "$undeclared" ]]; then
  printf '%s\n' "$undeclared" >&2
  err "scribe-core's root modules: a new root module needs a place (ROOT_PLACES here, ARCHITECTURE.md)"
fi
# ORB-27921 step 2 moved every use case into apps/ and deleted cases/
# (listed with the retired paths below). Nothing names it again: not the
# core, and not the binary, which reaches the core through each app's
# exec, Command, Outcome and Error.
ban_crate "$CORE" 'crate::cases(::|[^[:alnum:]_]|$)' "cases/ is retired: a use case lives in its app under apps/"
ban "$CLI" 'scribe_core::cases(::|[^[:alnum:]_]|$)' "cases/ is retired: call the app's exec through scribe_core::apps::<app>"
# The model callers were five flows. Session pass, catalog and rebuild now
# run as Orbit jobs (ORB-28143) and name no model port. Only the remaining
# flows, memory refresh and group close, and the mod.rs of their apps
# (which carries the adapter in its Command and the model failure in its
# Error), name providers, the Models port or ModelError; so does
# runtime/errors.rs, whose CommandError carries the model failure as the
# binary's error did (ORB-27921 step 3, PR 4). The request and answer shapes
# under crate::models stay free to use.
models_port='crate::providers(::|[^[:alnum:]_]|$)|(^|[^[:alnum:]_])(Models|ModelError|ModelsOnce|ModelsEach)([^[:alnum:]_]|$)'
while IFS= read -r file; do
  case "${file#"$CORE"/}" in
    apps/memory/mod.rs | apps/memory/refresh.rs | apps/group/mod.rs | apps/group/close.rs | \
      runtime/errors.rs) continue ;;
  esac
  ban_crate "$file" "$models_port" \
    "${file#crates/} names the Models port: only memory refresh and group close call a model"
done < <(find "$CORE/apps" "$CORE/runtime" -name '*.rs' -not -path '*/tests/*' | sort)
# The providers know nothing of the apps or the runtime that call them.
ban_crate "$CORE/providers" 'crate::(apps|runtime)(::|[^[:alnum:]_]|$)' "providers must not import the apps or the runtime"

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
# No clock and no randomness: a tool that needs the time takes it as an
# argument.
ban "$TOOLS" "$clock" "scribe-tools must not read the clock: take the time as an argument"
ban "$TOOLS" '(^|[^[:alnum:]_])(rand|fastrand|getrandom|uuid)::' "scribe-tools must not draw randomness"

# The model shapes must not import their callers.
ban_crate "$CORE/providers/models.rs" 'crate::(memory|group)(::|[^[:alnum:]_]|$)' "the model shapes must not import their callers"
# A grouped crate import can name a caller without spelling crate::caller.
port_grouped=$(resolve "$CORE/providers/models.rs" | FILE="$CORE/providers/models.rs" perl -0777 -ne '
  while (/^[ \t]*(?:pub(?:\([a-z]+\))?[ \t]+)?use[ \t]+(?:::)?crate::\{([^;]*)\};/mg) {
    my ($stmt, $group, $at) = ($&, $1, $-[0]);
    1 while $group =~ s/\{[^{}]*\}//g;
    next unless $group =~ /(?<![A-Za-z0-9_:])(?:memory|group)(?![A-Za-z0-9_])/;
    my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
    $stmt =~ s/\s+/ /g;
    print "$ENV{FILE}:$line:$stmt\n";
  }' 2>/dev/null || true)
if [[ -n "$port_grouped" ]]; then
  echo "$port_grouped" >&2
  err "the model shapes must not import their callers"
fi

# Inside scribe-store: the codec converts bytes and the audit repository
# writes rows through db/; neither does file I/O of its own (R4, R5). The
# file layer is db/files.rs, the cw-fs adapter held below to naming nothing
# of the store but its error: neither shape names it, cw-fs or std::fs.
for shape in db/codec:"converts bytes" repository/audit:"writes its rows through db/"; do
  file="$STORE/${shape%%:*}.rs"
  [[ -d "$STORE" && ! -f "$file" ]] && err "${file#crates/} is missing: update this guard with the file"
  ban_std "$file" 'fs' "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
  ban "$file" '(^|[^[:alnum:]_])(super|db)::files([^[:alnum:]_]|$)' "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
  # Flatten grouped use trees before checking their paths. This catches nested
  # and multiline groups without banning unrelated variables named `files`.
  grouped=$(FILE="$file" perl -0777 -ne '
    sub paths {
      my ($prefix, $tree) = @_;
      my (@items, @out); my ($depth, $item) = (0, "");
      for my $c (split //, $tree) {
        $depth++ if $c eq "{"; $depth-- if $c eq "}";
        if ($c eq "," && !$depth) { push @items, $item; $item = ""; next }
        $item .= $c;
      }
      push @items, $item;
      for (@items) {
        s/^\s+|\s+$//g;
        if (/^(.*?)\{(.*)\}$/s) {
          my ($p, $body) = ($1, $2); $p =~ s/\s+//g;
          push @out, paths($prefix . $p, $body);
        } else {
          s/\s+as\s+\w+$//; s/\s+//g;
          push @out, $prefix . $_;
        }
      }
      return @out;
    }
    # Blank comments first (keeping newlines, so line numbers hold): a
    # semicolon inside one must not end a statement early.
    s{/\*.*?\*/}{ (my $c = $&) =~ s/[^\n]//g; $c }gse;
    s{//[^\n]*}{}g;
    while (/^[ \t]*(?:pub(?:[ \t]*\([^)]*\))?[ \t]+)?use\s+([^;]+);/mg) {
      my ($tree, $at) = ($1, $-[0]);
      next unless grep { /^(?:::)?(?:(?:crate|self|super)::)*(?:super::files|db::files)(?:$|::)/ } paths("", $tree);
      my $line = 1 + (substr($_, 0, $at) =~ tr/\n//);
      print "$ENV{FILE}:$line: grouped import reaches db/files.rs\n";
    }' "$file" 2>/dev/null || true)
  if [[ -n "$grouped" ]]; then
    echo "$grouped" >&2
    err "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
  fi
  ban "$file" '(^|[^[:alnum:]_])cw_fs([^[:alnum:]_]|$)' "store/${shape%%:*}.rs ${shape#*:}; it must not do I/O"
done
# The core and the store receive resolved roots and configuration (R3).
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
ban_crate "$CORE/composition.rs" 'crate::apps(::|[^[:alnum:]_]|$)' "composition.rs must not import the apps"
ban "$STORE"         'std::env::|home_dir|current_dir'           "the store crate must not read the environment or discover paths"
# ORB-27921 step 4: db/ holds the storage mechanism. SQLite is its
# mechanism (2_design.md §15.7): only db/ names rusqlite (testing.rs, the
# test-support shim, hands tests a connection), every other store module
# reaches scribe.db through it, and the crate policies keep rusqlite out of
# every other crate. The file layer is the kit's cw-fs (ORB-28612): the store
# names it, scribe-core's project scaffold (apps/admin/init.rs) names it, and
# no other crate does. db/files.rs, its adapter, names nothing else of the
# store but its error type.
while IFS= read -r -d '' path; do
  case "$path" in "$STORE"/db/* | "$STORE"/testing.rs) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])rusqlite([^[:alnum:]_]|$)' "${path#crates/}: only scribe-store's db/ uses rusqlite"
done < <(find "$STORE" -name '*.rs' -not -path '*/tests/*' -print0)
files_hits=$(grep -nE '(crate|super)::' "$STORE/db/files.rs" | grep -vE '^[0-9]+:[[:space:]]*//' | grep -v 'crate::error::' || true)
if [[ -n "$files_hits" ]]; then
  echo "$files_hits" >&2
  err "scribe-store/src/db/files.rs: the cw-fs adapter imports nothing of the store but its error"
fi
# cw-fs reaches the data root through the store's modules; in the core only
# the project scaffold names it, and the binary and the tools never do.
while IFS= read -r -d '' path; do
  case "$path" in "$CORE"/apps/admin/init.rs) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])cw_fs([^[:alnum:]_]|$)' "${path#crates/}: only the store and the project scaffold (apps/admin/init.rs) use cw-fs"
done < <(find "$CORE" "$CLI" "$TOOLS" -name '*.rs' -not -path '*/tests/*' -print0)
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
# A contract reads no clock: a time is a field, or an argument.
ban "$STORE/contracts" "$clock" "scribe-store/src/contracts: a contract reads no clock"
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
  crates/scribe-store/src/db/fsio.rs \
  crates/scribe-store/src/tests/fsio.rs \
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
  crates/scribe-domain; do
  [[ -e "$retired" ]] && err "retired path exists again: $retired"
done
# ORB-27707 step 4 and ORB-27921: scribe-core's business modules left for
# the store's contracts, scribe-tools and the apps, and its store/ module
# became scribe-store. A module file under scribe-core with one of these
# names is a copy that should be in one of those.
for module in action activity annotation ask brief capture checks claim consistency \
  control entry error group handoff instruction memory message models origin pass proposal \
  provider record redact search session tag text transcript trigger verbatim voice \
  store testing; do
  for retired in "crates/scribe-core/src/$module.rs" "crates/scribe-core/src/$module"; do
    [[ -e "$retired" ]] && err "retired path exists again: $retired (now in scribe-store, scribe-tools or an app)"
  done
done

if [[ "$fail" -eq 0 ]]; then
  echo "dependency-direction: ok"
fi
exit "$fail"
