//! Mermaid source popup — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_mermaid_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let ctx = app.render_ctx();
    let surface =
        agent_tui_kit::render::popups::mermaid_popup::render_mermaid_popup(frame, area, &ctx);
    if !surface.mermaid_popup_area.is_empty() {
        app.mouse.mermaid_popup_area = surface.mermaid_popup_area;
    }
}
