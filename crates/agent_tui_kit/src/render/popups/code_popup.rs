//! Code-popup preview renderer (pure).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{FooterHint, PopupMouseSurface, scrollable_popup::ScrollableTextPopup};
use crate::render::ctx::RenderCtx;

pub fn render_code_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = &ctx.code_popup else {
        return surface;
    };
    if popup.block_idx >= ctx.code_blocks.len() {
        return surface;
    }
    let block = &ctx.code_blocks[popup.block_idx];
    let raw_lines: Vec<&str> = block.content.lines().collect();
    let total = raw_lines.len();
    if total == 0 {
        return surface;
    }

    let lang = if popup.lang.is_empty() {
        "code"
    } else {
        &popup.lang
    };

    // Build content lines: header + code rows (truncated to popup width).
    // The popup width is ~80% of the frame; the inner width is 2 cells narrower.
    let popup_width = super::centered_popup_area(area).width;
    let max_chars = popup_width.saturating_sub(4) as usize;

    let title_style = Style::default()
        .fg(ctx.theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(total + 2);
    lines.push(Line::from(Span::styled(
        format!("```{} ({} lines)", lang, total),
        title_style,
    )));
    lines.push(Line::from(""));
    for line in raw_lines {
        let display: String = line.chars().take(max_chars).collect();
        lines.push(Line::from(Span::styled(
            display,
            Style::default().fg(ctx.theme.fg),
        )));
    }

    let footer: &[FooterHint] = &[
        FooterHint {
            key: "y",
            label: " copy ",
        },
        FooterHint {
            key: "j/k",
            label: " scroll ",
        },
        FooterHint {
            key: "Esc",
            label: " close ",
        },
    ];

    let popup_area = ScrollableTextPopup::new(ctx.theme, &format!(" {} ", lang), &lines)
        .scroll(popup.scroll as usize)
        .footer_hints(footer)
        .copy_done(ctx.copy_flash.then_some(ctx.messages.popup_copy_done))
        .render(frame, area);

    surface.popup_area = popup_area;
    surface
}
