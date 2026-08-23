//! Mermaid source popup — double-click a rendered diagram in the log to copy
//! the original fence body (`y`).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use super::PopupMouseSurface;
use crate::render::ctx::RenderCtx;

pub fn render_mermaid_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = &ctx.mermaid_popup else {
        return surface;
    };
    if popup.block_idx >= ctx.mermaid_blocks.len() {
        return surface;
    }
    let source = ctx.mermaid_blocks[popup.block_idx].source.clone();
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len().max(1);

    let popup_area = super::centered_popup_area(area);
    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "y",
            label: " copy ",
        },
        super::FooterHint {
            key: "j/k",
            label: " scroll ",
        },
        super::FooterHint {
            key: "Esc",
            label: " close ",
        },
    ];
    let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, " mermaid ", Some(footer));

    let content_height = inner.height as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let start_line = scroll;
    let end_line = (scroll + content_height).min(total);

    let mut text = Text::default();
    let title_style = Style::default()
        .fg(ctx.theme.accent)
        .add_modifier(Modifier::BOLD);
    text.push_line(Line::from(Span::styled(
        format!("```mermaid ({} lines)", lines.len()),
        title_style,
    )));
    text.push_line(Line::from(""));

    let max_chars = popup_area.width.saturating_sub(4) as usize;
    if lines.is_empty() {
        text.push_line(Line::from(""));
    } else {
        for &line in &lines[start_line..end_line] {
            let display: String = line.chars().take(max_chars).collect();
            text.push_line(Line::from(Span::styled(
                display,
                Style::default().fg(ctx.theme.fg),
            )));
        }
    }

    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.mermaid_popup_area = popup_area;
    surface
}
