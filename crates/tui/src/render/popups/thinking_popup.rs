use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::selectable_text::{DisplayRow, layout_display_rows, scalar_styles, source_lines};
use crate::widgets::state::App;

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
    Content(DisplayRow),
    Spacer,
}

pub(crate) fn render_thinking_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match app.thinking.popup.clone() {
        Some(p) => p,
        None => return,
    };
    let (styled_lines, _raw_total) = if let Some(active) = app
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
    } else if let Some(block) = app
        .thinking
        .blocks
        .iter()
        .find(|block| block.phys_idx == popup.phys_idx)
    {
        (block.cached_markdown.clone(), block.content.lines().count())
    } else {
        return;
    };
    if styled_lines.is_empty() {
        return;
    }

    let popup_area = super::centered_popup_area(area);
    let footer: &[super::FooterHint] = &[
        super::FooterHint { key: "y", label: " copy " },
        super::FooterHint { key: "Esc", label: " close " },
        super::FooterHint { key: "j/k", label: " scroll " },
    ];
    let inner = super::render_popup_chrome(
        frame,
        popup_area,
        &app.theme,
        &popup.title,
        Some(footer),
    );
    let body_area = inner;
    let selection_text = styled_lines
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let selection = popup
        .selection
        .and_then(|selection| selection.normalized_non_empty(&selection_text));
    let source = source_lines(&selection_text);
    let fallback = Style::default().fg(app.theme.fg).bg(app.theme.bg);
    let mut display_rows = Vec::new();
    for (index, source_line) in source.iter().enumerate() {
        let styles = scalar_styles(
            styled_lines.get(index),
            fallback,
            source_line.text.chars().count(),
        );
        display_rows.extend(
            layout_display_rows(
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

    if let Some(active_popup) = app.thinking.popup.as_mut()
        && active_popup.phys_idx == popup.phys_idx
    {
        active_popup.selection_text = selection_text;
    }
    app.mouse.thinking_popup_area = popup_area;
    app.mouse.popup_text_body_area = body_area;
    app.mouse.popup_text_hit_rows = hit_rows;
}
