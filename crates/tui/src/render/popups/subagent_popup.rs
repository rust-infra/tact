//! Subagent popup — app-layer wrapper. The layout-cache rebuild (a side
//! effect on the popup state) runs in the prepare phase; the pure render
//! reads the cache.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    // Prepare: rebuild the layout cache when stale (live output grows,
    // width changes, live→completed transition).
    if app.subagent_popup.is_some() {
        let popup_area = agent_tui_kit::render::popups::centered_popup_area(area);
        let body_area = agent_tui_kit::render::popups::popup_inner(popup_area);
        if let Some(popup) = app.subagent_popup.as_mut() {
            let tools_state = app
                .registry
                .get::<agent_tui_kit::components::ToolComponent>()
                .expect("tool component registered")
                .state();
            agent_tui_kit::render::popups::subagent_popup::prepare_subagent_popup(
                popup,
                tools_state,
                &app.theme,
                body_area.width,
            );
        }
    }
    let ctx = app.render_ctx();
    let surface =
        agent_tui_kit::render::popups::subagent_popup::render_subagent_popup(frame, area, &ctx);
    if !surface.subagent_popup_area.is_empty() {
        app.mouse.subagent_popup_area = surface.subagent_popup_area;
        app.mouse.popup_text_body_area = surface.body_area;
        app.mouse.popup_text_hit_rows = surface.hit_rows;
    }
}
