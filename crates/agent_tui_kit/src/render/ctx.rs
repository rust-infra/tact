//! `RenderCtx` — the shared, read-only surface pure render functions take
//! instead of `&App`.
//!
//! Built per-frame by the app from disjoint `&` borrows of `App`; the only
//! mutation path from render code is the explicit `Vec<RenderCommand>`
//! drained by the app after the frame.
//!
//! Design: `docs/superpowers/specs/2026-08-18-tui-component-library-ctx-design.md`.

use ratatui::{layout::Rect, style::Style};

use crate::{
    i18n::{Language, Messages},
    state::{
        AccountState, CodeBlock, FocusedPanel, InputMode, LogCoordinator, LogScroll, MermaidBlock,
        MouseState, PlanPanel, SkillEntry, Status, StatusBarState, StreamState, ThinkingState,
        ToolState,
    },
    theme::Theme,
};

/// Shared read surface for render functions.
///
/// Grows as more render panels migrate from `&App` (see the design doc's
/// migration order); fields are added one panel at a time.
pub struct RenderCtx<'a> {
    pub theme: &'a Theme,
    /// Owned copy (all-`&'static str`, built once per frame from the language).
    pub messages: Messages,
    pub log_scroll: &'a LogScroll,
    pub log: &'a LogCoordinator,
    pub code_blocks: &'a [CodeBlock],
    pub mermaid_blocks: &'a [MermaidBlock],
    pub tools: &'a ToolState,
    pub thinking: &'a ThinkingState,
    pub stream: &'a StreamState,
    pub mouse: &'a MouseState,
    pub skills_data: &'a [SkillEntry],
    /// Loading placeholder row index, if present.
    pub loading_idx: Option<usize>,
    /// Spinner animation frame counter.
    pub spinner_frame: u8,
    // ── Status/bottom bar surface (migrated with `render/bar.rs`) ──
    pub status_bar: &'a StatusBarState,
    pub status: &'a Status,
    pub input_mode: InputMode,
    pub focused_panel: FocusedPanel,
    pub language: Language,
    pub workspace_dir: &'a str,
    pub model_context_window: usize,
    pub process_start_time: &'a chrono::DateTime<chrono::Local>,
    pub task_start_time: Option<&'a chrono::DateTime<chrono::Local>>,
    /// Transient flash message text (the expiry `Instant` lives in the app).
    pub flash_msg: Option<&'a str>,
    /// Account balance/quota surface; `None` when the host has no account
    /// channel (renders no `¤` segment on the bottom bar).
    pub account: Option<&'a AccountState>,
    pub plan: &'a PlanPanel,
    // ── Input surface (migrated with `render/input.rs`) ──
    pub input: &'a str,
    pub input_cursor: usize,
    /// Vertical scroll of the input box, updated by the host's prepare phase.
    pub input_scroll: u16,
    /// Palette-mode command line text.
    pub cmd_line: &'a str,
    /// Codex-style queued messages (hint rows above the input box).
    pub pending_messages: &'a [crate::PendingMessage],
    /// App-layer voice button title (extension slot; `None` when disabled).
    pub input_voice_title: Option<(String, Style)>,
}

/// A command emitted by render code, executed by the app after the frame.
///
/// `AgentUpdate` is a large protocol enum; boxed because commands are
/// low-frequency (UI gestures), not a hot path.
#[allow(clippy::large_enum_variant)]
pub enum RenderCommand {
    /// Replay an agent update from a UI gesture (double-click → copy/`MdInfo`).
    AgentUpdate(crate::protocol::AgentUpdate),
    /// Enqueue a pending input message.
    QueuePending(String),
    /// Set the pending `[Cancel]` button hit area.
    SetCancelButtonArea(Rect),
    /// Mark the frame dirty.
    Repaint,
}
