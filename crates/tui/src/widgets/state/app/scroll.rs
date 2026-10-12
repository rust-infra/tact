//! Visual-line log scrolling.
//!
//! The log panel's authoritative scroll position is `LogScroll::visual_top`
//! — the first visible visual line. The pure step functions below translate
//! "one scroll step" into a new visual position that can traverse a cell
//! taller than the viewport (a long Markdown table, an expanded tool card):
//!
//! - inside such a cell the position moves by `step` visual lines;
//! - at a cell boundary the position jumps to the adjacent row (entering a
//!   tall row from below lands on its bottom, from above on its top);
//! - for rows no taller than the viewport this degenerates to the old
//!   one-logical-row-per-step behavior.
//!
//! `vs` is the visual start prefix-sum cache (`visual_start_cache`), whose
//! last entry is the total visual line count.

use crate::widgets::state::App;

/// Scroll one step down (`j` / wheel down). Returns the new visual top.
///
/// `step` only matters inside a cell taller than the viewport; at row
/// boundaries the position jumps to the next row regardless of `step`.
pub(crate) fn visual_step_down(
    vs: &[usize],
    viewport_height: usize,
    v: usize,
    step: usize,
) -> usize {
    let Some(&total) = vs.last() else {
        return v;
    };
    if total <= viewport_height {
        return v;
    }
    let max_visual = total - viewport_height;
    if v >= max_visual {
        return v;
    }
    // Row containing `v` (partition_point finds the first start > v).
    let i = vs.partition_point(|&start| start <= v).saturating_sub(1);
    let row_top = vs[i];
    let row_end = vs.get(i + 1).copied().unwrap_or(total);
    if row_end - row_top > viewport_height {
        let bottom_pos = row_end - viewport_height;
        if v < bottom_pos {
            // Inside the tall cell: move down within it.
            return (v + step).min(bottom_pos);
        }
        // At the cell's bottom: enter the next row from above.
        return row_end.min(max_visual);
    }
    // Short row: jump to the next row's top.
    row_end.min(max_visual)
}

/// Scroll one step up (`k` / wheel up). Returns the new visual top.
pub(crate) fn visual_step_up(vs: &[usize], viewport_height: usize, v: usize, step: usize) -> usize {
    if v == 0 || vs.len() < 2 {
        return 0;
    }
    let total = *vs.last().unwrap_or(&0);
    let i = vs.partition_point(|&start| start <= v).saturating_sub(1);
    let row_top = vs[i];
    let row_end = vs.get(i + 1).copied().unwrap_or(total);
    if row_end - row_top > viewport_height && v > row_top {
        // Inside the tall cell: move up within it.
        return v.saturating_sub(step).max(row_top);
    }
    // At the top of the current row: enter the previous row. A tall previous
    // row is entered at its bottom so its middle stays traversable.
    if i == 0 {
        return 0;
    }
    let prev_top = vs[i - 1];
    let prev_height = row_top - prev_top;
    if prev_height > viewport_height {
        row_top - viewport_height
    } else {
        prev_top
    }
}

/// Keyboard step inside a cell taller than the viewport: half a screen.
pub(crate) fn key_cell_step(viewport_height: usize) -> usize {
    (viewport_height / 2).max(1)
}

/// Mouse-wheel step inside a cell taller than the viewport.
pub(crate) const WHEEL_CELL_STEP: usize = 3;

impl App {
    /// The viewport's first visible visual line, resolved from the persisted
    /// caches of the last render (`usize::MAX` sentinel → true bottom).
    pub(crate) fn log_viewport_top(&self) -> usize {
        let vs = &self.log_scroll.visual_start;
        let Some(&total) = vs.last() else {
            return 0;
        };
        let max_visual = total.saturating_sub(self.log_scroll.height as usize);
        self.log_scroll.visual_top.min(max_visual)
    }

    /// Derive the logical offset mirror from the current visual position so
    /// read-only consumers stay consistent between frames.
    fn sync_log_offset_mirror(&mut self) {
        let vs = &self.log_scroll.visual_start;
        let Some(&total) = vs.last() else {
            self.log_scroll.offset = 0;
            return;
        };
        let v = self.log_viewport_top();
        if total == 0 {
            self.log_scroll.offset = 0;
            return;
        }
        let i = vs.partition_point(|&start| start <= v).saturating_sub(1);
        self.log_scroll.offset = u16::try_from(i).unwrap_or(u16::MAX);
    }

    /// Record the back-to-bottom hint's hit area (render-time; clicking it
    /// returns to the tail, see the mouse handler).
    pub(crate) fn set_scroll_back_area(&mut self, area: ratatui::layout::Rect) {
        self.scroll_back_area = area;
    }

    /// Jump to the top of the log.
    pub(crate) fn scroll_log_to_top(&mut self) {
        self.log_scroll.visual_top = 0;
        self.log_scroll.offset = 0;
        self.log_scroll.follow = false;
    }

    /// Pin the viewport to the bottom of the log (also the pre-render
    /// sentinel state used while new content streams in).
    ///
    /// The one place that re-arms the tail: it both pins *now* and clears the
    /// "new activity" badge, because arriving at the bottom is what the badge
    /// was asking for.
    pub(crate) fn scroll_log_to_bottom(&mut self) {
        self.log_scroll.visual_top = usize::MAX;
        self.log_scroll.offset = u16::MAX;
        self.log_scroll.follow = true;
        self.log_scroll.unseen = false;
    }

    /// After new rows land at the **end** of the log: keep the tail pinned if
    /// we were following it, otherwise leave the reader where they are and
    /// light the "new activity" badge.
    ///
    /// This is the whole follow-the-tail policy, and it lives here rather than
    /// at the ~10 call sites that used to call `scroll_log_to_bottom()`
    /// unconditionally — those yanked the viewport back down on every streamed
    /// chunk, which made reading scrollback during a live turn impossible.
    pub(crate) fn note_log_rows_appended(&mut self) {
        if self.log_scroll.follow {
            self.scroll_log_to_bottom();
        } else {
            self.log_scroll.unseen = true;
        }
    }

    /// After rows **above** the viewport change height (a tool card or thinking
    /// block growing in place): re-pin only if we were following. No badge —
    /// nothing new appeared at the end, so there is nothing to scroll to.
    pub(crate) fn keep_log_tail_pinned(&mut self) {
        if self.log_scroll.follow {
            self.scroll_log_to_bottom();
        }
    }

    /// Scroll up by `step` visual lines inside a tall cell (row-boundary
    /// jumps otherwise). `key_cell_step` / `WHEEL_CELL_STEP` pick `step`.
    pub(crate) fn scroll_log_up(&mut self, step: usize) {
        let v = self.log_viewport_top();
        let vh = self.log_scroll.height as usize;
        self.log_scroll.visual_top = visual_step_up(&self.log_scroll.visual_start, vh, v, step);
        // Any deliberate move up is a decision to stop following the tail.
        self.log_scroll.follow = false;
        self.sync_log_offset_mirror();
    }

    /// Scroll down by `step` visual lines inside a tall cell (row-boundary
    /// jumps otherwise).
    pub(crate) fn scroll_log_down(&mut self, step: usize) {
        let v = self.log_viewport_top();
        let vh = self.log_scroll.height as usize;
        let next = visual_step_down(&self.log_scroll.visual_start, vh, v, step);
        self.log_scroll.visual_top = next;
        // Scrolling back down to the end resumes following — the reader asked
        // for the tail, and the badge has nothing left to point at. An empty
        // cache (nothing drawn yet) says nothing and must re-arm nothing.
        if let Some(&total) = self.log_scroll.visual_start.last() {
            let max_visual = total.saturating_sub(self.log_scroll.height as usize);
            if next >= max_visual {
                self.scroll_log_to_bottom();
            }
        }
        self.sync_log_offset_mirror();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::{make_app, render_log_panel_text};

    /// A reader who is at the bottom stays there as rows arrive.
    #[test]
    fn a_new_row_keeps_a_following_viewport_at_the_bottom() {
        let mut app = make_app();

        app.append_markdown("first");

        assert!(app.is_log_pinned_to_bottom());
        assert_eq!(app.log_scroll.visual_top, usize::MAX);
        assert!(
            !app.log_scroll.unseen,
            "a following reader has nothing to be told about"
        );
    }

    /// The point of the whole change: output arriving while the reader is
    /// scrolled up must not move them, and must say that it arrived.
    #[test]
    fn a_new_row_does_not_move_a_scrolled_away_viewport_and_lights_the_badge() {
        let mut app = make_app();
        app.append_markdown("first");
        app.scroll_log_up(1);
        let top = app.log_scroll.visual_top;
        assert!(!app.is_log_pinned_to_bottom());

        app.append_markdown("second");

        assert_eq!(
            app.log_scroll.visual_top, top,
            "the reader keeps their place"
        );
        assert!(app.log_scroll.unseen, "and is told there is more below");
    }

    #[test]
    fn scrolling_back_down_to_the_end_resumes_following() {
        let mut app = make_app();
        for i in 0..40 {
            app.append_markdown(format!("row-{i}"));
        }
        let _ = render_log_panel_text(&mut app, 60, 6);
        app.scroll_log_up(1);
        assert!(!app.is_log_pinned_to_bottom());

        app.scroll_log_down(100);

        assert!(
            app.is_log_pinned_to_bottom(),
            "reaching the end is asking for the tail again"
        );
        assert!(!app.log_scroll.unseen, "and clears the badge");
    }

    #[test]
    fn jumping_to_the_top_stops_following() {
        let mut app = make_app();
        app.append_markdown("first");

        app.scroll_log_to_top();

        assert!(!app.is_log_pinned_to_bottom());
    }

    /// The one append that always follows: the user just pressed Enter.
    #[test]
    fn the_users_own_message_always_comes_into_view() {
        let mut app = make_app();
        for i in 0..40 {
            app.append_markdown(format!("row-{i}"));
        }
        let _ = render_log_panel_text(&mut app, 60, 6);
        app.scroll_log_up(3);
        assert!(!app.is_log_pinned_to_bottom());

        app.add_user_message("my question".into());

        assert!(
            app.is_log_pinned_to_bottom(),
            "Enter means: show me my own words"
        );
    }

    /// Layout: 4 logical rows.
    ///   row0: visual [0..1)   short
    ///   row1: visual [1..11)  tall (10 lines, viewport 4)
    ///   row2: visual [11..12) short
    ///   row3: visual [12..13) short
    /// total = 13, vh = 4, max_visual = 9.
    const VS: [usize; 5] = [0, 1, 11, 12, 13];

    #[test]
    fn tall_cell_middle_is_traversable_from_above() {
        let vh = 4;
        // Down from row0: enter the tall row at its top.
        let v1 = visual_step_down(&VS, vh, 0, key_cell_step(vh));
        assert_eq!(v1, 1);
        // Inside the tall cell: step by half the viewport.
        let v2 = visual_step_down(&VS, vh, v1, key_cell_step(vh));
        assert_eq!(v2, 3);
        let v3 = visual_step_down(&VS, vh, v2, key_cell_step(vh));
        assert_eq!(v3, 5);
        // Overshooting clamps to the cell's bottom position (11 - 4 = 7).
        let v4 = visual_step_down(&VS, vh, v3, key_cell_step(vh));
        assert_eq!(v4, 7);
        // At the cell's bottom the next step reaches the true bottom (9):
        // rows 2/3 are short and fit into the final viewport.
        let v5 = visual_step_down(&VS, vh, v4, key_cell_step(vh));
        assert_eq!(v5, 9);
        assert_eq!(visual_step_down(&VS, vh, v5, key_cell_step(vh)), 9);
    }

    #[test]
    fn tall_cell_middle_is_traversable_from_below() {
        let vh = 4;
        // Bottom-pinned viewport (9) shows the cell tail plus the short rows.
        let v1 = visual_step_up(&VS, vh, 9, key_cell_step(vh));
        assert_eq!(v1, 7);
        // At the tall row's bottom (7): keep walking up inside it.
        let v2 = visual_step_up(&VS, vh, 7, key_cell_step(vh));
        assert_eq!(v2, 5);
        let v3 = visual_step_up(&VS, vh, 5, key_cell_step(vh));
        assert_eq!(v3, 3);
        let v4 = visual_step_up(&VS, vh, 3, key_cell_step(vh));
        assert_eq!(v4, 1);
        // At the tall row's top: enter the previous short row.
        assert_eq!(visual_step_up(&VS, vh, 1, key_cell_step(vh)), 0);
        assert_eq!(visual_step_up(&VS, vh, 0, key_cell_step(vh)), 0);
    }

    #[test]
    fn wheel_uses_three_line_steps_inside_tall_cells() {
        let vh = 4;
        assert_eq!(visual_step_down(&VS, vh, 1, WHEEL_CELL_STEP), 4);
        assert_eq!(visual_step_up(&VS, vh, 7, WHEEL_CELL_STEP), 4);
    }

    #[test]
    fn short_rows_keep_one_row_per_step_semantics() {
        // vs = [0, 1, 2, 3] → three 1-line rows, vh = 2, max_visual = 1.
        let vs = [0usize, 1, 2, 3];
        assert_eq!(visual_step_down(&vs, 2, 0, key_cell_step(2)), 1);
        assert_eq!(visual_step_down(&vs, 2, 1, key_cell_step(2)), 1);
        assert_eq!(visual_step_up(&vs, 2, 1, key_cell_step(2)), 0);
        assert_eq!(visual_step_up(&vs, 2, 0, key_cell_step(2)), 0);
    }

    #[test]
    fn content_shorter_than_viewport_does_not_scroll() {
        let vs = [0usize, 1, 2, 3];
        assert_eq!(visual_step_down(&vs, 30, 0, key_cell_step(30)), 0);
        assert_eq!(visual_step_up(&vs, 30, 0, key_cell_step(30)), 0);
    }

    #[test]
    fn entering_tall_row_from_below_lands_on_its_bottom() {
        // row0 short [0..1), row1 tall [1..11), row2 short [11..12).
        let vh = 4;
        let v = visual_step_up(&VS, vh, 11, key_cell_step(vh));
        assert_eq!(v, 7, "scrolling up from row2 shows the tall row's bottom");
    }

    #[test]
    fn empty_cache_is_a_noop() {
        assert_eq!(visual_step_down(&[], 4, 0, 2), 0);
        assert_eq!(visual_step_up(&[], 4, 0, 2), 0);
    }
}
