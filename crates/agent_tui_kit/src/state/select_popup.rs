/// Select popup state: independently manages prompt, options, selected index, and response channel.
pub struct SelectPopup {
    /// Popup prompt text.
    pub prompt: String,
    /// Option list.
    pub options: Vec<String>,
    /// Index of the currently focused option (cursor).
    pub selected: usize,
    /// Response channel for single-select (permission / default ask_user).
    pub respond: Option<tokio::sync::oneshot::Sender<Option<usize>>>,
    /// Response channel for multi-select (`ask_user` with `multi_select`).
    pub respond_multi: Option<tokio::sync::oneshot::Sender<Option<Vec<usize>>>>,
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
            respond: None,
            respond_multi: None,
            multi: false,
            checked: Vec::new(),
            log_confirm: true,
        }
    }
}

impl SelectPopup {
    fn clear_channels(&mut self) {
        self.respond = None;
        self.respond_multi = None;
    }

    /// Set popup content without a oneshot channel (local TUI flows like `/model`).
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
        self.clear_channels();
        self.multi = false;
        self.checked.clear();
        self.log_confirm = log_confirm;
    }

    /// Single-select popup (permission / default ask_user). Unchanged contract.
    pub fn set(
        &mut self,
        prompt: String,
        options: Vec<String>,
        respond: tokio::sync::oneshot::Sender<Option<usize>>,
        log_confirm: bool,
    ) {
        self.prompt = prompt;
        self.options = options;
        self.selected = 0;
        self.respond = Some(respond);
        self.respond_multi = None;
        self.multi = false;
        self.checked.clear();
        self.log_confirm = log_confirm;
    }

    /// Multi-select popup (`ask_user` with `multi_select: true`).
    pub fn set_multi(
        &mut self,
        prompt: String,
        options: Vec<String>,
        respond: tokio::sync::oneshot::Sender<Option<Vec<usize>>>,
        log_confirm: bool,
    ) {
        let n = options.len();
        self.prompt = prompt;
        self.options = options;
        self.selected = 0;
        self.respond = None;
        self.respond_multi = Some(respond);
        self.multi = true;
        self.checked = vec![false; n];
        self.log_confirm = log_confirm;
    }

    /// Confirm single-select: send the focused index. No-op for multi (use [`confirm_multi`]).
    pub fn confirm(&mut self) -> Option<usize> {
        if self.multi {
            return None;
        }
        let respond = self.respond.take();
        let idx = self.selected.min(self.options.len().saturating_sub(1));
        if let Some(tx) = respond {
            let _ = tx.send(Some(idx));
        }
        Some(idx)
    }

    /// Confirm multi-select: send all checked indices (may be empty).
    pub fn confirm_multi(&mut self) -> Vec<usize> {
        let idxs: Vec<usize> = self
            .checked
            .iter()
            .enumerate()
            .filter_map(|(i, on)| on.then_some(i))
            .collect();
        if let Some(tx) = self.respond_multi.take() {
            let _ = tx.send(Some(idxs.clone()));
        }
        self.respond = None;
        idxs
    }

    /// Cancel selection: send None on the active channel.
    pub fn cancel(&mut self) {
        if let Some(tx) = self.respond.take() {
            let _ = tx.send(None);
        }
        if let Some(tx) = self.respond_multi.take() {
            let _ = tx.send(None);
        }
        self.multi = false;
        self.checked.clear();
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
