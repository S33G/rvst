#!/usr/bin/env python3
"""Check local Markdown links, heading fragments and fences without network access.

A focused checker, not a full CommonMark parser: handles inline links/images,
reference definitions, ATX/setext headings, fenced/indented code and code spans.
HTML links, explicit HTML anchors and remote destinations are outside its scope.
"""
import argparse
import html
import re
from pathlib import Path, PureWindowsPath
from urllib.parse import unquote

FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")
CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`)(.*?)\1(?!`)", re.DOTALL)
SCHEME = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")


def destination(text):
    """Read an angle destination or one with balanced/escaped parentheses."""
    if text.startswith("<"):
        end = text.find(">")
        return text[1:end] if end >= 0 else ""
    depth, end = 0, 0
    while end < len(text):
        char = text[end]
        if char == "\\" and end + 1 < len(text):
            end += 2
            continue
        if char.isspace() or (char == ")" and depth == 0):
            break
        depth += (char == "(") - (char == ")")
        end += 1
    return re.sub(r"\\([!\"#$%&'()*+,\-./:;<=>?@\[\]\\^_`{|}~])", r"\1", text[:end])


def document(path):
    visible, headings, links, errors = [], set(), [], []
    fence, opened, previous = "", 0, ""
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        marker = FENCE.match(line)
        if fence:
            if marker and marker[1][0] == fence[0] and len(marker[1]) >= len(fence) and not marker[2].strip():
                fence = ""
            visible.append("")
            previous = ""
            continue
        if marker and (marker[1][0] == "~" or "`" not in marker[2]):
            fence, opened = marker[1], number
            visible.append("")
            previous = ""
            continue
        if line.startswith(("    ", "\t")):
            visible.append("")
            previous = ""
            continue
        visible.append(line)
        atx = re.match(r"^ {0,3}#{1,6}(?:\s+(.*?)|\s*)$", line)
        title = re.sub(r"\s+#+\s*$", "", atx[1] or "") if atx else None
        if previous.strip() and re.fullmatch(r" {0,3}(?:=+|-+)\s*", line):
            title = previous.strip()
        if title is not None:
            title = re.sub(r"\[([^]]+)\]\([^)]*\)", r"\1", title)
            title = re.sub(r"<[^>]*>", "", title)
            slug = re.sub(r"[^\w\- ]", "", html.unescape(title).lower()).replace(" ", "-")
            candidate, suffix = slug, 0
            while candidate in headings:
                suffix += 1
                candidate = f"{slug}-{suffix}"
            headings.add(candidate)
        previous = line
    if fence:
        errors.append((opened, f"unclosed {fence} fence"))
    prose = CODE_SPAN.sub(lambda m: re.sub(r"[^\n]", " ", m[0]), "\n".join(visible))
    prose_lines = prose.splitlines()
    for number, line in enumerate(prose_lines, 1):
        reference = re.match(r"^ {0,3}\[[^]]+\]:\s*(.*)", line)
        if reference:
            links.append((number, destination(reference[1])))
            prose_lines[number - 1] = ""
    prose = "\n".join(prose_lines)
    for match in re.finditer(r"(?<!\\)\]\(\s*", prose):
        number = prose.count("\n", 0, match.start()) + 1
        links.append((number, destination(prose[match.end():])))
    return headings, links, errors


def check(root, paths):
    cache, errors, count = {}, [], 0

    def read(path):
        if path not in cache:
            cache[path] = document(path)
        return cache[path]

    def report(path, line, message):
        label = path.relative_to(root) if path.is_relative_to(root) else path
        errors.append(f"{label}:{line}: {message}")

    for path in paths:
        if not path.is_relative_to(root):
            report(path, 1, "selected path leaves repository")
            continue
        try:
            _, links, fence_errors = read(path)
        except (OSError, UnicodeError) as error:
            report(path, 1, f"cannot read Markdown: {error}")
            continue
        for line, message in fence_errors:
            report(path, line, message)
        for line, raw in links:
            raw = html.unescape(raw)
            if raw.startswith("//"):
                continue
            if PureWindowsPath(unquote(raw)).is_absolute():
                count += 1
                report(path, line, f"local target leaves repository: {raw}")
                continue
            if SCHEME.match(raw):
                continue
            count += 1
            address, _, fragment = raw.partition("#")
            address = unquote(address.partition("?")[0])
            fragment = unquote(fragment)
            target = (path.parent / address).resolve() if address else path
            if Path(address).is_absolute() or not target.is_relative_to(root):
                report(path, line, f"local target leaves repository: {raw}")
            elif not target.exists():
                report(path, line, f"missing local target: {raw}")
            elif fragment and target.suffix.lower() == ".md" and target.is_file():
                try:
                    if fragment not in read(target)[0]:
                        report(path, line, f"missing heading #{fragment} in {target.relative_to(root)}")
                except (OSError, UnicodeError) as error:
                    report(path, line, f"cannot read target {raw}: {error}")
    for error in errors:
        print(error)
    print(f"Checked {len(paths)} Markdown files and {count} local links: {len(errors)} errors.")
    return bool(errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="*", help="Markdown files or directories relative to the repository")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1], help="repository root")
    args = parser.parse_args()
    root = args.root.resolve()
    if args.paths:
        paths = set()
        for item in args.paths:
            path = (root / item).resolve()
            paths.update(path.rglob("*.md") if path.is_dir() else [path])
    else:
        paths = set(root.glob("*.md")) | set((root / "docs").rglob("*.md"))
    return check(root, sorted(path.resolve() for path in paths))


if __name__ == "__main__":
    raise SystemExit(main())
