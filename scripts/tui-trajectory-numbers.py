#!/usr/bin/env python3
"""Reproduce the measurements in `docs/design/tui-trajectory.md`.

Both numbers the design leans on are arithmetic over constants that already
live in the TUI, so they are reproducible without a terminal. Re-run this after
changing the frame split or the overview's label gutter.

The frame split mirrors `crates/tui/src/lib.rs`:
    Layout::vertical([Length(1), Min(1), Length(input_lines + 2 + pending), Length(2)])
"""

BORDER_ROWS = 2
STATUS_ROWS = 1
BOTTOM_ROWS = 2
# The overview's lane-label gutter, left of the track.
LABEL_COLS = 6
BOX_COLS = 2


def main_area(rows, input_lines=1, pending=0):
    return rows - STATUS_ROWS - (input_lines + 2 + pending) - BOTTOM_ROWS


def content_rows(rows, input_lines=1, pending=0):
    return main_area(rows, input_lines, pending) - BORDER_ROWS


def track_cells(width):
    return width - BOX_COLS - LABEL_COLS


def main():
    print("HEIGHT: how much room the page has")
    for rows in (24, 30, 40, 50):
        print(
            f"  terminal={rows:>2}  main={main_area(rows):>2}  "
            f"content={content_rows(rows):>2}"
        )

    print("\nWIDTH: cells per record if one record gets one column")
    print("  (this is the desktop rule; 0 means the plot cannot show it)")
    for n in (12, 30, 60, 120, 300):
        cells = "  ".join(
            f"w={w}:{track_cells(w) // n:>3}" for w in (80, 120, 160)
        )
        print(f"  records={n:>3}  {cells}")

    print("\nWIDTH: records per bucket with one cell per bucket")
    for n in (12, 60, 120, 300):
        per = "  ".join(
            f"w={w}:{n / track_cells(w):5.1f}" for w in (80, 120, 160)
        )
        print(f"  records={n:>3}  {per}")


if __name__ == "__main__":
    main()
