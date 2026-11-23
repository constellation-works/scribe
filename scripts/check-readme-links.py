#!/usr/bin/env python3
"""scripts/check-readme-links.py [root] — STD-09 R12: every relative link,
heading anchor and image path in README.md resolves on the checked-out tree.

Checked: Markdown links and images, reference definitions, and the href, src
and srcset attributes of HTML tags. A fragment is checked against the README's
own headings and anchors, or against the headings of the Markdown file it names.
Not checked: URLs with a scheme or host (http, https, mailto, ...), which need
the network.

Paths must match the case on disk. A name that resolves only on a
case-insensitive filesystem (macOS) fails here, as it would on the Linux runner.

`root` defaults to the repository; scripts/tests/check-gates.sh points it at
fixture trees.
"""

from html import unescape
import os
from pathlib import Path, PurePosixPath
import re
import sys
import unicodedata
from urllib.parse import unquote, urlsplit


CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`)(.+?)(?<!`)\1(?!`)", re.DOTALL)
# A backtick fence line carries no backtick after it: "```x```" is a code span.
FENCE_OPEN = re.compile(r"^\s*(`{3,}(?=[^`]*$)|~{3,})")
LINK = re.compile(
    r"!?\[(?:[^\[\]\\]|\\.|\[[^\[\]]*\])*\]"
    r"\(\s*(?:<(?P<angle>[^>\n]+)>|"
    r"(?P<plain>(?:[^\s()\\]|\\.|\([^()]*\))+))"
    r"(?:\s+(?:\"[^\"]*\"|'[^']*'|\([^()]*\)))?\s*\)"
)
REFERENCE = re.compile(r"^ {0,3}\[([^]\n]+)\]:\s*(?:<([^>\n]+)>|(\S+))", re.MULTILINE)
# A tag runs to its closing `>`, which a quoted attribute value may contain.
TAG = re.compile(r"""<[A-Za-z](?:"[^"]*"|'[^']*'|[^'">])*>""")
# Attributes are read in order, so a quoted value is never taken for an attribute name.
ATTRIBUTE = re.compile(
    r"""\s([A-Za-z_:][-A-Za-z0-9_:.]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"""
)
LINKED_ATTRIBUTES = ("href", "src", "srcset")
NAMED_ANCHOR = re.compile(r"""<[A-Za-z][^>]*?\s(?:id|name)\s*=\s*["']([^"']+)["']""", re.IGNORECASE)
ATX_HEADING = re.compile(r"^ {0,3}#{1,6}(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$")
SETEXT_UNDERLINE = re.compile(r" {0,3}(?:=+|-+)[ \t]*")


def blank(match):
    """Replace a match with spaces, keeping its newlines so offsets and line numbers hold."""
    return re.sub(r"[^\n]", " ", match.group())


def mask(text):
    """Blank HTML comments and fenced code blocks, which are not live links."""
    text = re.sub(r"<!--.*?-->", blank, text, flags=re.DOTALL)
    lines = text.splitlines(keepends=True)
    fence = None
    for index, line in enumerate(lines):
        if fence:
            lines[index] = re.sub(r"[^\n]", " ", line)
            if re.match(r"^\s*" + re.escape(fence[0]) + "{" + str(fence[1]) + r",}\s*$", line):
                fence = None
        elif FENCE_OPEN.match(line):
            marker = FENCE_OPEN.match(line)[1]
            fence = (marker[0], len(marker))
            lines[index] = re.sub(r"[^\n]", " ", line)
    return "".join(lines)


def slug_of(heading):
    """The GitHub anchor for a heading: rendered text, lowercased, spaces to hyphens."""
    text = heading.replace("`", "")
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]*>", "", text)
    text = re.sub(r"(?<!\w)(_+)(.+?)\1(?!\w)", r"\2", text)
    text = unescape(text.replace("*", "").replace("~", "")).strip().lower()
    kept = "".join(char for char in text
                   if char in " -_" or unicodedata.category(char)[0] in "LNM")
    return kept.replace(" ", "-")


def anchors(text):
    """Every anchor a fragment may name: heading slugs, deduplicated the way GitHub does,
    plus explicit id and name attributes."""
    masked = mask(text)
    lines = masked.splitlines()
    result = set()
    for index, line in enumerate(lines):
        heading = None
        atx = ATX_HEADING.match(line)
        if atx:
            heading = atx[1] or ""
        elif (index and SETEXT_UNDERLINE.fullmatch(line) and lines[index - 1].strip()
              and not ATX_HEADING.match(lines[index - 1])):
            heading = lines[index - 1].strip()
        if heading is None:
            continue
        base = slug_of(heading)
        candidate, suffix = base, 0
        while candidate in result:
            suffix += 1
            candidate = f"{base}-{suffix}"
        result.add(candidate)
    result.update(NAMED_ANCHOR.findall(masked))
    return result


def exact_path(root, relative):
    """The path under root that `relative` names exactly, or None when a component is
    missing, differs in case, or leaves the root."""
    parts = PurePosixPath(os.path.normpath(relative)).parts
    if ".." in parts:
        return None
    current = root
    for part in parts:
        try:
            names = os.listdir(current)
        except OSError:
            return None
        if part not in names:
            return None
        current = current / part
    return current


def destinations(text):
    """(offset, destination) for every link, image, reference definition and HTML
    href, src or srcset in the masked text."""
    code_free = CODE_SPAN.sub(blank, text)
    code_free = re.sub(r"\\\[", "  ", code_free)
    found = []
    for match in LINK.finditer(code_free):
        found.append((match.start(), match["angle"] or match["plain"]))
    for match in REFERENCE.finditer(code_free):
        found.append((match.start(), match[2] or match[3]))
    for tag in TAG.finditer(code_free):
        for attribute in ATTRIBUTE.finditer(tag.group()):
            name = attribute[1].lower()
            value = next((group for group in attribute.groups()[1:] if group is not None), None)
            if name not in LINKED_ATTRIBUTES or value is None:
                continue
            if name == "srcset":
                # Candidates are comma-separated; each is a URL then an optional descriptor.
                for candidate in value.split(","):
                    tokens = candidate.split()
                    if tokens:
                        found.append((tag.start(), tokens[0]))
            else:
                found.append((tag.start(), value))
    return found


def check_destination(root, document, destination):
    """Return (error, internal). `error` is a message when the destination does not resolve,
    else None; `internal` is False for an external URL, which is not checked."""
    destination = unescape(re.sub(r"\\(.)", r"\1", destination.strip()))
    url = urlsplit(destination)
    if url.scheme or url.netloc:
        return None, False
    path = unquote(url.path)
    if path:
        target = exact_path(root, path.lstrip("/"))
        if target is None:
            return f"missing link target: {destination}", True
    else:
        target = document
    if url.fragment and target.is_file() and target.suffix.lower() in (".md", ".markdown"):
        if unquote(url.fragment) not in anchors(target.read_text(encoding="utf-8")):
            return f"missing heading anchor: {destination}", True
    return None, True


def check(root):
    document = root / "README.md"
    if not document.is_file():
        print("readme-links: README.md is missing", file=sys.stderr)
        return 1
    text = document.read_text(encoding="utf-8")
    masked = mask(text)
    errors = []
    checked = 0
    for offset, destination in destinations(masked):
        error, internal = check_destination(root, document, destination)
        if internal:
            checked += 1
        if error:
            line = masked.count("\n", 0, offset) + 1
            errors.append(f"README.md:{line}: {error}")
    for error in errors:
        print(error, file=sys.stderr)
    if errors:
        print(f"readme-links: {len(errors)} broken references in README.md (add or fix the "
              "path, anchor or case; STD-09 R12)", file=sys.stderr)
        return 1
    print(f"readme-links: ok ({checked} relative references)")
    return 0


if __name__ == "__main__":
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent
    sys.exit(check(root.resolve()))
