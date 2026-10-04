//! What the two sticky panels share.
//!
//! `task_panel` and `subagent_panel` are the same shape at the state layer: a
//! grouped list of rows, a viewport that shows a window of it, and a one-line
//! title that says how much is running and how long it has been running. The
//! window and the duration label had been written out in both, identically.

/// The scroll window of `all_lines` with a marker for what is out of view.
///
/// Returns the whole list when it fits, and otherwise the `max_visible` rows
/// starting at `scroll` (clamped so the window cannot run past the end) plus
/// one marker row naming what is left. The marker is *appended*, so the result
/// can be one row longer than `max_visible` — it displaces the row it reports
/// on rather than being counted separately.
pub fn scroll_window(all_lines: Vec<String>, scroll: usize, max_visible: usize) -> Vec<String> {
    let total = all_lines.len();
    if total <= max_visible {
        return all_lines;
    }
    let scroll = scroll.min(total.saturating_sub(max_visible));
    let mut visible: Vec<String> = all_lines
        .iter()
        .skip(scroll)
        .take(max_visible)
        .cloned()
        .collect();
    let remaining = total.saturating_sub(scroll + max_visible);
    if remaining > 0 {
        visible.push(format!("⋯ +{} more · scroll ▼", remaining));
    } else if scroll > 0 {
        visible.push("⋯ scroll ▲".into());
    }
    visible
}

/// `"12s"` / `"3m 05s"` / `"2h 07m"` for the span from `started_at` to
/// `end_at`, or `None` when there is nothing to say.
///
/// `end_at` is `None` while the span is still running and the label then runs
/// up to now, which is what a live row wants. `None` also comes back when the
/// span never started, is not over, or the clock went backwards — a zero or
/// negative duration is not a label, it is a missing one.
pub fn elapsed_label(started_at: Option<i64>, end_at: Option<i64>) -> Option<String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let end = end_at.unwrap_or(now_ms);
    let start = started_at?;
    if end <= start {
        return None;
    }
    let secs = (end - start) / 1000;
    if secs < 60 {
        Some(format!("{}s", secs))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("row {i}")).collect()
    }

    #[test]
    fn a_list_that_fits_comes_back_untouched() {
        assert_eq!(scroll_window(lines(3), 0, 5), lines(3));
    }

    /// The marker displaces a row instead of adding one: the window is what the
    /// viewport can show, and the row it replaces is the one it reports on.
    #[test]
    fn the_overflow_marker_replaces_the_last_row() {
        let visible = scroll_window(lines(20), 0, 5);
        assert_eq!(visible.len(), 6, "{visible:?}");
        assert_eq!(visible[5], "⋯ +15 more · scroll ▼");
    }

    #[test]
    fn scrolling_past_the_end_clamps_to_the_last_window() {
        let visible = scroll_window(lines(8), 99, 5);
        assert_eq!(visible[0], "row 3", "{visible:?}");
        assert_eq!(visible.last().unwrap(), "⋯ scroll ▲");
    }

    /// The two branches are exclusive, and mid-list only the "more" direction
    /// is shown: there is one marker row, and below is what it reports.
    #[test]
    fn mid_list_the_more_marker_wins_over_the_up_marker() {
        let visible = scroll_window(lines(20), 2, 5);
        assert_eq!(visible.last().unwrap(), "⋯ +13 more · scroll ▼");
    }

    #[test]
    fn a_running_span_is_labelled_up_to_now() {
        let started = 1_000_000;
        assert!(elapsed_label(Some(started), None).is_some());
    }

    #[test]
    fn the_label_shortens_by_scale() {
        let start = 1_000_000_000_000;
        assert_eq!(
            elapsed_label(Some(start), Some(start + 12_000)).unwrap(),
            "12s"
        );
        assert_eq!(
            elapsed_label(Some(start), Some(start + 185_000)).unwrap(),
            "3m 5s"
        );
        assert_eq!(
            elapsed_label(Some(start), Some(start + 7_620_000)).unwrap(),
            "2h 07m"
        );
    }

    #[test]
    fn an_unstarted_or_backwards_span_has_no_label() {
        assert_eq!(elapsed_label(None, Some(5)), None);
        assert_eq!(elapsed_label(Some(5), Some(5)), None);
        assert_eq!(elapsed_label(Some(9), Some(5)), None);
    }
}
