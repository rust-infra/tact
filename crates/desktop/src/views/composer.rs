//! The prompt composer: the draft, send and stop.

use std::path::PathBuf;

use nsh_base::{input, theme};
use nsh_core::element::{AccessibilityRole, ReadSignal};
use nsh_core::picker::PickRequest;
use nsh_core::prelude::*;
use tact_catalog::CommandArguments;
use tact_catalog::SettingId;
use tact_desktop_core::{
    CommandAvailability, DesktopController, DesktopIntent, PermissionModeView, SessionPhase,
    SettingValue, Submission, VoiceCommand,
};

use crate::app::{SnapshotRef, VoicePhase, VoiceState};

use super::RADIUS;
use super::overlays::Overlay;
use crate::ui::slash_overlay;

pub fn composer(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
    voice: Signal<VoiceState>,
    submit_tick: Signal<u64>,
) -> impl IntoElement {
    let draft = signal(String::new());
    let attachments = signal(Vec::<PathBuf>::new());
    effect(move || {
        if let Some(text) = voice.get().transcript.clone() {
            draft.update(|current| {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(&text);
            });
            voice.update(|state| state.transcript = None);
        }
    });
    let running = move || snapshot.get().0.session.phase == SessionPhase::Running;
    let queued = move || snapshot.get().0.composer.queue.len();
    let send = {
        let controller = controller.clone();
        move |text: &str| {
            let text = text.trim();
            if text.is_empty() {
                return;
            }
            if let Some(path) = slash_overlay::parse(text)
                && let Some(id) = slash_overlay::execute_id(&path.path)
            {
                let _ = controller.dispatch(DesktopIntent::InvokeCommand {
                    id,
                    args: path.tokens,
                });
                draft.set(String::new());
                return;
            }
            let _ = controller.dispatch(DesktopIntent::Submit(Submission {
                text: text.to_owned(),
                attachments: attachments.get(),
                model: None,
                permission_mode: None,
            }));
            draft.set(String::new());
            attachments.set(Vec::new());
        }
    };
    let last_submit = std::cell::Cell::new(0_u64);
    effect({
        let send = send.clone();
        move || {
            let tick = submit_tick.get();
            if tick != last_submit.get() {
                last_submit.set(tick);
                send(&draft.get());
            }
        }
    });
    let candidates = move || {
        let current = snapshot.get();
        let availability = CommandAvailability {
            has_project: current.0.project.id.is_some(),
            has_session: current.0.session.id.is_some(),
            has_account: current.0.account_supported,
            has_installed_skill: !current.0.extensions.snapshot.skills.is_empty(),
        };
        slash_overlay::command_candidates(&draft.get(), current.0.language, availability, &[])
    };
    view()
        .flex_col()
        .gap_2()
        .px_6()
        .pt_3()
        .pb_5()
        .child(dynamic({
            let controller = controller.clone();
            move || {
                let items = candidates();
                if items.is_empty() {
                    return view().into_element();
                }
                view()
                    .flex_col()
                    .gap(px(1.0))
                    .max_h(px(220.0))
                    .rounded(px(8.0))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || theme().border)
                    .p_1()
                    .child(each(
                        move || items.clone(),
                        |candidate: &slash_overlay::CommandCandidate| candidate.title.clone(),
                        {
                            let controller = controller.clone();
                            move |candidate: ReadSignal<slash_overlay::CommandCandidate>, _| {
                                let candidate = candidate.get_untracked();
                                let controller = controller.clone();
                                let chosen = candidate.clone();
                                view()
                                    .flex_col()
                                    .gap(px(1.0))
                                    .px_3()
                                    .py(px(3.0))
                                    .rounded(px(6.0))
                                    .cursor_pointer()
                                    .role(AccessibilityRole::Button)
                                    .label(candidate.title.clone())
                                    .hover(|style| style.bg(move || theme().muted))
                                    .on_press(move || {
                                        if chosen.has_children {
                                            draft.set(chosen.insertion.clone());
                                            return;
                                        }
                                        let tokens = slash_overlay::parse(&draft.get())
                                            .map(|path| path.tokens)
                                            .unwrap_or(CommandArguments { tokens: Vec::new() });
                                        let _ = controller.dispatch(DesktopIntent::InvokeCommand {
                                            id: chosen.id.clone(),
                                            args: tokens,
                                        });
                                        draft.set(String::new());
                                    })
                                    .child(
                                        text(candidate.title.clone())
                                            .text_size(px(12.0))
                                            .font_family("Lilex"),
                                    )
                                    .child(
                                        text(candidate.detail.clone())
                                            .text_size(px(10.0))
                                            .text_color(move || theme().muted_foreground),
                                    )
                            }
                        },
                    ))
                    .into_element()
            }
        }))
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
                        .test_id("composer-input")
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
        .child(attachment_chips(attachments))
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
                .child(voice_button(snapshot, controller.clone(), voice))
                .child(attach_button(attachments))
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

fn attach_button(attachments: Signal<Vec<PathBuf>>) -> impl IntoElement {
    view()
        .px_2()
        .py(px(6.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label("Attach image")
        .hover(|style| style.bg(move || theme().muted))
        .on_press(move || {
            spawn(async move {
                let mut request = PickRequest::default();
                request.limit = 4;
                let picked = pick_images(request).await;
                let dir = std::env::temp_dir().join("tact-attachments");
                if std::fs::create_dir_all(&dir).is_err() {
                    return;
                }
                let mut paths = Vec::new();
                for file in picked {
                    let name = file
                        .name
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or("image")
                        .to_owned();
                    let path = dir.join(format!("{}-{name}", std::process::id()));
                    if std::fs::write(&path, file.data.bytes()).is_ok() {
                        paths.push(path);
                    }
                }
                attachments.update(|current| current.extend(paths));
            });
        })
        .child(
            text("Attach")
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn attachment_chips(attachments: Signal<Vec<PathBuf>>) -> impl IntoElement {
    view().flex_row().flex_wrap().gap_1().child(each(
        move || attachments.get(),
        |path: &PathBuf| path.display().to_string(),
        move |path, _| {
            let path = path.get_untracked();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());
            view()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py(px(2.0))
                .rounded(px(5.0))
                .bg(move || theme().muted)
                .child(text(name).text_size(px(10.0)))
                .child(
                    view()
                        .px_1()
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label("Remove attachment")
                        .on_press(move || {
                            attachments.update(|current| current.retain(|item| item != &path));
                        })
                        .child(
                            text("×")
                                .text_size(px(10.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
        },
    ))
}

fn voice_button(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    voice: Signal<VoiceState>,
) -> impl IntoElement {
    view()
        .px_2()
        .py(px(6.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label("Voice input")
        .style_when(
            move || voice.get().phase == VoicePhase::Recording,
            |style| style.bg(move || theme().color("danger").unwrap_or(theme().primary)),
        )
        .hover(|style| style.bg(move || theme().muted))
        .on_press(move || {
            let current = snapshot.get();
            if !current.0.voice_enabled || current.0.voice_missing_api_key {
                return;
            }
            match voice.get().phase {
                VoicePhase::Idle => {
                    let _ = controller.dispatch(DesktopIntent::Voice(VoiceCommand::Start));
                }
                VoicePhase::Recording => {
                    let _ = controller.dispatch(DesktopIntent::Voice(VoiceCommand::Stop));
                }
                VoicePhase::Transcribing => {}
            }
        })
        .child(
            text(move || match voice.get().phase {
                VoicePhase::Idle => "Mic",
                VoicePhase::Recording => "Stop",
                VoicePhase::Transcribing => "…",
            })
            .text_size(px(11.0))
            .text_color(move || match voice.get().phase {
                VoicePhase::Recording => theme().primary_foreground,
                _ => theme().muted_foreground,
            }),
        )
}
