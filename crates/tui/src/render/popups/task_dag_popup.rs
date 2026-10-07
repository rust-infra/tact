//! `/tasks-dag` overlay — app-layer wrapper.
//!
//! The kit owns the chrome, scroll window and scrollbar; this wrapper keeps the
//! two things that are the app's: the re-render-on-width-change prepare step
//! and the mouse hit-area write-back.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::{App, SurfaceId, render_task_dag_lines};

pub(crate) fn render_task_dag_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    // Prepare: the mermaid layout depends on the popup's body width, so a cache
    // built for another width is stale and must be rebuilt before the render.
    let body =
        agent_tui_kit::render::popups::scrollable_popup::ScrollableTextPopup::body_area(area);
    let width = body.width as usize;
    if app
        .task_dag_popup
        .as_ref()
        .is_some_and(|p| p.render_width != width)
    {
        let (source, lines) = render_task_dag_lines(&app.task_panel().snapshot, &app.theme, width);
        if let Some(p) = app.task_dag_popup.as_mut() {
            p.lines = lines;
            p.mermaid_source = source;
            p.render_width = width;
        }
    }

    let surface = super::render_with_ctx(app, frame, area, |frame, area, ctx| {
        agent_tui_kit::render::popups::task_dag_popup::render_task_dag_popup(frame, area, ctx)
    });
    super::record_popup_area(app, SurfaceId::TaskDagPopup, &surface);
}
