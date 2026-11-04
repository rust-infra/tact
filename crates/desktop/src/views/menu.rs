//! The application menu bar: File, View and Session, drawn by the app
//! because NSH 0.3.0 exposes native menus only to TSX.

use nsh_base::theme;
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::{DesktopController, DesktopIntent};

use super::overlays::Overlay;
use crate::views::MenuKind;

pub fn bar(menu: Signal<Option<MenuKind>>) -> impl IntoElement {
    view().flex_row().items_center().gap(px(1.0)).children(
        [MenuKind::File, MenuKind::View, MenuKind::Session].map(|kind| {
            let active = move || menu.get() == Some(kind);
            view()
                .px_2()
                .py(px(3.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(kind.label())
                .style_when(active, |style| style.bg(move || theme().muted))
                .hover(|style| style.bg(move || theme().muted))
                .on_press(move || {
                    if active() {
                        menu.set(None);
                    } else {
                        menu.set(Some(kind));
                    }
                })
                .child(text(kind.label()).text_size(px(11.0)).text_color(move || {
                    if active() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }))
        }),
    )
}

pub fn layer(
    menu: Signal<Option<MenuKind>>,
    controller: DesktopController,
    rail_open: Signal<bool>,
    inspector_open: Signal<bool>,
    overlay: Signal<Option<Overlay>>,
) -> AnyElement {
    dynamic(move || {
        let Some(kind) = menu.get() else {
            return view().into_element();
        };
        let close = move || menu.set(None);
        let panel = match kind {
            MenuKind::File => view()
                .child(item("New Session", {
                    let controller = controller.clone();
                    move || {
                        let _ = controller.dispatch(DesktopIntent::NewSession);
                        close();
                    }
                }))
                .child(item("Command Center", move || {
                    overlay.set(Some(Overlay::CommandCenter));
                    close();
                }))
                .child(item("Settings", move || {
                    overlay.set(Some(Overlay::Settings));
                    close();
                }))
                .child(item("Quit", {
                    let controller = controller.clone();
                    move || {
                        let _ = controller.dispatch(DesktopIntent::Shutdown);
                        close();
                    }
                }))
                .into_element(),
            MenuKind::View => view()
                .child(item("Toggle Sessions", move || {
                    rail_open.update(|open| *open = !*open);
                    close();
                }))
                .child(item("Toggle Inspector", move || {
                    inspector_open.update(|open| *open = !*open);
                    close();
                }))
                .child(item("Back to Bottom", {
                    let controller = controller.clone();
                    move || {
                        let _ = controller.dispatch(DesktopIntent::BackToBottom);
                        close();
                    }
                }))
                .into_element(),
            MenuKind::Session => view()
                .child(item("Stop", {
                    let controller = controller.clone();
                    move || {
                        let _ = controller.dispatch(DesktopIntent::CancelRun);
                        close();
                    }
                }))
                .into_element(),
        };
        view()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .child(
                view()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_press(close),
            )
            .child(
                view()
                    .absolute()
                    .top(px(super::title_bar::HEIGHT))
                    .left(px(96.0))
                    .w(px(220.0))
                    .flex_col()
                    .gap(px(1.0))
                    .p_1()
                    .rounded(px(8.0))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || theme().border)
                    .child(panel),
            )
            .into_element()
    })
}

fn item(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    view()
        .px_3()
        .py(px(5.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .hover(|style| style.bg(move || theme().muted))
        .on_press(action)
        .child(text(label).text_size(px(12.0)))
}
