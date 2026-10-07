//! `/tasks-dag` popup — the pre-rendered task dependency diagram.
//!
//! The lines are built by the host's prepare phase (the DAG is markdown-rendered
//! at the popup's body width, which only the render geometry knows); this
//! renderer owns the chrome and the scroll window.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
};

use super::{FooterHint, PopupMouseSurface, scrollable_popup::ScrollableTextPopup};
use crate::render::ctx::RenderCtx;

pub fn render_task_dag_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = ctx.task_dag_popup else {
        return surface;
    };
    if popup.lines.is_empty() {
        return surface;
    }

    let header = Line::from(format!("Tasks DAG ({} lines)", popup.lines.len())).style(
        Style::default()
            .fg(ctx.theme.accent)
            .add_modifier(Modifier::BOLD),
    );
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(popup.lines.len() + 2);
    lines.push(header);
    lines.push(Line::from(""));
    lines.extend(popup.lines.iter().cloned());

    let footer: &[FooterHint] = super::COPY_SCROLL_CLOSE;

    surface.popup_area = ScrollableTextPopup::new(ctx.theme, " tasks-dag ", &lines)
        .scroll(popup.scroll as usize)
        .footer_hints(footer)
        .copy_done(ctx.copy_flash.then_some(ctx.messages.popup_copy_done))
        .render(frame, area);
    surface
}
