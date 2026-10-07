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
    text::{Line, Span},
};

use super::{FooterHint, PopupMouseSurface, scrollable_popup::ScrollableTextPopup};
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

    let body = ScrollableTextPopup::body_area(area);

    // Render the diagram first: we need to know whether it produced art before
    // we can pick the effective view.
    let diagram = (popup.view == MermaidPopupView::Diagram).then(|| {
        crate::render::render_md::render_mermaid_block(&source, ctx.theme, body.width as usize)
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

    let footer: &[FooterHint] = &[
        FooterHint {
            key: "Tab",
            label: view.toggled().toggle_label(),
        },
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
    let title = if view == MermaidPopupView::Diagram {
        " mermaid ".to_string()
    } else {
        " mermaid (source) ".to_string()
    };

    // Both views render into a `Vec<Line>`; only the body differs. The fallback
    // note, when present, is the first content line so it scrolls with the
    // source it describes.
    let mut lines: Vec<Line<'static>> = Vec::new();
    if fell_back {
        lines.push(Line::from(Span::styled(
            FALLBACK_NOTE,
            Style::default()
                .fg(ctx.theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
    }
    match &diagram_lines {
        Some(rendered) => lines.extend(rendered.iter().cloned()),
        None => lines.extend(source.lines().map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(ctx.theme.fg),
            ))
        })),
    }

    let popup_area = ScrollableTextPopup::new(ctx.theme, &title, &lines)
        .scroll(popup.scroll as usize)
        .footer_hints(footer)
        .copy_done(ctx.copy_flash.then_some(ctx.messages.popup_copy_done))
        .render(frame, area);

    surface.popup_area = popup_area;
    surface
}
