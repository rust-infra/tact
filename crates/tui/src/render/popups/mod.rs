//! Popup family — app-layer wrappers.
//!
//! Pure popup renderers live in `agent_tui_kit::render::popups`; this module
//! keeps the `&mut App` wrappers (mouse hit-area application, prepare phases)
//! plus the app-layer popups that own Tact-specific state:
//! `command_palette`, `file_picker`, `slash_command`.
//!
//! Every popup here is a wrapper over a kit renderer — the app layer no longer
//! draws a single border of its own. `task_dag_popup` was the last one that
//! did, and its chrome moved to the kit on 2026-10-04.

pub(crate) mod code_popup;
pub(crate) mod command_palette;
pub(crate) mod diff_popup;
pub(crate) mod file_picker;
pub(crate) mod help;
pub(crate) mod history;
pub(crate) mod mermaid_popup;
pub(crate) mod select;
pub(crate) mod slash_command;
pub(crate) mod subagent_popup;
pub(crate) mod system_prompt_popup;
pub(crate) mod task_dag_popup;
pub(crate) mod thinking_popup;

use ratatui::Frame;
use ratatui::layout::Rect;

use agent_tui_kit::render::ctx::RenderCtx;
use agent_tui_kit::render::popups::PopupMouseSurface;

use crate::widgets::state::{App, SurfaceId};

/// Record the popup rect a kit renderer returned.
///
/// The emptiness guard is not decoration: a kit renderer that drew nothing
/// (its popup is closed, or its content is not ready) returns a default
/// surface, and recording that would leave a zero-size surface — which happens
/// to be harmless for the hit test but silently clobbers a rect another
/// renderer recorded earlier in the same frame.
pub(crate) fn record_popup_area(app: &mut App, id: SurfaceId, surface: &PopupMouseSurface) {
    if !surface.popup_area.is_empty() {
        app.mouse.set_area(id, surface.popup_area);
    }
}

/// Record a text popup's rect *and* its selectable body.
///
/// Thinking / diff / subagent share one text-selection slot in `MouseState`
/// because only one of them is ever open — so the three wrappers must write
/// the same two fields, and writing them here keeps that trio in step (and
/// documents why one slot is the right shape rather than three).
///
/// Takes the surface by value so the hit rows move instead of being cloned;
/// callers that also want [`PopupMouseSurface::thinking_selection_text`] must
/// `take()` it first.
pub(crate) fn record_text_popup(app: &mut App, id: SurfaceId, surface: PopupMouseSurface) {
    if !surface.popup_area.is_empty() {
        app.mouse.set_area(id, surface.popup_area);
        app.mouse.popup_text_body_area = surface.body_area;
        app.mouse.popup_text_hit_rows = surface.hit_rows;
    }
}

/// Render a popup through the kit and hand the host the resulting surface.
///
/// The kit renderers take `&RenderCtx`; building it is the app's job and is
/// the only reason every wrapper opens with these two lines.
pub(crate) fn render_with_ctx(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    render: impl FnOnce(&mut Frame, Rect, &RenderCtx<'_>) -> PopupMouseSurface,
) -> PopupMouseSurface {
    let ctx = app.render_ctx();
    render(frame, area, &ctx)
}
