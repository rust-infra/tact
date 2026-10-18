#!/usr/bin/env python3
"""Render a book chapter (or an HTML fragment) as the house standalone page.

`book/*.md` is the source of truth; the `.html` twin is a shareable page that
keeps the chapter's mermaid diagrams alive in a browser. Re-run this after
editing the Markdown — the page is generated, never hand-edited:

    python3 scripts/book-chapter-html.py book/28_chapter_goal_zh.md \
        book/28_chapter_goal_zh.html --title "Agent Goal：持久化目标执行"

    python3 scripts/book-chapter-html.py docs/design/x.fragment.html \
        docs/design/x.html --title "..."

A non-Markdown source is spliced into the same shell as-is (the file name is
what tells the two modes apart).

Two things happen beyond `pandoc -s`:

* Mermaid fences are rewritten to `<pre class="mermaid">TEXT</pre>`. Pandoc
  emits `<pre class="mermaid"><code>TEXT</code></pre>`, and the extra `<code>`
  node only sits between mermaid and the diagram source it reads. Quotes are
  unescaped back to `"` for the same reason the diagram source is not markup.
* The page gets the house stylesheet and the mermaid initializer declared
  below, so every generated page is styled the same way.
"""
from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys
import tempfile

# House head: applied through pandoc's `--include-in-header`, after pandoc's own
# default stylesheet. Keep in sync with the pages already checked in.
HEAD = """<style>
  :root { color-scheme: light; }
  body {
    max-width: 900px; margin: 0 auto; padding: 40px 24px 120px;
    font: 16px/1.75 -apple-system, "SF Pro Text", "PingFang SC", "Hiragino Sans GB", sans-serif;
    color: #1f2328;
  }
  h1 { font-size: 30px; border-bottom: 2px solid #e4e6ea; padding-bottom: 12px; }
  h2 { font-size: 23px; margin-top: 44px; border-bottom: 1px solid #eceef1; padding-bottom: 8px; }
  h3 { font-size: 18px; margin-top: 32px; }
  code { background: #f4f5f7; padding: 2px 6px; border-radius: 4px; font-size: 13.5px; }
  pre:not(.mermaid) { background: #f6f7f9; padding: 14px 16px; border-radius: 8px; overflow-x: auto; }
  pre:not(.mermaid) code { background: none; padding: 0; }
  blockquote {
    margin: 18px 0; padding: 10px 18px; border-left: 4px solid #c9d4e6;
    background: #f7f9fc; border-radius: 0 8px 8px 0; color: #3d4653;
  }
  table { border-collapse: collapse; margin: 18px 0; width: 100%; font-size: 15px; }
  th, td { border: 1px solid #e0e3e8; padding: 8px 12px; text-align: left; vertical-align: top; }
  th { background: #f6f7f9; }
  hr { border: none; border-top: 1px solid #e4e6ea; margin: 40px 0; }
  .mermaid { margin: 24px 0; text-align: center; }
  .lab { background: #fffdf5; border: 1px dashed #d8c07a; border-radius: 10px; padding: 8px 22px 22px; }
  .lab h2 { border: none; }
</style>
<script type="module">
  import mermaid from "https://cdn.jsdelivr.net/npm/mermaid@11/dist/mermaid.esm.min.mjs";
  mermaid.initialize({
    startOnLoad: true,
    securityLevel: "loose",
    theme: "base",
    fontFamily: '-apple-system, "SF Pro Text", "PingFang SC", sans-serif',
    themeVariables: {
      fontSize: "15px",
      lineColor: "#8a93a3",
      primaryTextColor: "#1f2328",
      edgeLabelBackground: "#ffffff",
    },
    flowchart: { curve: "basis", nodeSpacing: 46, rankSpacing: 58, padding: 12 },
    state: { nodeSpacing: 40, rankSpacing: 52 },
  });
</script>
"""

MERMAID = re.compile(r'<pre class="mermaid"><code>(.*?)</code></pre>', re.S)
FENCE = re.compile(r"^```mermaid\s*$", re.M)


def pandoc(*args: str, stdin: str = "") -> str:
    try:
        finished = subprocess.run(
            ["pandoc", *args],
            input=stdin,
            capture_output=True,
            text=True,
            check=True,
        )
    except FileNotFoundError:
        sys.exit("pandoc not found; install it (brew install pandoc)")
    except subprocess.CalledProcessError as exc:
        sys.exit(f"pandoc failed:\n{exc.stderr.strip()}")
    return finished.stdout


def unwrap_mermaid(match: re.Match[str]) -> str:
    body = match.group(1).replace("&quot;", '"').replace("&#39;", "'")
    return f'<pre class="mermaid">{body}</pre>'


def render(source: pathlib.Path, title: str) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        head = pathlib.Path(tmp) / "head.html"
        head.write_text(HEAD, encoding="utf-8")
        common = [
            "-f",
            "gfm",
            "-t",
            "html5",
            "-s",
            "--syntax-highlighting=none",
            "--metadata",
            f"pagetitle={title}",
            "-H",
            str(head),
        ]
        if source.suffix.lower() in {".md", ".markdown"}:
            document = pandoc(str(source), *common)
        else:
            # A fragment has no Markdown to convert: borrow pandoc's empty page
            # so both modes share one shell, then splice the fragment in.
            document = pandoc(*common).replace(
                "</body>", source.read_text(encoding="utf-8") + "\n</body>"
            )
    document, count = MERMAID.subn(unwrap_mermaid, document)
    if source.suffix.lower() in {".md", ".markdown"}:
        fences = len(FENCE.findall(source.read_text(encoding="utf-8")))
        if count != fences:
            sys.exit(
                f"{source}: {fences} mermaid fences but {count} rendered blocks"
            )
    return document


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("source", type=pathlib.Path, help="chapter .md or an HTML fragment")
    parser.add_argument("destination", type=pathlib.Path, help="output .html")
    parser.add_argument(
        "--title",
        help="page title; defaults to the source file name",
    )
    args = parser.parse_args()

    if not args.source.is_file():
        sys.exit(f"no such source: {args.source}")
    title = args.title or args.source.stem.replace("_", " ").replace("-", " ")
    document = render(args.source, title)
    args.destination.parent.mkdir(parents=True, exist_ok=True)
    args.destination.write_text(document, encoding="utf-8")
    print(f"{args.destination} ({len(document)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
