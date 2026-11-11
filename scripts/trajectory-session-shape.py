#!/usr/bin/env python3
"""Measure what a real session looks like, so the trajectory page's costs are
sized against evidence rather than intuition.

Reads the local `.tact/transcripts` corpus and reports the tool-call count and
tool-result body sizes. The page's record count is bounded by the former and
its memory by the latter, so both numbers decide whether a cap is needed.

Run from the repo root. Nothing is written; this only reads.
"""

import glob
import os
import sys

TRANSCRIPTS = ".tact/transcripts/*.jsonl"


def iter_tool_results(obj):
    """Yield every `tool_result` text body in a decoded transcript line."""
    if isinstance(obj, dict):
        if obj.get("type") == "tool_result":
            content = obj.get("content")
            if isinstance(content, str):
                yield content
            elif isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and isinstance(block.get("text"), str):
                        yield block["text"]
        for value in obj.values():
            yield from iter_tool_results(value)
    elif isinstance(obj, list):
        for value in obj:
            yield from iter_tool_results(value)


def main() -> int:
    import json

    files = sorted(glob.glob(TRANSCRIPTS))
    if not files:
        print(f"no transcripts at {TRANSCRIPTS}", file=sys.stderr)
        return 1

    calls = []
    bodies = []
    for path in files:
        count = 0
        try:
            for line in open(path, errors="ignore"):
                if '"tool_result"' not in line:
                    count += 1 if '"tool_use"' in line else 0
                    continue
                count += 1
                try:
                    obj = json.loads(line)
                except ValueError:
                    continue
                bodies.extend(len(text) for text in iter_tool_results(obj))
        except OSError:
            continue
        calls.append((count, os.path.basename(path), os.path.getsize(path)))

    calls.sort(reverse=True)
    print(f"sessions: {len(calls)}")
    print("per-session tool calls (the page's record count ceiling):")
    for count, name, size in calls[:5]:
        print(f"  {count:>6} calls   {size / 1e6:6.2f} MB   {name}")
    counts = sorted(count for count, _, _ in calls)
    print(f"  median {counts[len(counts) // 2]}, max {counts[-1]}")

    if not bodies:
        print("\nno tool-result bodies found")
        return 0

    bodies.sort()
    total = sum(bodies)
    print(f"\ntool-result bodies: {len(bodies)}, {total / 1e6:.1f} MB total")
    for label, quantile in (("p50", 0.5), ("p90", 0.9), ("p99", 0.99), ("max", 1.0)):
        index = min(len(bodies) - 1, int(len(bodies) * quantile))
        print(f"  {label}: {bodies[index] / 1024:8.1f} KB")

    # What a 4 KB cap would keep and drop.
    cap = 4 * 1024
    over = [size for size in bodies if size > cap]
    print(
        f"\nwith a {cap // 1024} KB cap: {len(bodies) - len(over)} of {len(bodies)} "
        f"records stay complete ({100 * (len(bodies) - len(over)) / len(bodies):.0f}%), "
        f"the {len(over)} over it hold {sum(over) / 1e6:.1f} MB"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
