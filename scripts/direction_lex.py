"""Shared Rust lexer and item scanner for scripts/check-dependency-direction.sh.

Every Rust-source guard in the direction gate uses this module so comments,
string/char literals, raw identifiers (`r#name`), `#[cfg(...)]` attributes,
`use` trees, `mod` declarations and `crate::`/`super::`/`self::`/`std::` paths
are classified consistently by one rustc-faithful tokenizer.
"""

from __future__ import annotations

import re
from typing import List, NamedTuple, Optional, Sequence, Set, Tuple


class Token(NamedTuple):
    kind: str  # WS, COMMENT, STRING, CHAR, LIFETIME, RAW_IDENT, IDENT, NUMBER, PUNCT
    text: str
    start: int
    end: int
    line: int


class InlineMod(NamedTuple):
    name: str
    path: Tuple[str, ...]
    is_test_only: bool
    start_char: int
    end_char: int
    body_tok_start: int
    body_tok_end: int
    vis: Optional[str]
    line: int


class FileMod(NamedTuple):
    name: str
    path: Tuple[str, ...]
    is_test_only: bool
    brace_depth: int
    vis: Optional[str]
    line: int


class UseStmt(NamedTuple):
    mod_path: Tuple[str, ...]
    is_test_only: bool
    brace_depth: int
    vis: Optional[str]
    line: int
    start_char: int
    end_char: int
    tree_toks: Tuple[Token, ...]
    paths: Tuple[Tuple[Tuple[str, ...], Optional[str]], ...]
    paths_keep_self: Tuple[Tuple[Tuple[str, ...], Optional[str]], ...]


class InlinePath(NamedTuple):
    mod_path: Tuple[str, ...]
    is_test_only: bool
    segments: Tuple[str, ...]
    line: int
    start_char: int
    end_char: int


class ScannedFile(NamedTuple):
    tokens: Tuple[Token, ...]
    code: Tuple[Token, ...]
    inline_mods: Tuple[InlineMod, ...]
    file_mods: Tuple[FileMod, ...]
    use_stmts: Tuple[UseStmt, ...]
    inline_paths: Tuple[InlinePath, ...]
    item_defs: frozenset


RAW_STR_PREFIX_RE = re.compile(r'(?:br|cr|r)(#*)"')
CHAR_LIT_RE = re.compile(
    r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\n\r])'"
)
BYTE_CHAR_LIT_RE = re.compile(
    r"b'(?:\\(?:x[0-9a-fA-F]{2}|.)|[^'\\\n\r])'"
)
NUMBER_RE = re.compile(
    r"[0-9](?:[0-9A-Za-z_]|(?:\.(?![A-Za-z_.]))|(?:[eE][+-]))*"
)
ITEM_DEF_KEYWORDS = frozenset(
    ("fn", "struct", "enum", "trait", "type", "const", "static", "union", "mod")
)


def _is_ident_start(ch: str) -> bool:
    return ch == "_" or ch.isalpha()


def _is_ident_continue(ch: str) -> bool:
    return ch == "_" or ch.isalnum()


def canonical_ident(name: str) -> str:
    """Return the canonical name for a plain or raw Rust identifier."""
    return name[2:] if name.startswith("r#") else name


def _scan_ident_end(src: str, pos: int) -> int:
    n = len(src)
    while pos < n and _is_ident_continue(src[pos]):
        pos += 1
    return pos


def tokenize(src: str) -> List[Token]:
    """Tokenize Rust source preserving exact character offsets and line numbers."""
    tokens: List[Token] = []
    i = 0
    n = len(src)
    line = 1

    while i < n:
        ch = src[i]
        start = i
        start_line = line

        # Whitespace
        if ch.isspace():
            while i < n and src[i].isspace():
                if src[i] == "\n":
                    line += 1
                i += 1
            tokens.append(Token("WS", src[start:i], start, i, start_line))
            continue

        # Line comment: // ...
        if ch == "/" and i + 1 < n and src[i + 1] == "/":
            nl = src.find("\n", i + 2)
            i = n if nl < 0 else nl
            tokens.append(Token("COMMENT", src[start:i], start, i, start_line))
            continue

        # Nested block comment: /* ... */
        if ch == "/" and i + 1 < n and src[i + 1] == "*":
            depth = 1
            i += 2
            while i < n and depth > 0:
                if src[i] == "/" and i + 1 < n and src[i + 1] == "*":
                    depth += 1
                    i += 2
                elif src[i] == "*" and i + 1 < n and src[i + 1] == "/":
                    depth -= 1
                    i += 2
                else:
                    if src[i] == "\n":
                        line += 1
                    i += 1
            tokens.append(Token("COMMENT", src[start:i], start, i, start_line))
            continue

        # Raw strings: r#*"..."#*, br#*"..."#*, cr#*"..."#*
        if ch in ("r", "b", "c"):
            m = RAW_STR_PREFIX_RE.match(src, i)
            if m:
                hashes = m.group(1)
                end_pat = '"' + hashes
                end_pos = src.find(end_pat, m.end())
                i = n if end_pos < 0 else end_pos + len(end_pat)
                text = src[start:i]
                line += text.count("\n")
                tokens.append(Token("STRING", text, start, i, start_line))
                continue

        # Normal, byte, or C string: "...", b"...", c"..."
        if ch == '"' or (ch in ("b", "c") and i + 1 < n and src[i + 1] == '"'):
            i += 1 if ch == '"' else 2
            while i < n:
                c = src[i]
                if c == "\\" and i + 1 < n:
                    if src[i + 1] == "\n":
                        line += 1
                    i += 2
                elif c == '"':
                    i += 1
                    break
                else:
                    if c == "\n":
                        line += 1
                    i += 1
            tokens.append(Token("STRING", src[start:i], start, i, start_line))
            continue

        # Byte char literal: b'...'
        if ch == "b" and i + 1 < n and src[i + 1] == "'":
            m = BYTE_CHAR_LIT_RE.match(src, i)
            if m:
                i = m.end()
                tokens.append(Token("CHAR", src[start:i], start, i, start_line))
                continue

        # Char literal or lifetime: '...' vs 'a / '_ / 'r#ident
        if ch == "'":
            m = CHAR_LIT_RE.match(src, i)
            if m:
                i = m.end()
                tokens.append(Token("CHAR", src[start:i], start, i, start_line))
                continue
            if (
                i + 3 < n
                and src[i + 1] == "r"
                and src[i + 2] == "#"
                and _is_ident_start(src[i + 3])
            ):
                i = _scan_ident_end(src, i + 4)
                tokens.append(Token("LIFETIME", src[start:i], start, i, start_line))
                continue
            if i + 1 < n and _is_ident_start(src[i + 1]):
                i = _scan_ident_end(src, i + 2)
                tokens.append(Token("LIFETIME", src[start:i], start, i, start_line))
                continue
            i += 1
            tokens.append(Token("PUNCT", ch, start, i, start_line))
            continue

        # Raw identifier: r#<ident>
        if (
            ch == "r"
            and i + 2 < n
            and src[i + 1] == "#"
            and _is_ident_start(src[i + 2])
        ):
            i = _scan_ident_end(src, i + 3)
            tokens.append(Token("RAW_IDENT", src[start:i], start, i, start_line))
            continue

        # Identifier
        if _is_ident_start(ch):
            i = _scan_ident_end(src, i + 1)
            tokens.append(Token("IDENT", src[start:i], start, i, start_line))
            continue

        # Numeric literal
        if ch.isdigit():
            m = NUMBER_RE.match(src, i)
            i = m.end() if m else i + 1
            tokens.append(Token("NUMBER", src[start:i], start, i, start_line))
            continue

        # Path separator ::
        if ch == ":" and i + 1 < n and src[i + 1] == ":":
            i += 2
            tokens.append(Token("PUNCT", "::", start, i, start_line))
            continue

        # Single punctuation character
        i += 1
        tokens.append(Token("PUNCT", ch, start, i, start_line))

    return tokens


def code_tokens(src_or_tokens: object) -> List[Token]:
    """Return non-whitespace, non-comment tokens from source text or token list."""
    tokens = tokenize(src_or_tokens) if isinstance(src_or_tokens, str) else src_or_tokens
    return [t for t in tokens if t.kind not in ("WS", "COMMENT")]


def _blank_preserving_newlines(text: str) -> str:
    return "".join("\n" if ch == "\n" else " " for ch in text)


def strip_comments_and_strings(src: str) -> str:
    """Replace Rust comments and string/char literals with spaces, preserving newlines."""
    out: List[str] = []
    for tok in tokenize(src):
        if tok.kind in ("COMMENT", "STRING", "CHAR"):
            out.append(_blank_preserving_newlines(tok.text))
        else:
            out.append(tok.text)
    return "".join(out)


def strip_comments(src: str) -> str:
    """Replace Rust comments with spaces while keeping string literals and newlines intact."""
    out: List[str] = []
    for tok in tokenize(src):
        if tok.kind == "COMMENT":
            out.append(_blank_preserving_newlines(tok.text))
        else:
            out.append(tok.text)
    return "".join(out)


def unquote_rust_string(tok: Token) -> Optional[str]:
    """Decode a Rust string or raw-string token to its string value."""
    if tok.kind != "STRING":
        return None
    text = tok.text
    m = RAW_STR_PREFIX_RE.match(text)
    if m:
        hashes = m.group(1)
        body_start = m.end()
        body_end = len(text) - 1 - len(hashes)
        return text[body_start:body_end] if body_end >= body_start else ""
    start = 1 if text.startswith('"') else 2
    if not text.endswith('"') or len(text) <= start:
        return None
    body = text[start:-1]
    out: List[str] = []
    i = 0
    n = len(body)
    simple = {"n": "\n", "r": "\r", "t": "\t", "\\": "\\", '"': '"', "'": "'", "0": "\0"}
    while i < n:
        c = body[i]
        if c == "\\" and i + 1 < n:
            nxt = body[i + 1]
            if nxt in simple:
                out.append(simple[nxt])
                i += 2
            elif nxt == "\n":
                i += 2
                while i < n and body[i] in (" ", "\t", "\r"):
                    i += 1
            elif nxt == "x" and i + 3 < n:
                try:
                    out.append(chr(int(body[i + 2 : i + 4], 16)))
                    i += 4
                except ValueError:
                    out.append(body[i : i + 2])
                    i += 2
            elif nxt == "u" and i + 2 < n and body[i + 2] == "{":
                close = body.find("}", i + 3)
                if close > i + 3:
                    hex_val = body[i + 3 : close].replace("_", "")
                    try:
                        out.append(chr(int(hex_val, 16)))
                        i = close + 1
                        continue
                    except ValueError:
                        pass
                out.append(c)
                i += 1
            else:
                out.append(nxt)
                i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _find_matching_delim(
    toks: Sequence[Token], open_idx: int, open_ch: str, close_ch: str
) -> int:
    """Return index of matching close_ch for open_ch at open_idx, or len(toks) - 1."""
    depth = 0
    for idx in range(open_idx, len(toks)):
        tok = toks[idx]
        if tok.kind == "PUNCT":
            if tok.text == open_ch:
                depth += 1
            elif tok.text == close_ch:
                depth -= 1
                if depth == 0:
                    return idx
    return len(toks) - 1


def _split_top_level_commas(toks: Sequence[Token]) -> List[List[Token]]:
    """Split tokens on top-level commas outside (), [], {}."""
    groups: List[List[Token]] = []
    cur: List[Token] = []
    depth = 0
    for tok in toks:
        if tok.kind == "PUNCT":
            if tok.text in ("(", "[", "{"):
                depth += 1
            elif tok.text in (")", "]", "}"):
                depth = max(0, depth - 1)
            elif tok.text == "," and depth == 0:
                if cur:
                    groups.append(cur)
                    cur = []
                continue
        cur.append(tok)
    if cur:
        groups.append(cur)
    return groups


def is_test_only_cfg_pred(toks: Sequence[Token]) -> bool:
    """True when a cfg(...) predicate can hold only under test or test-support."""
    if not toks:
        return False
    # Single identifier: `test`
    if len(toks) == 1:
        return toks[0].kind == "IDENT" and toks[0].text == "test"
    # Key-value predicate: `feature = "test-support"`
    if (
        len(toks) == 3
        and toks[0].kind == "IDENT"
        and toks[0].text == "feature"
        and toks[1].kind == "PUNCT"
        and toks[1].text == "="
        and toks[2].kind == "STRING"
    ):
        return unquote_rust_string(toks[2]) == "test-support"
    # Compound predicate: `any(...)`, `all(...)`, `not(...)`
    if (
        len(toks) >= 3
        and toks[0].kind == "IDENT"
        and toks[1].kind == "PUNCT"
        and toks[1].text == "("
        and _find_matching_delim(toks, 1, "(", ")") == len(toks) - 1
    ):
        op = toks[0].text
        inner = toks[2:-1]
        if op == "any":
            preds = _split_top_level_commas(inner)
            return bool(preds) and all(is_test_only_cfg_pred(p) for p in preds)
        if op == "all":
            preds = _split_top_level_commas(inner)
            return any(is_test_only_cfg_pred(p) for p in preds)
        if op == "not":
            return False
    return False


def is_test_cfg_attr(attr_body: Sequence[Token]) -> bool:
    """True when the tokens inside `#[ ... ]` form a test-only `cfg(...)` attribute."""
    if (
        len(attr_body) >= 3
        and attr_body[0].kind == "IDENT"
        and attr_body[0].text == "cfg"
        and attr_body[1].kind == "PUNCT"
        and attr_body[1].text == "("
        and _find_matching_delim(attr_body, 1, "(", ")") == len(attr_body) - 1
    ):
        return is_test_only_cfg_pred(attr_body[2:-1])
    return False


def has_test_cfg(attrs: Sequence[Sequence[Token]]) -> bool:
    """True when any attribute body in `attrs` is a test-only `cfg(...)`."""
    return any(is_test_cfg_attr(body) for body in attrs)


def parse_visibility(toks: Sequence[Token], idx: int) -> Tuple[Optional[str], int]:
    """Parse optional Rust visibility (`pub` or `pub(...)`) at `idx`."""
    n = len(toks)
    if idx < n and toks[idx].kind == "IDENT" and toks[idx].text == "pub":
        if idx + 1 < n and toks[idx + 1].kind == "PUNCT" and toks[idx + 1].text == "(":
            close = _find_matching_delim(toks, idx + 1, "(", ")")
            return "pub_restricted", close + 1
        return "pub", idx + 1
    return None, idx


def _parse_outer_attrs(
    toks: Sequence[Token], idx: int
) -> Tuple[List[Tuple[Token, ...]], int]:
    """Consume consecutive outer attributes `#[...]` starting at `idx`."""
    attrs: List[Tuple[Token, ...]] = []
    n = len(toks)
    while (
        idx + 1 < n
        and toks[idx].kind == "PUNCT"
        and toks[idx].text == "#"
        and toks[idx + 1].kind == "PUNCT"
        and toks[idx + 1].text == "["
    ):
        close = _find_matching_delim(toks, idx + 1, "[", "]")
        attrs.append(tuple(toks[idx + 2 : close]))
        idx = close + 1
    return attrs, idx


def flatten_use_tokens(
    toks: Sequence[Token],
    prefix: Tuple[str, ...] = (),
    keep_self: bool = False,
) -> List[Tuple[Tuple[str, ...], Optional[str]]]:
    """Flatten a Rust `use` tree token sequence into `(canonical_segments, alias)` pairs."""
    out: List[Tuple[Tuple[str, ...], Optional[str]]] = []
    for item in _split_top_level_commas(toks):
        if not item:
            continue
        # Check for grouped subtree: `<prefix> { ... }`
        if item[-1].kind == "PUNCT" and item[-1].text == "}":
            # Locate matching `{` for the trailing `}`
            brace_open = None
            depth = 0
            for j in range(len(item) - 1, -1, -1):
                if item[j].kind == "PUNCT":
                    if item[j].text == "}":
                        depth += 1
                    elif item[j].text == "{":
                        depth -= 1
                        if depth == 0:
                            brace_open = j
                            break
            if brace_open is not None:
                head_segs = _extract_path_segments(item[:brace_open])
                out.extend(
                    flatten_use_tokens(
                        item[brace_open + 1 : -1],
                        prefix + head_segs,
                        keep_self=keep_self,
                    )
                )
                continue

        # Leaf item: optional `as <alias>` at end
        alias: Optional[str] = None
        path_part = item
        if (
            len(item) >= 2
            and item[-2].kind == "IDENT"
            and item[-2].text == "as"
            and item[-1].kind in ("IDENT", "RAW_IDENT")
        ):
            cand = canonical_ident(item[-1].text)
            alias = None if cand == "_" else cand
            path_part = item[:-2]

        segs = _extract_path_segments(path_part)
        if not segs:
            continue
        if segs == ("self",) and prefix and not keep_self:
            out.append((prefix, alias))
        else:
            out.append((prefix + segs, alias))
    return out


def _extract_path_segments(toks: Sequence[Token]) -> Tuple[str, ...]:
    """Extract canonical path segments (and `*`) from a `::`-separated token slice."""
    segs: List[str] = []
    for tok in toks:
        if tok.kind in ("IDENT", "RAW_IDENT"):
            segs.append(canonical_ident(tok.text))
        elif tok.kind == "PUNCT" and tok.text == "*":
            segs.append("*")
    return tuple(segs)


def resolve_path(
    segs: Sequence[str], mod_parts: Sequence[str]
) -> Optional[List[str]]:
    """Resolve a crate-relative path (`crate`/`super`/`self`) to module segments, or None."""
    if not segs:
        return None
    head = segs[0]
    if head == "crate":
        return list(segs[1:])
    if head == "super":
        up = 0
        while up < len(segs) and segs[up] == "super":
            up += 1
        if up > len(mod_parts):
            return None
        return list(mod_parts[: len(mod_parts) - up]) + list(segs[up:])
    if head == "self":
        return list(mod_parts) + list(segs[1:])
    return None


def resolve_super_text(src: str, mod_parts: Sequence[str]) -> str:
    """Strip comments/strings and rewrite standalone `super::` prefixes to `crate::...`.

    Line numbers are preserved so regex bans over the returned text report the
    original file lines.
    """
    toks = tokenize(src)
    out: List[str] = []
    i = 0
    n = len(toks)
    prev_code: Optional[Token] = None

    while i < n:
        tok = toks[i]
        if tok.kind in ("COMMENT", "STRING", "CHAR"):
            out.append(_blank_preserving_newlines(tok.text))
            i += 1
            continue
        if tok.kind == "WS":
            out.append(tok.text)
            i += 1
            continue

        # Check for start of a `super::` chain not preceded by `::` or `.`
        if (
            tok.kind == "IDENT"
            and tok.text == "super"
            and (
                prev_code is None
                or not (prev_code.kind == "PUNCT" and prev_code.text in ("::", "."))
            )
        ):
            # Look ahead for one or more `super ::` pairs (allowing WS/COMMENT between)
            j = i
            up = 0
            ws_between: List[str] = []
            while True:
                if j >= n or not (toks[j].kind == "IDENT" and toks[j].text == "super"):
                    break
                k = j + 1
                mid: List[str] = []
                while k < n and toks[k].kind in ("WS", "COMMENT"):
                    mid.append(
                        _blank_preserving_newlines(toks[k].text)
                        if toks[k].kind == "COMMENT"
                        else toks[k].text
                    )
                    k += 1
                if k < n and toks[k].kind == "PUNCT" and toks[k].text == "::":
                    up += 1
                    ws_between.extend(mid)
                    j = k + 1
                    # Consume trailing WS/COMMENT before checking another `super`
                    k2 = j
                    mid2: List[str] = []
                    while k2 < n and toks[k2].kind in ("WS", "COMMENT"):
                        mid2.append(
                            _blank_preserving_newlines(toks[k2].text)
                            if toks[k2].kind == "COMMENT"
                            else toks[k2].text
                        )
                        k2 += 1
                    if (
                        k2 < n
                        and toks[k2].kind == "IDENT"
                        and toks[k2].text == "super"
                    ):
                        ws_between.extend(mid2)
                        j = k2
                        continue
                    break
                break
            if up > 0:
                keep = max(0, len(mod_parts) - up)
                base = "::".join(mod_parts[:keep])
                rewritten = "crate::" if not base else f"crate::{base}::"
                # Preserve any newlines that occurred inside the replaced super:: chain
                newlines = "".join("\n" for s in ws_between for ch in s if ch == "\n")
                out.append(newlines + rewritten)
                prev_code = toks[j - 1]
                i = j
                continue

        out.append(tok.text)
        prev_code = tok
        i += 1

    return "".join(out)


def is_facade_body(body_code_toks: Sequence[Token]) -> bool:
    """True when an inline module body holds only bare `pub use` statements."""
    n = len(body_code_toks)
    if n == 0:
        return False
    idx = 0
    count = 0
    while idx < n:
        _attrs, idx = _parse_outer_attrs(body_code_toks, idx)
        vis, idx = parse_visibility(body_code_toks, idx)
        if vis != "pub":
            return False
        if idx >= n or not (
            body_code_toks[idx].kind == "IDENT" and body_code_toks[idx].text == "use"
        ):
            return False
        idx += 1
        semi = None
        brace_depth = 0
        while idx < n:
            t = body_code_toks[idx]
            if t.kind == "PUNCT":
                if t.text == "{":
                    brace_depth += 1
                elif t.text == "}":
                    brace_depth = max(0, brace_depth - 1)
                elif t.text == ";" and brace_depth == 0:
                    semi = idx
                    idx += 1
                    break
            idx += 1
        if semi is None:
            return False
        count += 1
    return count > 0


class _Frame(NamedTuple):
    is_mod: bool
    mod_path: Tuple[str, ...]
    is_test_only: bool
    mod_name: Optional[str]
    start_char: int
    body_tok_start: int
    vis: Optional[str]
    line: int


def scan_file(src: str, base_mod: Sequence[str] = ()) -> ScannedFile:
    """Parse modules, `use` statements, inline paths, and item defs in one pass."""
    tokens = tuple(tokenize(src))
    code = tuple(t for t in tokens if t.kind not in ("WS", "COMMENT"))
    n = len(code)
    base_tuple = tuple(base_mod)

    inline_mods: List[InlineMod] = []
    file_mods: List[FileMod] = []
    use_stmts: List[UseStmt] = []
    inline_paths: List[InlinePath] = []
    item_defs: Set[str] = set()

    stack: List[_Frame] = []
    paren_depth = 0
    bracket_depth = 0
    at_item_start = True
    pending_test_cfg = False
    i = 0

    def current_mod_path() -> Tuple[str, ...]:
        return stack[-1].mod_path if stack else base_tuple

    def current_test_only() -> bool:
        return (bool(stack) and stack[-1].is_test_only) or pending_test_cfg

    def at_module_scope() -> bool:
        return all(frame.is_mod for frame in stack)

    while i < n:
        tok = code[i]

        # Inner attribute `#![...]` (does not attach to the following item)
        if (
            paren_depth == 0
            and bracket_depth == 0
            and tok.kind == "PUNCT"
            and tok.text == "#"
            and i + 2 < n
            and code[i + 1].kind == "PUNCT"
            and code[i + 1].text == "!"
            and code[i + 2].kind == "PUNCT"
            and code[i + 2].text == "["
        ):
            close = _find_matching_delim(code, i + 2, "[", "]")
            i = close + 1
            continue

        # Outer attributes `#[...]` at item/statement start
        if (
            at_item_start
            and paren_depth == 0
            and bracket_depth == 0
            and tok.kind == "PUNCT"
            and tok.text == "#"
            and i + 1 < n
            and code[i + 1].kind == "PUNCT"
            and code[i + 1].text == "["
        ):
            attrs, next_i = _parse_outer_attrs(code, i)
            if has_test_cfg(attrs):
                pending_test_cfg = True
            i = next_i
            continue

        # Check for `[vis] use ...;` or `[vis] mod <name> ({|;)` at item start
        if at_item_start and paren_depth == 0 and bracket_depth == 0:
            vis, after_vis = parse_visibility(code, i)
            if after_vis < n and code[after_vis].kind == "IDENT":
                kw = code[after_vis].text
                # `use <tree>;`
                if kw == "use":
                    stmt_start_char = code[i].start
                    stmt_line = code[after_vis].line
                    stmt_test_only = current_test_only()
                    tree_start = after_vis + 1
                    j = tree_start
                    use_brace = 0
                    while j < n:
                        t = code[j]
                        if t.kind == "PUNCT":
                            if t.text == "{":
                                use_brace += 1
                            elif t.text == "}":
                                use_brace = max(0, use_brace - 1)
                            elif t.text == ";" and use_brace == 0:
                                break
                        j += 1
                    if j < n:
                        tree_toks = code[tree_start:j]
                        paths = tuple(flatten_use_tokens(tree_toks, keep_self=False))
                        paths_keep = tuple(flatten_use_tokens(tree_toks, keep_self=True))
                        use_stmts.append(
                            UseStmt(
                                mod_path=current_mod_path(),
                                is_test_only=stmt_test_only,
                                brace_depth=len(stack),
                                vis=vis,
                                line=stmt_line,
                                start_char=stmt_start_char,
                                end_char=code[j].end,
                                tree_toks=tree_toks,
                                paths=paths,
                                paths_keep_self=paths_keep,
                            )
                        )
                        i = j + 1
                        pending_test_cfg = False
                        at_item_start = True
                        continue

                # `mod <IDENT|RAW_IDENT> {` or `mod <IDENT|RAW_IDENT> ;`
                if (
                    kw == "mod"
                    and after_vis + 2 < n
                    and code[after_vis + 1].kind in ("IDENT", "RAW_IDENT")
                    and code[after_vis + 2].kind == "PUNCT"
                    and code[after_vis + 2].text in ("{", ";")
                ):
                    name_tok = code[after_vis + 1]
                    mod_name = canonical_ident(name_tok.text)
                    delim = code[after_vis + 2]
                    mod_test_only = current_test_only()
                    parent_path = current_mod_path()
                    if not stack and not mod_test_only:
                        item_defs.add(mod_name)
                    if delim.text == ";":
                        if at_module_scope():
                            file_mods.append(
                                FileMod(
                                    name=mod_name,
                                    path=parent_path + (mod_name,),
                                    is_test_only=mod_test_only,
                                    brace_depth=len(stack),
                                    vis=vis,
                                    line=name_tok.line,
                                )
                            )
                        i = after_vis + 3
                        pending_test_cfg = False
                        at_item_start = True
                        continue
                    # Inline module `mod <name> {`
                    is_mod_unit = at_module_scope()
                    new_path = parent_path + (mod_name,) if is_mod_unit else parent_path
                    stack.append(
                        _Frame(
                            is_mod=is_mod_unit,
                            mod_path=new_path,
                            is_test_only=mod_test_only,
                            mod_name=mod_name if is_mod_unit else None,
                            start_char=delim.start,
                            body_tok_start=after_vis + 3,
                            vis=vis,
                            line=name_tok.line,
                        )
                    )
                    i = after_vis + 3
                    pending_test_cfg = False
                    at_item_start = True
                    continue

                # Top-level item definitions (for facade resolution)
                if not stack and not current_test_only():
                    if kw in ITEM_DEF_KEYWORDS and after_vis + 1 < n:
                        nxt = code[after_vis + 1]
                        if nxt.kind in ("IDENT", "RAW_IDENT"):
                            item_defs.add(canonical_ident(nxt.text))
                    elif (
                        kw == "macro_rules"
                        and after_vis + 2 < n
                        and code[after_vis + 1].kind == "PUNCT"
                        and code[after_vis + 1].text == "!"
                        and code[after_vis + 2].kind in ("IDENT", "RAW_IDENT")
                    ):
                        item_defs.add(canonical_ident(code[after_vis + 2].text))

        # Track parens/brackets/braces and statement boundaries
        if tok.kind == "PUNCT":
            if tok.text == "(":
                paren_depth += 1
                at_item_start = False
            elif tok.text == ")":
                paren_depth = max(0, paren_depth - 1)
                at_item_start = False
            elif tok.text == "[":
                bracket_depth += 1
                at_item_start = False
            elif tok.text == "]":
                bracket_depth = max(0, bracket_depth - 1)
                at_item_start = False
            elif tok.text == "{":
                stack.append(
                    _Frame(
                        is_mod=False,
                        mod_path=current_mod_path(),
                        is_test_only=current_test_only(),
                        mod_name=None,
                        start_char=tok.start,
                        body_tok_start=i + 1,
                        vis=None,
                        line=tok.line,
                    )
                )
                pending_test_cfg = False
                at_item_start = True
            elif tok.text == "}":
                if stack:
                    frame = stack.pop()
                    if frame.is_mod and frame.mod_name is not None:
                        inline_mods.append(
                            InlineMod(
                                name=frame.mod_name,
                                path=frame.mod_path,
                                is_test_only=frame.is_test_only,
                                start_char=frame.start_char,
                                end_char=tok.start,
                                body_tok_start=frame.body_tok_start,
                                body_tok_end=i,
                                vis=frame.vis,
                                line=frame.line,
                            )
                        )
                pending_test_cfg = False
                at_item_start = paren_depth == 0 and bracket_depth == 0
            elif tok.text == ";" and paren_depth == 0 and bracket_depth == 0:
                pending_test_cfg = False
                at_item_start = True
            else:
                at_item_start = False
            i += 1
            continue

        # Inline paths outside `use` statements: crate::..., super::..., self::..., std::...
        if (
            tok.kind == "IDENT"
            and tok.text in ("crate", "super", "self", "std")
            and i + 2 < n
            and code[i + 1].kind == "PUNCT"
            and code[i + 1].text == "::"
            and code[i + 2].kind in ("IDENT", "RAW_IDENT")
        ):
            # Reject mid-path segments like `foo::self::bar`, allow leading `::std::...`
            preceded_by_path = (
                i >= 1
                and code[i - 1].kind == "PUNCT"
                and code[i - 1].text == "::"
                and (
                    i >= 2
                    and (
                        code[i - 2].kind in ("IDENT", "RAW_IDENT")
                        or (code[i - 2].kind == "PUNCT" and code[i - 2].text == ">")
                    )
                )
            )
            if not preceded_by_path:
                segs: List[str] = [tok.text]
                j = i + 1
                while (
                    j + 1 < n
                    and code[j].kind == "PUNCT"
                    and code[j].text == "::"
                    and code[j + 1].kind in ("IDENT", "RAW_IDENT")
                ):
                    segs.append(canonical_ident(code[j + 1].text))
                    j += 2
                inline_paths.append(
                    InlinePath(
                        mod_path=current_mod_path(),
                        is_test_only=current_test_only(),
                        segments=tuple(segs),
                        line=tok.line,
                        start_char=tok.start,
                        end_char=code[j - 1].end,
                    )
                )
                at_item_start = False
                i = j
                continue

        # Also catch `unsafe fn`, `async fn`, `const fn`, `extern "C" fn` top-level defs
        if (
            not stack
            and not current_test_only()
            and tok.kind == "IDENT"
            and tok.text in ITEM_DEF_KEYWORDS
            and i + 1 < n
            and code[i + 1].kind in ("IDENT", "RAW_IDENT")
        ):
            item_defs.add(canonical_ident(code[i + 1].text))

        at_item_start = False
        i += 1

    return ScannedFile(
        tokens=tokens,
        code=code,
        inline_mods=tuple(inline_mods),
        file_mods=tuple(file_mods),
        use_stmts=tuple(use_stmts),
        inline_paths=tuple(inline_paths),
        item_defs=frozenset(item_defs),
    )
