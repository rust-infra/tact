//! Thinking popup renderer (pure).

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::{PopupMouseSurface, markdown_plan};
use crate::render::{
    ctx::RenderCtx, render_md::render_markdown_with_tables, selectable_text::MarkdownDisplayRow,
};

pub fn render_thinking_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let popup = match ctx.thinking.popup.clone() {
        Some(p) => p,
        None => return surface,
    };
    // The popup body width is needed before we can render Markdown at the
    // right width, so geometry comes first (the chrome paints later).
    let popup_area = super::centered_popup_area(area);
    let body_width = popup_area.width.saturating_sub(2).max(1);

    let (styled_lines, _raw_total) = if let Some(active) = ctx
        .thinking
        .active
        .as_ref()
        .filter(|active| active.phys_idx == popup.phys_idx)
    {
        // Streaming content is incomplete, so it is shown as plain lines.
        let lines = active
            .content
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect::<Vec<_>>();
        (lines, active.content.lines().count())
    } else if let Some(block) = ctx
        .thinking
        .blocks
        .iter()
        .find(|block| block.phys_idx == popup.phys_idx)
    {
        // Re-render at the actual popup body width instead of reusing the
        // card's 80-column cache: the width-aware pipeline shrinks pipe-table
        // columns, wraps long cells inside the table and routes Mermaid at
        // this width, so rows never exceed the popup and pipes stay aligned.
        let (mut styled, _raw) =
            render_markdown_with_tables(&block.content, ctx.theme, Some(body_width as usize));
        markdown_plan::decorate_headings(&mut styled, ctx.theme);
        (styled, block.content.lines().count())
    } else {
        return surface;
    };
    if styled_lines.is_empty() {
        return surface;
    }

    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "y",
            label: " copy ",
        },
        super::FooterHint {
            key: "Esc",
            label: " close ",
        },
        super::FooterHint {
            key: "j/k",
            label: " scroll ",
        },
    ];
    let inner =
        super::render_popup_chrome(frame, popup_area, ctx.theme, &popup.title, Some(footer));
    let body_area = inner;
    let selection_text = styled_lines
        .iter()
        .map(markdown_plan::line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let selection = popup
        .selection
        .and_then(|selection| selection.normalized_non_empty(&selection_text));
    let fallback = Style::default().fg(ctx.theme.fg).bg(ctx.theme.bg);
    let code_bg = ctx.theme.code_block_bg();
    let display_rows = markdown_plan::plan_markdown_display(
        &styled_lines,
        &selection_text,
        fallback,
        code_bg,
        body_area.width as usize,
    );

    let total = display_rows.len();
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
        let (line, hit_row) = match display {
            MarkdownDisplayRow::Code(display) => {
                let mut line = Line::from(display.spans(selection.as_ref()));
                markdown_plan::fill_code_row_tail(&mut line, display, body_area.width, code_bg);
                (line, display.hit_row(screen_y, body_area.x))
            }
            MarkdownDisplayRow::Content(display) => (
                Line::from(display.spans(selection.as_ref())),
                display.hit_row(screen_y, body_area.x),
            ),
            MarkdownDisplayRow::Spacer => continue,
        };
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
        hit_rows.push(hit_row);
    }

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.thinking_popup_area = popup_area;
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    // The active popup's selection cache is updated by the host after the frame.
    surface.thinking_selection_text = Some(selection_text);
    surface
}
