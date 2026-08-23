//! `agent_tui_kit` — a reusable agent-TUI component kit.
//!
//! This crate is the library extracted from Tact's TUI: thinking cards, tool
//! cards, the streaming markdown log, the popup family, task/plan panels, the
//! input box, and the status/bottom bars, wired together by one contract:
//!
//! - **In:** a stream of [`protocol::AgentUpdate`] events from the host's agent.
//! - **Out:** [`bridge::Command`] values sent through the host's
//!   [`bridge::AgentBridge`] implementation.
//!
//! The kit depends only on `tact_protocol` (the types-only wire contract) and
//! the ratatui/crossterm family. It never depends on a concrete agent.
//!
//! Design doc: `docs/superpowers/specs/2026-08-18-tui-component-library-design.md`.
//! Execution plan: `docs/superpowers/plans/2026-08-18-tui-component-library.md`.
//!
//! # Status
//!
//! Phase 0 skeleton — the [`Component`] trait, [`Ctx`], and bridge signatures
//! below are compile-only drafts; no rendering code has moved in yet.

pub mod bridge;
pub mod components;
pub mod i18n;
pub mod protocol;
pub mod render;
pub mod state;
pub mod theme;
pub mod widgets;

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

/// Cross-component access: the minimal shared surface a component may touch.
///
/// Components **never** see each other's state. Shared state lives here and is
/// owned by the shell / the `LogCoordinator` (priority-0 component).
///
/// TODO(Phase 3): add `theme: &'a Theme`, `messages: &'a Messages`,
/// `scroll: &'a mut LogScroll` when those modules move into the kit.
pub struct Ctx<'a> {
    /// Shared log rows (user/assistant/system) — owned by the coordinator.
    pub log: &'a mut LogCoordinator,
    /// Current keyboard input mode, deciding how key presses are interpreted.
    pub input_mode: InputMode,
    /// Messages queued while the agent is busy (submitted on idle).
    pub pending: &'a mut PendingQueue,
    /// Outbox for parsed stream events: components push, the shell applies
    /// them to the log/UI after the update dispatch.
    pub stream_events: &'a mut Vec<crate::state::StreamEvent>,
    /// Outbox for tool-lifecycle events (placeholder-row allocation/resize/
    /// finalize): the `ToolComponent` pushes, the shell applies the log
    /// side effects (phys_idx allocation, gap rows, scroll) after dispatch.
    pub tool_events: &'a mut Vec<crate::components::tool::ToolEvent>,
}

/// Shared-log ownership + model (priority-0 component), re-exported at the
/// crate root for the `Ctx` / prelude.
pub use state::{InputMode, LogCoordinator, LogItem, LogItemKind, SystemMsgStyle};

/// A message queued while the agent is busy (Codex-style submit-on-idle).
///
/// Carries both the text shown in the pending block (`display`) and the text
/// dispatched to the agent (`agent_task`) so the host can distinguish
/// display-only decorations from the real prompt.
#[derive(Debug, Clone)]
pub struct PendingMessage {
    /// Text shown in the pending block and later in the user bubble.
    pub display: String,
    /// Text dispatched to the agent as the `SubmitTask` payload.
    pub agent_task: String,
}

/// Messages queued while the agent is busy.
///
/// TODO(Phase 1): moves from `App::pending_messages` (draft payload `String`;
/// the real `PendingMessage` carries more metadata).
#[derive(Default)]
pub struct PendingQueue {
    pub items: Vec<PendingMessage>,
}

/// A self-contained UI unit: state + update intake + rendering + key handling.
///
/// `U` defaults to [`protocol::AgentUpdate`]; hosts that emit a different
/// update enum implement `Component<TheirUpdate>` and map at the boundary.
///
/// `Send` is required so hosts can move a shell holding a `ComponentRegistry`
/// onto a worker task (Tact's `run_tui` runs on a `tokio::spawn`).
pub trait Component<U = protocol::AgentUpdate>: 'static + Send {
    /// Handle one protocol update. Returns `true` if the frame must repaint.
    fn on_update(&mut self, update: &U, ctx: &mut Ctx<'_>) -> bool;

    /// Handle a key event. Returns `true` if consumed (stops bubbling).
    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx<'_>) -> bool {
        let _ = (key, ctx);
        false
    }

    /// Render into `buf` within `area`; returns the visual height used.
    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &Ctx<'_>) -> u16;

    /// Height at width (for layout). Default renders into a scratch buffer.
    fn height(&self, width: u16, ctx: &Ctx<'_>) -> u16 {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 0));
        self.render(Rect::new(0, 0, width, u16::MAX), &mut buf, ctx)
    }

    /// Update/key ordering priority; lower runs first (coordinator = 0).
    fn priority(&self) -> u8 {
        100
    }

    /// Downcast support: the shell needs to read a component's state for
    /// rendering (state lives in the component, render reads it).
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

impl<U: 'static> Component<U> for Box<dyn Component<U>> {
    fn on_update(&mut self, update: &U, ctx: &mut Ctx<'_>) -> bool {
        (**self).on_update(update, ctx)
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx<'_>) -> bool {
        (**self).on_key(key, ctx)
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &Ctx<'_>) -> u16 {
        (**self).render(area, buf, ctx)
    }

    fn height(&self, width: u16, ctx: &Ctx<'_>) -> u16 {
        (**self).height(width, ctx)
    }

    fn priority(&self) -> u8 {
        (**self).priority()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        (**self).as_any()
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        (**self).as_any_mut()
    }
}

/// Re-export surface for hosts.
pub mod prelude {
    pub use crate::bridge::{AgentBridge, BridgeExtension, Command, ExtensionEvent};
    pub use crate::protocol::*;
    pub use crate::{Component, Ctx, InputMode, LogCoordinator, PendingMessage, PendingQueue};
}
