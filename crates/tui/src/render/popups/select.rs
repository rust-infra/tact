//! Selection popup — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_select_popup(frame: &mut Frame, area: Rect, app: &App) {
    let ctx = app.render_ctx();
    agent_tui_kit::render::popups::select::render_select_popup(frame, area, &ctx);
}
