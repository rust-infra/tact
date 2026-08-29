//! Selection popup renderer (permission / model pickers).

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{render::ctx::RenderCtx, widgets::select_popup_widget::SelectPopupWidget};

/// Build the bottom-border navigation hint, matching the code/mermaid popup
/// footer style: keys in accent, labels muted, separated by " | ".
fn select_footer(ctx: &RenderCtx) -> Line<'static> {
    let accent = ctx.theme.accent;
    let muted = ctx.theme.muted;
    let push_hint = |spans: &mut Vec<Span<'static>>, key: &'static str, label: &'static str| {
        if !spans.is_empty() {
            spans.push(Span::styled(" | ", Style::default().fg(muted)));
        }
        spans.push(Span::styled(
            key,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(label, Style::default().fg(muted)));
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_hint(&mut spans, "↑↓/j/k", ctx.messages.select_hint_nav);
    if ctx.select.multi {
        push_hint(&mut spans, "Space", ctx.messages.select_hint_toggle);
    }
    push_hint(&mut spans, "Enter", ctx.messages.select_hint_confirm);
    push_hint(&mut spans, "Esc", ctx.messages.select_hint_cancel);
    Line::from(spans).alignment(Alignment::Center)
}

/// Render the selection popup and return its outer rect (the app layer uses
/// the rect to route mouse-wheel scrolls to the popup).
pub fn render_select_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> Rect {
    let widget = SelectPopupWidget::new(
        ctx.select,
        ctx.theme.highlight,
        ctx.theme.fg,
        ctx.theme.bottom_bar_bg,
        ctx.messages.select_empty,
        ctx.messages.select_arrow,
    )
    .with_border_type(ctx.theme.block_border_type())
    .with_footer(select_footer(ctx));
    let popup_area = widget.popup_area(area);
    frame.render_widget(widget, area);
    popup_area
}
