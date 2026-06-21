/// Lightweight file-picker popup state.
///
/// Holds a flat list of relative file paths under the project root and the
/// currently selected index. Navigation and confirmation are handled by the
/// file-picker key handler.
#[derive(Debug)]
pub(crate) struct FilePicker {
    pub(crate) options: Vec<String>,
    pub(crate) selected: usize,
}

impl FilePicker {
    pub(crate) fn new() -> Self {
        Self {
            options: Vec::new(),
            selected: 0,
        }
    }

    pub(crate) fn set(&mut self, options: Vec<String>) {
        self.options = options;
        self.selected = 0;
    }

    pub(crate) fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(crate) fn move_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        }
    }

    pub(crate) fn selected_path(&self) -> Option<&str> {
        self.options.get(self.selected).map(|s| s.as_str())
    }
}
