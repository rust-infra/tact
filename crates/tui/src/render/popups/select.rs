//! Selection popup — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, InputMode, SurfaceId};

pub(crate) fn render_select_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.input_mode != InputMode::Select {
        // The wrapper runs every frame; clear the mouse hit area recorded
        // while the popup was active.
        app.mouse.clear_area(SurfaceId::SelectPopup);
        return;
    }
    let popup_area = {
        let ctx = app.render_ctx();
        agent_tui_kit::render::popups::select::render_select_popup(frame, area, &ctx)
    };
    // Expose the popup rect so mouse-wheel scrolls over the list move the
    // selection instead of scrolling the log behind the popup.
    app.mouse.set_area(SurfaceId::SelectPopup, popup_area);
}
