//! The modal surfaces over the shell: command center, settings, sessions and
//! extensions.

use nsh_base::{input, theme};
use nsh_core::element::{AccessibilityRole, AnyElement, ReadSignal};
use nsh_core::prelude::*;
use tact_catalog::{CommandArguments, SettingId};
use tact_desktop_core::{DesktopController, DesktopIntent, SessionId, SettingValue};

use crate::app::SnapshotRef;
use crate::ui::{command_center, session_manager, settings as settings_ui};

use super::{RADIUS, token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    CommandCenter,
    Settings,
    SessionManager,
    Extensions,
    Models,
}

pub fn layer(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
) -> AnyElement {
    let query = signal(String::new());
    dynamic(move || match overlay.get() {
        None => view().into_element(),
        Some(kind) => view()
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
                    .w_full()
                    .max_w(px(640.0))
                    .max_h(px(540.0))
                    .flex_col()
                    .rounded(px(RADIUS + 4.0))
                    .bg(move || theme().popover)
                    .border_1()
                    .border_color(move || theme().border)
                    .child(header(kind, overlay))
                    .child(scroll().flex_1().child(
                        view()
                            .flex_col()
                            .gap_1()
                            .px_4()
                            .py_3()
                            .child(panel(kind, snapshot, controller.clone(), overlay, query)),
                    )),
            )
            .into_element(),
    })
}

fn header(kind: Overlay, overlay: Signal<Option<Overlay>>) -> impl IntoElement {
    view()
        .h(px(44.0))
        .flex_row()
        .items_center()
        .px_4()
        .gap_2()
        .border_b_1()
        .border_b_color(move || theme().border)
        .child(
            view().flex_1().child(
                text(match kind {
                    Overlay::CommandCenter => "Command center",
                    Overlay::Settings => "Settings",
                    Overlay::SessionManager => "Sessions",
                    Overlay::Extensions => "Extensions",
                    Overlay::Models => "Model",
                })
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
                .on_press(move || overlay.set(None))
                .child(
                    text("Close")
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn panel(
    kind: Overlay,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
    query: Signal<String>,
) -> AnyElement {
    match kind {
        Overlay::CommandCenter => command_center_panel(controller, overlay, query).into_element(),
        Overlay::Settings => settings_panel(snapshot).into_element(),
        Overlay::SessionManager => session_panel(snapshot, controller).into_element(),
        Overlay::Extensions => extensions_panel(snapshot).into_element(),
        Overlay::Models => models_panel(snapshot, controller, overlay).into_element(),
    }
}

fn command_center_panel(
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
    query: Signal<String>,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap_2()
        .child(
            input()
                .value(move || query.get())
                .on_change(move |text: &str| query.set(text.to_owned()))
                .placeholder("Search commands")
                .auto_focus(true)
                .px_3()
                .py(px(7.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(move || theme().border)
                .bg(move || theme().background),
        )
        .child(
            view()
                .flex_col()
                .gap(px(2.0))
                .child(each(
                    move || command_center::items(&query.get()),
                    |item: &command_center::CommandCenterItem| item.title.clone(),
                    {
                        let controller = controller.clone();
                        move |item: ReadSignal<command_center::CommandCenterItem>, _| {
                            let item = item.get_untracked();
                            let controller = controller.clone();
                            view()
                                .flex_col()
                                .gap(px(1.0))
                                .px_3()
                                .py_2()
                                .rounded(px(7.0))
                                .cursor_pointer()
                                .role(AccessibilityRole::Button)
                                .label(item.title.clone())
                                .hover(|style| style.bg(move || theme().muted))
                                .on_press(move || {
                                    let _ = controller.dispatch(DesktopIntent::InvokeCommand {
                                        id: item.id.clone(),
                                        args: CommandArguments { tokens: Vec::new() },
                                    });
                                    overlay.set(None);
                                })
                                .child(
                                    text(item.title.clone())
                                        .text_size(px(13.0))
                                        .font_weight(FontWeight::MEDIUM),
                                )
                                .child(
                                    text(item.subtitle.clone())
                                        .text_size(px(11.0))
                                        .text_color(move || theme().muted_foreground),
                                )
                        }
                    },
                )),
        )
}

fn settings_panel(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        move || settings_ui::rows("").collect::<Vec<_>>(),
        |row: &settings_ui::SettingRow| row.id.as_str().to_owned(),
        move |row, _| {
            let row = row.get_untracked();
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(px(7.0))
                .child(
                    text(row.id.label().get(snapshot.get().0.language))
                        .flex_1()
                        .text_size(px(12.0)),
                )
                .child(
                    text(settings_ui::label(row))
                        .text_size(px(10.0))
                        .text_color(move || theme().muted_foreground),
                )
        },
    ))
}

fn session_panel(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        move || {
            let snapshot = snapshot.get();
            let view = session_manager::SessionManagerView::default();
            session_manager::filter_sessions(&snapshot.0.sessions.items, &view)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        },
        |session: &tact_desktop_core::SessionSummaryView| session.id.clone(),
        move |session, _| {
            let session = session.get_untracked();
            let controller = controller.clone();
            let id = SessionId(session.id.clone());
            let archived = session.archived_at.is_some();
            let archive_controller = controller.clone();
            let archive_id = id.clone();
            let open_controller = controller.clone();
            let open_id = id;
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(px(7.0))
                .child(
                    text(session.title.clone())
                        .flex_1()
                        .number_of_lines(1)
                        .text_size(px(12.0)),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label(if archived { "Restore" } else { "Archive" })
                        .on_press(move || {
                            let _ = archive_controller.dispatch(DesktopIntent::SetSessionArchived {
                                session_id: archive_id.clone(),
                                archived: !archived,
                            });
                        })
                        .child(
                            text(if archived { "Restore" } else { "Archive" })
                                .text_size(px(10.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label("Open")
                        .on_press(move || {
                            let _ = open_controller.dispatch(DesktopIntent::ResumeSession {
                                session_id: open_id.clone(),
                            });
                        })
                        .child(
                            text("Open")
                                .text_size(px(10.0))
                                .text_color(move || theme().primary),
                        ),
                )
        },
    ))
}

fn extensions_panel(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    view()
        .flex_col()
        .gap_2()
        .child(section(
            "Skills",
            move || {
                snapshot
                    .get()
                    .0
                    .extensions
                    .snapshot
                    .skills
                    .iter()
                    .map(|skill| format!("{} · {}", skill.name, skill.description))
                    .collect::<Vec<_>>()
            },
        ))
        .child(section(
            "Plugins",
            move || {
                snapshot
                    .get()
                    .0
                    .extensions
                    .snapshot
                    .plugins
                    .iter()
                    .map(|plugin| {
                        format!(
                            "{} · {}{}",
                            plugin.name,
                            plugin.marketplace,
                            plugin
                                .version
                                .as_deref()
                                .map(|version| format!(" · {version}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
            },
        ))
        .child(section(
            "Marketplaces",
            move || {
                snapshot
                    .get()
                    .0
                    .extensions
                    .snapshot
                    .marketplaces
                    .iter()
                    .map(|marketplace| format!("{} · {}", marketplace.name, marketplace.source))
                    .collect::<Vec<_>>()
            },
        ))
}

fn section(
    title: &'static str,
    rows: impl Fn() -> Vec<String> + 'static,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(1.0))
        .child(
            text(title)
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(move || {
                if rows().is_empty() {
                    "None".to_owned()
                } else {
                    rows().join("\n")
                }
            })
            .text_size(px(12.0)),
        )
}

fn models_panel(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        move || {
            snapshot
                .get()
                .0
                .composer
                .models
                .iter()
                .map(|model| model.to_string())
                .collect::<Vec<_>>()
        },
        |model: &String| model.clone(),
        move |model, _| {
            let model = model.get_untracked();
            let controller = controller.clone();
            let label = model.clone();
            let selected = move || snapshot.get().0.composer.model.as_ref() == label.as_str();
            let chosen = model.clone();
            view()
                .px_3()
                .py_2()
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(model.clone())
                .style_when(selected, |style| style.bg(move || theme().muted))
                .hover(|style| style.bg(move || theme().muted))
                .on_press(move || {
                    let _ = controller.dispatch(DesktopIntent::SetSetting {
                        id: SettingId::Model,
                        value: SettingValue::Choice(chosen.clone()),
                    });
                    overlay.set(None);
                })
                .child(text(model).text_size(px(12.0)))
        },
    ))
}
