//! Code popup — app-layer wrapper (mouse hit area + kit pure render).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_code_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::code_popup::render_code_popup(frame, area, ctx)
    });
    super::record_popup_area(app, SurfaceId::CodePopup, &surface);
}
