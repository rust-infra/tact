//! System prompt popup — app-layer wrapper (mouse hit areas + selection cache
//! write-back; the pure render lives in the kit).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId};

pub(crate) fn render_system_prompt_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let mut surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::system_prompt_popup::render_system_prompt_popup(
            frame, area, ctx,
        )
    });
    // Render-time write-back, like the thinking popup: the layout decides where
    // the rendered lines end, so the copy buffer can only be built here. Taken
    // before the surface is consumed so the hit rows still move, not clone.
    let selection_text = surface.system_prompt_selection_text.take();
    super::record_text_popup(app, SurfaceId::SystemPromptPopup, surface);
    if let Some(text) = selection_text
        && let Some(popup) = app.system_prompt_popup.as_mut()
    {
        popup.selection_text = text;
    }
}
