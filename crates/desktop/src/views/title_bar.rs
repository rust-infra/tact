//! The window's own title bar, with the native controls over it.

use nsh_base::theme;
use nsh_core::element::AccessibilityRole;
use nsh_core::element::desktop::title_bar as native_title_bar;
use nsh_core::prelude::*;

use crate::app::SnapshotRef;
use crate::ui::title_bar as title_bar_text;

use super::overlays::Overlay;
use super::token;
use crate::views::MenuKind;
use crate::views::menu;

pub const HEIGHT: f32 = 44.0;

pub fn title_bar(
    snapshot: Signal<SnapshotRef>,
    overlay: Signal<Option<Overlay>>,
    menu: Signal<Option<MenuKind>>,
) -> impl IntoElement {
    view()
        .h(px(HEIGHT))
        .w_full()
        .flex_row()
        .items_center()
        .pl(move || native_title_bar().left)
        .pr(move || native_title_bar().right)
        .gap_3()
        .window_drag(true)
        .bg(move || token("title-bar"))
        .border_b_1()
        .border_b_color(move || token("title-bar-border"))
        .child(
            text("Tact")
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(menu::bar(menu))
        .child(
            text(move || {
                let snapshot = snapshot.get();
                title_bar_text::title(
                    &snapshot.0.project.display_name,
                    snapshot.0.project.branch.as_deref(),
                )
            })
            .flex_1()
            .number_of_lines(1)
            .text_size(px(13.0))
            .text_color(move || theme().muted_foreground),
        )
        .children(account_chip(snapshot))
        .child(chrome_button("Commands", move || {
            overlay.set(Some(Overlay::CommandCenter));
        }))
        .child(chrome_button("Settings", move || {
            overlay.set(Some(Overlay::Settings));
        }))
}

fn chrome_button(label: &'static str, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .px_2()
        .py(px(3.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .hover(|style| style.bg(move || theme().muted))
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn account_chip(snapshot: Signal<SnapshotRef>) -> Option<impl IntoElement> {
    let account = snapshot.get().0.account.clone()?;
    let label = match account {
        tact_desktop_core::AccountView::Balance { currency, total } => {
            format!("{total} {currency}")
        }
        tact_desktop_core::AccountView::UsageQuota {
            plan, used, limit, ..
        } => match (used, limit) {
            (Some(used), Some(limit)) => format!("{used} / {limit}"),
            _ => plan
                .map(|plan| plan.to_string())
                .unwrap_or_else(|| "Quota".to_owned()),
        },
        tact_desktop_core::AccountView::Error(_) => return None,
    };
    Some(
        text(label)
            .px_2()
            .py(px(2.0))
            .rounded(px(5.0))
            .bg(move || theme().muted)
            .text_size(px(10.0))
            .font_family("Lilex")
            .text_color(move || theme().muted_foreground),
    )
}
