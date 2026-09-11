//! Mermaid popup — the rendered terminal diagram (`Tab` switches to source).
//!
//! The log panel draws Mermaid diagrams at its own (narrow) width. This popup
//! re-renders the same fence body at the popup's width — roughly 80% of the
//! frame — so dense flowcharts stay readable. `Tab` switches to the raw fence
//! body, which `y` copies.
//!
//! A fence that cannot be parsed (e.g. Mermaid `style` / `classDef` /
//! `linkStyle` statements, which the upstream renderer does not support) falls
//! back to the source view, and the header says so rather than silently
//! showing unrendered Mermaid.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use super::PopupMouseSurface;
use crate::{render::ctx::RenderCtx, state::MermaidPopupView};

/// Header line shown when the diagram cannot be rendered.
const FALLBACK_NOTE: &str = "⚠ this diagram does not render (unsupported syntax) — showing source";

pub fn render_mermaid_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = &ctx.mermaid_popup else {
        return surface;
    };
    if popup.block_idx >= ctx.mermaid_blocks.len() {
        return surface;
    }
    let source = ctx.mermaid_blocks[popup.block_idx].source.clone();

    let popup_area = super::centered_popup_area(area);
    let inner = super::popup_inner(popup_area);

    // Render the diagram first: we need to know whether it produced art before
    // we can pick the effective view or size the scrollbar.
    let diagram = (popup.view == MermaidPopupView::Diagram).then(|| {
        crate::render::render_md::render_mermaid_block(&source, ctx.theme, inner.width as usize)
    });
    let (view, diagram_lines) = match diagram {
        // Requested diagram and it rendered.
        Some(Some(lines)) => (MermaidPopupView::Diagram, Some(lines)),
        // Requested diagram, render failed → show source so nothing is hidden.
        Some(None) => (MermaidPopupView::Source, None),
        // Source requested explicitly.
        None => (MermaidPopupView::Source, None),
    };
    let fell_back = popup.view == MermaidPopupView::Diagram && diagram_lines.is_none();

    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "Tab",
            label: view.toggled().toggle_label(),
        },
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
    let title = if view == MermaidPopupView::Diagram {
        " mermaid ".to_string()
    } else {
        " mermaid (source) ".to_string()
    };
    let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, &title, Some(footer));

    let content_height = inner.height as usize;

    // Both views render into a `Vec<Line>`; only the body differs.
    let body: Vec<Line<'static>> = match &diagram_lines {
        Some(lines) => lines.clone(),
        None => source
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(ctx.theme.fg),
                ))
            })
            .collect(),
    };
    let total = body.len().max(1);

    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let end_line = (scroll + content_height).min(total);

    let mut text = Text::default();
    if fell_back {
        text.push_line(Line::from(Span::styled(
            FALLBACK_NOTE,
            Style::default()
                .fg(ctx.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if body.is_empty() {
        text.push_line(Line::from(""));
    } else {
        text.extend(body[scroll.min(body.len())..end_line].iter().cloned());
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
