//! The reader: a tool output, diff, thinking, prompt or report shown whole.

use nsh_base::{text_view, theme};
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::ReaderRequest;

use super::{RADIUS, token};

pub fn layer(reader: Signal<Option<ReaderRequest>>) -> AnyElement {
    dynamic(move || match reader.get() {
        None => view().into_element(),
        Some(request) => view()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .items_center()
            .justify_center()
            .p_6()
            .bg(move || token("scrim"))
            .child(
                view()
                    .test_id("reader")
                    .w_full()
                    .max_w(px(760.0))
                    .max_h(px(600.0))
                    .flex_col()
                    .rounded(px(RADIUS + 4.0))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || theme().border)
                    .child(header(request.title.to_string(), reader))
                    .child(scroll().flex_1().child(
                        view().px_5().py_4().child(
                            text_view()
                                .markdown(request.markdown.to_string())
                                .selectable(true),
                        ),
                    )),
            )
            .into_element(),
    })
}

fn header(title: String, reader: Signal<Option<ReaderRequest>>) -> impl IntoElement {
    view()
        .h(px(44.0))
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .border_b_1()
        .border_b_color(move || theme().border)
        .child(
            view().flex_1().child(
                text(title)
                    .number_of_lines(1)
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD),
            ),
        )
        .child(
            view()
                .flex_shrink_0()
                .px_2()
                .py(px(3.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label("Close")
                .on_press(move || reader.set(None))
                .child(
                    text("Close")
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}
