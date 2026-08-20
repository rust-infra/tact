//! Inline UI types extracted from `App`'s definition module.
//!
//! Pure data types with no `App` / agent-loop / provider coupling. App-layer
//! types that depend on `tact`/`tact_llm` (e.g. `SelectKind`, palette command
//! tables) stay in the consuming app.

use ratatui::text::Line;

/// Current keyboard input mode, determining how key presses are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    /// Vim-like normal mode (default).
    #[default]
    Normal,
    /// Text entry.
    Insert,
    /// Slash-command palette.
    Palette,
    /// List selection popup.
    Select,
    /// File picker.
    FilePicker,
}

/// Which panel has keyboard focus (currently only the Log).
#[derive(Clone, Copy, PartialEq)]
pub enum FocusedPanel {
    Log,
}

/// A skill available in the slash / palette picker.
#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    /// Markdown body after frontmatter (from disk at load / skill-reload time).
    pub body: String,
}

/// A persisted task-history entry.
#[derive(Clone)]
pub struct HistoryEntry {
    pub task: String,
    pub timestamp: String,
    pub summary: String,
}

/// A completed LLM code block, rendered as a card overlay in the log panel.
#[derive(Debug, Clone)]
pub struct CodeBlock {
    /// First placeholder line index in messages (inclusive).
    pub start_idx: usize,
    /// One-past-last placeholder line index in messages.
    pub end_idx: usize,
    pub lang: String,
    /// Raw source lines (without ``` fences), used for copy and rendering.
    pub content: String,
    /// Pre-rendered styled lines for the card interior.
    pub styled: Vec<Line<'static>>,
}

/// A successfully rendered Mermaid diagram spliced into the log as terminal art.
///
/// Unlike [`CodeBlock`], there is no card chrome — `start_idx..end_idx` covers
/// the diagram rows themselves. Double-click opens [`MermaidPopup`] so the
/// original fence body can be copied.
#[derive(Debug, Clone)]
pub struct MermaidBlock {
    /// First diagram line index in messages (inclusive).
    pub start_idx: usize,
    /// One-past-last diagram line index in messages.
    pub end_idx: usize,
    /// Fence body only (no ```mermaid / closing ```).
    pub source: String,
}

/// Code block popup state (similar to ThinkingPopup / DiffPopup).
#[derive(Debug, Clone)]
pub struct CodePopup {
    pub block_idx: usize,
    pub lang: String,
    pub scroll: u16,
}

/// Mermaid source popup (double-click a rendered diagram in the log).
#[derive(Debug, Clone)]
pub struct MermaidPopup {
    pub block_idx: usize,
    pub scroll: u16,
}

/// System-prompt / session-stats popup state.
#[derive(Debug, Clone)]
pub struct SystemPromptPopup {
    pub title: String,
    /// Raw Markdown source; laid out width-aware at popup render time.
    pub source: String,
    pub scroll: u16,
}

/// Current agent execution state, driving the status bar and UI feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Planning,
    Executing { current_step: usize, total: usize },
    Done,
}
