use super::filtered_list::FilteredList;

/// Select popup state: a [`FilteredList`] plus the flags that make it a
/// *prompt* rather than a picker.
///
/// The popup no longer holds a oneshot sender. Agent-originated selects carry a
/// `request_id`; confirming or cancelling produces a [`tact_protocol::UiResponse`]
/// that the caller (the TUI) sends over the reverse command channel.
///
/// `Deref`/`DerefMut` expose the list's `options` / `query` / `selected` and its
/// cursor methods, so a call site reads `popup.options` exactly as it did when
/// those were fields here — while the filter and cursor rules live in one place
/// instead of a copy per picker.
pub struct SelectPopup {
    /// Options, filter and cursor.
    pub list: FilteredList,
    /// Popup prompt text.
    pub prompt: String,
    /// Request id for agent-originated selects (`RequestSelect` /
    /// `RequestMultiSelect`); `None` for local TUI flows like `/model`.
    pub request_id: Option<u64>,
    /// When true, Space toggles checkboxes; Enter submits all checked indices.
    pub multi: bool,
    /// Checkbox state per option (only used when `multi`).
    pub checked: Vec<bool>,
    /// When false, confirming does not append a separate log line (e.g. permission
    /// choices are already shown on the tool meta row).
    pub log_confirm: bool,
}

impl std::ops::Deref for SelectPopup {
    type Target = FilteredList;

    fn deref(&self) -> &FilteredList {
        &self.list
    }
}

impl std::ops::DerefMut for SelectPopup {
    fn deref_mut(&mut self) -> &mut FilteredList {
        &mut self.list
    }
}

impl Default for SelectPopup {
    fn default() -> Self {
        Self {
            list: FilteredList::default(),
            prompt: String::new(),
            request_id: None,
            multi: false,
            checked: Vec::new(),
            log_confirm: true,
        }
    }
}

impl SelectPopup {
    /// Set popup content without a request id (local TUI flows like `/model`).
    pub fn set_local(
        &mut self,
        prompt: String,
        options: Vec<String>,
        selected: usize,
        log_confirm: bool,
    ) {
        self.prompt = prompt;
        self.list.reset(options, selected);
        self.request_id = None;
        self.multi = false;
        self.checked.clear();
        self.log_confirm = log_confirm;
    }

    /// Single-select popup (permission / default ask_user).
    pub fn set(
        &mut self,
        prompt: String,
        options: Vec<String>,
        request_id: u64,
        log_confirm: bool,
    ) {
        self.prompt = prompt;
        self.list.reset(options, 0);
        self.request_id = Some(request_id);
        self.multi = false;
        self.checked.clear();
        self.log_confirm = log_confirm;
    }

    /// Multi-select popup (`ask_user` with `multi_select: true`).
    pub fn set_multi(
        &mut self,
        prompt: String,
        options: Vec<String>,
        request_id: u64,
        log_confirm: bool,
    ) {
        let n = options.len();
        self.prompt = prompt;
        self.list.reset(options, 0);
        self.request_id = Some(request_id);
        self.multi = true;
        self.checked = vec![false; n];
        self.log_confirm = log_confirm;
    }

    /// Whether the user may type to narrow this list.
    ///
    /// Local picks (`/model`, `/theme`, `/permission`, `/view-system-prompt`)
    /// are the user's own list, and `/model` unions the config list with
    /// `/v1/models`, so it is long enough to need a filter. Agent-originated
    /// prompts are not: the agent is blocked waiting, and a stray keystroke
    /// must not hide the choices.
    pub fn filterable(&self) -> bool {
        self.request_id.is_none()
    }

    /// Consume and return the pending request id, if this was agent-originated.
    pub fn take_request_id(&mut self) -> Option<u64> {
        self.request_id.take()
    }

    /// Focused index for single-select (no side effects). No-op for multi.
    ///
    /// Shadows [`FilteredList::confirm`], which has no multi concept.
    pub fn confirm(&mut self) -> Option<usize> {
        if self.multi {
            return None;
        }
        self.list.confirm()
    }

    /// All checked indices for multi-select (may be empty).
    pub fn confirm_multi(&mut self) -> Vec<usize> {
        self.checked
            .iter()
            .enumerate()
            .filter_map(|(i, on)| on.then_some(i))
            .collect()
    }

    /// Build the cancellation response for an agent-originated request, if any.
    /// Resets multi/checked state. Returns `None` for local flows.
    pub fn cancel(&mut self) -> Option<tact_protocol::UiResponse> {
        let request_id = self.request_id.take();
        let response = request_id.map(|id| {
            if self.multi {
                tact_protocol::UiResponse::MultiSelect {
                    request_id: id,
                    choices: None,
                }
            } else {
                tact_protocol::UiResponse::Select {
                    request_id: id,
                    choice: None,
                }
            }
        });
        self.multi = false;
        self.checked.clear();
        self.query.clear();
        response
    }

    pub fn toggle_checked(&mut self) {
        if !self.multi || self.options.is_empty() {
            return;
        }
        let i = self.selected.min(self.options.len().saturating_sub(1));
        if let Some(slot) = self.checked.get_mut(i) {
            *slot = !*slot;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local pick: filterable, the way `/model` opens one.
    fn local(options: &[&str], selected: usize) -> SelectPopup {
        let mut popup = SelectPopup::default();
        popup.set_local(
            "Select model".into(),
            options.iter().map(|o| o.to_string()).collect(),
            selected,
            true,
        );
        popup
    }

    /// An agent prompt: not filterable.
    fn agent(options: &[&str]) -> SelectPopup {
        let mut popup = SelectPopup::default();
        popup.set(
            "Allow?".into(),
            options.iter().map(|o| o.to_string()).collect(),
            7,
            true,
        );
        popup
    }

    #[test]
    fn only_local_picks_are_filterable() {
        assert!(local(&["a"], 0).filterable());
        assert!(!agent(&["a"]).filterable());
    }

    #[test]
    fn an_empty_query_shows_every_option() {
        let popup = local(&["a", "b", "c"], 1);
        assert_eq!(popup.filtered_indices(), vec![0, 1, 2]);
    }

    #[test]
    fn the_filter_matches_case_insensitively() {
        let mut popup = local(&["Kimi-K2.5", "gpt-5", "KIMI-for-coding"], 0);
        popup.push_query('k');
        popup.push_query('i');
        assert_eq!(
            popup.filtered_indices(),
            vec![0, 2],
            "the query is lowercased, not the option"
        );
    }

    #[test]
    fn the_filter_matches_anywhere_in_the_label() {
        let mut popup = local(&["kimi-k2.5", "kimi-for-coding"], 0);
        popup.push_query('c');
        popup.push_query('o');
        popup.push_query('d');
        assert_eq!(popup.filtered_indices(), vec![1]);
    }

    #[test]
    fn typing_re_anchors_the_cursor_on_the_first_match() {
        let mut popup = local(&["kimi-k2.5", "kimi-for-coding"], 0);
        popup.push_query('c');
        assert_eq!(
            popup.selected, 1,
            "the cursor must land on the row the filter leaves visible"
        );
    }

    #[test]
    fn deleting_a_character_widens_the_filter_again() {
        let mut popup = local(&["kimi-for-coding", "claude-sonnet"], 0);
        popup.push_query('c');
        popup.push_query('l');
        assert_eq!(
            popup.filtered_indices(),
            vec![1],
            "only the label starting with `cl`"
        );
        popup.pop_query();
        assert_eq!(
            popup.filtered_indices(),
            vec![0, 1],
            "back to every label containing `c`"
        );
        popup.clear_query();
        assert!(popup.query.is_empty());
        assert_eq!(popup.selected, 0, "clearing re-anchors at the top");
    }

    #[test]
    fn a_query_matching_nothing_leaves_no_visible_rows() {
        let mut popup = local(&["kimi-k2.5"], 0);
        popup.push_query('z');
        assert!(popup.filtered_indices().is_empty());
    }

    #[test]
    fn confirm_reports_the_original_option_index() {
        // `ThemePick` and `PermissionModePick` map the confirmed index onto a
        // semantic value, so filtering must not renumber the options.
        let mut popup = local(&["theme-a", "theme-b", "theme-c"], 0);
        popup.push_query('c');
        assert_eq!(popup.confirm(), Some(2));
    }

    #[test]
    fn movement_skips_the_rows_the_filter_hides() {
        let mut popup = local(&["kimi-a", "gpt-b", "kimi-c"], 0);
        popup.push_query('k');
        assert_eq!(popup.filtered_indices(), vec![0, 2]);
        assert_eq!(popup.selected, 0);

        popup.move_down();
        assert_eq!(popup.selected, 2, "the hidden row in between is skipped");
        popup.move_down();
        assert_eq!(popup.selected, 2, "and the cursor stops at the last match");

        popup.move_up();
        assert_eq!(popup.selected, 0);
        popup.move_up();
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn movement_without_a_query_is_the_plain_next_option_step() {
        let mut popup = local(&["a", "b", "c"], 0);
        popup.move_down();
        assert_eq!(popup.selected, 1);
        popup.move_down();
        popup.move_down();
        assert_eq!(popup.selected, 2, "clamped at the end");
        popup.move_up();
        assert_eq!(popup.selected, 1);
    }

    #[test]
    fn reopening_the_popup_clears_the_previous_query() {
        let mut popup = local(&["a", "b"], 0);
        popup.push_query('b');
        assert!(!popup.query.is_empty());

        popup.set_local("Again".into(), vec!["a".into(), "b".into()], 0, true);
        assert!(
            popup.query.is_empty(),
            "a stale filter would hide options the new list does have"
        );
        assert_eq!(popup.filtered_indices(), vec![0, 1]);
    }

    #[test]
    fn cancelling_clears_the_query() {
        let mut popup = local(&["a", "b"], 0);
        popup.push_query('b');
        popup.cancel();
        assert!(popup.query.is_empty());
    }
}
