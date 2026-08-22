//! System prompt popup — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_system_prompt_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let ctx = app.render_ctx();
    agent_tui_kit::render::popups::system_prompt_popup::render_system_prompt_popup(
        frame, area, &ctx,
    );
}
