//! Mermaid source popup — app-layer wrapper.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_mermaid_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::mermaid_popup::render_mermaid_popup(frame, area, ctx)
    });
    super::record_popup_area(app, SurfaceId::MermaidPopup, &surface);
}
