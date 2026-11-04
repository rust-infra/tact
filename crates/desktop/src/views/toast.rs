//! Toasts the controller asks for: a title, a body and a timer.

use nsh_base::theme;
use nsh_core::element::AnyElement;
use nsh_core::prelude::*;

use crate::app::ToastMessage;

use super::{RADIUS, token};

pub fn layer(toast: Signal<Option<ToastMessage>>) -> AnyElement {
    dynamic(move || match toast.get() {
        None => view().into_element(),
        Some(message) => view()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(24.0))
            .items_center()
            .child(
                view()
                    .test_id("toast")
                    .max_w(px(520.0))
                    .flex_col()
                    .gap(px(2.0))
                    .px_4()
                    .py_3()
                    .rounded(px(RADIUS))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || token("warning"))
                    .child(
                        text(message.title)
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        text(message.body)
                            .text_size(px(11.0))
                            .text_color(move || theme().muted_foreground),
                    ),
            )
            .into_element(),
    })
}
