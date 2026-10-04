//! System prompt popup renderer (full prompt preview with internal scroll).

use ratatui::{Frame, layout::Rect};

use crate::render::{ctx::RenderCtx, render_md::render_markdown_ratatui};

use super::{FooterHint, scrollable_popup::ScrollableTextPopup};

pub fn render_system_prompt_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    let Some(popup) = ctx.system_prompt_popup else {
        return;
    };
    // Plain ratatui-markdown render at the popup's content width; the popup
    // scrolls internally so lines wrap at the renderer's max width.
    let body = ScrollableTextPopup::body_area(area);
    let lines = render_markdown_ratatui(&popup.source, ctx.theme, body.width as usize);
    let footer: &[FooterHint] = &[
        FooterHint {
            key: "j/k",
            label: " scroll ",
        },
        FooterHint {
            key: "Esc",
            label: " close ",
        },
    ];
    ScrollableTextPopup::new(ctx.theme, &format!(" {} ", popup.title), &lines)
        .scroll(popup.scroll as usize)
        .footer_hints(footer)
        .copy_done(ctx.copy_flash.then_some(ctx.messages.popup_copy_done))
        .render(frame, area);
}
