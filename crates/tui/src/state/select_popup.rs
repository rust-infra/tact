/// Select popup state: independently manages prompt, options, selected index, and response channel.
pub(crate) struct SelectPopup {
    /// Popup prompt text.
    pub(crate) prompt: String,
    /// Option list.
    pub(crate) options: Vec<String>,
    /// Index of the currently selected option.
    pub(crate) selected: usize,
    /// Response channel for sending the selected option index back to the caller.
    pub(crate) respond: Option<tokio::sync::oneshot::Sender<Option<usize>>>,
}

impl SelectPopup {
    pub(crate) fn new() -> Self {
        Self {
            prompt: String::new(),
            options: Vec::new(),
            selected: 0,
            respond: None,
        }
    }

    /// Set popup content and activate.
    pub(crate) fn set(
        &mut self,
        prompt: String,
        options: Vec<String>,
        respond: tokio::sync::oneshot::Sender<Option<usize>>,
    ) {
        self.prompt = prompt;
        self.options = options;
        self.selected = 0;
        self.respond = Some(respond);
    }

    /// Confirm current selection: send the selected index and clear respond.
    pub(crate) fn confirm(&mut self) -> Option<usize> {
        let respond = self.respond.take();
        let idx = self.selected.min(self.options.len().saturating_sub(1));
        if let Some(tx) = respond {
            let _ = tx.send(Some(idx));
        }
        Some(idx)
    }

    /// Cancel selection: send None and clear respond.
    pub(crate) fn cancel(&mut self) {
        if let Some(tx) = self.respond.take() {
            let _ = tx.send(None);
        }
    }

    /// Move selection down.
    pub(crate) fn move_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        }
    }

    /// Move selection up.
    pub(crate) fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }
}
