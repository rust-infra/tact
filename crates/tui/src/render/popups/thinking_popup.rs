//! Thinking popup — app-layer wrapper (mouse hit areas + selection cache
//! write-back; the pure render lives in the kit).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_thinking_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let ctx = app.render_ctx();
    let surface =
        agent_tui_kit::render::popups::thinking_popup::render_thinking_popup(frame, area, &ctx);
    if !surface.thinking_popup_area.is_empty() {
        app.mouse.thinking_popup_area = surface.thinking_popup_area;
        app.mouse.popup_text_body_area = surface.body_area;
        app.mouse.popup_text_hit_rows = surface.hit_rows;
    }
    // The popup's selection cache is a render-time write-back (mirrors the
    // original inline logic: any rendered thinking popup refreshes it).
    if let Some(text) = surface.thinking_selection_text
        && let Some(popup) = app.thinking_mut().popup.as_mut()
    {
        popup.selection_text = text;
    }
}
