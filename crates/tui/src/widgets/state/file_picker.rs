/// Lightweight file-picker popup state.
///
/// Holds the current directory being browsed, a flat list of entries in that
/// directory, and a typed filter query. Directories are represented with a
/// trailing `/` so the renderer can show folder icons and the handler can
/// navigate into them.
#[derive(Debug)]
pub(crate) struct FilePicker {
    pub(crate) options: Vec<String>,
    pub(crate) selected: usize,
    /// Filter query typed while the picker is open.
    pub(crate) query: String,
    /// Directory currently being browsed (absolute path).
    pub(crate) current_dir: std::path::PathBuf,
    /// Project root used to compute relative paths for insertion.
    pub(crate) base_dir: std::path::PathBuf,
}

const FILE_PICKER_EXCLUDES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".tact",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".next",
];

/// Collect entries (files and directories) in `dir`. Directories are returned
/// with a trailing `/`. The returned paths are relative to `dir`.
fn collect_entries(dir: &std::path::Path, base: &std::path::Path) -> Vec<String> {
    let mut options = Vec::new();
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(it) => it.flatten().collect(),
        Err(_) => return options,
    };
    entries.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        a_is_dir
            .cmp(&b_is_dir)
            .reverse()
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    if dir != base {
        if let Some(parent) = dir.parent() {
            if parent.starts_with(base) || parent == base {
                options.push("../".to_string());
            }
        }
    }

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || FILE_PICKER_EXCLUDES.contains(&name) {
            continue;
        }
        if path.is_dir() {
            options.push(format!("{}/", name));
        } else {
            options.push(name.to_string());
        }
    }
    options
}

impl FilePicker {
    pub(crate) fn new() -> Self {
        Self {
            options: Vec::new(),
            selected: 0,
            query: String::new(),
            current_dir: std::path::PathBuf::new(),
            base_dir: std::path::PathBuf::new(),
        }
    }

    pub(crate) fn set_dir(
        &mut self,
        current_dir: std::path::PathBuf,
        base_dir: std::path::PathBuf,
    ) {
        self.current_dir = current_dir;
        self.base_dir = base_dir;
        self.refresh();
    }

    pub(crate) fn refresh(&mut self) {
        let mut options = collect_entries(&self.current_dir, &self.base_dir);
        if !self.query.is_empty() {
            let query = self.query.to_lowercase();
            options.retain(|e| e.to_lowercase().contains(&query));
        }
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

    /// Returns true if the selected entry is a directory.
    pub(crate) fn selected_is_dir(&self) -> bool {
        self.selected_path()
            .map(|p| p.ends_with('/'))
            .unwrap_or(false)
    }

    /// Remove the last character from the query; if the query is empty,
    /// navigate up to the parent directory (stopping at base_dir).
    pub(crate) fn backspace(&mut self) {
        if self.query.pop().is_none() && self.current_dir != self.base_dir {
            if let Some(parent) = self.current_dir.parent() {
                self.current_dir = parent.to_path_buf();
            }
        }
        self.refresh();
    }

    /// Append a character to the filter query.
    pub(crate) fn push_query(&mut self, c: char) {
        self.query.push(c);
        self.refresh();
    }

    /// Navigate into the selected directory. The name is expected to include
    /// the trailing `/`.
    pub(crate) fn enter_selected_dir(&mut self, name: &str) {
        let name = name.trim_end_matches('/');
        if name.is_empty() || name == ".." {
            if let Some(parent) = self.current_dir.parent() {
                self.current_dir = parent.to_path_buf();
            }
        } else {
            self.current_dir = self.current_dir.join(name);
        }
        self.query.clear();
        self.refresh();
    }
}
