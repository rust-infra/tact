/// Select popup state: independently manages prompt, options, selected index,
/// and the request id used to answer agent-originated requests.
///
/// The popup no longer holds a oneshot sender. Agent-originated selects carry a
/// `request_id`; confirming or cancelling produces a [`tact_protocol::UiResponse`]
/// that the caller (the TUI) sends over the reverse command channel.
pub struct SelectPopup {
    /// Popup prompt text.
    pub prompt: String,
    /// Option list.
    pub options: Vec<String>,
    /// Index of the currently focused option (cursor).
    pub selected: usize,
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

impl Default for SelectPopup {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            options: Vec::new(),
            selected: 0,
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
        self.options = options;
        self.selected = selected.min(self.options.len().saturating_sub(1));
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
        self.options = options;
        self.selected = 0;
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
        self.options = options;
        self.selected = 0;
        self.request_id = Some(request_id);
        self.multi = true;
        self.checked = vec![false; n];
        self.log_confirm = log_confirm;
    }

    /// Consume and return the pending request id, if this was agent-originated.
    pub fn take_request_id(&mut self) -> Option<u64> {
        self.request_id.take()
    }

    /// Focused index for single-select (no side effects). No-op for multi.
    pub fn confirm(&mut self) -> Option<usize> {
        if self.multi {
            return None;
        }
        Some(self.selected.min(self.options.len().saturating_sub(1)))
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

    /// Move selection down.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        }
    }

    /// Move selection up.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }
}
