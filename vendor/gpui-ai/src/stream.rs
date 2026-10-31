//! Shared lifecycle and revision tracking for progressively arriving content.
//!
//! Applications own a [`Progressive`] value and mutate it as work advances.
//! Components receive snapshots, so no component needs its own timer or
//! competing lifecycle state machine.

use gpui::SharedString;

/// Lifecycle shared by every progressive component.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProgressState {
    /// Work has been declared but has not started.
    #[default]
    Pending,
    /// Work is active and its content may still change.
    Running,
    /// Work completed successfully.
    Complete,
    /// Work stopped with a user-visible reason.
    Failed(SharedString),
}

/// Typed content paired with a shared lifecycle and monotonic revision.
///
/// The revision changes only when lifecycle or content actually changes. It
/// can therefore be used as a cheap cache key by expensive renderers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progressive<T> {
    content: T,
    state: ProgressState,
    revision: u64,
}

impl<T> Progressive<T> {
    /// Creates pending progressive content.
    pub fn pending(content: T) -> Self {
        Self::with_state(content, ProgressState::Pending)
    }

    /// Creates running progressive content.
    pub fn running(content: T) -> Self {
        Self::with_state(content, ProgressState::Running)
    }

    /// Creates completed progressive content.
    pub fn complete(content: T) -> Self {
        Self::with_state(content, ProgressState::Complete)
    }

    /// Creates failed progressive content.
    pub fn failed(content: T, reason: impl Into<SharedString>) -> Self {
        Self::with_state(content, ProgressState::Failed(reason.into()))
    }

    fn with_state(content: T, state: ProgressState) -> Self {
        Self {
            content,
            state,
            revision: 0,
        }
    }

    /// Returns the current typed content.
    pub fn content(&self) -> &T {
        &self.content
    }

    /// Returns the current lifecycle.
    pub fn state(&self) -> &ProgressState {
        &self.state
    }

    /// Returns the monotonic content/lifecycle revision.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Moves pending or terminal work into the running state.
    pub fn start(&mut self) {
        self.set_state(ProgressState::Running);
    }

    /// Marks work complete.
    pub fn finish(&mut self) {
        self.set_state(ProgressState::Complete);
    }

    /// Marks work failed with a user-visible reason.
    pub fn fail(&mut self, reason: impl Into<SharedString>) {
        self.set_state(ProgressState::Failed(reason.into()));
    }

    fn set_state(&mut self, state: ProgressState) {
        if self.state != state {
            self.state = state;
            self.bump_revision();
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

/// Progressively arriving text.
///
/// # Example
///
/// ```
/// use gpui_ai::stream::{ProgressState, StreamedContent};
///
/// let mut answer = StreamedContent::new();
/// answer.append("Hello, ");
/// answer.append("world.");
/// answer.finish();
/// assert_eq!(answer.text(), "Hello, world.");
/// assert_eq!(answer.state(), &ProgressState::Complete);
/// ```
pub type StreamedContent = Progressive<String>;

impl Progressive<String> {
    /// Creates empty running text.
    pub fn new() -> Self {
        Self::running(String::new())
    }

    /// Creates text that is already complete.
    pub fn done(text: impl Into<String>) -> Self {
        Self::complete(text.into())
    }

    /// Returns the accumulated text.
    pub fn text(&self) -> &str {
        &self.content
    }

    /// Appends a non-empty chunk and advances the revision.
    pub fn append(&mut self, chunk: &str) {
        if !chunk.is_empty() {
            self.content.push_str(chunk);
            self.bump_revision();
        }
    }

    /// Replaces text when the replacement differs and advances the revision.
    pub fn replace(&mut self, text: impl Into<String>) {
        let text = text.into();
        if self.content != text {
            self.content = text;
            self.bump_revision();
        }
    }

    /// Returns whether the text is still arriving.
    pub fn is_streaming(&self) -> bool {
        self.state == ProgressState::Running
    }
}

impl Default for Progressive<String> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressState, Progressive, StreamedContent};

    #[test]
    fn constructors_preserve_content_and_initial_state() {
        let pending = Progressive::pending(vec![1, 2]);
        let running = Progressive::running(vec![3]);
        let complete = Progressive::complete(vec![4]);
        let failed = Progressive::failed(vec![5], "offline");

        assert_eq!(pending.content(), &[1, 2]);
        assert_eq!(pending.state(), &ProgressState::Pending);
        assert_eq!(running.state(), &ProgressState::Running);
        assert_eq!(complete.state(), &ProgressState::Complete);
        assert_eq!(failed.state(), &ProgressState::Failed("offline".into()));
        assert_eq!(pending.revision(), 0);
    }

    #[test]
    fn lifecycle_revision_changes_only_for_real_transitions() {
        let mut value = Progressive::pending(());
        value.start();
        assert_eq!(value.revision(), 1);
        value.start();
        assert_eq!(value.revision(), 1);
        value.finish();
        assert_eq!(value.revision(), 2);
        value.finish();
        assert_eq!(value.revision(), 2);
        value.fail("late failure");
        assert_eq!(value.revision(), 3);
        value.fail("late failure");
        assert_eq!(value.revision(), 3);
    }

    #[test]
    fn text_mutations_track_meaningful_revisions() {
        let mut text = StreamedContent::new();
        assert_eq!(text.state(), &ProgressState::Running);
        text.append("");
        assert_eq!(text.revision(), 0);
        text.append("hello");
        assert_eq!(text.text(), "hello");
        assert_eq!(text.revision(), 1);
        text.replace("hello");
        assert_eq!(text.revision(), 1);
        text.replace("hello, world");
        assert_eq!(text.text(), "hello, world");
        assert_eq!(text.revision(), 2);
        text.finish();
        assert!(!text.is_streaming());
        assert_eq!(text.revision(), 3);
    }
}
