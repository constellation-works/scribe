#!/usr/bin/env bash
# scripts/check-dependency-direction.sh [root] — STD-02 R1–R6, R16, R17, R39 for this workspace.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
#
# Three layers of checks, all from source files alone (no build, no network,
# no jq), so the gate runs in `make ci-fast`:
#
#   1. Crate edges. Every workspace crate needs a policy below. A crate may
#      depend only on the internal crates in its allowlist, and never on the
#      external crates in its banlist. Dev-dependencies are exempt. A crate
#      without a policy fails the check, so a new member forces a decision.
#   2. Module edges inside a crate: grep bans over non-test sources for
#      what a crate edge and clippy.toml cannot express (`std::net`, one
#      module's mechanism inside a crate, the app order, the core's root
#      places). `std::fs`, `std::env`, `std::process`, `mpsc::channel` and
#      `rusqlite::Connection::open*` are enforced by clippy.toml (STD-02 R6).
#      A `super::` path counts as the `crate::` path it names, so a relative
#      import meets the same rules (resolve, ban_crate).
#   3. Unit import cycles and declared in-layer orders (STD-02 R6, R39): the
#      import graph between workspace crates, between the top-level modules of
#      each crate, and between the modules of each layer/directory must be
#      acyclic, and every layer with more than about ten units obeys its
#      declared order in ARCHITECTURE.md.
#
# The crates, the declared layer orders and these guards are written down in
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
      # The only crate that touches the data root and scribe.db; uses rusqlite
      # only through cw-sqlite's re-export.
      ALLOWED=""
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite"
      ;;
    scribe-tools)
      # Functions of host files, bytes and text. It depends on the store for
      # the capture position a window resumes from and the record shapes.
      ALLOWED="scribe-store"
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite cw-sqlite tempfile"
      ;;
    scribe-core)
      ALLOWED="scribe-store scribe-tools"
      # The use cases reach files and scribe.db only through the store. Its
      # tests may stage database states with rusqlite: dev-dependencies are
      # exempt (declared_deps reads normal dependencies only). rustix and
      # tempfile are allowed for providers/, whose model adapters supervise
      # process groups and hand the codex child its schema in a private
      # temporary directory.
      BANNED="clap tracing-subscriber anstream anstyle termcolor terminal_size crossterm owo-colors colored rusqlite cw-sqlite"
      ;;
    scribe)
      # The CLI reaches scribe.db through scribe_core::Store; its tests may
      # stage database states with rusqlite as a dev-dependency. Composition,
      # the tools and the audit guard live in the core (ORB-27921 step 3).
      ALLOWED="scribe-core"
      BANNED="rusqlite cw-sqlite"
      ;;
    *) return 1 ;;
  esac
}

# The Cargo packages a manifest depends on through its non-dev dependency
# tables, as "<package> <line>" pairs, one per declaration. A declaration is its
# table ([dependencies], [build-dependencies] or a [target.<cfg>.dependencies]
# table) and its key: Cargo keeps the same key in two tables as two
# dependencies, so each gets its own pair with its own package and line
# (ORB-28908). A dependency's package is its table key unless
# `package = "..."` renames it, so the policy and the crate graph see the crate
# Cargo resolves, not the alias. Every declaration form is read:
#   [dependencies]  key = "1"  /  key = { package = "x" }  /  key.package = "x"
#   [dependencies.key]  package = "x"  (and a dotted `dependencies.key = ...`
#     line outside a dependency table)
#   key.workspace = true  /  key = { workspace = true }  (the package comes
#     from the root manifest's [workspace.dependencies])
# in [dependencies], [build-dependencies] and [target.<cfg>.*]; dev-dependencies
# are exempt. A TOML comment is not part of a declaration, so each line is read
# without its comment first: `# package = "x"` names no package. POSIX awk only
# (macOS ships BWK awk, not gawk). The first file is the root manifest, the
# second the member's.
DECLARED_DEPS_AWK='
# The line without its TOML comment and trailing blanks. A "#" outside a basic
# ("...", with backslash escapes) or literal (single-quoted) string starts the
# comment.
function uncomment(text,    i, c, q, out) {
  q = ""; out = ""
  for (i = 1; i <= length(text); i++) {
    c = substr(text, i, 1)
    if (q == "") {
      if (c == "#") break
      if (c == "\"" || c == "\047") q = c
    } else if (c == "\\" && q == "\"") {
      out = out c; i++; c = substr(text, i, 1)
    } else if (c == q) q = ""
    out = out c
  }
  sub(/[ \t]+$/, "", out)
  return out
}
function unquote(s) {
  gsub(/^[ \t]+|[ \t]+$/, "", s)
  if (s ~ /^".*"$/ || s ~ /^\047.*\047$/) s = substr(s, 2, length(s) - 2)
  return s
}
# The string value of `package = "x"` inside an inline table or a dotted line.
function package_of(text,    s) {
  if (!match(text, /(^|[{,. \t])(package|"package"|\047package\047)[ \t]*=[ \t]*("[^"]*"|\047[^\047]*\047)/)) return ""
  s = substr(text, RSTART, RLENGTH)
  sub(/^[^=]*=[ \t]*/, "", s)
  return unquote(s)
}
# A declaration is identified by its table and its key (id), never by the key
# alone: Cargo treats the same key in [dependencies], [build-dependencies] and
# each target table as separate dependencies (ORB-28908). The keys of the root
# [workspace.dependencies] table are unique, so wspkg stays keyed by name.
function note(key, where, pkg, ws,    id) {
  if (FILENAME == ARGV[1]) {
    if (pkg != "") wspkg[key] = pkg
    return
  }
  id = tbl SUBSEP key
  if (!(id in seen)) { order[++n] = id; seen[id] = where; keyof[id] = key }
  if (pkg != "") named[id] = pkg
  if (ws) viaws[id] = 1
}
# Sets mode: "" (not a dependency table), "table" ([dependencies]) or "sub"
# (a [dependencies.key] subtable, key in subkey). tbl names the table the
# declarations of this section belong to: the header without a subtable key.
# Normalize dotted header keys without touching dots inside quoted keys.
function header_keys(h,    i, c, q, part, out) {
  q = ""; part = ""; out = ""
  for (i = 1; i <= length(h); i++) {
    c = substr(h, i, 1)
    if (q != "" && c == "\\" && q == "\"") {
      part = part c; i++; part = part substr(h, i, 1); continue
    }
    if (q == "" && (c == "\"" || c == "\047")) q = c
    else if (c == q) q = ""
    if (q == "" && c == ".") {
      out = out unquote(part) "."; part = ""
    } else part = part c
  }
  return out unquote(part)
}
function classify(h,    root) {
  mode = ""; subkey = ""; tbl = ""
  root = (FILENAME == ARGV[1])
  sub(/^\[[ \t]*/, "", h); sub(/[ \t]*\]$/, "", h); h = header_keys(h)
  if (root) {
    if (h == "workspace") mode = "workspace"
    else if (h == "workspace.dependencies") mode = "table"
    else if (h ~ /^workspace\.dependencies\./) {
      mode = "sub"; subkey = unquote(substr(h, length("workspace.dependencies.") + 1))
    }
  } else if (h ~ /^(target\..*\.)?(build-)?dependencies$/) { mode = "table"; tbl = h }
  else if (match(h, /^(target\..*\.)?(build-)?dependencies\./)) {
    mode = "sub"; tbl = substr(h, 1, RLENGTH - 1)
    subkey = unquote(substr(h, RLENGTH + 1))
  }
  if (mode == "sub") note(subkey, FNR, "", 0)
}
{ $0 = uncomment($0) }
FNR == 1 { mode = ""; subkey = ""; tbl = "" }
/^[ \t]*\[/ { h = $0; sub(/[ \t]*#.*$/, "", h); sub(/^[ \t]+/, "", h); classify(h); next }
/^[ \t]*(#|$)/ { next }
mode == "sub" {
  if ($0 ~ /^[ \t]*(package|"package"|\047package\047)[ \t]*=/) note(subkey, FNR, package_of($0), 0)
  else if ($0 ~ /^[ \t]*(workspace|"workspace"|\047workspace\047)[ \t]*=[ \t]*true/) note(subkey, FNR, "", 1)
  next
}
mode == "table" || mode == "workspace" || (mode == "" && FILENAME != ARGV[1] && /^[ \t]*((build-)?dependencies|"(build-)?dependencies"|\047(build-)?dependencies\047)[ \t]*\./) {
  line = $0; sub(/^[ \t]+/, "", line)
  if (mode == "workspace") {
    if (line !~ /^(dependencies|"dependencies"|\047dependencies\047)[ \t]*\./) next
    sub(/^(dependencies|"dependencies"|\047dependencies\047)[ \t]*\.[ \t]*/, "", line)
  }
  if (mode == "") {
    match(line, /^((build-)?dependencies|"(build-)?dependencies"|\047(build-)?dependencies\047)/)
    tbl = unquote(substr(line, 1, RLENGTH))
    sub(/^((build-)?dependencies|"(build-)?dependencies"|\047(build-)?dependencies\047)[ \t]*\.[ \t]*/, "", line)
  }
  if (!match(line, /^("[^"]*"|\047[^\047]*\047|[A-Za-z0-9_-]+)/)) next
  key = unquote(substr(line, 1, RLENGTH)); rest = substr(line, RLENGTH + 1)
  sub(/^[ \t]+/, "", rest)
  if (rest ~ /^\.[ \t]*(package|"package"|\047package\047)[ \t]*=/) note(key, FNR, package_of(rest), 0)
  else if (rest ~ /^\.[ \t]*(workspace|"workspace"|\047workspace\047)[ \t]*=[ \t]*true/) note(key, FNR, "", 1)
  else if (rest ~ /^\.[ \t]*[A-Za-z0-9_-]+[ \t]*=/) note(key, FNR, "", 0)
  else if (rest ~ /^=/) note(key, FNR, package_of(rest), rest ~ /[{,][ \t]*(workspace|"workspace"|\047workspace\047)[ \t]*=[ \t]*true/)
  next
}
END {
  for (i = 1; i <= n; i++) {
    id = order[i]; key = keyof[id]
    pkg = (id in named) ? named[id] : ((id in viaws && key in wspkg) ? wspkg[key] : key)
    print pkg, seen[id]
  }
}'

# Print "<package> <line>" for each non-dev declaration of a Cargo.toml (a key
# reused in another dependency table is a declaration of its own).
declared_deps() {
  local root=Cargo.toml
  [[ -f "$root" ]] || root=/dev/null
  awk "$DECLARED_DEPS_AWK" "$root" "$1"
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
  while read -r dep _; do
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
# of every crate but the store. std::fs, std::env and std::process are
# enforced by clippy.toml; only std::net stays as a source grep (STD-02 R6).
ban_std "$CORE" 'net' "scribe-core opens no socket"

# scribe-core's apps (ORB-27921 step 2; ARCHITECTURE.md, the architecture
# spec §Apps): every directory under apps/ is in the order below, and an app
# imports only apps earlier in it, and only through what that app's mod.rs
# exports (`crate::apps::entry::Item`), never a flow file
# (`crate::apps::entry::flow::…`). A new app fails until it is given a place.
APP_ORDER='tag mark memory entry proposal session transcript group search brief consistency hook init store'
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
# The python block (3d) reads lib.rs with the same cfg reader as the import
# checks. The inline re-export modules lib.rs declares name no file and are not read.
ROOT_PLACES="apps composition config providers runtime"
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

# scribe-tools: std::fs, std::env and std::process are enforced by clippy.toml;
# nothing opens a socket, names the core or writes the store.
TOOLS=crates/scribe-tools/src
ban_std "$TOOLS" 'net' "scribe-tools opens no socket"
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
# of the store but its error: neither shape names it, cw-fs or cw-sqlite
# (std::fs is banned workspace-wide by clippy.toml).
for shape in db/codec:"converts bytes" repository/audit:"writes its rows through db/"; do
  file="$STORE/${shape%%:*}.rs"
  [[ -d "$STORE" && ! -f "$file" ]] && err "${file#crates/} is missing: update this guard with the file"
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
    # Lex left to right: comment delimiters inside literals or other comments
    # are inert. Blank each token with spaces while retaining its newlines and
    # token boundaries, so statements and their reported lines stay intact.
    my $source = $_;
    my $clean = "";
    pos($source) = 0;
    while (pos($source) < length($source)) {
      my $start = pos($source);
      if ($source =~ /\G\/\//gc) {
        $source =~ /\G[^\n]*/gc;
      } elsif ($source =~ /\G\/\*/gc) {
        my $depth = 1;
        while ($depth && $source =~ m{\G(?:(?!/\*|\*/).)*(/\*|\*/)}gcs) {
          $depth += $1 eq "/*" ? 1 : -1;
        }
        pos($source) = length($source) if $depth;
      } elsif ($source =~ /\G(?:br|cr|r)(\#*)"/gc) {
        my $end = index($source, "\"" . $1, pos($source));
        pos($source) = $end < 0 ? length($source) : $end + 1 + length($1);
      } elsif ($source =~ /\G"(?:\\.|[^"\\])*"/gcs ||
               $source =~ /\G\x27(?:\\.|[^\x27\\\n])\x27/gc) {
        # Ordinary strings and character literals, including escapes.
      } else {
        $source =~ /\G(.)/gcs;
        $clean .= $1;
        next;
      }
      my $token = substr($source, $start, pos($source) - $start);
      $token =~ s/[^\n]/ /g;
      $clean .= $token;
    }
    $_ = $clean;
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
  ban "$file" '(^|[^[:alnum:]_])cw_sqlite([^[:alnum:]_]|$)' "store/${shape%%:*}.rs ${shape#*:}; it must not open SQLite"
done
# rustix and tempfile serve the model processes: only providers/ names them.
for path in "$CORE"/*; do
  case "$(basename "$path")" in tests | providers) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])(rustix|tempfile)::' "${path#crates/}: only providers/ uses rustix and tempfile"
done
# Composition builds the context the apps run against; it imports no app.
ban_crate "$CORE/composition.rs" 'crate::apps(::|[^[:alnum:]_]|$)' "composition.rs must not import the apps"
# ORB-27921 step 4 and ORB-28614: db/ holds the storage mechanism. SQLite is
# opened, configured and migrated through the kit's cw-sqlite (2_design.md
# §15.7): only db/ names cw_sqlite or rusqlite (through cw_sqlite::rusqlite),
# every other store module reaches scribe.db through it, and no scribe source
# sets journal_mode, synchronous, busy_timeout, user_version, secure_delete,
# PersistWal or wal_checkpoint itself. The file layer is the kit's cw-fs
# (ORB-28612): the store names it, scribe-core's project scaffold
# (apps/init/install.rs) names it, and no other crate does. db/files.rs, its
# adapter, names nothing else of the store but its error type.
while IFS= read -r -d '' path; do
  case "$path" in "$STORE"/db/*) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])(rusqlite|cw_sqlite)([^[:alnum:]_]|$)' "${path#crates/}: only scribe-store's db/ uses cw-sqlite and rusqlite"
done < <(find "$STORE" -name '*.rs' -not -path '*/tests/*' -print0)
ban "$STORE" 'PRAGMA[[:space:]]+(journal_mode|synchronous|busy_timeout|user_version|secure_delete|wal_checkpoint)|\.busy_timeout\(|PersistWal' \
  "scribe-store must open, configure, checkpoint and migrate scribe.db only through cw-sqlite"
files_hits=$(grep -nE '(crate|super)::' "$STORE/db/files.rs" | grep -vE '^[0-9]+:[[:space:]]*//' | grep -v 'crate::error::' || true)
if [[ -n "$files_hits" ]]; then
  echo "$files_hits" >&2
  err "scribe-store/src/db/files.rs: the cw-fs adapter imports nothing of the store but its error"
fi
# cw-fs reaches the data root through the store's modules; in the core only
# the project scaffold names it, and the binary and the tools never do.
while IFS= read -r -d '' path; do
  case "$path" in "$CORE"/apps/init/install.rs) continue ;; esac
  ban "$path" '(^|[^[:alnum:]_])cw_fs([^[:alnum:]_]|$)' "${path#crates/}: only the store and the project scaffold (apps/init/install.rs) use cw-fs"
done < <(find "$CORE" "$CLI" "$TOOLS" -name '*.rs' -not -path '*/tests/*' -print0)
ban "$CORE" '(^|[^[:alnum:]_])cw_sqlite([^[:alnum:]_]|$)' "only scribe-store's db/ uses cw-sqlite"
ban "$CLI"  '(^|[^[:alnum:]_])cw_sqlite([^[:alnum:]_]|$)' "only scribe-store's db/ uses cw-sqlite"
ban "$TOOLS" '(^|[^[:alnum:]_])cw_sqlite([^[:alnum:]_]|$)' "only scribe-store's db/ uses cw-sqlite"
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
# Rendering is derived from payloads only; it never reaches back up (R1).
ban "$CLI/output"    'crate::(app|cli|commands)'    "output must not import app, cli or commands"
# The binary reaches the core through its public surface only: the
# composition context, the apps and the re-exported types. It names no
# lower crate and no runtime path; the audit guard runs inside each app's
# exec (ORB-27921 step 3).
ban "$CLI" '(^|[^[:alnum:]_])scribe_(domain|store|tools)::' "the binary must not name scribe_domain, scribe_store or scribe_tools: it reaches them through scribe_core"
ban "$CLI" 'scribe_core::runtime(::|[^[:alnum:]_]|$)' "the binary must not name scribe_core::runtime: the runtime is internal to the core"

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
  crates/scribe/src/output/payload.rs \
  crates/scribe/src/output/payload \
  crates/scribe/src/output/render.rs \
  crates/scribe/src/output/render \
  crates/scribe/src/output/color.rs \
  crates/scribe/src/output/printable.rs \
  crates/scribe/src/output/usage.rs \
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

# --- 3. unit import cycles and declared in-layer orders (STD-02 R6, R39) -----
# Every level of units must have an acyclic import graph: workspace crates,
# top-level modules of each crate, and sibling modules within each layer or
# module directory. Layers with more than about ten units declare a bottom-up
# order in ARCHITECTURE.md and below (apps/ is already ordered by APP_ORDER
# above); every unit in a declared layer must appear in its order and import
# only strictly earlier units.
DB_ORDER='archive files log scope codec locks memory messages groups sessions docs import write documents'
CONTRACTS_ORDER='audit provider record session entry handoff tag action group mark memory message proposal'
REPOSITORY_ORDER='sealed ids memory audit entries groups marks messages pickups proposals sessions tags'
TOOLS_ORDER='fs host origin text voice markers project payload transcript window group_close memory_refresh session_distill'
COMMANDS_ORDER='instruction store brief consistency entry export group hook init mark memory proposal search session tag transcript plugin'
OUTPUT_ORDER='common entry export group hook init mark memory proposal search session store tag transcript brief consistency plugin'

if ! command -v python3 >/dev/null 2>&1; then
  err "python3 is required for the import-cycle and in-layer order check"
elif ! DECLARED_DEPS_AWK="$DECLARED_DEPS_AWK" \
  DB_ORDER="$DB_ORDER" \
  CONTRACTS_ORDER="$CONTRACTS_ORDER" \
  REPOSITORY_ORDER="$REPOSITORY_ORDER" \
  TOOLS_ORDER="$TOOLS_ORDER" \
  COMMANDS_ORDER="$COMMANDS_ORDER" \
  OUTPUT_ORDER="$OUTPUT_ORDER" \
  ROOT_PLACES="$ROOT_PLACES" \
  python3 -I - <<'PY'
import collections
import glob
import os
import re
import subprocess
import sys

errors = []

def err(msg):
    errors.append(msg)

# --- 3a. Workspace crate dependency graph (non-dev dependencies) ------------
crate_edges = collections.defaultdict(set)
crate_manifests = {}
for manifest in sorted(glob.glob("crates/*/Cargo.toml")):
    name = None
    with open(manifest, encoding="utf-8") as fh:
        for line in fh:
            m = re.match(r'^name\s*=\s*"([^"]+)"', line.strip())
            if m:
                name = m.group(1)
                break
    root = "Cargo.toml" if os.path.isfile("Cargo.toml") else os.devnull
    # The same parser as the policy check: it resolves `package = ...`
    # renames, dependency subtables and workspace-inherited dependencies.
    out = subprocess.run(["awk", os.environ["DECLARED_DEPS_AWK"], root, manifest],
                         check=True, capture_output=True, text=True).stdout
    deps = [(pkg, int(lineno)) for pkg, lineno in (row.split() for row in out.splitlines())]
    if name:
        crate_manifests[name] = (manifest, deps)

workspace_crates = set(crate_manifests)
crate_edge_loc = {}
for crate, (manifest, deps) in sorted(crate_manifests.items()):
    for dep, lineno in deps:
        if dep in workspace_crates:
            crate_edges[crate].add(dep)
            crate_edge_loc.setdefault((crate, dep), f"{manifest}:{lineno}")

def find_cycles(nodes, edges):
    """Return deterministic elementary cycle paths for every SCC with a cycle."""
    index = {}
    lowlink = {}
    stack = []
    on_stack = set()
    sccs = []
    counter = [0]

    def strongconnect(v):
        index[v] = lowlink[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on_stack.add(v)
        for w in sorted(edges.get(v, ())):
            if w not in index:
                strongconnect(w)
                lowlink[v] = min(lowlink[v], lowlink[w])
            elif w in on_stack:
                lowlink[v] = min(lowlink[v], index[w])
        if lowlink[v] == index[v]:
            scc = []
            while True:
                w = stack.pop()
                on_stack.remove(w)
                scc.append(w)
                if w == v:
                    break
            if len(scc) > 1 or (len(scc) == 1 and v in edges.get(v, ())):
                sccs.append(sorted(scc))

    for v in sorted(nodes):
        if v not in index:
            strongconnect(v)

    cycles = []
    for scc in sorted(sccs):
        scc_set = set(scc)
        start = scc[0]
        # BFS from start's neighbors in the SCC to find a shortest cycle back to start.
        queue = collections.deque()
        parent = {}
        found = None
        for nxt in sorted(edges.get(start, ())):
            if nxt == start:
                found = [start, start]
                break
            if nxt in scc_set and nxt not in parent:
                parent[nxt] = start
                queue.append(nxt)
        while found is None and queue:
            cur = queue.popleft()
            for nxt in sorted(edges.get(cur, ())):
                if nxt == start:
                    path = [start]
                    node = cur
                    rev = []
                    while node != start:
                        rev.append(node)
                        node = parent[node]
                    path.extend(reversed(rev))
                    path.append(start)
                    found = path
                    break
                if nxt in scc_set and nxt not in parent:
                    parent[nxt] = cur
                    queue.append(nxt)
        if found:
            cycles.append(found)
    return cycles

for cyc in find_cycles(workspace_crates, crate_edges):
    locs = ", ".join(
        crate_edge_loc.get((cyc[i], cyc[i + 1]), "")
        for i in range(len(cyc) - 1)
    )
    err(f"crate import cycle: {' -> '.join(cyc)} ({locs})")

# --- 3b. Rust lexer and import resolver -------------------------------------
RAW_STR_RE = re.compile(r'(?:br|cr|r)(#*)"')
CHAR_LIT_RE = re.compile(r"b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\n])'")
USE_STMT_RE = re.compile(
    r'(?:^|(?<=[;{}]))[ \t\n]*((?:#\[[^\]]*\][ \t\n]*)*)'
    r'(?:pub(?:\s*\([^)]*\))?\s+)?use\s+([^;]+);',
    re.M,
)
INLINE_PATH_RE = re.compile(r'(?<![\w:])((?:crate|super|self)(?:::(?:r#)?\w+)+)')
# A test-only cfg is read from the attribute's tokens, never from its text:
# comments drop out and string literals stay whole, so cfg-like text in a doc
# string or a comment is no cfg, a comment inside a cfg is ignored, and
# any(...) takes its two operands in either order (ORB-28901).
TEST_CFG_FORMS = {
    'cfg ( test )',
    'cfg ( any ( test , feature = "test-support" ) )',
    'cfg ( any ( feature = "test-support" , test ) )',
}
ATTR_STR_RE = re.compile(r'"(?:\\.|[^"\\])*"', re.S)
ATTR_WORD_RE = re.compile(r'\w+')

def attr_tokens(text):
    """Tokens of an attribute run: comments dropped, each string literal whole."""
    toks = []
    i, n = 0, len(text)
    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            i = n if end < 0 else end
        elif text.startswith("/*", i):
            depth = 0
            while i < n:
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                    if depth == 0:
                        break
                else:
                    i += 1
        elif text[i].isspace():
            i += 1
        elif text[i] == '"' and (m := ATTR_STR_RE.match(text, i)):
            toks.append(m.group(0))
            i = m.end()
        elif (m := ATTR_WORD_RE.match(text, i)):
            toks.append(m.group(0))
            i = m.end()
        else:
            toks.append(text[i])
            i += 1
    return toks

def has_test_cfg(attrs):
    """True when an attribute run holds a test-only cfg (TEST_CFG_FORMS)."""
    toks = attr_tokens(attrs)
    i = 0
    while i + 1 < len(toks):
        if toks[i] != "#" or toks[i + 1] != "[":
            i += 1
            continue
        depth, j, body = 0, i + 1, []
        while j < len(toks):
            if toks[j] == "[":
                depth += 1
            elif toks[j] == "]":
                depth -= 1
                if depth == 0:
                    break
            if j > i + 1:
                body.append(toks[j])
            j += 1
        if " ".join(body) in TEST_CFG_FORMS:
            return True
        i = j + 1
    return False

def strip_comments_and_strings(src):
    """Replace Rust comments and string/char literals with spaces, preserving newlines."""
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            while i < n and src[i] != "\n":
                out.append(" ")
                i += 1
        elif c == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 1
            out.append("  ")
            i += 2
            while i < n and depth > 0:
                if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
                    depth += 1
                    out.append("  ")
                    i += 2
                elif src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    depth -= 1
                    out.append("  ")
                    i += 2
                else:
                    out.append("\n" if src[i] == "\n" else " ")
                    i += 1
        elif c in ("r", "b", "c") and (m := RAW_STR_RE.match(src, i)):
            end_pat = '"' + m.group(1)
            end_pos = src.find(end_pat, m.end())
            end_pos = n if end_pos < 0 else end_pos + len(end_pat)
            out.extend("\n" if ch == "\n" else " " for ch in src[i:end_pos])
            i = end_pos
        elif c == '"' or (c in ("b", "c") and i + 1 < n and src[i + 1] == '"'):
            if c != '"':
                out.append(" ")
                i += 1
            out.append(" ")
            i += 1
            while i < n:
                if src[i] == "\\" and i + 1 < n:
                    # Two source characters stay two, so offsets match the raw text.
                    out.append(" \n" if src[i + 1] == "\n" else "  ")
                    i += 2
                elif src[i] == '"':
                    out.append(" ")
                    i += 1
                    break
                else:
                    out.append("\n" if src[i] == "\n" else " ")
                    i += 1
        elif c == "'" or (c == "b" and i + 1 < n and src[i + 1] == "'"):
            m = CHAR_LIT_RE.match(src, i)
            if m:
                out.append(" " * (m.end() - i))
                i = m.end()
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out)

def flatten_use_tree(prefix, tree):
    """Flatten a use tree into (full path, optional binding) pairs."""
    items = []
    depth = 0
    cur = []
    for ch in tree:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            items.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    items.append("".join(cur))
    out = []
    for item in items:
        item = item.strip()
        if not item:
            continue
        m = re.match(r"^(.*?)\{(.*)\}$", item, re.S)
        if m:
            p = re.sub(r"\s+", "", m.group(1))
            out.extend(flatten_use_tree(prefix + p, m.group(2)))
        else:
            alias_match = re.search(r"\s+as\s+(\w+)$", item)
            alias = alias_match.group(1) if alias_match else None
            if alias_match:
                item = item[:alias_match.start()]
            item = re.sub(r"\s+", "", item)
            if item == "self":
                out.append((prefix.rstrip(":"), alias))
            else:
                out.append((prefix + item, alias))
    return out

def canonical_ident(name):
    """A raw identifier (r#store) names the same module as its plain spelling."""
    return name[2:] if name.startswith("r#") else name

def path_segments(path_str):
    return [canonical_ident(s) for s in path_str.lstrip(":").split("::") if s]

def resolve_path(path_str, mod_parts):
    """Resolve a crate-relative path to module segments, or None."""
    segs = path_segments(path_str)
    if not segs:
        return None
    if segs[0] == "crate":
        return segs[1:]
    if segs[0] == "super":
        up = 0
        while up < len(segs) and segs[up] == "super":
            up += 1
        if up > len(mod_parts):
            return None
        return mod_parts[: len(mod_parts) - up] + segs[up:]
    if segs[0] == "self":
        return mod_parts + segs[1:]
    return None

def expand_alias(path, aliases):
    if path and path[0] in aliases:
        return aliases[path[0]] + path[1:]
    return path

def resolve_to_crate(path_str, mod_parts, aliases, facades=None):
    """Return every module path a crate-relative path can name (usually one).

    A root alias expands the first segment. An inline root facade (a `pub mod`
    holding only `pub use`) is followed to where its re-export is defined
    (RootFacades, ORB-28903).
    """
    res = resolve_path(path_str, mod_parts)
    if res is None:
        return []
    if res and res[0] in aliases:
        return [aliases[res[0]] + res[1:]]
    if res and facades is not None and res[0] in facades.table:
        return facades.expand(res)
    return [res]

def collect_root_aliases(src_dir, top_units):
    """Collect unambiguous root-level re-exports in lib.rs / main.rs."""
    candidates = collections.defaultdict(set)
    for root_file in ("lib.rs", "main.rs"):
        fp = os.path.join(src_dir, root_file)
        if not os.path.isfile(fp):
            continue
        raw = open(fp, encoding="utf-8").read()
        clean = strip_comments_and_strings(raw)
        # Ignore #[cfg(test)] mod ... { ... } blocks and #[cfg(test)] use statements.
        for m in USE_STMT_RE.finditer(clean):
            if has_test_cfg(raw[m.start(1):m.end(1)]):
                continue
            tree = m.group(2)
            # Only top-level use statements (not indented inside an inline mod { ... }).
            stmt_start = clean.rfind("\n", 0, m.start(2)) + 1
            if clean[stmt_start : m.start(2)].startswith((" ", "\t")):
                continue
            for raw, binding in flatten_use_tree("", tree):
                segs = path_segments(raw)
                if segs and segs[0] == "crate":
                    segs = segs[1:]
                if len(segs) >= 2 and segs[0] in top_units and segs[-1] != "*":
                    alias = binding or segs[-1]
                    if alias not in top_units:
                        candidates[alias].add(tuple(segs))
    # Keep only aliases that map unambiguously to a single internal path.
    return {k: list(next(iter(v))) for k, v in candidates.items() if len(v) == 1}

def has_non_test_rs(directory):
    for r, dirs, files in os.walk(directory):
        dirs[:] = [d for d in dirs if d != "tests"]
        if any(f.endswith(".rs") for f in files):
            return True
    return False

def list_units(directory, is_crate_root):
    units = set()
    if not os.path.isdir(directory):
        return units
    skip_files = {"lib.rs", "main.rs"} if is_crate_root else {"mod.rs"}
    skip_dirs = {"tests", "bin"} if is_crate_root else {"tests"}
    for entry in os.listdir(directory):
        if entry.startswith("."):
            continue
        full = os.path.join(directory, entry)
        if os.path.isdir(full):
            if entry not in skip_dirs and has_non_test_rs(full):
                units.add(entry)
        elif entry.endswith(".rs") and entry not in skip_files:
            units.add(entry[:-3])
    return units

def file_mod_parts(src_dir, fp):
    rel = os.path.relpath(fp, src_dir)[:-3].split(os.sep)
    if rel[-1] == "mod":
        return rel[:-1]
    if len(rel) == 1 and rel[0] in ("lib", "main"):
        return []
    return rel

INLINE_MOD_RE = re.compile(
    r'(?:^|(?<=[;{}]))[ \t\n]*((?:#\[[^\]]*\][ \t\n]*)*)'
    r'(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+((?:r#)?\w+)\s*\{', re.M
)

def inline_modules(clean, raw, base):
    """Return inline module bodies and paths, excluding test-only modules."""
    declarations = {m.end() - 1: m for m in INLINE_MOD_RE.finditer(clean)}
    stack = []
    modules = []
    for pos, ch in enumerate(clean):
        if ch == "{":
            m = declarations.get(pos)
            # A module declaration is a unit only at module scope, not inside a function.
            if m and all(item is not None for item in stack):
                parent = stack[-1][0] if stack else tuple(base)
                ignored = (stack and stack[-1][1]) or has_test_cfg(raw[m.start(1):m.end(1)])
                path = parent + (canonical_ident(m.group(2)),)
                stack.append((path, ignored, pos))
            else:
                stack.append(None)
        elif ch == "}" and stack:
            item = stack.pop()
            if item is not None:
                modules.append((item[2], pos, item[0], item[1]))
    return modules

def is_facade_body(body):
    """True when a module body holds only `pub use` statements (a compatibility facade)."""
    return (bool(USE_STMT_RE.search(body)) and
            not USE_STMT_RE.sub("", body).strip() and
            all(re.search(r"\bpub\s+use\b", m.group(0))
                for m in USE_STMT_RE.finditer(body)))

ITEM_DEF_RE = re.compile(r"\b(?:fn|struct|enum|trait|type|const|static|union|mod|macro_rules!)\s+(?:r#)?(\w+)")

class RootFacades:
    """Inline root facades and the modules their re-exports are defined in.

    A facade is no unit of its own, so a path through it must reach what it
    re-exports or the dependency edge vanishes (ORB-28903). A re-export is
    followed through further re-exports to the module that defines the item;
    a name another crate defines carries no edge, and a name that cannot be
    placed stays on the module the facade names.
    """

    DEPTH = 8

    def __init__(self, src_dir, aliases):
        self.src_dir = src_dir
        self.aliases = aliases
        self.table = {}
        self.sources = {}
        self.collect()

    def collect(self):
        for root_file in ("lib.rs", "main.rs"):
            fp = os.path.join(self.src_dir, root_file)
            if not os.path.isfile(fp):
                continue
            raw = open(fp, encoding="utf-8").read()
            clean = strip_comments_and_strings(raw)
            for start, end, path, ignored in inline_modules(clean, raw, []):
                body = clean[start + 1:end]
                if ignored or len(path) != 1 or not is_facade_body(body):
                    continue
                self.table[path[0]] = self.reexports(body, list(path))

    def reexports(self, body, here):
        """The (named, globs) re-exports of a use-only body, each as (binding, target)."""
        named, globs = {}, []
        for m in USE_STMT_RE.finditer(body):
            if has_test_cfg(body[m.start(1):m.end(1)]):
                continue
            for target, binding in flatten_use_tree("", m.group(2)):
                segs = path_segments(target)
                res = resolve_path(target, here)
                if segs and segs[-1] == "*":
                    globs.append(res[:-1] if res else None)
                elif segs:
                    named[binding or segs[-1]] = res
        return named, globs

    def module_source(self, parts):
        """The cleaned source and raw text of a module's own file, or None."""
        key = tuple(parts)
        if key not in self.sources:
            base = os.path.join(self.src_dir, *parts)
            found = None
            for fp in (base + ".rs", os.path.join(base, "mod.rs")):
                if parts and os.path.isfile(fp):
                    raw = open(fp, encoding="utf-8").read()
                    found = (strip_comments_and_strings(raw), raw)
                    break
            self.sources[key] = found
        return self.sources[key]

    def is_module(self, parts):
        base = os.path.join(self.src_dir, *parts)
        return bool(parts) and (os.path.isfile(base + ".rs") or os.path.isdir(base))

    def expand(self, res, depth=0):
        """Module paths for `res`, whose first segment is a facade."""
        if depth > self.DEPTH:
            return []
        named, globs = self.table[res[0]]
        name, tail = res[1:2], res[2:]
        if not name:
            return []
        if name[0] in named:
            target = named[name[0]]
            return [] if target is None else self.locate(self.alias(target) + tail, depth)[1]
        found, external = [], False
        for glob in globs:
            if glob is None:
                external = True
                continue
            ok, paths = self.locate(self.alias(glob) + name + tail, depth)
            if ok:
                found.extend(paths)
        if found:
            return found
        # Nothing places the name: a lone internal glob is the best answer.
        if len(globs) == 1 and not external and globs[0] is not None:
            return [self.alias(globs[0]) + name + tail]
        return []

    def alias(self, path):
        return expand_alias(path, self.aliases)

    def locate(self, path, depth):
        """(placed, paths): where the item `path` names is defined.

        `paths` is empty when another crate defines it; `placed` is False when
        no source here says, and `paths` is then `path` itself.
        """
        for k in range(len(path) - 1, 0, -1):
            if self.is_module(path[:k]):
                module, name, tail = path[:k], path[k], path[k + 1:]
                break
        else:
            return False, [path]
        src = self.module_source(module)
        if depth > self.DEPTH or src is None:
            return False, [path]
        clean, raw = src
        if self.defines(module, name):
            return True, [path]
        globs = []
        for m in USE_STMT_RE.finditer(clean):
            if has_test_cfg(raw[m.start(1):m.end(1)]):
                continue
            for target, binding in flatten_use_tree("", m.group(2)):
                segs = path_segments(target)
                if segs and segs[-1] == "*":
                    globs.append(target)
                elif segs and (binding or segs[-1]) == name:
                    return True, self.follow(target, module, tail, depth)
        found = []
        for target in globs:
            res = resolve_path(target, module)
            if res is None:
                continue
            ok, paths = self.locate(self.alias(res[:-1]) + [name] + tail, depth + 1)
            if ok:
                found.extend(paths)
        return (True, found) if found else (False, [path])

    def defines(self, module, name):
        src = self.module_source(module)
        return src is not None and (name in ITEM_DEF_RE.findall(src[0]) or self.is_module(module + [name]))

    def follow(self, target, module, tail, depth):
        res = resolve_path(target, module)
        if res is None:
            return []
        if res and res[0] in self.aliases:
            return self.locate(self.aliases[res[0]] + res[1:] + tail, depth + 1)[1]
        if res and res[0] in self.table:
            return self.expand(res + tail, depth + 1)
        return self.locate(res + tail, depth + 1)[1]

def extract_resolved_refs(fp, src_dir, aliases, facades=None):
    raw = open(fp, encoding="utf-8").read()
    clean = strip_comments_and_strings(raw)
    mod_parts = file_mod_parts(src_dir, fp)
    modules = inline_modules(clean, raw, mod_parts)
    def module_at(pos):
        containing = [(start, path) for start, end, path, ignored in modules
                      if not ignored and start < pos < end]
        return list(max(containing, default=(-1, tuple(mod_parts)))[1])
    refs = []
    # Strip spans of #[cfg(test)] use statements so inline test-only imports are skipped.
    skip_spans = [(start, end) for start, end, _path, ignored in modules if ignored]
    for m in USE_STMT_RE.finditer(clean):
        attrs = raw[m.start(1):m.end(1)]
        if any(a < m.start(2) < b for a, b in skip_spans):
            continue
        if has_test_cfg(attrs):
            skip_spans.append((m.start(), m.end()))
            continue
        lineno = 1 + clean.count("\n", 0, m.start(2))
        for p, _binding in flatten_use_tree("", m.group(2)):
            for res in resolve_to_crate(p, module_at(m.start(2)), aliases, facades):
                refs.append((module_at(m.start(2)), res, lineno))
    for m in INLINE_PATH_RE.finditer(clean):
        pos = m.start(1)
        if any(a <= pos < b for a, b in skip_spans):
            continue
        lineno = 1 + clean.count("\n", 0, pos)
        for res in resolve_to_crate(m.group(1), module_at(pos), aliases, facades):
            refs.append((module_at(pos), res, lineno))
    return refs, modules

# Declared in-layer orders (STD-02 R39: layers with > ~10 units; apps/ is
# enforced with its mod.rs-only reach-in check above).
DECLARED_ORDERS = {
    ("scribe-store", ("db",)): os.environ["DB_ORDER"].split(),
    ("scribe-store", ("contracts",)): os.environ["CONTRACTS_ORDER"].split(),
    ("scribe-store", ("repository",)): os.environ["REPOSITORY_ORDER"].split(),
    ("scribe-tools", ()): os.environ["TOOLS_ORDER"].split(),
    ("scribe", ("commands",)): os.environ["COMMANDS_ORDER"].split(),
    ("scribe", ("output",)): os.environ["OUTPUT_ORDER"].split(),
}

# --- 3c. Walk every crate and every module directory ------------------------
for manifest in sorted(glob.glob("crates/*/Cargo.toml")):
    crate_dir = os.path.dirname(manifest)
    crate_name = os.path.basename(crate_dir)
    with open(manifest, encoding="utf-8") as fh:
        for line in fh:
            m = re.match(r'^name\s*=\s*"([^"]+)"', line.strip())
            if m:
                crate_name = m.group(1)
                break
    src_dir = os.path.join(crate_dir, "src")
    if not os.path.isdir(src_dir):
        continue

    top_units = list_units(src_dir, is_crate_root=True)
    aliases = collect_root_aliases(src_dir, top_units)
    facades = RootFacades(src_dir, aliases)
    # Also map `crate::session::*` in scribe-store to `contracts::session` for
    # intra-contracts references (lib.rs re-exports contracts::session in an
    # inline `pub mod session`).
    if crate_name == "scribe-store" and "contracts" in top_units and "session" not in aliases:
        aliases["session"] = ["contracts", "session"]

    # Cache extracted refs per file once for all scopes in this crate.
    file_refs = {}
    inline_units = collections.defaultdict(set)
    for r, dirs, files in os.walk(src_dir):
        dirs[:] = sorted(d for d in dirs if d not in ("tests", "bin"))
        for f in sorted(files):
            if f.endswith(".rs"):
                fp = os.path.join(r, f)
                file_refs[fp], modules = extract_resolved_refs(fp, src_dir, aliases, facades)
                clean = strip_comments_and_strings(open(fp, encoding="utf-8").read())
                for start, end, path, ignored in modules:
                    if ignored:
                        continue
                    # Root compatibility facades made solely of public re-exports
                    # are aliases, not independent implementation units.
                    body = clean[start + 1:end]
                    facade = len(path) == 1 and is_facade_body(body)
                    if not facade or (crate_name, ()) in DECLARED_ORDERS:
                        inline_units[path[:-1]].add(path[-1])

    # Collect all scopes: () for top-level modules, plus every subdirectory.
    scopes = [()]
    for r, dirs, _files in os.walk(src_dir):
        dirs[:] = sorted(d for d in dirs if d not in ("tests", "bin"))
        rel = os.path.relpath(r, src_dir)
        if rel != ".":
            scopes.append(tuple(rel.split(os.sep)))

    scopes = sorted(set(scopes) | set(inline_units))
    for prefix in scopes:
        base_dir = os.path.join(src_dir, *prefix) if prefix else src_dir
        units = list_units(base_dir, is_crate_root=(len(prefix) == 0)) | inline_units[prefix]
        scope_label = crate_name + ("::" + "::".join(prefix) if prefix else "")
        order = DECLARED_ORDERS.get((crate_name, prefix))

        if order is not None:
            rank = {u: i for i, u in enumerate(order)}
            for u in sorted(units):
                if u not in rank:
                    err(
                        f"{scope_label}: module '{u}' has no place in the declared order (ARCHITECTURE.md)"
                    )

        if len(units) < 2 and order is None:
            continue

        edges = collections.defaultdict(set)
        edge_loc = {}
        order_violations = []

        for fp, refs in sorted(file_refs.items()):
            for mp, res, lineno in refs:
                if len(mp) <= len(prefix) or tuple(mp[: len(prefix)]) != prefix:
                    continue
                src_u = mp[len(prefix)]
                if src_u not in units:
                    continue
                if len(res) <= len(prefix) or tuple(res[: len(prefix)]) != prefix:
                    continue
                dst_u = res[len(prefix)]
                if dst_u in units and dst_u != src_u:
                    edges[src_u].add(dst_u)
                    loc = f"{fp}:{lineno}"
                    edge_loc.setdefault((src_u, dst_u), loc)
                    if order is not None and src_u in rank and dst_u in rank:
                        if rank[dst_u] >= rank[src_u]:
                            order_violations.append(
                                (fp, lineno, src_u, dst_u)
                            )

        for cyc in find_cycles(units, edges):
            locs = ", ".join(
                edge_loc.get((cyc[i], cyc[i + 1]), "")
                for i in range(len(cyc) - 1)
            )
            err(f"{scope_label} import cycle: {' -> '.join(cyc)} ({locs})")

        seen_ord = set()
        for fp, lineno, src_u, dst_u in order_violations:
            key = (fp, lineno, src_u, dst_u)
            if key in seen_ord:
                continue
            seen_ord.add(key)
            err(
                f"{fp}:{lineno}: {scope_label}: {src_u} imports {dst_u}, which is not lower in the declared order (ARCHITECTURE.md)"
            )

# --- 3d. scribe-core's root modules (ROOT_PLACES, ARCHITECTURE.md) ------------
# A root `mod x;` is read with the attribute run before it (comments and doc
# strings are not cfgs). A test-only module is exempt, as is one of the places.
# A raw identifier (`mod r#x;`) names the same module as `mod x;` (ORB-28902).
ROOT_MOD_RE = re.compile(
    r'^[ \t]*((?:#\[[^\]]*\]\s*)*)(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+((?:r#)?\w+)\s*;', re.M
)
lib_rs = "crates/scribe-core/src/lib.rs"
if os.path.isfile(lib_rs):
    raw = open(lib_rs, encoding="utf-8").read()
    clean = strip_comments_and_strings(raw)
    places = set(os.environ["ROOT_PLACES"].split())
    undeclared = False
    for m in ROOT_MOD_RE.finditer(clean):
        name = canonical_ident(m.group(2))
        if name in places or has_test_cfg(raw[m.start(1):m.end(1)]):
            continue
        undeclared = True
        line = 1 + clean.count("\n", 0, m.start(2))
        err(f"{lib_rs}:{line}: lib.rs declares the root module {name}, which is not one of the places")
    if undeclared:
        err("scribe-core's root modules: a new root module needs a place (ROOT_PLACES here, ARCHITECTURE.md)")

if errors:
    for msg in errors:
        print(f"dependency-direction: {msg}", file=sys.stderr)
    sys.exit(1)
PY
then
  fail=1
fi

if [[ "$fail" -eq 0 ]]; then
  echo "dependency-direction: ok"
fi
exit "$fail"
