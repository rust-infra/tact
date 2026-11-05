use gpui::{App, Entity, Focusable as _, Hsla, Styled as _, Window, px};
use gpui_component::{
    ActiveTheme as _,
    input::{Input, InputState},
};

pub(crate) fn quiet_search_input(state: &Entity<InputState>, window: &Window, cx: &App) -> Input {
    let focused = state.read(cx).focus_handle(cx).is_focused(window);
    let border = if focused {
        if crate::theme::high_contrast() {
            cx.theme().foreground
        } else {
            cx.theme().ring.alpha(0.45)
        }
    } else {
        cx.theme().transparent
    };

    Input::new(state)
        .appearance(true)
        .bordered(false)
        .focus_bordered(false)
        .rounded(px(8.))
        .border(px(crate::theme::border_width()))
        .border_color(border)
        .bg(if focused {
            cx.theme().popover
        } else {
            cx.theme().secondary
        })
}

pub(crate) fn selection_fill(selected: bool, active: bool, filled: bool, cx: &App) -> Hsla {
    if selected {
        cx.theme().accent
    } else if active {
        cx.theme().muted
    } else if filled {
        cx.theme().secondary
    } else {
        cx.theme().transparent
    }
}

pub(crate) fn selection_hover_fill(selected: bool, filled: bool, cx: &App) -> Hsla {
    if selected {
        cx.theme().accent
    } else if filled {
        cx.theme().secondary_hover
    } else {
        cx.theme().muted
    }
}
