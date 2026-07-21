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

    if dir != base
        && let Some(parent) = dir.parent()
        && (parent.starts_with(base) || parent == base)
    {
        options.push("../".to_string());
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
        if self.query.pop().is_none()
            && self.current_dir != self.base_dir
            && let Some(parent) = self.current_dir.parent()
        {
            self.current_dir = parent.to_path_buf();
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
            if self.current_dir != self.base_dir
                && let Some(parent) = self.current_dir.parent()
            {
                self.current_dir = parent.to_path_buf();
            }
        } else {
            self.current_dir = self.current_dir.join(name);
        }
        self.query.clear();
        self.refresh();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    // ─── Pure unit tests (no filesystem) ───────────────────────────────────

    #[test]
    fn new_creates_empty_state() {
        let fp = FilePicker::new();
        assert!(fp.options.is_empty());
        assert_eq!(fp.selected, 0);
        assert!(fp.query.is_empty());
        assert_eq!(fp.current_dir, PathBuf::new());
        assert_eq!(fp.base_dir, PathBuf::new());
    }

    #[test]
    fn move_up_at_zero_stays_zero() {
        let mut fp = FilePicker::new();
        fp.options = vec!["a".into(), "b".into()];
        fp.selected = 0;

        fp.move_up();
        assert_eq!(fp.selected, 0);

        fp.move_up();
        fp.move_up();
        assert_eq!(fp.selected, 0);
    }

    #[test]
    fn move_down_at_last_stays_last() {
        let mut fp = FilePicker::new();
        fp.options = vec!["a".into(), "b".into()];
        fp.selected = 1;

        fp.move_down();
        assert_eq!(fp.selected, 1);

        fp.move_down();
        fp.move_down();
        assert_eq!(fp.selected, 1);
    }

    #[test]
    fn move_down_does_nothing_when_empty() {
        let mut fp = FilePicker::new();
        fp.selected = 0;

        fp.move_down();
        assert_eq!(fp.selected, 0);
    }

    #[test]
    fn selected_path_returns_none_when_empty() {
        let fp = FilePicker::new();
        assert!(fp.selected_path().is_none());
    }

    #[test]
    fn selected_is_dir_returns_false_when_empty() {
        let fp = FilePicker::new();
        assert!(!fp.selected_is_dir());
    }

    #[test]
    fn selected_path_returns_current_option() {
        let fp = FilePicker {
            options: vec!["src/".into(), "main.rs".into()],
            selected: 1,
            ..FilePicker::new()
        };
        assert_eq!(fp.selected_path(), Some("main.rs"));
    }

    #[test]
    fn selected_is_dir_checks_trailing_slash() {
        let fp = FilePicker {
            options: vec!["src/".into(), "main.rs".into()],
            selected: 0,
            ..FilePicker::new()
        };
        assert!(fp.selected_is_dir());

        let fp = FilePicker { selected: 1, ..fp };
        assert!(!fp.selected_is_dir());
    }

    #[test]
    fn push_query_updates_query_string() {
        let mut fp = FilePicker::new();
        fp.push_query('a');
        assert_eq!(fp.query, "a");
        fp.push_query('b');
        assert_eq!(fp.query, "ab");
    }

    // ─── Navigation tests (no real filesystem needed) ──────────────────────
    // refresh() will try to read current_dir, but an empty PathBuf or a
    // non-existent path simply yields zero entries, so these are safe.

    #[test]
    fn backspace_with_non_empty_query_only_pops_char() {
        let mut fp = FilePicker::new();
        fp.query = "ab".into();

        fp.backspace();
        assert_eq!(fp.query, "a");
    }

    #[test]
    fn backspace_clears_query_then_stays_at_base_dir() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");
        fp.query = "x".into();

        fp.backspace();
        assert_eq!(fp.query, "");
        // Should NOT navigate up since query was popped first
        assert_eq!(fp.current_dir, PathBuf::from("/project"));
    }

    #[test]
    fn backspace_with_empty_query_at_base_dir_does_not_navigate_up() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");

        fp.backspace();
        // current_dir unchanged
        assert_eq!(fp.current_dir, PathBuf::from("/project"));
    }

    #[test]
    fn enter_selected_dir_skips_parent_when_at_base_dir() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");

        // Simulate what would happen if "../" were selected (defensive guard)
        fp.enter_selected_dir("../");
        // Should NOT go above base_dir
        assert_eq!(fp.current_dir, PathBuf::from("/project"));
    }

    #[test]
    fn enter_selected_dir_empty_trimmed_stays_at_base_dir() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");

        fp.enter_selected_dir("/");
        assert_eq!(fp.current_dir, PathBuf::from("/project"));
    }

    #[test]
    fn enter_selected_dir_joins_subdirectory() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");

        fp.enter_selected_dir("src/");
        assert_eq!(fp.current_dir, PathBuf::from("/project/src"));
    }

    #[test]
    fn enter_selected_dir_clears_query() {
        let mut fp = FilePicker::new();
        fp.current_dir = PathBuf::from("/project");
        fp.base_dir = PathBuf::from("/project");
        fp.query = "fil".into();

        fp.enter_selected_dir("src/");
        assert!(fp.query.is_empty());
    }

    // ─── Filesystem-backed tests ───────────────────────────────────────────

    fn create_temp_picker() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().expect("temp dir");
        let root = tmp
            .path()
            .canonicalize()
            .unwrap_or_else(|_| tmp.path().to_path_buf());

        // Create a standard test structure
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap(); // hidden
        std::fs::create_dir_all(root.join("target")).unwrap(); // excluded
        std::fs::File::create(root.join("Cargo.toml")).unwrap();
        std::fs::File::create(root.join("README.md")).unwrap();
        std::fs::File::create(root.join("src").join("lib.rs")).unwrap();
        std::fs::File::create(root.join("src").join("main.rs")).unwrap();
        std::fs::File::create(root.join("docs").join("guide.md")).unwrap();
        std::fs::File::create(root.join("docs").join("api.md")).unwrap();

        (tmp, root)
    }

    #[test]
    fn set_dir_populates_options() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.clone(), root.clone());

        // Directories first (sorted), then files (sorted); excludes hidden/build
        assert_eq!(fp.options, vec!["docs/", "src/", "Cargo.toml", "README.md"]);
    }

    #[test]
    fn set_dir_top_level_shows_no_parent_entry() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.clone(), root.clone());

        // No "../" at top level
        assert!(!fp.options.iter().any(|o| o == "../"));
    }

    #[test]
    fn subdirectory_has_parent_entry() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.join("src"), root.clone());

        assert!(fp.options.iter().any(|o| o == "../"));
    }

    #[test]
    fn refresh_filters_by_query() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.clone(), root.clone());
        assert_eq!(fp.options.len(), 4); // docs/, src/, Cargo.toml, README.md

        fp.query = "doc".into();
        fp.refresh();
        assert_eq!(fp.options, vec!["docs/"]);

        // Case-insensitive
        fp.query = "README".into();
        fp.refresh();
        assert_eq!(fp.options, vec!["README.md"]);

        // No match → empty
        fp.query = "zzzzz".into();
        fp.refresh();
        assert!(fp.options.is_empty());
    }

    #[test]
    fn refresh_resets_selected_to_zero() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.clone(), root.clone());
        fp.selected = 3;

        fp.refresh();
        assert_eq!(fp.selected, 0);
    }

    #[test]
    fn collect_entries_excludes_hidden_and_build_dirs() {
        let (_tmp, root) = create_temp_picker();
        let options = collect_entries(&root, &root);

        assert!(
            !options.iter().any(|o| o == ".git/" || o == "target/"),
            "hidden/excluded dirs should not appear: {options:?}"
        );
    }

    #[test]
    fn backspace_with_empty_query_in_subdir_navigates_up() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.join("src"), root.clone());
        assert!(fp.current_dir.ends_with("src"));

        fp.backspace();
        assert_eq!(fp.current_dir, root);
    }

    #[test]
    fn enter_selected_dir_parent_navigates_up() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.join("src"), root.clone());
        assert!(fp.current_dir.ends_with("src"));

        fp.enter_selected_dir("../");
        assert_eq!(fp.current_dir, root);
    }

    #[test]
    fn enter_selected_dir_subdirectory_navigates_down_and_clears_query() {
        let (_tmp, root) = create_temp_picker();
        let mut fp = FilePicker::new();

        fp.set_dir(root.clone(), root.clone());
        fp.query = "x".into();

        fp.enter_selected_dir("src/");
        assert_eq!(fp.current_dir, root.join("src"));
        assert!(fp.query.is_empty());
    }

    #[test]
    fn collect_entries_returns_empty_for_nonexistent_dir() {
        let options = collect_entries(
            &PathBuf::from("/nonexistent_dir_xyz"),
            &PathBuf::from("/nonexistent_dir_xyz"),
        );
        assert!(options.is_empty());
    }
}
