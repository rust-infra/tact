#!/usr/bin/env python3
"""Verify the goal-lifecycle animation without eyes: DOM state + frame pixels.

Part 1 (DOM): re-render each step in headless chrome and dump the DOM, asserting the
reveal state machine actually reached the intended state (is-on / is-lit counts grow
monotonically, data-step matches, label + detail text are present).

Part 2 (pixels): decode each PNG to raw RGB (numpy) and check the things a human
looking at the frame would check -- themed background actually resolved, text/cards
rendered, consecutive steps visibly differ, accents present, content bottom for
letterboxing.
"""
from __future__ import annotations

import html
import json
import pathlib
import re
import shutil
import subprocess
import sys

import numpy as np

ROOT = pathlib.Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs/design/goal-lifecycle.html"
FRAMES = ROOT / ".scratch/frames-lifecycle"
SCRATCH = ROOT / ".scratch"
STEPS = 10


def find_chrome() -> str:
    """Same resolution order as the skill's capture_frames.py: shell, playwright cache, app."""
    for name in ("chrome-headless-shell", "chromium-headless-shell"):
        found = shutil.which(name)
        if found:
            return found
    cache = pathlib.Path.home() / "Library/Caches/ms-playwright"
    if cache.is_dir():
        shells = sorted(cache.glob(
            "chromium_headless_shell-*/chrome-headless-shell-*/chrome-headless-shell"), reverse=True)
        if shells:
            return str(shells[0])
    for candidate in ("/Applications/Chromium.app/Contents/MacOS/Chromium",
                      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                      "/usr/bin/chromium", "/usr/bin/google-chrome"):
        if pathlib.Path(candidate).exists():
            return candidate
    sys.exit("no headless Chrome found")


CHROME = find_chrome()

failures: list[str] = []


def check(cond: bool, msg: str) -> None:
    print(("  ok   " if cond else "  FAIL ") + msg)
    if not cond:
        failures.append(msg)


def inner_document() -> str:
    match = re.search(r'data-srcdoc="(.*?)"\s*>', PAGE.read_text(), re.S)
    if not match:
        sys.exit("could not extract inner document from export")
    return html.unescape(match.group(1))


def dom_for(page: pathlib.Path, step: int) -> str:
    out = subprocess.run(
        [CHROME, "--headless", "--disable-gpu", "--no-sandbox",
         "--user-data-dir=/tmp/gl-verify", "--virtual-time-budget=1200", "--dump-dom",
         f"file://{page}?step={step}&static=1"],
        capture_output=True, text=True, timeout=60,
    )
    if out.returncode != 0:
        sys.exit(f"chrome dump-dom failed at step {step}: {out.stderr[-400:]}")
    # Drop <style> and <script>: their *source text* contains the class names too, and
    # counting those would score a CSS rule or a classList call as a revealed element.
    return re.sub(r"<(style|script)\b.*?</\1>", "", out.stdout, flags=re.S | re.I)


def pixel_scan(path: pathlib.Path) -> dict:
    probe = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v", "-show_entries",
         "stream=width,height", "-of", "csv=p=0", str(path)],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    w, h = (int(x) for x in probe.split(","))
    raw = subprocess.run(
        # -nostdin is required: without it ffmpeg reads stdin for interactive keys and
        # blocks forever when stdin is not a tty (the parent then sleeps on the pipe).
        ["ffmpeg", "-nostdin", "-v", "error", "-i", str(path), "-f", "rawvideo",
         "-pix_fmt", "rgb24", "-"],
        capture_output=True, check=True, timeout=60,
    ).stdout
    img = np.frombuffer(raw, dtype=np.uint8)
    if img.size != w * h * 3:
        sys.exit(f"unexpected raw size for {path.name}: {img.size}")
    img = img.reshape(h, w, 3)

    bg = img[0, 0]
    nonbg = np.any(img != bg, axis=2)
    rows = np.flatnonzero(nonbg.any(axis=1))
    sel = img[nonbg]
    packed, counts = np.unique(
        (sel[:, 0].astype(np.uint32) << 16) | (sel[:, 1].astype(np.uint32) << 8) | sel[:, 2],
        return_counts=True,
    )
    return {
        "w": w, "h": h, "bg": tuple(int(v) for v in bg), "packed": packed, "counts": counts,
        "last_content_row": int(rows[-1]) if rows.size else 0,
    }


def rgb(p: int) -> tuple[int, int, int]:
    return (p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF


def count_where(scan: dict, pred) -> int:
    total = 0
    for p, n in zip(scan["packed"], scan["counts"]):
        if pred(rgb(int(p))):
            total += int(n)
    return total


print("== part 1: DOM state machine ==")
SCRATCH.mkdir(exist_ok=True)
page = SCRATCH / "verify-page.html"
page.write_text(inner_document())

prev_on = prev_lit = 0
# Exact expected reveal/light-up counts per step, derived from the fragment markup.
EXPECT_ON = [2, 5, 6, 8, 11, 15, 18, 21, 23, 24]
EXPECT_LIT = [2, 4, 5, 7, 7, 14, 16, 18, 20, 21]
for step in range(1, STEPS + 1):
    dom = dom_for(page, step)
    on = len(re.findall(r'\bis-on\b', dom))
    lit = len(re.findall(r'\bis-lit\b', dom))
    m = re.search(r'id="goal-lifecycle"[^>]*data-step="(\d+)"', dom)
    got = int(m.group(1)) if m else -1
    lab = re.search(r'id="gl-label"[^>]*>([^<]*)<', dom)
    det = re.search(r'id="gl-detail"[^>]*>(.*?)</p>', dom, re.S)
    lab_txt = (lab.group(1) if lab else "").strip()
    det_txt = re.sub(r"<[^>]+>", "", det.group(1)).strip() if det else ""

    check(got == step, f"step {step}: data-step={got}")
    check(len(lab_txt) > 8, f"step {step}: label {lab_txt[:44]!r}")
    check(len(det_txt) > 60, f"step {step}: detail {len(det_txt)} chars")
    check(on >= 1, f"step {step}: {on} revealed blocks, {lit} lit chips/pills")
    check(on == EXPECT_ON[step - 1],
          f"step {step}: exactly {EXPECT_ON[step - 1]} revealed blocks (got {on})")
    check(lit == EXPECT_LIT[step - 1],
          f"step {step}: exactly {EXPECT_LIT[step - 1]} lit chips/pills (got {lit})")
    check(on >= prev_on and lit >= prev_lit,
          f"step {step}: monotonic (is-on {prev_on}->{on}, is-lit {prev_lit}->{lit})")
    prev_on, prev_lit = on, lit
check(prev_on > 1, f"final step reveals {prev_on} blocks (not stuck at step 1)")

print("\n== part 2: frame pixels ==")
frames = [FRAMES / f"s{i}.png" for i in range(1, STEPS + 1)]
present = [p for p in frames if p.exists()]
check(len(present) == STEPS, f"{len(present)}/{STEPS} frames present")

scans = {p.name: pixel_scan(p) for p in present}
for name, s in scans.items():
    bg = s["bg"]
    dark, light = sum(bg) < 240, sum(bg) > 620
    check(dark or light, f"{name}: opaque themed bg={bg} ({'dark' if dark else 'light'})")
    check(s["last_content_row"] > s["h"] * 0.10,
          f"{name}: content bottom row {s['last_content_row']}/{s['h']}")
    check(len(s["packed"]) > 100,
          f"{name}: {len(s['packed'])} distinct non-bg colours")

for a, b in zip(present, present[1:]):
    sa, sb = scans[a.name], scans[b.name]
    # Count-weighted symmetric difference. NOT shared/|A|: a frame that only *adds*
    # content keeps nearly all of A, so a one-way overlap scores ~1.0 and reports a
    # false "no change" for exactly the steps that reveal the most.
    pa, ca = sa["packed"].astype(np.int64), sa["counts"].astype(np.int64)
    pb, cb = sb["packed"].astype(np.int64), sb["counts"].astype(np.int64)
    allc = np.union1d(pa, pb)
    da = np.zeros(allc.size, dtype=np.int64)
    db = np.zeros(allc.size, dtype=np.int64)
    da[np.searchsorted(allc, pa)] = ca
    db[np.searchsorted(allc, pb)] = cb
    delta = int(np.abs(da - db).sum()) / int(np.maximum(da, db).sum())
    check(delta > 0.004, f"{a.name}->{b.name}: frame changed (symmetric diff {delta:.4f})")

print("\n== part 3: accent colours (theme vars resolved) ==")
last = scans[present[-1].name]
check(count_where(last, lambda c: c[2] > c[0] + 24 and c[2] > 90) > 40, "primary blue present")
check(count_where(last, lambda c: c[1] > c[0] + 20 and c[1] > c[2] + 12) > 40, "success green present")
check(count_where(last, lambda c: c[0] > c[2] + 40 and c[1] > c[2] + 12 and c[0] > 110) > 40,
      "warning amber present")
check(count_where(last, lambda c: c[0] > c[1] + 40 and c[0] > 90) > 40, "danger red present")

print("\n== part 4: letterbox / canvas ==")
bottom = max(s["last_content_row"] for s in scans.values())
print(f"  canvas: {last['w']}x{last['h']}")
for name, s in scans.items():
    gap = bottom - s["last_content_row"]
    print(f"  {name}: content bottom {s['last_content_row']}"
          + (f"  (+{gap}px to pad)" if gap else "  (tallest)"))


def decode_rgb(path: pathlib.Path, w: int, h: int) -> np.ndarray:
    raw = subprocess.run(
        ["ffmpeg", "-nostdin", "-v", "error", "-i", str(path), "-f", "rawvideo",
         "-pix_fmt", "rgb24", "-"],
        capture_output=True, check=True, timeout=120,
    ).stdout
    return np.frombuffer(raw, dtype=np.uint8).reshape(h, w, 3).astype(np.int16)


print("\n== part 5: chapter frame integrity (MP4 -> source PNG) ==")
MP4 = ROOT / "docs/design/goal-lifecycle.mp4"
if MP4.exists():
    chapters = json.loads(subprocess.run(
        ["ffprobe", "-v", "error", "-show_chapters", "-of", "json", str(MP4)],
        capture_output=True, text=True, check=True).stdout)["chapters"]
    check(len(chapters) == STEPS, f"{len(chapters)} chapter markers for {STEPS} steps")
    src = [decode_rgb(FRAMES / f"s{i}.png", last["w"], last["h"]) for i in range(1, STEPS + 1)]
    tmp = SCRATCH / "chapter-frame.png"
    for i, ch in enumerate(chapters):
        start, end = float(ch["start_time"]), float(ch["end_time"])
        # Sample past the crossfade: mid-chapter is a settled frame of the source PNG.
        mid = (start + end) / 2
        subprocess.run(
            ["ffmpeg", "-nostdin", "-v", "error", "-ss", f"{mid:.3f}", "-i", str(MP4),
             "-frames:v", "1", "-y", str(tmp)],
            capture_output=True, check=True, timeout=120,
        )
        got = decode_rgb(tmp, last["w"], last["h"])
        wanted = src[i]
        mad = float(np.abs(got - wanted).mean())
        # Compare against every other source frame too: the nearest match must be the
        # one this chapter claims, otherwise the chapters are shifted by one.
        others = {j: float(np.abs(got - src[j]).mean()) for j in range(STEPS)}
        best = min(others, key=lambda j: others[j])
        check(best == i,
              f"chapter {i + 1} mid-frame ({mid:.2f}s) matches s{i + 1}.png "
              f"(mad {mad:.2f}; nearest s{best + 1} of 10)")
        check(mad < 14.0,
              f"chapter {i + 1}: mid-frame close to source (mad {mad:.2f} < 14)")
    tmp.unlink(missing_ok=True)
else:
    print("  (no MP4 found; run build_media.py first)")

print()
if failures:
    print(f"FAILED {len(failures)} check(s):")
    for f in failures:
        print(f"  - {f}")
    sys.exit(1)
print("all checks passed")
