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
}

/// Shared-log ownership + model (priority-0 component), re-exported at the
/// crate root for the `Ctx` / prelude.
pub use state::{InputMode, LogCoordinator, LogItem, LogItemKind, SystemMsgStyle};

/// A message queued while the agent is busy (Codex-style submit-on-idle).
pub struct PendingMessage(pub String);

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
pub trait Component<U = protocol::AgentUpdate> {
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
}

impl<U> Component<U> for Box<dyn Component<U>> {
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
}

/// Re-export surface for hosts.
pub mod prelude {
    pub use crate::bridge::{AgentBridge, BridgeExtension, Command, ExtensionEvent};
    pub use crate::protocol::*;
    pub use crate::{Component, Ctx, InputMode, LogCoordinator, PendingMessage, PendingQueue};
}
