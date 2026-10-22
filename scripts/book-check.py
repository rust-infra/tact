#!/usr/bin/env python3
"""Check the book pages: links, anchors, and sidebar coverage.

    python3 scripts/book-check.py book

`book/*.html` is hand-edited source; `scripts/book-chapter-html.py --refresh`
only regenerates the sidebar shell. This verifies the result without a browser:

* every relative `href` points at a file that exists;
* every `#fragment` matches an `id` in the page it is used on;
* every page has the sidebar directory and a `<main>` wrapper;
* every `h2`/`h3` inside `<main>` has a sidebar entry (re-run
  `book-chapter-html.py --refresh` after adding a section);
* no page still links to a sibling `.md` file (the book is HTML-only now).

Exits non-zero with one line per problem.
"""
from __future__ import annotations

import argparse
import pathlib
import re
import sys
from urllib.parse import unquote

HREF = re.compile(r'href="([^"]+)"')
ID = re.compile(r'\bid="([^"]+)"')
MAIN = re.compile(r"<main>(.*?)</main>", re.S)
HEADING = re.compile(r"<h([23])\b([^>]*)>(.*?)</h\1>", re.S | re.I)
NAV_HREF = re.compile(r'<nav class="book-toc".*?</nav>', re.S)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("directory", nargs="?", type=pathlib.Path, default=pathlib.Path("book"))
    args = parser.parse_args()

    pages = sorted(args.directory.glob("*.html"))
    if not pages:
        sys.exit(f"no .html pages in {args.directory}")

    problems: list[str] = []
    links = anchors = sections = 0

    for page in pages:
        text = page.read_text(encoding="utf-8")
        ids = set(ID.findall(text))
        if 'class="book-toc"' not in text:
            problems.append(f"{page.name}: no sidebar directory")
        main = MAIN.search(text)
        nav = NAV_HREF.search(text)
        if main is None:
            problems.append(f"{page.name}: no <main> wrapper")
        elif nav is not None:
            indexed = {
                unquote(href[1:])
                for href in HREF.findall(nav.group(0))
                if href.startswith("#")
            }
            for match in HEADING.finditer(main.group(1)):
                id_match = ID.search(match.group(2))
                if not id_match:
                    problems.append(
                        f"{page.name}: heading without id: {match.group(3)[:40]}"
                    )
                    continue
                # `--refresh` deliberately leaves a heading titled 目录 out of
                # the sidebar (the index page's own chapter list covers it).
                if " ".join(match.group(3).split()) == "目录":
                    continue
                sections += 1
                if id_match.group(1) not in indexed:
                    problems.append(
                        f"{page.name}: section #{id_match.group(1)} missing from "
                        "the sidebar (re-run --refresh)"
                    )
        for href in HREF.findall(text):
            if href.startswith(("http://", "https://", "mailto:")):
                continue
            path, _, fragment = href.partition("#")
            if not path:
                anchors += 1
                if unquote(fragment) not in ids:
                    problems.append(f"{page.name}: #{fragment} has no matching id")
                continue
            links += 1
            target = page.parent / unquote(path)
            if not target.exists():
                problems.append(f"{page.name}: {href} -> missing file")
            elif path.endswith(".md") and target.parent == page.parent:
                problems.append(
                    f"{page.name}: {href} still points at Markdown; "
                    "expected the .html twin"
                )

    for problem in problems:
        print(problem)
    print(
        f"{len(pages)} pages, {links} file links, {anchors} anchors, "
        f"{sections} sections, {len(problems)} problem(s)"
    )
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
