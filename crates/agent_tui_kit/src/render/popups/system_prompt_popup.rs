//! System prompt popup renderer (full prompt preview with internal scroll).

use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};

use crate::render::{ctx::RenderCtx, render_md::render_markdown_ratatui, selectable_text};

use super::{FooterHint, PopupMouseSurface};

/// Render the read-out popup: chrome, the scrolled body, and the hit rows a
/// drag selection needs.
///
/// The body is laid out here rather than handed to [`super::scrollable_popup::ScrollableTextPopup`]
/// because a selection has to know where every glyph sits: `selectable_text`
/// wraps the rendered lines *and* returns the byte range each display row owns,
/// which is what the mouse maps a column onto. The same layout decides the
/// popup's scroll range, so the scrollbar and the hit rows cannot disagree.
pub fn render_system_prompt_popup(
    frame: &mut Frame,
    area: Rect,
    ctx: &RenderCtx,
) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = ctx.system_prompt_popup else {
        return surface;
    };

    let popup_area = super::centered_popup_area(area);
    let footer: &[FooterHint] = super::COPY_SCROLL_CLOSE;
    let body_area = super::render_popup_chrome(
        frame,
        popup_area,
        ctx.theme,
        &format!(" {} ", popup.title),
        None,
        Some(footer),
        ctx.copy_flash.then_some(ctx.messages.popup_copy_done),
    );

    // Rendered Markdown at the body width: markers are consumed, so this *is*
    // the text the reader sees — and therefore the text a selection copies.
    let styled_lines = render_markdown_ratatui(&popup.source, ctx.theme, body_area.width as usize);
    let selection_text = styled_lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let selection = popup
        .selection
        .and_then(|selection| selection.normalized_non_empty(&selection_text));

    let fallback = Style::default().fg(ctx.theme.fg).bg(ctx.theme.bg);
    let display_rows = selectable_text::layout_all_display_rows(
        &selection_text,
        &styled_lines,
        fallback,
        body_area.width as usize,
    );

    let total = display_rows.len().max(1);
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (popup.scroll as usize).min(max_scroll);

    let mut hit_rows = Vec::new();
    for (visible_row, display) in display_rows
        .iter()
        .skip(scroll)
        .take(content_height)
        .enumerate()
    {
        let screen_y = body_area.y.saturating_add(visible_row as u16);
        frame.render_widget(
            Paragraph::new(Line::from(display.spans(selection.as_ref()))),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
        hit_rows.push(display.hit_row(screen_y, body_area.x));
    }

    super::render_popup_scrollbar(frame, popup_area, total, content_height, scroll);

    surface.popup_area = popup_area;
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    // The host stores this on the popup after the frame; without it a selection
    // could only be normalized against the renderer's own idea of the text.
    surface.system_prompt_selection_text = Some(selection_text);
    surface
}
