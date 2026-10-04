//! Options + filter + cursor: the list-popup state every picker shares.
//!
//! Four pickers used to carry their own copy of three rules — how a query
//! matches a row, how a step clamps at the ends, and the fact that the cursor
//! indexes `options` rather than the *visible* subset. Only the first was ever
//! spelled the same way twice; the third is the subtle one, and it is what
//! makes a filtered confirm report the original index.

/// The substring match rule, in one place.
///
/// Case-insensitive containment, and an empty query matches everything — which
/// is also the answer to "what is on screen" for an unfiltered list. A caller
/// that re-derived this (three of the four pickers did) could drift from the
/// renderer and hide rows the cursor was still sitting on.
pub fn contains_ignore_case(haystack: &str, query: &str) -> bool {
    query.is_empty() || haystack.to_lowercase().contains(&query.to_lowercase())
}

/// Move `cursor` by `delta` within `len` rows, clamped to the ends.
///
/// `len == 0` yields `0`: an empty list has exactly one legal cursor position,
/// and every caller wants that rather than a panic or a stale index.
pub fn clamp_step(len: usize, cursor: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    let last = len.saturating_sub(1) as i32;
    (cursor as i32 + delta).clamp(0, last) as usize
}

/// A filtered option list and the cursor on it.
#[derive(Default, Clone)]
pub struct FilteredList {
    /// Every option, in display order. The cursor indexes *this*, never the
    /// filtered subset.
    pub options: Vec<String>,
    /// The typed filter. Always empty for an agent-originated prompt, which is
    /// answered with the arrow keys only.
    pub query: String,
    /// Index into `options` (not into [`Self::filtered_indices`]).
    pub selected: usize,
}

/// `Debug` prints the option *count*, not the options: a `/model` list is
/// long enough that dumping it buries the `query` and `selected` that a failing
/// assertion is actually about.
impl std::fmt::Debug for FilteredList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilteredList")
            .field("options", &self.options.len())
            .field("query", &self.query)
            .field("selected", &self.selected)
            .finish()
    }
}

impl FilteredList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the options and reset the filter and cursor (a new list is a
    /// new question — a stale query would hide options the list does have).
    pub fn reset(&mut self, options: Vec<String>, selected: usize) {
        self.options = options;
        self.selected = selected.min(self.options.len().saturating_sub(1));
        self.query.clear();
    }

    /// Replace the options, keeping the filter (used when the backing list
    /// refreshes under an open popup).
    pub fn set_options(&mut self, options: Vec<String>) {
        self.options = options;
        self.selected = 0;
    }

    pub fn len(&self) -> usize {
        self.options.len()
    }

    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    /// Indices of the options matching the current query, in list order.
    ///
    /// The renderer and the movement helpers share this so the window and the
    /// cursor can never disagree about the visible set.
    pub fn filtered_indices(&self) -> Vec<usize> {
        self.options
            .iter()
            .enumerate()
            .filter(|(_, option)| contains_ignore_case(option, &self.query))
            .map(|(i, _)| i)
            .collect()
    }

    /// The option the cursor is on, if any.
    pub fn selected_option(&self) -> Option<&str> {
        self.options.get(self.selected).map(String::as_str)
    }

    /// Re-anchor the cursor on the first match after the query changed.
    fn anchor_to_first_match(&mut self) {
        if let Some(&first) = self.filtered_indices().first() {
            self.selected = first;
        }
    }

    /// Append a character to the filter.
    pub fn push_query(&mut self, c: char) {
        self.query.push(c);
        self.anchor_to_first_match();
    }

    /// Delete the last filter character.
    pub fn pop_query(&mut self) {
        self.query.pop();
        self.anchor_to_first_match();
    }

    /// Clear the filter (Esc's first meaning on a filterable popup).
    pub fn clear_query(&mut self) {
        self.query.clear();
        self.anchor_to_first_match();
    }

    /// Move the cursor down, within the filtered set.
    ///
    /// With an empty query the filtered set is every option, so this is the
    /// plain "next option" step; with a filter it skips the rows that are not
    /// on screen. `selected` stays an index into `options` — the callers that
    /// map the confirmed index onto a semantic value depend on that.
    pub fn move_down(&mut self) {
        let visible = self.filtered_indices();
        if let Some(pos) = visible.iter().position(|&i| i == self.selected)
            && let Some(&next) = visible.get(pos + 1)
        {
            self.selected = next;
        }
    }

    /// Move the cursor up, within the filtered set.
    pub fn move_up(&mut self) {
        let visible = self.filtered_indices();
        if let Some(pos) = visible.iter().position(|&i| i == self.selected)
            && pos > 0
        {
            self.selected = visible[pos - 1];
        }
    }

    /// Step the cursor by `delta`, clamped — the wheel/arrow entry point for
    /// callers whose cursor is a plain index into the visible rows.
    pub fn step(&mut self, delta: i32) {
        self.selected = clamp_step(self.len(), self.selected, delta);
    }

    /// The confirmed option index, or `None` when nothing is on screen.
    ///
    /// Reported as an index into `options`, never renumbered by the filter.
    pub fn confirm(&mut self) -> Option<usize> {
        if self.options.is_empty() {
            return None;
        }
        Some(self.selected.min(self.options.len() - 1))
    }

    /// Focused option while clamping the cursor back into range after the
    /// option list shrank.
    pub fn clamp_cursor(&mut self) {
        self.selected = self.selected.min(self.options.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(options: &[&str]) -> FilteredList {
        let mut l = FilteredList::default();
        l.reset(options.iter().map(|o| o.to_string()).collect(), 0);
        l
    }

    #[test]
    fn an_empty_query_shows_every_option() {
        let l = list(&["a", "b", "c"]);
        assert_eq!(l.filtered_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn the_filter_matches_case_insensitively_and_anywhere_in_the_label() {
        let mut l = list(&["Kimi-K2.5", "gpt-5", "KIMI-for-coding"]);
        l.push_query('k');
        l.push_query('i');
        assert_eq!(
            l.filtered_indices(),
            vec![0, 2],
            "the query is lowercased, not the option"
        );

        // A fresh query: the match is a substring, not a prefix.
        let mut l = list(&["kimi-k2.5", "gpt-5", "kimi-for-coding"]);
        for c in "coding".chars() {
            l.push_query(c);
        }
        assert_eq!(l.filtered_indices(), vec![2], "matched mid-label");
    }

    #[test]
    fn typing_re_anchors_the_cursor_on_the_first_match() {
        let mut l = list(&["kimi-k2.5", "kimi-for-coding"]);
        l.push_query('c');
        assert_eq!(l.selected, 1, "the cursor lands on the row still visible");
    }

    #[test]
    fn deleting_a_character_widens_the_filter_again() {
        let mut l = list(&["kimi-for-coding", "claude-sonnet"]);
        l.push_query('c');
        l.push_query('l');
        assert_eq!(l.filtered_indices(), vec![1]);
        l.pop_query();
        assert_eq!(l.filtered_indices(), vec![0, 1]);
        l.clear_query();
        assert!(l.query.is_empty());
        assert_eq!(l.selected, 0);
    }

    #[test]
    fn movement_skips_the_rows_the_filter_hides() {
        let mut l = list(&["kimi-a", "gpt-b", "kimi-c"]);
        l.push_query('k');
        assert_eq!(l.filtered_indices(), vec![0, 2]);
        l.move_down();
        assert_eq!(l.selected, 2, "the hidden row in between is skipped");
        l.move_down();
        assert_eq!(l.selected, 2, "and the cursor stops at the last match");
        l.move_up();
        assert_eq!(l.selected, 0);
        l.move_up();
        assert_eq!(l.selected, 0, "clamped at the first match");
    }

    /// The whole reason the cursor indexes `options`: callers map the confirmed
    /// index onto a semantic value (a theme name, a permission mode), so a
    /// filter must not renumber it.
    #[test]
    fn confirm_reports_the_original_option_index() {
        let mut l = list(&["theme-a", "theme-b", "theme-c"]);
        l.push_query('c');
        assert_eq!(l.confirm(), Some(2));
    }

    #[test]
    fn an_empty_list_has_no_confirmation() {
        let mut l = FilteredList::default();
        assert_eq!(l.confirm(), None);
        assert_eq!(l.selected_option(), None);
        l.move_down();
        l.move_up();
        assert_eq!(l.selected, 0);
    }

    #[test]
    fn a_query_matching_nothing_leaves_no_visible_rows() {
        let mut l = list(&["kimi-k2.5"]);
        l.push_query('z');
        assert!(l.filtered_indices().is_empty());
    }

    #[test]
    fn reset_drops_a_stale_filter() {
        let mut l = list(&["a", "b"]);
        l.push_query('b');
        l.reset(vec!["a".into(), "b".into()], 0);
        assert!(l.query.is_empty());
        assert_eq!(l.filtered_indices(), vec![0, 1]);
    }

    #[test]
    fn clamp_step_handles_the_ends_and_an_empty_list() {
        assert_eq!(clamp_step(3, 0, -1), 0, "cannot go above the first row");
        assert_eq!(clamp_step(3, 2, 1), 2, "cannot go past the last row");
        assert_eq!(clamp_step(3, 1, 1), 2);
        assert_eq!(clamp_step(0, 7, 1), 0, "an empty list pins the cursor");
    }
}
