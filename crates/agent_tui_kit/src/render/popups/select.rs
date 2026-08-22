//! Selection popup renderer (permission / model pickers).

use ratatui::{Frame, layout::Rect};

use crate::{render::ctx::RenderCtx, widgets::select_popup_widget::SelectPopupWidget};

pub fn render_select_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    let widget = SelectPopupWidget::new(
        ctx.select,
        ctx.theme.highlight,
        ctx.theme.fg,
        ctx.theme.bottom_bar_bg,
        ctx.messages.select_empty,
        ctx.messages.select_arrow,
    )
    .with_border_type(ctx.theme.block_border_type());
    frame.render_widget(widget, area);
}
