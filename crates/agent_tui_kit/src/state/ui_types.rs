//! Inline UI types extracted from `App`'s definition module.
//!
//! Pure data types with no `App` / agent-loop / provider coupling. App-layer
//! types that depend on `tact`/`tact_llm` (e.g. `SelectKind`, palette command
//! tables) stay in the consuming app.

use ratatui::text::Line;

use super::PopupTextSelection;

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
    /// Markdown body after frontmatter (from disk at load / skill reload time).
    pub body: String,
    /// Directory holding this skill's `SKILL.md`.
    ///
    /// Carried into the `<skill>` wrapper on invocation so the model can
    /// resolve a bundled `references/` / `scripts/` path. Never sent in the
    /// system prompt's catalog — see the catalog-budget spec.
    pub dir: String,
    /// Kept out of the system prompt's catalog by `[agent].skill_disable`.
    ///
    /// Still invocable by name; `/skill list` marks the row rather than hiding
    /// it, because a row that silently vanishes is indistinguishable from a
    /// broken install.
    pub disabled: bool,
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
    /// Stable identity used by popup state; independent of vector position.
    pub block_id: String,
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
/// the diagram rows themselves. The Open affordance opens [`MermaidPopup`] so the
/// original fence body can be copied.
#[derive(Debug, Clone)]
pub struct MermaidBlock {
    /// Stable identity used by popup state; independent of vector position.
    pub block_id: String,
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
    pub block_id: String,
    pub lang: String,
    pub scroll: u16,
}

/// Which representation the Mermaid popup shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MermaidPopupView {
    /// Rendered terminal diagram, re-laid out at the popup's (wider) width.
    #[default]
    Diagram,
    /// Original fence body, so it can be read/copied verbatim.
    Source,
}

impl MermaidPopupView {
    /// Cycles [`Diagram`](Self::Diagram) → [`Source`](Self::Source) → ….
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Diagram => Self::Source,
            Self::Source => Self::Diagram,
        }
    }

    /// Footer label for the key that switches *to the other* view.
    #[must_use]
    pub fn toggle_label(self) -> &'static str {
        match self {
            Self::Diagram => " source ",
            Self::Source => " diagram ",
        }
    }
}

/// Mermaid popup opened from a rendered diagram's Open affordance.
///
/// Opens on the rendered [`Diagram`](MermaidPopupView::Diagram) at the popup's
/// width — wider than the log panel, so dense flowcharts stay readable — with
/// `Tab` switching to the [`Source`](MermaidPopupView::Source) fence body for
/// copying.
#[derive(Debug, Clone)]
pub struct MermaidPopup {
    pub block_id: String,
    pub scroll: u16,
    /// Diagram vs. raw source (`Tab`).
    pub view: MermaidPopupView,
}

impl MermaidPopup {
    /// Creates a popup for `block_id`, opened on the rendered diagram.
    #[must_use]
    pub fn new(block_id: String) -> Self {
        Self {
            block_id,
            scroll: 0,
            view: MermaidPopupView::default(),
        }
    }
}

/// System-prompt / session-stats popup state.
///
/// The read-out popup every `/`-command listing opens (`/stats`, `/background`,
/// `/view-system-prompt`, `/goal show`, `/mcp list`, `/hooks list` …). Its body
/// is *rendered* Markdown, so a text selection indexes into the rendered lines
/// rather than the raw source — the same split the thinking popup makes, and
/// why the copy buffer is cached at render time.
#[derive(Debug, Clone)]
pub struct SystemPromptPopup {
    pub title: String,
    /// Raw Markdown source; laid out width-aware at popup render time.
    pub source: String,
    pub scroll: u16,
    /// Drag selection over the rendered body, if any.
    pub selection: Option<PopupTextSelection>,
    /// The rendered text the selection indexes into. Written during the render
    /// pass, because only the layout knows where the rendered lines end.
    pub selection_text: String,
}

impl SystemPromptPopup {
    pub fn new(title: String, source: String) -> Self {
        Self {
            title,
            source,
            scroll: 0,
            selection: None,
            selection_text: String::new(),
        }
    }

    /// The selected slice, or the whole source when nothing is selected — the
    /// same contract the thinking popup's `y` follows.
    pub fn copy_content(&self, full_content: &str) -> String {
        self.selection
            .and_then(|selection| selection.normalized_non_empty(&self.selection_text))
            .map(|range| self.selection_text[range].to_string())
            .unwrap_or_else(|| full_content.to_string())
    }
}

/// Current agent execution state, driving the status bar and UI feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Planning,
    Executing { current_step: usize, total: usize },
    Done,
}

/// Which domain the sticky strip under the Log is showing.
///
/// The sticky host has four domains: durable Goals, persistent tasks, the
/// current-process subagent overview, and current-process background tasks. A domain is
/// only drawn when it is visible; `active` selects which expanded body / scroll
/// the host renders when more than one is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StickyTab {
    #[default]
    Goals,
    Tasks,
    Subagent,
    /// Running `background_run` tasks, derived from the live tool cards (no
    /// protocol snapshot drives this domain).
    Background,
}

impl StickyTab {
    /// All sticky domains in their stable display order.
    pub const ALL: [Self; 4] = [Self::Goals, Self::Tasks, Self::Subagent, Self::Background];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `y` copies the drag selection when there is one, and the source the
    /// popup was built from when there is not.
    #[test]
    fn copy_content_prefers_the_selection_over_the_source() {
        let mut popup =
            SystemPromptPopup::new("Goal: review code".into(), "**ID:** `216ae6be`".into());
        assert_eq!(
            popup.copy_content("**ID:** `216ae6be`"),
            "**ID:** `216ae6be`",
            "no selection copies the source"
        );

        // The selection indexes the *rendered* text, which is why `copy_content`
        // needs the cached buffer rather than the raw source.
        popup.selection_text = "ID: 216ae6be".into();
        popup.selection = Some(PopupTextSelection::new(4, 12));
        assert_eq!(popup.copy_content("**ID:** `216ae6be`"), "216ae6be");
    }
}
