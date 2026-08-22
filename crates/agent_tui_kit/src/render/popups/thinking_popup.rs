//! Thinking popup renderer (pure).

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::PopupMouseSurface;
use crate::render::ctx::RenderCtx;

fn is_ordered_list_item(line: &Line<'_>) -> bool {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let trimmed = text.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && trimmed
            .as_bytes()
            .get(digits)
            .is_some_and(|byte| *byte == b'.')
        && trimmed
            .as_bytes()
            .get(digits + 1)
            .is_some_and(u8::is_ascii_whitespace)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

enum ThinkingDisplayRow {
    Content(crate::render::selectable_text::DisplayRow),
    Spacer,
}

pub fn render_thinking_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let popup = match ctx.thinking.popup.clone() {
        Some(p) => p,
        None => return surface,
    };
    let (styled_lines, _raw_total) = if let Some(active) = ctx
        .thinking
        .active
        .as_ref()
        .filter(|active| active.phys_idx == popup.phys_idx)
    {
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
        (block.cached_markdown.clone(), block.content.lines().count())
    } else {
        return surface;
    };
    if styled_lines.is_empty() {
        return surface;
    }

    let popup_area = super::centered_popup_area(area);
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
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let selection = popup
        .selection
        .and_then(|selection| selection.normalized_non_empty(&selection_text));
    let source = crate::render::selectable_text::source_lines(&selection_text);
    let fallback = Style::default().fg(ctx.theme.fg).bg(ctx.theme.bg);
    let mut display_rows = Vec::new();
    for (index, source_line) in source.iter().enumerate() {
        let styles = crate::render::selectable_text::scalar_styles(
            styled_lines.get(index),
            fallback,
            source_line.text.chars().count(),
        );
        display_rows.extend(
            crate::render::selectable_text::layout_display_rows(
                source_line.text,
                source_line.start,
                &styles,
                body_area.width as usize,
                true,
            )
            .into_iter()
            .map(ThinkingDisplayRow::Content),
        );
        if styled_lines.get(index).is_some_and(is_ordered_list_item)
            && styled_lines
                .get(index + 1)
                .is_some_and(is_ordered_list_item)
        {
            display_rows.push(ThinkingDisplayRow::Spacer);
        }
    }

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
        let ThinkingDisplayRow::Content(display) = display else {
            continue;
        };
        frame.render_widget(
            Paragraph::new(Line::from(display.spans(selection.as_ref()))),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
        hit_rows.push(display.hit_row(screen_y, body_area.x));
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
