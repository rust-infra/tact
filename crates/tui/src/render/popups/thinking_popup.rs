//! Thinking popup — app-layer wrapper (mouse hit areas + selection cache
//! write-back; the pure render lives in the kit).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_thinking_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::thinking_popup::render_thinking_popup(frame, area, ctx)
    });
    // The popup's selection cache is a render-time write-back (mirrors the
    // original inline logic: any rendered thinking popup refreshes it). Taken
    // before the surface is consumed so the hit rows still move, not clone.
    let selection_text = surface.thinking_selection_text.take();
    super::record_text_popup(app, SurfaceId::ThinkingPopup, surface);
    if let Some(text) = selection_text
        && let Some(popup) = app.thinking_mut().popup.as_mut()
    {
        popup.selection_text = text;
    }
}
