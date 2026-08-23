//! Code-popup preview renderer (pure).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use super::PopupMouseSurface;
use crate::render::ctx::RenderCtx;

//    total = 10 lines, content_height = 4, scroll = 3
//
//    lines[0]  ─┐
//    lines[1]   │ skipped (above visible area)
//    lines[2]  ─┘
//    lines[3]  ─┐ ← start_line = 3
//    lines[4]   │
//    lines[5]   │ visible in viewport
//    lines[6]  ─┘ ← end_line = min(3+4, 10) = 7
//    lines[7]  ─┐
//    lines[8]   │ skipped (below visible area)
//    lines[9]  ─┘
pub fn render_code_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = &ctx.code_popup else {
        return surface;
    };
    if popup.block_idx >= ctx.code_blocks.len() {
        return surface;
    }
    let block = &ctx.code_blocks[popup.block_idx];
    let lines: Vec<&str> = block.content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return surface;
    }

    let popup_area = super::centered_popup_area(area);

    let lang = if popup.lang.is_empty() {
        "code"
    } else {
        &popup.lang
    };
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
    let inner = super::render_popup_chrome(
        frame,
        popup_area,
        ctx.theme,
        &format!(" {} ", lang),
        Some(footer),
    );

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
        format!("```{} ({} lines)", lang, total),
        title_style,
    )));
    text.push_line(Line::from(""));

    // Render code lines, truncating to popup width minus borders/padding
    let max_chars = popup_area.width.saturating_sub(4) as usize;
    for &line in &lines[start_line..end_line] {
        let display: String = line.chars().take(max_chars).collect();
        text.push_line(Line::from(Span::styled(
            display,
            Style::default().fg(ctx.theme.fg),
        )));
    }

    let para = Paragraph::new(text).wrap(Wrap { trim: false });

    frame.render_widget(para, inner);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.code_popup_area = popup_area;
    surface
}
