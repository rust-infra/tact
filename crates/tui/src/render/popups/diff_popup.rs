//! Diff popup — app-layer wrapper. The lazy content load (git diff / file
//! read, a side effect) runs in the prepare phase; the pure render reads the
//! cache.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = app.theme;
    if let Some(popup) = app.tools_mut().popup.as_mut() {
        agent_tui_kit::render::popups::diff_popup::prepare_diff_popup(popup, &theme);
    }
    let surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::diff_popup::render_diff_popup(frame, area, ctx)
    });
    super::record_text_popup(app, SurfaceId::DiffPopup, surface);
}

pub(crate) fn popup_lang_for_path(path: &str) -> String {
    agent_tui_kit::render::popups::diff_popup::lang_from_path(path)
}
