//! Code popup — app-layer wrapper (mouse hit area + kit pure render).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_code_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let ctx = app.render_ctx();
    let surface = agent_tui_kit::render::popups::code_popup::render_code_popup(frame, area, &ctx);
    if !surface.code_popup_area.is_empty() {
        app.mouse.code_popup_area = surface.code_popup_area;
    }
}
