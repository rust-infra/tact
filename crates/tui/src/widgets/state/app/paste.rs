//! Handling a paste that carries files or an image rather than plain text.
//!
//! A terminal hands a paste over as text. Two payloads have no text form a
//! terminal can keep: a screenshot (a bitmap) and a file-manager copy on a
//! compositor that publishes only `text/uri-list`. Both are read straight from
//! the pasteboard here and become `@` references in the input box.
//!
//! The rule is **fallback, never interception**: whatever the pasteboard
//! cannot turn into a reference, the caller still pastes as text. A headless or
//! SSH session has no clipboard at all, and a paste there must keep working.

use crate::{
    clipboard_paste::{self, Snapshot},
    widgets::state::{App, InputMode},
};

/// What one paste event should do.
pub(crate) enum PasteAction {
    /// Insert this text at the cursor: the ordinary text path, unchanged.
    Text(String),
    /// Insert these `@` references instead, and report how many files had to be
    /// copied into the workspace first.
    References {
        references: Vec<String>,
        copied: usize,
    },
}

impl App {
    /// Decide what a paste event carries.
    ///
    /// `terminal_text` is what the terminal already decoded. When it is
    /// **non-empty** it *is* the paste, and the pasteboard is not consulted:
    /// bracketed paste hands over the payload, and a screenshot sitting in the
    /// clipboard must not replace text the user deliberately pasted from a
    /// selection.
    ///
    /// The pasteboard is read only when the terminal forwarded nothing —
    /// pasting a screenshot sends an empty payload, because a bitmap has no
    /// text form to forward — or when the text is a `file://` list.
    pub(crate) fn resolve_paste(&mut self, terminal_text: Option<&str>) -> PasteAction {
        let terminal_text = terminal_text.unwrap_or_default();

        let text_references = clipboard_paste::files_in_uri_text(terminal_text);
        if !text_references.is_empty() {
            return self.references_for(text_references);
        }
        if !terminal_text.is_empty() {
            return PasteAction::Text(terminal_text.to_string());
        }

        let Some(snapshot) = self.take_clipboard_snapshot() else {
            return PasteAction::Text(String::new());
        };
        if !snapshot.files.is_empty() || snapshot.image.is_some() {
            return self.references_from_snapshot(snapshot);
        }

        // An empty terminal payload and nothing referenceable on the pasteboard:
        // a paste the app has nothing to do with.
        PasteAction::Text(snapshot.text.unwrap_or_default())
    }

    fn references_for(&self, paths: Vec<std::path::PathBuf>) -> PasteAction {
        let mut references = Vec::new();
        let mut copied = 0;
        for path in paths {
            let Some(reference) = clipboard_paste::reference_for(&self.work_dir, &path) else {
                continue;
            };
            copied += usize::from(reference.copied);
            references.push(reference.text);
        }

        if references.is_empty() {
            // Every path was unusable (deleted, a directory). Keep whatever the
            // terminal sent so the user does not lose their paste silently.
            return PasteAction::Text(String::new());
        }
        PasteAction::References { references, copied }
    }

    /// Read the pasteboard, unless the input box is not the paste target.
    ///
    /// Copying are still clipboard reads, so a paste into a picker or the
    /// command palette must not pop an attachment reference into an unrelated
    /// field.
    fn take_clipboard_snapshot(&self) -> Option<Snapshot> {
        matches!(self.input_mode, InputMode::Insert).then(clipboard_paste::snapshot)
    }

    /// Read the pasteboard and paste whatever it holds.
    ///
    /// Returns whether anything was pasted. `false` means the pasteboard was
    /// unavailable or empty, and the caller lets the terminal's own handling
    /// run instead.
    ///
    /// A file or an image becomes an `@` reference. Plain text is inserted
    /// directly, because a terminal that never sends `Event::Paste` (no
    /// bracketed paste) would otherwise lose `Ctrl+V` entirely — the key is
    /// swallowed by raw mode, so nothing else can deliver the text.
    pub(crate) fn paste_from_clipboard(&mut self) -> bool {
        if !matches!(self.input_mode, InputMode::Insert) {
            return false;
        }
        let Some(snapshot) = self.take_clipboard_snapshot() else {
            return false;
        };
        if !snapshot.files.is_empty() || snapshot.image.is_some() {
            self.apply_paste(self.references_from_snapshot(snapshot), None);
            return true;
        }
        match snapshot.text {
            Some(text) if !text.is_empty() => {
                self.paste_text(&text);
                true
            }
            _ => false,
        }
    }

    /// Turn a snapshot that has a file or an image into a [`PasteAction`].
    fn references_from_snapshot(&self, snapshot: Snapshot) -> PasteAction {
        if !snapshot.files.is_empty() {
            return self.references_for(snapshot.files);
        }
        let Some((width, height, rgba)) = snapshot.image else {
            return PasteAction::Text(String::new());
        };
        match clipboard_paste::save_image(&self.work_dir, width, height, &rgba) {
            Some(reference) => PasteAction::References {
                references: vec![reference.text],
                copied: usize::from(reference.copied),
            },
            None => PasteAction::Text(String::new()),
        }
    }

    /// Apply a resolved paste in the input box.
    ///
    /// `terminal_text` is only consulted for the `Palette` mode and for the
    /// text fallback, which is why it is passed in rather than carried on
    /// [`PasteAction`]: a `References` action has already consumed the payload.
    pub(crate) fn apply_paste(&mut self, action: PasteAction, terminal_text: Option<&str>) {
        match action {
            PasteAction::References { references, copied } => {
                if matches!(self.input_mode, InputMode::Insert) {
                    self.insert_paste_references(&references, copied);
                } else {
                    // A picker has no `@` grammar; keep the paste as text so the
                    // keystroke still does the obvious thing.
                    self.paste_text(terminal_text.unwrap_or_default());
                }
            }
            PasteAction::Text(text) => self.paste_text(&text),
        }
    }

    /// The pre-existing paste behaviour: insert at the cursor, preserving
    /// newlines in Insert mode and flattening them for the palette.
    fn paste_text(&mut self, data: &str) {
        match self.input_mode {
            InputMode::Insert => {
                let cursor = self
                    .input
                    .floor_char_boundary(self.input_cursor.min(self.input.len()));
                let before = self.input[..cursor].to_string();
                let after = self.input[cursor..].to_string();
                self.input = before.clone() + data + &after;
                self.input_cursor = before.len() + data.len();
            }
            InputMode::Palette => {
                let text: String = data.replace(['\n', '\r'], " ");
                self.cmd_line.push_str(&text);
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Insert `references` at the cursor as one space-separated run, then
    /// report any copy that happened.
    pub(crate) fn insert_paste_references(&mut self, references: &[String], copied: usize) {
        let insertion = references.join(" ");
        let separated = self.needs_separator(&insertion);

        self.save_undo();
        let cursor = self
            .input
            .floor_char_boundary(self.input_cursor.min(self.input.len()));
        let prefix = if separated { " " } else { "" };
        let after = self.input[cursor..].to_string();
        // A trailing space keeps the next typed word out of the reference's
        // `@` token, which would otherwise swallow it as part of the path.
        self.input = format!("{}{prefix}{insertion} {after}", &self.input[..cursor]);
        self.input_cursor = cursor + prefix.len() + insertion.len() + 1;
        self.input_history.index = None;
        self.input_history.saved.clear();

        if copied > 0 {
            let message = self
                .msgs()
                .pasted_attachment_tmpl
                .replace("{}", &copied.to_string());
            self.add_system_message(message);
        }
        self.dirty = true;
    }

    /// Whether `insertion` needs a space before it to stay a separate token.
    fn needs_separator(&self, insertion: &str) -> bool {
        let cursor = self
            .input
            .floor_char_boundary(self.input_cursor.min(self.input.len()));
        let previous = self.input[..cursor].chars().next_back();
        let next = insertion.chars().next();
        matches!((previous, next), (Some(before), Some(after))
            if !before.is_whitespace() && !after.is_whitespace())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::PasteAction;
    use crate::{test_fixtures::TestApp, widgets::state::InputMode};

    #[test]
    fn plain_text_pastes_unchanged() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input_mode = InputMode::Insert;

        let action = app.resolve_paste(Some("hello"));

        assert!(matches!(action, PasteAction::Text(ref text) if text == "hello"));
    }

    #[test]
    fn a_uri_list_paste_becomes_references_not_text() {
        let workspace = tempfile::tempdir().unwrap();
        let shot = workspace.path().join("shot.png");
        fs::write(&shot, b"x").unwrap();

        let (mut app, _rx) = TestApp::new().into_commands();
        app.work_dir = workspace.path().to_path_buf();
        app.input_mode = InputMode::Insert;

        let action = app.resolve_paste(Some(&format!("file://{}", shot.display())));

        match action {
            PasteAction::References { references, copied } => {
                assert_eq!(references, vec!["@shot.png".to_string()]);
                assert_eq!(copied, 0, "the file was already in the workspace");
            }
            PasteAction::Text(text) => panic!("expected a reference, got text {text:?}"),
        }
    }

    #[test]
    fn pasted_prose_wins_over_a_screenshot_left_on_the_clipboard() {
        // The bug the first draft had: any non-`file://` text fell through to
        // the pasteboard, so a screenshot copy sitting in the clipboard turned
        // a deliberate text paste into an image reference. The terminal's text
        // *is* the paste; the pasteboard is only a fallback for an empty one.
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input_mode = InputMode::Insert;

        let action = app.resolve_paste(Some("fn main() {}"));

        assert!(
            matches!(action, PasteAction::Text(ref text) if text == "fn main() {}"),
            "a deliberate text paste must not become a reference"
        );
    }

    #[test]
    fn an_empty_terminal_payload_falls_back_to_the_pasteboard() {
        // Pasting a screenshot sends nothing the terminal can forward; the
        // empty payload is what tells the app to look at the pasteboard. The
        // payload of *this* machine's clipboard is whatever the developer left
        // there, so the assertion is only that nothing panics and that text
        // never arrives as a reference.
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input_mode = InputMode::Insert;

        let action = app.resolve_paste(Some(""));

        assert!(
            matches!(
                action,
                PasteAction::Text(_) | PasteAction::References { .. }
            ),
            "an empty payload must resolve without panicking"
        );
    }

    #[test]
    fn a_paste_outside_insert_mode_is_never_read_from_the_pasteboard() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input_mode = InputMode::Palette;

        let action = app.resolve_paste(Some(""));

        assert!(
            matches!(action, PasteAction::Text(ref text) if text.is_empty()),
            "a picker or palette must not receive an attachment reference"
        );
    }
    #[test]
    fn references_land_at_the_cursor_with_separators() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input = "look atthis".to_string();
        app.input_cursor = "look at".len();

        app.insert_paste_references(&["@a.png".to_string(), "@b.png".to_string()], 0);

        assert_eq!(app.input, "look at @a.png @b.png this");
    }

    #[test]
    fn the_copied_notice_names_the_attachment_directory() {
        let (mut app, _rx) = TestApp::new().into_commands();
        app.input.clear();
        app.input_cursor = 0;

        app.insert_paste_references(&["@.tact/attachments/a.png".to_string()], 2);

        let messages: Vec<&str> = app.log.items.iter().map(|item| item.raw.as_str()).collect();
        assert!(
            messages
                .iter()
                .any(|line| line.contains(".tact/attachments")),
            "the user must learn their file was copied somewhere else: {messages:?}"
        );
    }
}
