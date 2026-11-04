//! The window shell: title bar, session rail, run room and inspector.

use nsh_core::desktop::TitleBarStyle;
use nsh_core::element::desktop::set_title_bar;
use nsh_core::prelude::*;

use crate::app::use_app;
use crate::ui::layout;
use crate::ui::shell::RunTab;
use crate::views::MenuKind;
use crate::views::menu;
use crate::views::overlays::{self, Overlay};
use crate::views::{composer, inspector, rail, reader, surface, title_bar, toast, transcript};

pub fn shell() -> impl IntoElement {
    let app = use_app();
    let snapshot = app.snapshot;
    let controller = app.controller.clone();
    let tab = signal(RunTab::Chat);
    let overlay = signal(None::<Overlay>);
    let menu = signal(None::<MenuKind>);
    let rail_open = signal(true);
    let inspector_open = signal(true);
    set_title_bar(TitleBarStyle::Hidden, Some(px(title_bar::HEIGHT)));
    if std::env::var_os("TACT_DESKTOP_SMOKE").is_some() {
        set_timeout(std::time::Duration::from_millis(250), || {
            nsh_core::element::desktop::close_window();
        });
    }
    on_key("mod+k", move |_| {
        overlay.set(Some(Overlay::CommandCenter));
    });
    on_key("mod+,", move |_| {
        overlay.set(Some(Overlay::Settings));
    });
    let reader_state = app.reader;
    on_key("escape", move |_| {
        reader_state.set(None);
        overlay.set(None);
        menu.set(None);
    });
    let controller_for_layer = controller.clone();
    let controller_for_menu = controller.clone();
    view()
        .flex_1()
        .flex_col()
        .bg(move || nsh_base::theme().background)
        .text_color(move || nsh_base::theme().foreground)
        .child(title_bar::title_bar(snapshot, overlay, menu))
        .child(when(
            move || snapshot.get().0.failure.is_some(),
            move || failure_banner(snapshot),
        ))
        .child(body(
            snapshot,
            controller,
            tab,
            overlay,
            app.voice,
            rail_open,
            inspector_open,
        ))
        .child(menu::layer(
            menu,
            controller_for_menu,
            rail_open,
            inspector_open,
            overlay,
        ))
        .child(overlays::layer(snapshot, controller_for_layer, overlay))
        .child(surface::layer(snapshot))
        .child(toast::layer(app.toast))
        .child(reader::layer(reader_state))
}

fn body(
    snapshot: Signal<crate::app::SnapshotRef>,
    controller: tact_desktop_core::DesktopController,
    tab: Signal<RunTab>,
    overlay: Signal<Option<Overlay>>,
    voice: Signal<crate::app::VoiceState>,
    rail_open: Signal<bool>,
    inspector_open: Signal<bool>,
) -> impl IntoElement {
    let columns = move || {
        layout::layout(
            window().width.0,
            untracked(|| snapshot.get().0.text_scale),
            rail_open.get(),
            inspector_open.get(),
        )
    };
    view()
        .flex_1()
        .flex_row()
        .child(when(move || columns().rail_inline, {
            let controller = controller.clone();
            move || rail::rail(snapshot, controller.clone())
        }))
        .child(
            view()
                .flex_1()
                .h_full()
                .flex_col()
                .child(transcript::run_room(snapshot, controller.clone(), tab))
                .child(composer::composer(
                    snapshot,
                    controller.clone(),
                    overlay,
                    voice,
                )),
        )
        .child(when(
            move || columns().inspector_inline,
            move || inspector::inspector(snapshot, controller.clone()),
        ))
}

fn failure_banner(snapshot: Signal<crate::app::SnapshotRef>) -> impl IntoElement {
    view()
        .px_4()
        .py_2()
        .bg(move || {
            nsh_base::theme()
                .color("danger")
                .unwrap_or(nsh_base::theme().foreground)
        })
        .child(
            text(move || {
                snapshot
                    .get()
                    .0
                    .failure
                    .as_ref()
                    .map(|failure| failure.message.to_string())
                    .unwrap_or_default()
            })
            .text_size(px(12.0))
            .text_color(move || nsh_base::theme().primary_foreground),
        )
}
