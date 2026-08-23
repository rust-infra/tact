//! History panel — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_history_panel(frame: &mut Frame, area: Rect, app: &App) {
    let ctx = app.render_ctx();
    agent_tui_kit::render::popups::history::render_history_panel(frame, area, &ctx);
}
