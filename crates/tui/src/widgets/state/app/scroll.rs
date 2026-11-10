//! Visual-line log scrolling.
//!
//! The log panel's authoritative scroll position is `LogScroll::visual_top`,
//! the first visible visual line. A scroll step moves that position by `step`
//! visual lines, clamped to the content, whatever the row under it happens to
//! be. A row is a storage unit, not a scroll unit: one press moves one line
//! whether that row is a single line or a paragraph that wrapped into twelve.
//!
//! `j` and `k` pass [`LOG_LINE_STEP`], the wheel [`WHEEL_STEP`]. The prefix-sum
//! cache `LogScroll::visual_start` says where each row begins, and its last
//! entry is the total visual line count, which bounds the position.

use crate::widgets::state::App;

/// Keyboard step in the log: one visual line, every press.
///
/// A row is a storage unit, not a scroll unit. Taking a row whole moves the
/// viewport by however many lines that row wrapped into: measured, a press
/// over a three-line row moved three lines, and the first press up over a
/// four-line row moved five. `G` and `g` stay the fast routes to the ends.
pub(crate) const LOG_LINE_STEP: usize = 1;

/// Mouse-wheel step: three visual lines, every notch. Used by the log and the
/// subagent transcript.
pub(crate) const WHEEL_STEP: usize = 3;

/// Keyboard step inside the subagent transcript: half a screen.
///
/// That view is one bounded card rather than a column the reader scans line by
/// line, so it keeps the coarser step.
pub(crate) fn key_cell_step(viewport_height: usize) -> usize {
    (viewport_height / 2).max(1)
}

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

    /// Keep the reader's eye on the same content after rows disappear above it.
    ///
    /// Folding a run removes `rows_removed` visual rows. When all of them sit at
    /// or above the viewport top, everything the reader is looking at slides up
    /// by that much, and without this the line they were reading jumps off the
    /// top of the screen (measured: screen row 6 became row -1).
    ///
    /// A run that is only partly above the top is left alone on purpose. The
    /// reader can see it collapsing, and moving the content under their eyes
    /// while they watch would be the stranger of the two behaviours.
    ///
    /// Only the non-following case needs this: a following viewport is already
    /// pinned to the tail, which `render_log_panel` recomputes from `usize::MAX`.
    pub(crate) fn compensate_for_removed_rows(&mut self, rows_removed: usize, above_top: bool) {
        if rows_removed == 0 || !above_top || self.log_scroll.follow {
            return;
        }
        self.log_scroll.visual_top = self.log_scroll.visual_top.saturating_sub(rows_removed);
        self.sync_log_offset_mirror();
    }

    /// Scroll up by `step` visual lines, clamped to the top of the log.
    ///
    /// `step` counts visual lines, not rows: a paragraph that wrapped into
    /// four lines takes four presses to pass, the same as four one-line rows.
    pub(crate) fn scroll_log_up(&mut self, step: usize) {
        let v = self.log_viewport_top();
        self.log_scroll.visual_top = v.saturating_sub(step);
        // Any deliberate move up is a decision to stop following the tail.
        self.log_scroll.follow = false;
        self.sync_log_offset_mirror();
    }

    /// Scroll down by `step` visual lines, clamped to the bottom of the log.
    pub(crate) fn scroll_log_down(&mut self, step: usize) {
        // An empty cache (nothing drawn yet) says nothing and must re-arm
        // nothing, so it is the one case that does not move.
        let Some(&total) = self.log_scroll.visual_start.last() else {
            return;
        };
        let max_visual = total.saturating_sub(self.log_scroll.height as usize);
        let next = self.log_viewport_top().saturating_add(step);
        // Scrolling back down to the end resumes following — the reader asked
        // for the tail, and the badge has nothing left to point at.
        if next >= max_visual {
            self.scroll_log_to_bottom();
            return;
        }
        self.log_scroll.visual_top = next;
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

    /// A log whose second row wraps to ten lines. Viewport 4, so the content
    /// is four viewports' worth and the tall row's middle is what used to be
    /// unreachable.
    const VS: [usize; 5] = [0, 1, 11, 12, 13];

    fn app_with_layout(vs: &[usize], height: u16) -> App {
        let mut app = make_app();
        app.log_scroll.visual_start = vs.to_vec();
        app.log_scroll.height = height;
        app.log_scroll.visual_top = 0;
        app.log_scroll.follow = false;
        app
    }

    /// Every visual line inside a tall row is a stopping place, one press at a
    /// time, in both directions.
    ///
    /// The row-boundary rule stepped the whole 10-line row at once, so its
    /// middle could not be read at all.
    #[test]
    fn every_line_inside_a_tall_row_is_reachable() {
        let mut app = app_with_layout(&VS, 4);

        let mut tops = vec![app.log_viewport_top()];
        while app.log_viewport_top() < 9 {
            app.scroll_log_down(LOG_LINE_STEP);
            tops.push(app.log_viewport_top());
        }
        assert_eq!(tops, (0..=9).collect::<Vec<_>>());

        let mut back = vec![app.log_viewport_top()];
        while app.log_viewport_top() > 0 {
            app.scroll_log_up(LOG_LINE_STEP);
            back.push(app.log_viewport_top());
        }
        assert_eq!(back, (0..=9).rev().collect::<Vec<_>>());
    }

    #[test]
    fn the_wheel_moves_three_lines_per_notch() {
        let mut app = app_with_layout(&VS, 4);

        app.scroll_log_down(WHEEL_STEP);
        assert_eq!(app.log_viewport_top(), 3);
        app.scroll_log_down(WHEEL_STEP);
        assert_eq!(app.log_viewport_top(), 6);
        app.scroll_log_up(WHEEL_STEP);
        assert_eq!(app.log_viewport_top(), 3);
    }

    #[test]
    fn stepping_past_either_end_clamps_instead_of_wrapping() {
        let mut app = app_with_layout(&VS, 4);

        app.scroll_log_up(LOG_LINE_STEP);
        assert_eq!(app.log_viewport_top(), 0, "the top is a wall");

        app.scroll_log_down(1000);
        assert!(
            app.is_log_pinned_to_bottom(),
            "reaching the end re-arms follow"
        );
        app.scroll_log_down(LOG_LINE_STEP);
        assert!(app.is_log_pinned_to_bottom());
    }

    /// A wheel notch lands three lines down even when those lines belong to
    /// three different rows, which is how a normal log is shaped.
    #[test]
    fn a_short_row_log_steps_by_lines_too() {
        let mut app = app_with_layout(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], 2);

        app.scroll_log_down(WHEEL_STEP);
        assert_eq!(app.log_viewport_top(), 3);

        app.scroll_log_up(LOG_LINE_STEP);
        assert_eq!(app.log_viewport_top(), 2);
        assert_eq!(app.log_scroll.offset, 2, "the row mirror follows the lines");
    }

    #[test]
    fn an_empty_cache_never_moves() {
        let mut app = app_with_layout(&[], 4);
        app.scroll_log_down(LOG_LINE_STEP);
        assert_eq!(app.log_viewport_top(), 0);
        app.scroll_log_up(LOG_LINE_STEP);
        assert_eq!(app.log_viewport_top(), 0);
    }
}
