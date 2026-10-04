//! Subagent popup — app-layer wrapper. The layout-cache rebuild (a side
//! effect on the popup state) runs in the prepare phase; the pure render
//! reads the cache.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    // Prepare: rebuild the layout cache when stale (live output grows,
    // width changes, live→completed transition).
    if app.has_subagent_popup() {
        let popup_area = agent_tui_kit::render::popups::centered_popup_area(area);
        let body_area = agent_tui_kit::render::popups::popup_inner(popup_area);
        if let Some(id) = app.active_subagent_popup.clone()
            && let Some(popup) = app.subagent_popups.get_mut(&id)
        {
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
    let surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::subagent_popup::render_subagent_popup(frame, area, ctx)
    });
    super::record_text_popup(app, SurfaceId::SubagentPopup, surface);
}
