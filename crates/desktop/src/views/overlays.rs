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
    Projects,
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
                        view().flex_col().gap_1().px_4().py_3().child(panel(
                            kind,
                            snapshot,
                            controller.clone(),
                            overlay,
                            query,
                        )),
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
                    Overlay::Projects => "Projects",
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
        Overlay::Settings => settings_panel(snapshot, controller).into_element(),
        Overlay::SessionManager => session_panel(snapshot, controller).into_element(),
        Overlay::Extensions => extensions_panel(snapshot, controller).into_element(),
        Overlay::Models => models_panel(snapshot, controller, overlay).into_element(),
        Overlay::Projects => projects_panel(snapshot, controller, overlay).into_element(),
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
        .child(view().flex_col().gap(px(2.0)).child(each(
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
        )))
}

fn settings_panel(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        move || settings_ui::rows("").collect::<Vec<_>>(),
        |row: &settings_ui::SettingRow| row.id.as_str().to_owned(),
        move |row, _| {
            let row = row.get_untracked();
            setting_row(row, snapshot, controller.clone())
        },
    ))
}

fn setting_row(
    row: settings_ui::SettingRow,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> AnyElement {
    let id = row.id;
    let value = move || setting_value(id, &snapshot.get().0);
    view()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .rounded(px(7.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(id.label().get(snapshot.get().0.language))
                        .flex_1()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(
                    text(settings_ui::label(row))
                        .text_size(px(9.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(control(id, value, controller))
        .into_element()
}

fn control(
    id: SettingId,
    value: impl Fn() -> String + Clone + 'static,
    controller: DesktopController,
) -> AnyElement {
    match id {
        SettingId::Notifications
        | SettingId::HookOutput
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject => {
            let current_value = value.clone();
            let current = move || current_value() == "on";
            let toggle = controller.clone();
            let next = move || !current();
            let label_value = value.clone();
            let text_value = value.clone();
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(label_value)
                .bg(move || theme().muted)
                .on_press(move || {
                    let _ = toggle.dispatch(DesktopIntent::SetSetting {
                        id,
                        value: SettingValue::Bool(next()),
                    });
                })
                .child(
                    text(text_value)
                        .text_size(px(12.0))
                        .text_color(move || theme().foreground),
                )
                .into_element()
        }
        SettingId::PermissionMode
        | SettingId::Theme
        | SettingId::Language
        | SettingId::ReasoningEffort => {
            let cycle = controller.clone();
            let next_value = value.clone();
            let next = move || next_choice(id, &next_value());
            let label_value = value.clone();
            let text_value = value.clone();
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(label_value)
                .bg(move || theme().muted)
                .on_press(move || {
                    let _ = cycle.dispatch(DesktopIntent::SetSetting {
                        id,
                        value: SettingValue::Choice(next()),
                    });
                })
                .child(
                    text(text_value)
                        .text_size(px(12.0))
                        .text_color(move || theme().foreground),
                )
                .into_element()
        }
        SettingId::ConfigPath => text(value)
            .text_size(px(11.0))
            .font_family("Lilex")
            .text_color(move || theme().muted_foreground)
            .into_element(),
        _ => {
            let draft = signal(value());
            let masked = id == SettingId::ApiKey;
            let submit_controller = controller.clone();
            let submit_value = value.clone();
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    input()
                        .value(move || draft.get())
                        .on_change(move |text: &str| draft.set(text.to_owned()))
                        .on_submit({
                            let controller = submit_controller.clone();
                            let current = submit_value.clone();
                            move |text: &str| {
                                let text = text.trim();
                                let value = if text.is_empty() {
                                    current()
                                } else {
                                    text.to_owned()
                                };
                                let _ = controller.dispatch(DesktopIntent::SetSetting {
                                    id,
                                    value: SettingValue::Text(value),
                                });
                            }
                        })
                        .masked(masked)
                        .flex_1()
                        .px_3()
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(move || theme().border)
                        .bg(move || theme().background)
                        .text_size(px(12.0)),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label("Set")
                        .hover(|style| style.bg(move || theme().muted))
                        .on_press({
                            let controller = submit_controller;
                            move || {
                                let text = draft.get();
                                if !text.trim().is_empty() {
                                    let _ = controller.dispatch(DesktopIntent::SetSetting {
                                        id,
                                        value: SettingValue::Text(text),
                                    });
                                }
                            }
                        })
                        .child(
                            text("Set")
                                .text_size(px(10.0))
                                .text_color(move || theme().primary),
                        ),
                )
                .into_element()
        }
    }
}

fn next_choice(id: SettingId, current: &str) -> String {
    let options: &[&str] = match id {
        SettingId::PermissionMode => &["default", "plan", "auto"],
        SettingId::Theme => &["light", "dark"],
        SettingId::Language => &["en", "zh"],
        SettingId::ReasoningEffort => &["low", "medium", "high", "xhigh", "max"],
        _ => &[],
    };
    let index = options
        .iter()
        .position(|option| *option == current)
        .unwrap_or(0);
    options
        .get((index + 1) % options.len().max(1))
        .copied()
        .unwrap_or(current)
        .to_owned()
}

fn setting_value(id: SettingId, snapshot: &tact_desktop_core::DesktopSnapshot) -> String {
    use tact_desktop_core::{Appearance, PermissionModeView};
    match id {
        SettingId::Model => snapshot
            .runtime
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .unwrap_or(&snapshot.composer.model)
            .to_owned(),
        SettingId::SubagentModel => snapshot.composer.subagent_model.to_string(),
        SettingId::ReasoningEffort => snapshot
            .composer
            .effort
            .as_deref()
            .unwrap_or("—")
            .to_owned(),
        SettingId::PermissionMode => match snapshot.composer.permission_mode {
            PermissionModeView::Default => "default",
            PermissionModeView::Plan => "plan",
            PermissionModeView::Auto => "auto",
        }
        .to_owned(),
        SettingId::Notifications => on_off(snapshot.notifications),
        SettingId::HookOutput => on_off(snapshot.show_hook_output),
        SettingId::Theme => match snapshot.appearance {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
        .to_owned(),
        SettingId::Language => match snapshot.language {
            tact_catalog::CatalogLocale::En => "en",
            tact_catalog::CatalogLocale::Zh => "zh",
        }
        .to_owned(),
        SettingId::Provider => snapshot.settings.provider.to_string(),
        SettingId::ApiKey => {
            if snapshot.settings.api_key_set {
                "••••••".to_owned()
            } else {
                String::new()
            }
        }
        SettingId::BaseUrl => snapshot.settings.base_url.to_string(),
        SettingId::MaxTokens => optional(snapshot.settings.max_tokens.map(u64::from)),
        SettingId::ModelContextWindow => {
            optional(snapshot.settings.context_window.map(|value| value as u64))
        }
        SettingId::SnapshotMaxItems => optional(
            snapshot
                .settings
                .snapshot_max_items
                .map(|value| value as u64),
        ),
        SettingId::MicroCompact => on_off(snapshot.settings.micro_compact),
        SettingId::TokioConsole => on_off(snapshot.settings.tokio_console),
        SettingId::SkillBodyAutoInject => on_off(snapshot.settings.skill_body_auto_inject),
        SettingId::ConfigPath => snapshot.settings.config_path.clone().unwrap_or_default(),
        SettingId::ThinkingBudget => optional(snapshot.composer.thinking_budget.map(u64::from)),
    }
}

fn on_off(value: bool) -> String {
    if value { "on" } else { "off" }.to_owned()
}

fn optional(value: Option<u64>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn session_panel(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
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
            let rename_draft = signal(session.title.clone());
            let rename_controller = controller.clone();
            let rename_id = SessionId(session.id.clone());
            view()
                .flex_row()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(px(7.0))
                .child(
                    input()
                        .value(move || rename_draft.get())
                        .on_change(move |text: &str| rename_draft.set(text.to_owned()))
                        .on_submit(move |text: &str| {
                            if !text.trim().is_empty() {
                                let _ = rename_controller.dispatch(DesktopIntent::RenameSession {
                                    session_id: rename_id.clone(),
                                    title: text.trim().to_owned(),
                                });
                            }
                        })
                        .flex_1()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(move || theme().border)
                        .bg(move || theme().background)
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
                            let _ =
                                archive_controller.dispatch(DesktopIntent::SetSessionArchived {
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

fn extensions_panel(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    let tab = signal(snapshot.get().0.extensions.tab);
    view()
        .flex_col()
        .gap_2()
        .child(
            view().flex_row().gap_1().children(
                [
                    tact_desktop_core::ExtensionsTab::Mcp,
                    tact_desktop_core::ExtensionsTab::Skills,
                    tact_desktop_core::ExtensionsTab::Plugins,
                    tact_desktop_core::ExtensionsTab::Hooks,
                ]
                .map(|value| {
                    let active = move || tab.get() == value;
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label(extension_tab_label(value))
                        .style_when(active, |style| style.bg(move || theme().muted))
                        .on_press(move || tab.set(value))
                        .child(
                            text(extension_tab_label(value))
                                .text_size(px(10.0))
                                .text_color(move || {
                                    if active() {
                                        theme().foreground
                                    } else {
                                        theme().muted_foreground
                                    }
                                }),
                        )
                }),
            ),
        )
        .child(dynamic(move || {
            let controller = controller.clone();
            match tab.get() {
                tact_desktop_core::ExtensionsTab::Skills => {
                    skills_section(snapshot, controller.clone()).into_element()
                }
                tact_desktop_core::ExtensionsTab::Plugins => section("Plugins", move || {
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
                })
                .into_element(),
                tact_desktop_core::ExtensionsTab::Mcp => {
                    surface_list(snapshot, controller.clone(), "MCP servers", "mcp list")
                }
                tact_desktop_core::ExtensionsTab::Hooks => {
                    surface_list(snapshot, controller, "Hooks", "hooks list")
                }
            }
        }))
}

fn extension_tab_label(tab: tact_desktop_core::ExtensionsTab) -> &'static str {
    match tab {
        tact_desktop_core::ExtensionsTab::Mcp => "MCP",
        tact_desktop_core::ExtensionsTab::Skills => "Skills",
        tact_desktop_core::ExtensionsTab::Plugins => "Plugins",
        tact_desktop_core::ExtensionsTab::Hooks => "Hooks",
    }
}

fn surface_list(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    title: &'static str,
    command: &'static str,
) -> AnyElement {
    let root = if command.starts_with("mcp") {
        tact_catalog::BuiltinCommand::Mcp
    } else {
        tact_catalog::BuiltinCommand::Hooks
    };
    let subcommand = command.split_whitespace().nth(1).unwrap_or("list");
    view()
        .flex_col()
        .gap_2()
        .child(
            text(title)
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(dynamic(move || match snapshot.get().0.surface.clone() {
            Some(surface) => {
                let (_, body) = crate::views::surface::render(&surface);
                nsh_base::text_view()
                    .markdown(body)
                    .selectable(true)
                    .into_element()
            }
            None => text("Run the command to load this list.")
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground)
                .into_element(),
        }))
        .child(
            view()
                .px_2()
                .py(px(3.0))
                .rounded(px(6.0))
                .bg(move || theme().muted)
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(format!("Load {title}"))
                .on_press(move || {
                    let _ = controller.dispatch(DesktopIntent::InvokeCommand {
                        id: tact_catalog::CommandId::BuiltinPath {
                            root,
                            path: vec![subcommand],
                        },
                        args: CommandArguments { tokens: Vec::new() },
                    });
                })
                .child(
                    text("Load")
                        .text_size(px(10.0))
                        .text_color(move || theme().primary),
                ),
        )
        .into_element()
}

fn skills_section(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(2.0))
        .child(
            text("Skills")
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(each(
            move || snapshot.get().0.extensions.snapshot.skills.clone(),
            |skill| skill.name.clone(),
            move |skill, _| {
                let skill = skill.get_untracked();
                let controller = controller.clone();
                let name = skill.name.clone();
                let enabled = skill.enabled;
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .child(
                        view()
                            .flex_1()
                            .flex_col()
                            .child(text(skill.name.clone()).text_size(px(12.0)))
                            .child(
                                text(skill.description.clone())
                                    .number_of_lines(1)
                                    .text_size(px(10.0))
                                    .text_color(move || theme().muted_foreground),
                            ),
                    )
                    .child(
                        view()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .cursor_pointer()
                            .role(AccessibilityRole::Button)
                            .label(if enabled { "Disable" } else { "Enable" })
                            .hover(|style| style.bg(move || theme().muted))
                            .on_press(move || {
                                let _ = controller.dispatch(DesktopIntent::SetSkillEnabled {
                                    name: name.clone(),
                                    enabled: !enabled,
                                });
                            })
                            .child(
                                text(if enabled { "Disable" } else { "Enable" })
                                    .text_size(px(10.0))
                                    .text_color(move || {
                                        if enabled {
                                            theme().muted_foreground
                                        } else {
                                            theme().primary
                                        }
                                    }),
                            ),
                    )
            },
        ))
}

fn section(title: &'static str, rows: impl Fn() -> Vec<String> + 'static) -> impl IntoElement {
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
                        value: SettingValue::Text(chosen.clone()),
                    });
                    overlay.set(None);
                })
                .child(text(model).text_size(px(12.0)))
        },
    ))
}

fn projects_panel(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    overlay: Signal<Option<Overlay>>,
) -> impl IntoElement {
    let path = signal(String::new());
    view()
        .flex_col()
        .gap_2()
        .child(view().flex_col().gap(px(1.0)).child(each(
            move || snapshot.get().0.projects.to_vec(),
            |project: &tact_desktop_core::ProjectView| {
                project
                    .root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| project.display_name.to_string())
            },
            {
                let controller = controller.clone();
                move |project: ReadSignal<tact_desktop_core::ProjectView>, _| {
                    let project = project.get_untracked();
                    let controller = controller.clone();
                    let root = project.root.clone();
                    let current = move || snapshot.get().0.project.id == project.id;
                    view()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label(project.display_name.to_string())
                        .style_when(current, |style| style.bg(move || theme().muted))
                        .on_press(move || {
                            if let Some(root) = root.clone() {
                                let _ = controller.dispatch(DesktopIntent::OpenProject { root });
                                overlay.set(None);
                            }
                        })
                        .child(
                            text(project.display_name.to_string())
                                .flex_1()
                                .number_of_lines(1)
                                .text_size(px(12.0)),
                        )
                        .child(
                            text(project.branch.as_deref().unwrap_or("").to_owned())
                                .text_size(px(10.0))
                                .text_color(move || theme().muted_foreground),
                        )
                }
            },
        )))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    input()
                        .value(move || path.get())
                        .on_change(move |text: &str| path.set(text.to_owned()))
                        .placeholder("Project path")
                        .flex_1()
                        .px_3()
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(move || theme().border)
                        .bg(move || theme().background)
                        .text_size(px(12.0)),
                )
                .child(
                    view()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label("Open project")
                        .on_press({
                            let controller = controller.clone();
                            move || {
                                let root = path.get();
                                if !root.trim().is_empty() {
                                    let _ = controller.dispatch(DesktopIntent::OpenProject {
                                        root: std::path::PathBuf::from(root.trim()),
                                    });
                                    overlay.set(None);
                                }
                            }
                        })
                        .child(
                            text("Open")
                                .text_size(px(10.0))
                                .text_color(move || theme().primary),
                        ),
                ),
        )
}
