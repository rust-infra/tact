//! The prompt composer: the draft, send and stop.

use nsh_base::{input, theme};
use nsh_core::element::AccessibilityRole;
use nsh_core::prelude::*;
use tact_catalog::SettingId;
use tact_desktop_core::{
    DesktopController, DesktopIntent, PermissionModeView, SessionPhase, SettingValue, Submission,
};

use crate::app::SnapshotRef;

use super::RADIUS;
use super::overlays::Overlay;

pub fn composer(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
) -> impl IntoElement {
    let draft = signal(String::new());
    let running = move || snapshot.get().0.session.phase == SessionPhase::Running;
    let queued = move || snapshot.get().0.composer.queue.len();
    let send = {
        let controller = controller.clone();
        move |text: &str| {
            let text = text.trim();
            if text.is_empty() {
                return;
            }
            let _ = controller.dispatch(DesktopIntent::Submit(Submission {
                text: text.to_owned(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }));
            draft.set(String::new());
        }
    };
    view()
        .flex_col()
        .gap_2()
        .px_6()
        .pt_3()
        .pb_5()
        .child(
            view()
                .flex_row()
                .items_end()
                .gap_2()
                .p_2()
                .rounded(px(RADIUS + 2.0))
                .bg(move || theme().card)
                .border_1()
                .border_color(move || theme().border)
                .child(
                    input()
                        .value(move || draft.get())
                        .on_change(move |text: &str| draft.set(text.to_owned()))
                        .on_submit({
                            let send = send.clone();
                            move |text: &str| send(text)
                        })
                        .multiline(true)
                        .placeholder(crate::i18n::text(
                            snapshot.get().0.language,
                            crate::i18n::TextKey::SteerPlaceholder,
                        ))
                        .flex_1()
                        .text_size(px(14.0))
                        .px_2()
                        .py(px(6.0)),
                )
                .child(
                    view()
                        .px_3()
                        .py(px(6.0))
                        .rounded(px(8.0))
                        .bg(move || {
                            if running() {
                                theme().color("danger").unwrap_or(theme().primary)
                            } else {
                                theme().primary
                            }
                        })
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label(move || if running() { "Stop" } else { "Send" })
                        .on_press({
                            let controller = controller.clone();
                            let send = send.clone();
                            move || {
                                if running() {
                                    let _ = controller.dispatch(DesktopIntent::CancelRun);
                                } else {
                                    send(&draft.get());
                                }
                            }
                        })
                        .child(
                            text(move || if running() { "Stop" } else { "Send" })
                                .text_size(px(12.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(move || theme().primary_foreground),
                        ),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .px_1()
                .child(control(
                    move || {
                        let model = snapshot.get().0.composer.model.to_string();
                        if model.is_empty() {
                            "Model".to_owned()
                        } else {
                            model
                        }
                    },
                    move || overlay.set(Some(Overlay::Models)),
                ))
                .child(control(
                    move || match snapshot.get().0.composer.permission_mode {
                        PermissionModeView::Default => "Default".to_owned(),
                        PermissionModeView::Plan => "Plan".to_owned(),
                        PermissionModeView::Auto => "Auto".to_owned(),
                    },
                    {
                        let controller = controller.clone();
                        move || {
                            let next = match snapshot.get().0.composer.permission_mode {
                                PermissionModeView::Default => "plan",
                                PermissionModeView::Plan => "auto",
                                PermissionModeView::Auto => "default",
                            };
                            let _ = controller.dispatch(DesktopIntent::SetSetting {
                                id: SettingId::PermissionMode,
                                value: SettingValue::Choice(next.to_owned()),
                            });
                        }
                    },
                ))
                .child(view().flex_1())
                .child(
                    text(move || {
                        if queued() == 0 {
                            String::new()
                        } else {
                            format!("{} queued", queued())
                        }
                    })
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground),
                ),
        )
}

fn control(
    label: impl Fn() -> String + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view()
        .px_2()
        .py(px(3.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label())
        .hover(|style| style.bg(move || theme().muted))
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}
