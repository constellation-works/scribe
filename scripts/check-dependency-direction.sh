#!/usr/bin/env bash
# scripts/check-dependency-direction.sh [root] — STD-02 R1–R6, R16, R17, R39 for this workspace.
#
# `root` defaults to the repository; scripts/tests/check-gates.sh points it at
# fixture trees.
#
# Three layers of checks, from manifests and Rust source files alone (no build,
# no network, no jq, no perl), so the gate runs in `make ci-fast`. Needs cargo
# (for `cargo metadata --offline --no-deps --format-version 1`) and python3
# (with the shared Rust lexer in scripts/direction_lex.py):
#
#   1. Crate edges. Workspace members and their dependency package identities,
#      kinds and targets come from `cargo metadata --offline --no-deps
#      --format-version 1`. Every workspace crate needs a policy below. A crate
#      may depend only on the internal crates in its allowlist, and never on the
#      external crates in its banlist. Dev-dependencies (`kind == "dev"`) are
#      exempt; normal and build dependencies across all targets are checked. A
#      crate without a policy fails the check, so a new member forces a
#      decision.
#   2. Module edges inside a crate: source bans over non-test sources through
#      scripts/direction_lex.py for what a crate edge and clippy.toml cannot
#      express (`std::net`, one module's mechanism inside a crate, the app
#      order, the core's root places). `std::fs`, `std::env`, `std::process`,
#      `mpsc::channel` and `rusqlite::Connection::open*` are enforced by
#      clippy.toml (STD-02 R6). A `super::` path counts as the `crate::` path it
#      names, so a relative import meets the same rules.
#   3. Unit import cycles and declared in-layer orders (STD-02 R6, R39): the
#      import graph between workspace crates (from `cargo metadata`), between
#      the top-level modules of each crate, and between the modules of each
#      layer/directory (via scripts/direction_lex.py) must be acyclic, and every
#      layer with more than about ten units obeys its declared order in
#      ARCHITECTURE.md.
#
# The crates, the declared layer orders and these guards are written down in
# ARCHITECTURE.md, from the architecture spec's Guards section
# (docs/design/scribe/specs/architecture.md); change them and this script in
# the same commit (STD-02 R6).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "${1:-$SCRIPT_DIR/..}"

PREFIX="scribe"
fail=0
err() { echo "dependency-direction: $*" >&2; fail=1; }

for tool in cargo python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "dependency-direction: cannot run: $tool not found" >&2
    exit 2
  fi
done

if ! metadata=$(cargo metadata --offline --no-deps --format-version 1 2>&1); then
  echo "dependency-direction: cannot run: cargo metadata failed" >&2
  printf '%s\n' "$metadata" >&2
  exit 2
fi

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
      # exempt (cargo metadata dependencies with kind == "dev" are skipped).
      # rustix and tempfile are allowed for providers/, whose model adapters
      # supervise process groups and hand the codex child its schema in a
      # private temporary directory.
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

contains() { # contains <word> <space-separated list>
  local word="$1" item
  for item in $2; do [[ "$item" == "$word" ]] && return 0; done
  return 1
}

# Workspace crates and their non-dev dependency package identities come from
# `cargo metadata --offline --no-deps --format-version 1`.
current_crate=""
current_has_policy=0
while IFS=$'\t' read -r record crate val; do
  if [[ "$record" == "CRATE" ]]; then
    current_crate="$crate"
    if policy "$crate"; then
      current_has_policy=1
    else
      current_has_policy=0
      err "crate '$crate' ($val) has no policy; add one here and a row in ARCHITECTURE.md"
    fi
  elif [[ "$record" == "DEP" && "$current_has_policy" -eq 1 && "$crate" == "$current_crate" ]]; then
    if [[ "$val" == "$PREFIX" || "$val" == "$PREFIX"-* ]]; then
      contains "$val" "$ALLOWED" || err "$crate must not depend on internal crate $val"
    elif contains "$val" "$BANNED"; then
      err "$crate must not depend on $val (banned for this crate)"
    fi
  fi
done < <(python3 -IB - <<'PY' 3<<<"$metadata"
import json, os

with os.fdopen(3, encoding="utf-8") as fh:
    meta = json.load(fh)

root = os.getcwd()
members = set(meta.get("workspace_members") or [])
packages = [
    pkg for pkg in meta["packages"]
    if not members or pkg["id"] in members
]
packages.sort(key=lambda p: os.path.relpath(p["manifest_path"], root))

for pkg in packages:
    rel_manifest = os.path.relpath(pkg["manifest_path"], root)
    print(f"CRATE\t{pkg['name']}\t{rel_manifest}")
    seen = set()
    for dep in pkg.get("dependencies", []):
        if dep.get("kind") == "dev":
            continue
        dep_pkg = dep["name"]
        if dep_pkg not in seen:
            seen.add(dep_pkg)
            print(f"DEP\t{pkg['name']}\t{dep_pkg}")
PY
)

# --- 2 & 3. filesystem guards, module bans, cycles and in-layer orders -------
STORE=crates/scribe-store/src
CORE=crates/scribe-core/src
CLI=crates/scribe/src
TOOLS=crates/scribe-tools/src

# scribe-core's apps (ORB-27921 step 2; ARCHITECTURE.md, the architecture
# spec §Apps): every directory under apps/ is in the order below, and an app
# imports only apps earlier in it, and only through what that app's mod.rs
# exports (`crate::apps::entry::Item`), never a flow file
# (`crate::apps::entry::flow::…`). A new app fails until it is given a place.
APP_ORDER='tag mark memory entry proposal session transcript group search brief consistency hook init store'
for shared in common shared util; do
  [[ -e "$CORE/apps/$shared" ]] && err "apps/$shared: code two apps need goes to the lower one, to runtime or to the tools"
done

# The core's root modules are its places (ARCHITECTURE.md). lib.rs declares a
# file module (`mod x;`, any visibility, not test-only) only for one of them,
# so a new root module fails until it is given a place here and in the docs.
ROOT_PLACES="apps composition config providers runtime"

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

# Declared in-layer orders (STD-02 R39: layers with > ~10 units; apps/ is
# enforced with its mod.rs-only reach-in check in section 2).
DB_ORDER='archive files log scope codec locks memory messages groups sessions docs import write documents'
CONTRACTS_ORDER='audit provider record session entry handoff tag action group mark memory message proposal'
REPOSITORY_ORDER='sealed ids memory audit entries groups marks messages pickups proposals sessions tags'
TOOLS_ORDER='fs host origin text voice markers project payload transcript window group_close memory_refresh session_distill'
COMMANDS_ORDER='instruction store brief consistency entry export group hook init mark memory proposal search session tag transcript plugin'
OUTPUT_ORDER='common entry export group hook init mark memory proposal search session store tag transcript brief consistency plugin'

if ! APP_ORDER="$APP_ORDER" \
  DB_ORDER="$DB_ORDER" \
  CONTRACTS_ORDER="$CONTRACTS_ORDER" \
  REPOSITORY_ORDER="$REPOSITORY_ORDER" \
  TOOLS_ORDER="$TOOLS_ORDER" \
  COMMANDS_ORDER="$COMMANDS_ORDER" \
  OUTPUT_ORDER="$OUTPUT_ORDER" \
  ROOT_PLACES="$ROOT_PLACES" \
  python3 -IB - "$SCRIPT_DIR" 3<<<"$metadata" <<'PY'
import collections
import json
import os
import re
import sys

sys.path.insert(0, sys.argv[1])
import direction_lex as lex

errors = []


def err(msg):
    errors.append(msg)


with os.fdopen(3, encoding="utf-8") as fh:
    meta = json.load(fh)

ROOT = os.getcwd()
STORE = "crates/scribe-store/src"
CORE = "crates/scribe-core/src"
CLI = "crates/scribe/src"
TOOLS = "crates/scribe-tools/src"

# Reading the clock: the contracts and the tools take a time as an argument.
CLOCK_RX = (
    r"(^|[^A-Za-z0-9_])(now_utc|now_local)\("
    r"|(OffsetDateTime|SystemTime|Instant|Utc|Local)::now([^A-Za-z0-9_]|$)"
)

_raw_cache = {}
_clean_cache = {}
_nocomment_cache = {}
_scan_cache = {}


def read_raw(path):
    if path not in _raw_cache:
        with open(path, encoding="utf-8") as fh:
            _raw_cache[path] = fh.read()
    return _raw_cache[path]


def read_clean(path):
    if path not in _clean_cache:
        _clean_cache[path] = lex.strip_comments_and_strings(read_raw(path))
    return _clean_cache[path]


def read_nocomment(path):
    if path not in _nocomment_cache:
        _nocomment_cache[path] = lex.strip_comments(read_raw(path))
    return _nocomment_cache[path]


def read_scanned(path, base_mod=()):
    key = (path, tuple(base_mod))
    if key not in _scan_cache:
        _scan_cache[key] = lex.scan_file(read_raw(path), base_mod)
    return _scan_cache[key]


def iter_rs_files(path, exclude_tests=True):
    if os.path.isfile(path):
        if path.endswith(".rs"):
            yield path
        return
    if not os.path.isdir(path):
        return
    for r, dirs, files in os.walk(path):
        if exclude_tests:
            dirs[:] = sorted(d for d in dirs if d != "tests")
        else:
            dirs.sort()
        for f in sorted(files):
            if f.endswith(".rs"):
                yield os.path.join(r, f)


def file_mod_parts(src_dir, fp):
    rel = os.path.relpath(fp, src_dir)[:-3].split(os.sep)
    if rel[-1] == "mod":
        return rel[:-1]
    if len(rel) == 1 and rel[0] in ("lib", "main"):
        return []
    return rel


def ban(path, pattern, msg, keep_strings=False):
    rx = re.compile(pattern)
    hits = []
    for fp in iter_rs_files(path):
        text = read_nocomment(fp) if keep_strings else read_clean(fp)
        raw_lines = read_raw(fp).splitlines()
        for lineno, line in enumerate(text.splitlines(), 1):
            if rx.search(line):
                shown = raw_lines[lineno - 1] if lineno <= len(raw_lines) else line
                hits.append(f"{fp}:{lineno}:{shown}")
    if hits:
        print("\n".join(hits), file=sys.stderr)
        err(msg)


def ban_crate(path, pattern, msg):
    rx = re.compile(pattern)
    hits = []
    for fp in iter_rs_files(path):
        mod_parts = file_mod_parts(CORE, fp)
        resolved = lex.resolve_super_text(read_raw(fp), mod_parts)
        for lineno, line in enumerate(resolved.splitlines(), 1):
            if rx.search(line):
                hits.append(f"{fp}:{lineno}:{line}")
    if hits:
        print("\n".join(hits), file=sys.stderr)
        err(msg)


def ban_std(path, modules, msg):
    mod_set = set(modules)
    hits = []
    for fp in iter_rs_files(path):
        scanned = read_scanned(fp)
        raw_lines = read_raw(fp).splitlines()
        seen_lines = set()
        for stmt in scanned.use_stmts:
            if stmt.is_test_only:
                continue
            for segs, _alias in stmt.paths:
                if len(segs) >= 2 and segs[0] == "std" and segs[1] in mod_set:
                    if stmt.line not in seen_lines:
                        seen_lines.add(stmt.line)
                        shown = raw_lines[stmt.line - 1] if stmt.line <= len(raw_lines) else ""
                        hits.append(f"{fp}:{stmt.line}:{shown}")
                    break
        for ip in scanned.inline_paths:
            if ip.is_test_only:
                continue
            if len(ip.segments) >= 2 and ip.segments[0] == "std" and ip.segments[1] in mod_set:
                if ip.line not in seen_lines:
                    seen_lines.add(ip.line)
                    shown = raw_lines[ip.line - 1] if ip.line <= len(raw_lines) else ""
                    hits.append(f"{fp}:{ip.line}:{shown}")
    if hits:
        print("\n".join(hits), file=sys.stderr)
        err(msg)


# --- 2. module bans (through scripts/direction_lex.py) ----------------------

# The crate edges keep the use cases out of scribe-store, and rusqlite out
# of every crate but the store. std::fs, std::env and std::process are
# enforced by clippy.toml; only std::net stays as a source guard (STD-02 R6).
ban_std(CORE, ["net"], "scribe-core opens no socket")

# scribe-core's app order: an app imports only lower apps, through their mod.rs.
app_order = os.environ["APP_ORDER"].split()
app_rank = {name: idx for idx, name in enumerate(app_order)}
app_hits = []
apps_dir = os.path.join(CORE, "apps")
if os.path.isdir(apps_dir):
    for entry in sorted(os.listdir(apps_dir)):
        full = os.path.join(apps_dir, entry)
        if os.path.isdir(full) and entry not in app_rank:
            app_hits.append(f"{CORE}/apps/{entry}: app {entry} has no place in the app order")

    for fp in iter_rs_files(apps_dir):
        rel = os.path.relpath(fp, apps_dir)
        parts = rel.split(os.sep)
        if len(parts) < 2 or parts[0] not in app_rank:
            continue
        src = parts[0]
        mod_parts = file_mod_parts(CORE, fp)
        scanned = read_scanned(fp, mod_parts)
        refs = []
        for stmt in scanned.use_stmts:
            if stmt.is_test_only:
                continue
            for segs, _alias in stmt.paths_keep_self:
                res = lex.resolve_path(segs, stmt.mod_path)
                if res:
                    refs.append((res, stmt.line))
        for ip in scanned.inline_paths:
            if ip.is_test_only:
                continue
            res = lex.resolve_path(ip.segments, ip.mod_path)
            if res:
                refs.append((res, ip.line))
        seen_app_refs = set()
        for res, line in refs:
            if len(res) < 2 or res[0] != "apps":
                continue
            target = res[1]
            rest = res[2:]
            if target == src:
                continue
            if target not in app_rank:
                key = ("unknown", line, target)
                if key not in seen_app_refs:
                    seen_app_refs.add(key)
                    app_hits.append(f"{fp}:{line}: {src} imports unknown app {target}")
                continue
            if app_rank[target] >= app_rank[src]:
                key = ("order", line, target)
                if key not in seen_app_refs:
                    seen_app_refs.add(key)
                    app_hits.append(
                        f"{fp}:{line}: {src} imports {target}, which is not lower in the app order"
                    )
            if len(rest) >= 2 and re.match(r"^[a-z_]", rest[0]):
                rest_s = "::" + "::".join(rest)
                key = ("reach", line, target, rest_s)
                if key not in seen_app_refs:
                    seen_app_refs.add(key)
                    app_hits.append(
                        f"{fp}:{line}: {src} reaches into {target}{rest_s}: import what its mod.rs exports"
                    )

if app_hits:
    print("\n".join(app_hits), file=sys.stderr)
    err("scribe-core's app order: an app imports only lower apps, through their mod.rs (ARCHITECTURE.md)")

# cases/ is retired.
ban_crate(CORE, r"crate::cases(::|[^A-Za-z0-9_]|$)", "cases/ is retired: a use case lives in its app under apps/")
ban(CLI, r"scribe_core::cases(::|[^A-Za-z0-9_]|$)", "cases/ is retired: call the app's exec through scribe_core::apps::<app>")

# Only memory refresh, group close, their apps' mod.rs files, and
# runtime/errors.rs name providers, the Models port or ModelError.
models_port = r"crate::providers(::|[^A-Za-z0-9_]|$)|(^|[^A-Za-z0-9_])(Models|ModelError|ModelsOnce|ModelsEach)([^A-Za-z0-9_]|$)"
model_exempt = {
    "apps/memory/mod.rs",
    "apps/memory/refresh.rs",
    "apps/group/mod.rs",
    "apps/group/close.rs",
    "runtime/errors.rs",
}
for sub in (os.path.join(CORE, "apps"), os.path.join(CORE, "runtime")):
    for fp in iter_rs_files(sub):
        rel = os.path.relpath(fp, CORE)
        if rel in model_exempt:
            continue
        ban_crate(
            fp,
            models_port,
            f"{fp[len('crates/'):]} names the Models port: only memory refresh and group close call a model",
        )

# The providers know nothing of the apps or the runtime that call them.
ban_crate(
    os.path.join(CORE, "providers"),
    r"crate::(apps|runtime)(::|[^A-Za-z0-9_]|$)",
    "providers must not import the apps or the runtime",
)

# scribe-tools: no socket, no scribe_core, no Store::write, no clock, no randomness.
ban_std(TOOLS, ["net"], "scribe-tools opens no socket")
ban(TOOLS, r"scribe_core(::|[^A-Za-z0-9_]|$)", "scribe-tools must not name scribe_core: the core calls the tools")
ban(TOOLS, r"Store::write|\.write\(\s*\|", "scribe-tools must not write the store")
ban(TOOLS, CLOCK_RX, "scribe-tools must not read the clock: take the time as an argument")
ban(TOOLS, r"(^|[^A-Za-z0-9_])(rand|fastrand|getrandom|uuid)::", "scribe-tools must not draw randomness")

# The model shapes must not import their callers (direct or grouped imports).
models_rs = os.path.join(CORE, "providers", "models.rs")
ban_crate(
    models_rs,
    r"crate::(memory|group)(::|[^A-Za-z0-9_]|$)",
    "the model shapes must not import their callers",
)
if os.path.isfile(models_rs):
    scanned = read_scanned(models_rs, ["providers", "models"])
    port_hits = []
    raw_lines = read_raw(models_rs).splitlines()
    for stmt in scanned.use_stmts:
        if stmt.is_test_only:
            continue
        for segs, _alias in stmt.paths:
            res = lex.resolve_path(segs, stmt.mod_path)
            if res and res[0] in ("memory", "group"):
                shown = raw_lines[stmt.line - 1] if stmt.line <= len(raw_lines) else ""
                port_hits.append(f"{models_rs}:{stmt.line}:{shown}")
                break
    if port_hits:
        print("\n".join(port_hits), file=sys.stderr)
        err("the model shapes must not import their callers")


def reaches_db_files(segs):
    """True when a flattened `use` path matches `(crate|self|super)* (super|db)::files`."""
    idx = 0
    while idx < len(segs) and segs[idx] in ("crate", "self", "super"):
        if (
            segs[idx] == "super"
            and idx + 1 < len(segs)
            and segs[idx + 1] == "files"
        ):
            return True
        idx += 1
    return idx + 1 < len(segs) and segs[idx] == "db" and segs[idx + 1] == "files"


# Inside scribe-store: db/codec.rs and repository/audit.rs do no file I/O.
for shape, desc in (("db/codec", "converts bytes"), ("repository/audit", "writes its rows through db/")):
    fp = os.path.join(STORE, f"{shape}.rs")
    if os.path.isdir(STORE) and not os.path.isfile(fp):
        err(f"{fp[len('crates/'):]} is missing: update this guard with the file")
        continue
    io_msg = f"store/{shape}.rs {desc}; it must not do I/O"
    ban(fp, r"(^|[^A-Za-z0-9_])(super|db)::files([^A-Za-z0-9_]|$)", io_msg)
    if os.path.isfile(fp):
        scanned = read_scanned(fp, file_mod_parts(STORE, fp))
        grouped_hits = []
        for stmt in scanned.use_stmts:
            if stmt.is_test_only:
                continue
            if any(reaches_db_files(segs) for segs, _alias in stmt.paths):
                grouped_hits.append(f"{fp}:{stmt.line}: grouped import reaches db/files.rs")
        if grouped_hits:
            print("\n".join(grouped_hits), file=sys.stderr)
            err(io_msg)
    ban(fp, r"(^|[^A-Za-z0-9_])cw_fs([^A-Za-z0-9_]|$)", io_msg)
    ban(fp, r"(^|[^A-Za-z0-9_])cw_sqlite([^A-Za-z0-9_]|$)", f"store/{shape}.rs {desc}; it must not open SQLite")

# rustix and tempfile serve the model processes: only providers/ names them.
if os.path.isdir(CORE):
    for entry in sorted(os.listdir(CORE)):
        if entry in ("tests", "providers"):
            continue
        p = os.path.join(CORE, entry)
        ban(
            p,
            r"(^|[^A-Za-z0-9_])(rustix|tempfile)::",
            f"{p[len('crates/'):]}: only providers/ uses rustix and tempfile",
        )

# Composition builds the context the apps run against; it imports no app.
ban_crate(
    os.path.join(CORE, "composition.rs"),
    r"crate::apps(::|[^A-Za-z0-9_]|$)",
    "composition.rs must not import the apps",
)

# Only scribe-store's db/ uses cw-sqlite and rusqlite, and no scribe source
# configures or checkpoints SQLite directly.
for fp in iter_rs_files(STORE):
    rel = os.path.relpath(fp, STORE)
    if rel.startswith("db" + os.sep):
        continue
    ban(
        fp,
        r"(^|[^A-Za-z0-9_])(rusqlite|cw_sqlite)([^A-Za-z0-9_]|$)",
        f"{fp[len('crates/'):]}: only scribe-store's db/ uses cw-sqlite and rusqlite",
    )

ban(
    STORE,
    r"PRAGMA\s+(journal_mode|synchronous|busy_timeout|user_version|secure_delete|wal_checkpoint)|\.busy_timeout\(|PersistWal",
    "scribe-store must open, configure, checkpoint and migrate scribe.db only through cw-sqlite",
    keep_strings=True,
)

# db/files.rs imports nothing of the store but its error.
files_rs = os.path.join(STORE, "db", "files.rs")
if os.path.isfile(files_rs):
    scanned = read_scanned(files_rs, ["db", "files"])
    raw_lines = read_raw(files_rs).splitlines()
    files_hits = []
    seen_lines = set()
    for stmt in scanned.use_stmts:
        if stmt.is_test_only:
            continue
        for segs, _alias in stmt.paths:
            if segs and segs[0] in ("crate", "super"):
                res = lex.resolve_path(segs, stmt.mod_path)
                if not (res and res[0] == "error"):
                    if stmt.line not in seen_lines:
                        seen_lines.add(stmt.line)
                        shown = raw_lines[stmt.line - 1] if stmt.line <= len(raw_lines) else ""
                        files_hits.append(f"{files_rs}:{stmt.line}:{shown}")
                    break
    for ip in scanned.inline_paths:
        if ip.is_test_only:
            continue
        if ip.segments and ip.segments[0] in ("crate", "super"):
            res = lex.resolve_path(ip.segments, ip.mod_path)
            if not (res and res[0] == "error"):
                if ip.line not in seen_lines:
                    seen_lines.add(ip.line)
                    shown = raw_lines[ip.line - 1] if ip.line <= len(raw_lines) else ""
                    files_hits.append(f"{files_rs}:{ip.line}:{shown}")
    if files_hits:
        print("\n".join(files_hits), file=sys.stderr)
        err("scribe-store/src/db/files.rs: the cw-fs adapter imports nothing of the store but its error")

# Only the store and the project scaffold (apps/init/install.rs) use cw-fs.
install_rs = os.path.join(CORE, "apps", "init", "install.rs")
for root_dir in (CORE, CLI, TOOLS):
    for fp in iter_rs_files(root_dir):
        if fp == install_rs:
            continue
        ban(
            fp,
            r"(^|[^A-Za-z0-9_])cw_fs([^A-Za-z0-9_]|$)",
            f"{fp[len('crates/'):]}: only the store and the project scaffold (apps/init/install.rs) use cw-fs",
        )

for root_dir in (CORE, CLI, TOOLS):
    ban(root_dir, r"(^|[^A-Za-z0-9_])cw_sqlite([^A-Za-z0-9_]|$)", "only scribe-store's db/ uses cw-sqlite")

# contracts/ holds record shapes only: no db/ or repository/, no check_*, no clock.
contracts_dir = os.path.join(STORE, "contracts")
ban(
    contracts_dir,
    r"(crate|super)::(db|repository)([^A-Za-z0-9_]|$)",
    "scribe-store/src/contracts: contracts import nothing from db/ or repository/",
)
contracts_hits = []
for fp in iter_rs_files(contracts_dir):
    scanned = read_scanned(fp, file_mod_parts(STORE, fp))
    raw_lines = read_raw(fp).splitlines()
    for stmt in scanned.use_stmts:
        if stmt.is_test_only:
            continue
        for segs, _alias in stmt.paths:
            if len(segs) >= 2 and segs[0] in ("crate", "super") and segs[1] in ("db", "repository"):
                shown = raw_lines[stmt.line - 1] if stmt.line <= len(raw_lines) else ""
                contracts_hits.append(f"{fp}:{stmt.line}:{shown}")
                break
if contracts_hits:
    print("\n".join(contracts_hits), file=sys.stderr)
    err("scribe-store/src/contracts: contracts import nothing from db/ or repository/")

ban(
    contracts_dir,
    r"(^|[^A-Za-z0-9_])fn\s+check_",
    "scribe-store/src/contracts: a contract decides nothing (no check_* function)",
)
ban(contracts_dir, CLOCK_RX, "scribe-store/src/contracts: a contract reads no clock")

# Rendering never reaches back up; the binary reaches core only through public surface.
ban(os.path.join(CLI, "output"), r"crate::(app|cli|commands)", "output must not import app, cli or commands")
ban(
    CLI,
    r"(^|[^A-Za-z0-9_])scribe_(domain|store|tools)::",
    "the binary must not name scribe_domain, scribe_store or scribe_tools: it reaches them through scribe_core",
)
ban(
    CLI,
    r"scribe_core::runtime(::|[^A-Za-z0-9_]|$)",
    "the binary must not name scribe_core::runtime: the runtime is internal to the core",
)

# Persisted shapes never fabricate a timestamp on read (R16).
ban(
    "crates",
    r'serde\(default *= *"[A-Za-z_:]*(now|now_utc)"',
    "a persisted timestamp must not default to the current time",
    keep_strings=True,
)


# --- 3a. Workspace crate dependency graph (from cargo metadata) -------------
def _split_toml_keys(text):
    """Split a TOML dotted key sequence into unquoted segments."""
    parts = []
    cur = []
    quote = None
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if quote is not None:
            if quote == '"' and ch == "\\" and i + 1 < n:
                cur.append(text[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            else:
                cur.append(ch)
        else:
            if ch in ('"', "'"):
                quote = ch
            elif ch == ".":
                seg = "".join(cur).strip()
                if seg:
                    parts.append(seg)
                cur = []
            else:
                cur.append(ch)
        i += 1
    seg = "".join(cur).strip()
    if seg:
        parts.append(seg)
    return parts


def _strip_toml_comment(line, ml_delim):
    """Return (code_without_comment, updated_ml_delim) for diagnostic line lookup."""
    out = []
    i = 0
    n = len(line)
    quote = None
    while i < n:
        if ml_delim is not None:
            if ml_delim == '"""' and line[i] == "\\":
                i += 2
                continue
            if line.startswith(ml_delim, i):
                i += 3
                ml_delim = None
                continue
            i += 1
            continue
        ch = line[i]
        if quote is None:
            if ch == "#":
                break
            if line.startswith('"""', i) or line.startswith("'''", i):
                ml_delim = line[i : i + 3]
                out.append(ml_delim)
                i += 3
                continue
            if ch in ('"', "'"):
                quote = ch
        else:
            if quote == '"' and ch == "\\" and i + 1 < n:
                out.append(line[i : i + 2])
                i += 2
                continue
            if ch == quote:
                quote = None
        out.append(ch)
        i += 1
    return "".join(out).strip(), ml_delim


def find_dep_location(rel_manifest, dep):
    """Locate the declaration line for a cargo-metadata dependency for diagnostics."""
    key = dep.get("rename") or dep["name"]
    want_kind = "build-dependencies" if dep.get("kind") == "build" else "dependencies"
    want_target = dep.get("target")

    cur_target = None
    cur_kind = None
    cur_subkey = None
    top_level = True
    ml_delim = None

    try:
        with open(rel_manifest, encoding="utf-8") as fh:
            lines = fh.readlines()
    except OSError:
        return rel_manifest

    for lineno, raw_line in enumerate(lines, 1):
        in_ml = ml_delim is not None
        line, ml_delim = _strip_toml_comment(raw_line, ml_delim)
        if in_ml or not line:
            continue
        if line.startswith("[") and line.endswith("]") and not line.startswith("[["):
            top_level = False
            segs = _split_toml_keys(line[1:-1].strip())
            cur_target = None
            cur_kind = None
            cur_subkey = None
            if len(segs) >= 1 and segs[0] in ("dependencies", "build-dependencies"):
                cur_kind = segs[0]
                if len(segs) == 2:
                    cur_subkey = segs[1]
            elif (
                len(segs) >= 3
                and segs[0] == "target"
                and segs[2] in ("dependencies", "build-dependencies")
            ):
                cur_target = segs[1]
                cur_kind = segs[2]
                if len(segs) == 4:
                    cur_subkey = segs[3]
            if cur_subkey == key and cur_kind == want_kind and cur_target == want_target:
                return f"{rel_manifest}:{lineno}"
            continue

        if "=" not in line:
            continue
        lhs = line.split("=", 1)[0].strip()
        lhs_keys = _split_toml_keys(lhs)
        if not lhs_keys:
            continue
        if cur_kind == want_kind and cur_target == want_target and cur_subkey is None:
            if lhs_keys[0] == key:
                return f"{rel_manifest}:{lineno}"
        elif top_level:
            if (
                want_target is None
                and len(lhs_keys) >= 2
                and lhs_keys[0] == want_kind
                and lhs_keys[1] == key
            ):
                return f"{rel_manifest}:{lineno}"
            if (
                want_target is not None
                and len(lhs_keys) >= 4
                and lhs_keys[0] == "target"
                and lhs_keys[1] == want_target
                and lhs_keys[2] == want_kind
                and lhs_keys[3] == key
            ):
                return f"{rel_manifest}:{lineno}"

    return rel_manifest


members = set(meta.get("workspace_members") or [])
workspace_packages = [
    pkg for pkg in meta["packages"] if not members or pkg["id"] in members
]
workspace_packages.sort(key=lambda p: os.path.relpath(p["manifest_path"], ROOT))

workspace_crates = {pkg["name"] for pkg in workspace_packages}
crate_edges = collections.defaultdict(set)
crate_edge_loc = {}

for pkg in workspace_packages:
    crate = pkg["name"]
    rel_manifest = os.path.relpath(pkg["manifest_path"], ROOT)
    for dep in pkg.get("dependencies", []):
        if dep.get("kind") == "dev":
            continue
        dep_pkg = dep["name"]
        if dep_pkg in workspace_crates:
            crate_edges[crate].add(dep_pkg)
            crate_edge_loc.setdefault((crate, dep_pkg), find_dep_location(rel_manifest, dep))


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


# --- 3b. Import resolution via scripts/direction_lex.py ---------------------
def expand_alias(path, aliases):
    if path and path[0] in aliases:
        return aliases[path[0]] + path[1:]
    return path


def resolve_to_crate(segs, mod_parts, aliases, facades=None):
    """Return every module path a crate-relative path can name (usually one)."""
    res = lex.resolve_path(segs, mod_parts)
    if res is None:
        return []
    if res and res[0] in aliases:
        return [aliases[res[0]] + res[1:]]
    if res and facades is not None and res[0] in facades.table:
        return facades.expand(res)
    return [res]


def collect_root_aliases(src_dir, top_units):
    """Collect unambiguous top-level re-exports in lib.rs / main.rs."""
    candidates = collections.defaultdict(set)
    for root_file in ("lib.rs", "main.rs"):
        fp = os.path.join(src_dir, root_file)
        if not os.path.isfile(fp):
            continue
        scanned = read_scanned(fp, ())
        for stmt in scanned.use_stmts:
            if stmt.is_test_only or stmt.brace_depth != 0:
                continue
            for segs, binding in stmt.paths:
                seg_list = list(segs)
                if seg_list and seg_list[0] == "crate":
                    seg_list = seg_list[1:]
                if len(seg_list) >= 2 and seg_list[0] in top_units and seg_list[-1] != "*":
                    alias = binding or seg_list[-1]
                    if alias not in top_units:
                        candidates[alias].add(tuple(seg_list))
    return {k: list(next(iter(v))) for k, v in candidates.items() if len(v) == 1}


def has_non_test_rs(directory):
    for _r, dirs, files in os.walk(directory):
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


class RootFacades:
    """Inline root facades and the modules their re-exports are defined in."""

    DEPTH = 8

    def __init__(self, src_dir, aliases):
        self.src_dir = src_dir
        self.aliases = aliases
        self.table = {}
        self.scanned_modules = {}
        self.collect()

    def collect(self):
        for root_file in ("lib.rs", "main.rs"):
            fp = os.path.join(self.src_dir, root_file)
            if not os.path.isfile(fp):
                continue
            scanned = read_scanned(fp, ())
            for m in scanned.inline_mods:
                if m.is_test_only or len(m.path) != 1:
                    continue
                body_toks = scanned.code[m.body_tok_start : m.body_tok_end]
                if not lex.is_facade_body(body_toks):
                    continue
                self.table[m.path[0]] = self.reexports(scanned, m.path)

    def reexports(self, scanned, facade_path):
        named, globs = {}, []
        for stmt in scanned.use_stmts:
            if stmt.is_test_only or stmt.mod_path != facade_path:
                continue
            for segs, binding in stmt.paths:
                res = lex.resolve_path(segs, facade_path)
                if segs and segs[-1] == "*":
                    globs.append(res[:-1] if res else None)
                elif segs:
                    named[binding or segs[-1]] = res
        return named, globs

    def module_scanned(self, parts):
        key = tuple(parts)
        if key not in self.scanned_modules:
            base = os.path.join(self.src_dir, *parts)
            found = None
            for fp in (base + ".rs", os.path.join(base, "mod.rs")):
                if parts and os.path.isfile(fp):
                    found = read_scanned(fp, parts)
                    break
            self.scanned_modules[key] = found
        return self.scanned_modules[key]

    def is_module(self, parts):
        base = os.path.join(self.src_dir, *parts)
        return bool(parts) and (os.path.isfile(base + ".rs") or os.path.isdir(base))

    def expand(self, res, depth=0):
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
        if len(globs) == 1 and not external and globs[0] is not None:
            return [self.alias(globs[0]) + name + tail]
        return []

    def alias(self, path):
        return expand_alias(path, self.aliases)

    def locate(self, path, depth):
        for k in range(len(path) - 1, 0, -1):
            if self.is_module(path[:k]):
                module, name, tail = path[:k], path[k], path[k + 1 :]
                break
        else:
            return False, [path]
        scanned = self.module_scanned(module)
        if depth > self.DEPTH or scanned is None:
            return False, [path]
        if self.defines(module, name):
            return True, [path]
        globs = []
        for stmt in scanned.use_stmts:
            if stmt.is_test_only or stmt.brace_depth != 0:
                continue
            for segs, binding in stmt.paths:
                if segs and segs[-1] == "*":
                    globs.append(segs)
                elif segs and (binding or segs[-1]) == name:
                    return True, self.follow(segs, module, tail, depth)
        found = []
        for segs in globs:
            res = lex.resolve_path(segs, module)
            if res is None:
                continue
            ok, paths = self.locate(self.alias(res[:-1]) + [name] + tail, depth + 1)
            if ok:
                found.extend(paths)
        return (True, found) if found else (False, [path])

    def defines(self, module, name):
        scanned = self.module_scanned(module)
        return scanned is not None and (name in scanned.item_defs or self.is_module(module + [name]))

    def follow(self, segs, module, tail, depth):
        res = lex.resolve_path(segs, module)
        if res is None:
            return []
        if res and res[0] in self.aliases:
            return self.locate(self.aliases[res[0]] + res[1:] + tail, depth + 1)[1]
        if res and res[0] in self.table:
            return self.expand(res + tail, depth + 1)
        return self.locate(res + tail, depth + 1)[1]


def extract_resolved_refs(fp, src_dir, aliases, facades=None):
    mod_parts = file_mod_parts(src_dir, fp)
    scanned = read_scanned(fp, mod_parts)
    refs = []
    for stmt in scanned.use_stmts:
        if stmt.is_test_only:
            continue
        for segs, _binding in stmt.paths:
            for res in resolve_to_crate(segs, stmt.mod_path, aliases, facades):
                refs.append((list(stmt.mod_path), res, stmt.line))
    for ip in scanned.inline_paths:
        if ip.is_test_only:
            continue
        for res in resolve_to_crate(ip.segments, ip.mod_path, aliases, facades):
            refs.append((list(ip.mod_path), res, ip.line))
    return refs, scanned


DECLARED_ORDERS = {
    ("scribe-store", ("db",)): os.environ["DB_ORDER"].split(),
    ("scribe-store", ("contracts",)): os.environ["CONTRACTS_ORDER"].split(),
    ("scribe-store", ("repository",)): os.environ["REPOSITORY_ORDER"].split(),
    ("scribe-tools", ()): os.environ["TOOLS_ORDER"].split(),
    ("scribe", ("commands",)): os.environ["COMMANDS_ORDER"].split(),
    ("scribe", ("output",)): os.environ["OUTPUT_ORDER"].split(),
}

# --- 3c. Walk every crate and every module directory ------------------------
for pkg in workspace_packages:
    crate_name = pkg["name"]
    crate_dir = os.path.relpath(os.path.dirname(pkg["manifest_path"]), ROOT)
    src_dir = os.path.join(crate_dir, "src")
    if not os.path.isdir(src_dir):
        continue

    top_units = list_units(src_dir, is_crate_root=True)
    aliases = collect_root_aliases(src_dir, top_units)
    facades = RootFacades(src_dir, aliases)
    if crate_name == "scribe-store" and "contracts" in top_units and "session" not in aliases:
        aliases["session"] = ["contracts", "session"]

    file_refs = {}
    inline_units = collections.defaultdict(set)
    for r, dirs, files in os.walk(src_dir):
        dirs[:] = sorted(d for d in dirs if d not in ("tests", "bin"))
        for f in sorted(files):
            if f.endswith(".rs"):
                fp = os.path.join(r, f)
                file_refs[fp], scanned = extract_resolved_refs(fp, src_dir, aliases, facades)
                for m in scanned.inline_mods:
                    if m.is_test_only:
                        continue
                    body_toks = scanned.code[m.body_tok_start : m.body_tok_end]
                    facade = len(m.path) == 1 and lex.is_facade_body(body_toks)
                    if not facade or (crate_name, ()) in DECLARED_ORDERS:
                        inline_units[m.path[:-1]].add(m.path[-1])

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
                            order_violations.append((fp, lineno, src_u, dst_u))

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
lib_rs = "crates/scribe-core/src/lib.rs"
if os.path.isfile(lib_rs):
    scanned = read_scanned(lib_rs, ())
    places = set(os.environ["ROOT_PLACES"].split())
    undeclared = False
    for fm in scanned.file_mods:
        if fm.brace_depth != 0 or fm.is_test_only or fm.name in places:
            continue
        undeclared = True
        err(f"{lib_rs}:{fm.line}: lib.rs declares the root module {fm.name}, which is not one of the places")
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
