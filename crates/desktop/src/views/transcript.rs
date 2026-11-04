//! The run room: the conversation, its rows and the Trajectory page.

use std::sync::Arc;

use nsh_base::{DiffLayout, diff_view, text_view, theme};
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::{
    DesktopController, DesktopIntent, GateCard, GateKind, GateResponse, GateSelection, GateState,
    PlanRow, ReaderRef, RowBody, RowId, RowStatus, ToolBody, ToolGroupItem, ToolGroupRow, ToolRow,
    TranscriptRow, TranscriptView,
};

use crate::app::{SnapshotRef, use_app};
use crate::i18n::{self, TextKey};
use crate::ui::shell::RunTab;
use crate::ui::transcript::visible_row;
use crate::ui::{ansi, mermaid, web_search};

use super::RADIUS;

pub fn run_room(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    tab: Signal<RunTab>,
) -> impl IntoElement {
    view()
        .flex_1()
        .h_full()
        .flex_col()
        .bg(move || theme().background)
        .child(tab_strip(tab))
        .child(dynamic(move || match tab.get() {
            RunTab::Chat => chat(snapshot, controller.clone()).into_element(),
            RunTab::Trajectory => super::trajectory::page(snapshot).into_element(),
        }))
}

fn tab_strip(tab: Signal<RunTab>) -> impl IntoElement {
    view()
        .h(px(40.0))
        .flex_row()
        .items_center()
        .gap_1()
        .px_4()
        .border_b_1()
        .border_b_color(move || theme().border)
        .children(RunTab::ALL.map(|value| {
            let active = move || tab.get() == value;
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(match value {
                    RunTab::Chat => "Chat",
                    RunTab::Trajectory => "Trajectory",
                })
                .style_when(active, |style| style.bg(move || theme().muted))
                .on_press(move || tab.set(value))
                .child(
                    text(match value {
                        RunTab::Chat => "Chat",
                        RunTab::Trajectory => "Trajectory",
                    })
                    .text_size(px(12.0))
                    .font_weight(FontWeight::NORMAL)
                    .style_when(active, |style| style.font_weight(FontWeight::SEMIBOLD))
                    .text_color(move || {
                        if active() {
                            theme().foreground
                        } else {
                            theme().muted_foreground
                        }
                    }),
                )
        }))
}

fn chat(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> AnyElement {
    let rows = signal(Vec::<Arc<TranscriptRow>>::new());
    effect(move || {
        let current = snapshot.get();
        rows.set(
            current
                .0
                .transcript
                .iter()
                .filter(|row| visible_row(row, &current.0))
                .cloned()
                .collect(),
        );
    });
    let empty = move || rows.with(|rows| rows.is_empty());
    view()
        .flex_1()
        .flex_col()
        .child(run_header(snapshot))
        .child(sticky_strip(snapshot))
        .child(when(empty, {
            let controller = controller.clone();
            move || empty_state(snapshot, controller.clone())
        }))
        .child(when(move || !empty(), {
            let row_controller = controller.clone();
            move || {
                scroll()
                    .flex_1()
                    .child(view().flex_col().px(px(12.0)).py_2().child(each(
                        move || rows.get(),
                        |row: &Arc<TranscriptRow>| row.ordinal,
                        {
                            let controller = row_controller.clone();
                            move |row, _| {
                                let controller = controller.clone();
                                dynamic(move || {
                                    row_element(row.get(), snapshot, controller.clone(), 0)
                                })
                            }
                        },
                    )))
            }
        }))
        .into_element()
}

fn run_header(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    let app = use_app();
    let draft = app.draft;
    view()
        .flex_col()
        .gap_2()
        .px_6()
        .pt_5()
        .pb_4()
        .border_b_1()
        .border_b_color(move || theme().border)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(view().size(px(6.0)).rounded(px(3.0)).bg(move || {
                    if matches!(
                        snapshot.get().0.session.phase,
                        tact_desktop_core::SessionPhase::Running
                    ) {
                        theme().color("success").unwrap_or(theme().primary)
                    } else {
                        theme().muted_foreground
                    }
                }))
                .child(
                    text(move || {
                        let current = snapshot.get();
                        let id = current
                            .0
                            .session
                            .id
                            .as_ref()
                            .map(|id| {
                                let raw = id.0.to_string();
                                let short = raw.rsplit('-').next().unwrap_or(&raw).to_owned();
                                short[..short.len().min(4)].to_uppercase()
                            })
                            .unwrap_or_else(|| "—".to_owned());
                        format!("RUN {id}")
                    })
                    .text_size(px(10.0))
                    .font_family("Lilex")
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(move || {
                        format!(
                            "{} · {} TURNS",
                            elapsed_label(snapshot.get().0.runtime.run_started_at),
                            snapshot.get().0.runtime.turns_taken
                        )
                    })
                    .text_size(px(10.0))
                    .font_family("Lilex")
                    .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            text(move || {
                snapshot
                    .get()
                    .0
                    .transcript
                    .iter()
                    .find_map(|row| match &row.body {
                        RowBody::User(user) => Some(user.text.to_string()),
                        _ => None,
                    })
                    .map(|text| first_line(&text))
                    .unwrap_or_else(|| "New session".to_owned())
            })
            .text_size(px(26.0))
            .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            view()
                .flex_row()
                .gap_2()
                .child(insert_chip("/", draft, "/"))
                .child(insert_chip("@", draft, "@")),
        )
}

fn insert_chip(
    label: &'static str,
    draft: Signal<String>,
    insertion: &'static str,
) -> impl IntoElement {
    view()
        .px_2()
        .py(px(2.0))
        .rounded(px(5.0))
        .bg(move || theme().muted)
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .on_press(move || {
            let mut current = draft.get();
            current.push_str(insertion);
            draft.set(current);
        })
        .child(
            text(label)
                .text_size(px(10.0))
                .font_family("Lilex")
                .text_color(move || theme().muted_foreground),
        )
}

fn empty_state(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    let app = use_app();
    let draft = app.draft;
    view().flex_1().items_center().justify_center().p_8().child(
        view()
            .w_full()
            .max_w(px(520.0))
            .flex_col()
            .gap_3()
            .rounded(px(RADIUS))
            .border_1()
            .border_color(move || theme().border)
            .bg(move || theme().popover)
            .p_6()
            .child(
                text(i18n::text(
                    snapshot.get().0.language,
                    TextKey::EmptySessionTitle,
                ))
                .text_size(px(20.0))
                .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                text(i18n::text(
                    snapshot.get().0.language,
                    TextKey::EmptySessionBody,
                ))
                .text_size(px(13.0))
                .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .flex_row()
                    .gap_2()
                    .child(insert_chip("/", draft, "/"))
                    .child(insert_chip("@", draft, "@")),
            )
            .child(
                text(i18n::text(
                    snapshot.get().0.language,
                    TextKey::EmptySessionHint,
                ))
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .px_3()
                    .py(px(5.0))
                    .rounded(px(7.0))
                    .bg(move || theme().primary)
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label("New session")
                    .on_press({
                        let controller = controller.clone();
                        move || {
                            let _ = controller.dispatch(DesktopIntent::NewSession);
                        }
                    })
                    .child(
                        text("New session")
                            .text_size(px(12.0))
                            .text_color(move || theme().primary_foreground),
                    ),
            ),
    )
}

fn row_element(
    row: Arc<TranscriptRow>,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    depth: usize,
) -> AnyElement {
    let locale = snapshot.get().0.language;
    let label = crate::ui::transcript_row::accessibility_label(&row, locale);
    let role = crate::ui::transcript_row::accessibility_role(&row);
    let content = match &row.body {
        RowBody::User(user) => user_row(row.time.clone(), user, snapshot),
        RowBody::Assistant(assistant) => assistant_row(row.time.clone(), assistant, snapshot),
        RowBody::Thinking(thinking) => thinking_row(&row, thinking, snapshot),
        RowBody::Plan(plan) => plan_row(&row, plan, snapshot),
        RowBody::ToolGroup(group) => tool_group(&row, group, snapshot, controller.clone(), depth),
        RowBody::Diff(diff) => diff_row(&row, diff, controller.clone()),
        RowBody::Gate(gate) => gate_card(gate, controller.clone()),
        RowBody::Notice(notice) => view()
            .px_3()
            .py(px(6.0))
            .rounded(px(7.0))
            .bg(move || theme().muted)
            .child(
                text(notice.text.to_string())
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground),
            )
            .into_element(),
        RowBody::HookContext(context) => view()
            .flex_col()
            .gap_1()
            .border_l_1()
            .border_l_color(move || theme().border)
            .pl_3()
            .child(
                text(context.source.as_deref().unwrap_or("hook").to_owned())
                    .text_size(px(10.0))
                    .font_family("Lilex")
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                text_view()
                    .markdown(context.markdown.to_string())
                    .selectable(true),
            )
            .into_element(),
        RowBody::HookStatus(status) => text(status.message.to_string())
            .text_size(px(11.0))
            .text_color(move || theme().muted_foreground)
            .into_element(),
        RowBody::Error(error) => view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded(px(7.0))
            .bg(move || theme().color("danger").unwrap_or(theme().primary))
            .child(
                text(error.message.to_string())
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(move || theme().primary_foreground),
            )
            .children(error.retryable.then(|| {
                let controller = controller.clone();
                view()
                    .px_2()
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .bg(move || theme().primary_foreground)
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label(i18n::text(locale, TextKey::Retry))
                    .on_press(move || {
                        let _ = controller.dispatch(DesktopIntent::RetryProvider);
                    })
                    .child(
                        text(i18n::text(locale, TextKey::Retry))
                            .text_size(px(10.0))
                            .text_color(move || theme().color("danger").unwrap_or(theme().primary)),
                    )
            }))
            .into_element(),
    };
    view().role(role).label(label).child(content).into_element()
}

fn user_row(
    time: Option<Arc<str>>,
    user: &tact_desktop_core::transcript::UserRow,
    snapshot: Signal<SnapshotRef>,
) -> AnyElement {
    let locale = snapshot.get().0.language;
    view()
        .flex_row()
        .items_start()
        .justify_end()
        .gap_3()
        .py(px(18.0))
        .pr(px(10.0))
        .child(
            view()
                .flex_1()
                .max_w(px(620.0))
                .items_end()
                .flex_col()
                .gap_1()
                .child(
                    view()
                        .flex_row()
                        .items_baseline()
                        .gap(px(7.0))
                        .child(
                            text(i18n::text(locale, TextKey::You))
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .children(time.map(|time| {
                            text(time.to_string())
                                .text_size(px(10.0))
                                .font_family("Lilex")
                                .text_color(move || theme().muted_foreground)
                        })),
                )
                .child(
                    view()
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(move || theme().border)
                        .bg(move || theme().card)
                        .px(px(14.0))
                        .py(px(10.0))
                        .child(text_view().markdown(user.text.to_string()).selectable(true)),
                )
                .children((!user.attachments.is_empty()).then(|| {
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap_1()
                        .children(user.attachments.iter().map(|path| {
                            text(format!(
                                "{} · {}",
                                path.file_name()
                                    .map(|name| name.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string()),
                                attachment_size(path)
                            ))
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .bg(move || theme().muted)
                            .text_size(px(10.0))
                            .text_color(move || theme().muted_foreground)
                        }))
                })),
        )
        .child(
            view()
                .size(px(28.0))
                .rounded(px(8.0))
                .bg(move || theme().foreground)
                .items_center()
                .justify_center()
                .child(
                    text(user_initials())
                        .text_size(px(10.0))
                        .font_family("Lilex")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().background),
                ),
        )
        .into_element()
}

fn assistant_row(
    time: Option<Arc<str>>,
    assistant: &tact_desktop_core::transcript::AssistantRow,
    snapshot: Signal<SnapshotRef>,
) -> AnyElement {
    let app = use_app();
    let draft = app.draft;
    let locale = snapshot.get().0.language;
    let markdown = assistant.markdown.to_string();
    let diagrams: Vec<mermaid::Diagram> = mermaid::blocks(&markdown)
        .iter()
        .filter_map(|block| mermaid::parse(block))
        .collect();
    let body = mermaid::strip_blocks(&markdown);
    view()
        .flex_row()
        .items_start()
        .gap_3()
        .py_3()
        .child(
            view()
                .size(px(28.0))
                .rounded(px(8.0))
                .bg(move || theme().card)
                .border_1()
                .border_color(move || theme().border)
                .items_center()
                .justify_center()
                .child(
                    text("T")
                        .text_size(px(16.0))
                        .text_color(move || theme().primary),
                ),
        )
        .child(
            view()
                .flex_1()
                .flex_col()
                .gap_2()
                .child(
                    view()
                        .flex_row()
                        .items_baseline()
                        .gap(px(7.0))
                        .child(
                            text("Tact")
                                .text_size(px(11.0))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .children(time.map(|time| {
                            text(time.to_string())
                                .text_size(px(10.0))
                                .font_family("Lilex")
                                .text_color(move || theme().muted_foreground)
                        }))
                        .child(
                            text(format!("· {}", i18n::text(locale, TextKey::Assistant)))
                                .text_size(px(10.0))
                                .font_family("Lilex")
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(text_view().markdown(body).selectable(true))
                .children(diagrams.iter().enumerate().map(|(index, diagram)| {
                    mermaid::element(&format!("assistant-mermaid-{index}"), diagram)
                }))
                .children((!assistant.suggestions.is_empty()).then(|| {
                    view().flex_row().flex_wrap().gap_1().children(
                        assistant.suggestions.iter().map(|suggestion| {
                            let value = suggestion.to_string();
                            view()
                                .px_2()
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .border_1()
                                .border_color(move || theme().border)
                                .cursor_pointer()
                                .role(AccessibilityRole::Button)
                                .label(value.clone())
                                .hover(|style| style.bg(move || theme().muted))
                                .on_press(move || draft.set(value.clone()))
                                .child(
                                    text(suggestion.to_string())
                                        .text_size(px(11.0))
                                        .text_color(move || theme().primary),
                                )
                        }),
                    )
                })),
        )
        .into_element()
}

fn thinking_row(
    row: &TranscriptRow,
    thinking: &tact_desktop_core::transcript::ThinkingRow,
    snapshot: Signal<SnapshotRef>,
) -> AnyElement {
    let app = use_app();
    let expanded = app.expanded_thinking;
    let id = row.id;
    let open = move || expanded.with(|ids| ids.contains(&id));
    let locale = snapshot.get().0.language;
    let markdown = thinking.markdown.to_string();
    let duration = thinking.duration_ms;
    view()
        .flex_col()
        .gap_1()
        .py(px(6.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(i18n::text(locale, TextKey::Thinking))
                .on_press(move || {
                    expanded.update(|ids| {
                        if let Some(index) = ids.iter().position(|value| *value == id) {
                            ids.remove(index);
                        } else {
                            ids.push(id);
                        }
                    });
                })
                .child(
                    text(move || {
                        let mut label = i18n::text(locale, TextKey::Thinking).to_owned();
                        if let Some(duration) = duration {
                            label.push_str(" · ");
                            label.push_str(&format_duration(duration));
                        }
                        label
                    })
                    .text_size(px(11.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(if open() { "▾" } else { "▸" })
                        .text_size(px(10.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(when(open, move || {
            view()
                .pl_3()
                .child(text_view().markdown(markdown.clone()).selectable(true))
        }))
        .into_element()
}

fn plan_row(row: &TranscriptRow, plan: &PlanRow, snapshot: Signal<SnapshotRef>) -> AnyElement {
    if plan_is_tool_execution(row, plan, &snapshot.get().0.transcript) {
        return view().into_element();
    }
    let locale = snapshot.get().0.language;
    view()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(move || theme().border)
        .bg(move || theme().card)
        .child(
            text(plan.title.to_string())
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .children(plan.steps.iter().enumerate().map(|(index, step)| {
            let status = step.status;
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(match status {
                        RowStatus::Success | RowStatus::Resolved => "✓",
                        RowStatus::Running => "●",
                        RowStatus::Failed => "×",
                        _ => "○",
                    })
                    .text_size(px(11.0))
                    .text_color(move || status_color(status)),
                )
                .child(
                    text(step.label.to_string())
                        .flex_1()
                        .text_size(px(12.0))
                        .text_color(move || {
                            if matches!(status, RowStatus::Pending | RowStatus::Cancelled) {
                                theme().muted_foreground
                            } else {
                                theme().foreground
                            }
                        }),
                )
                .child(
                    text(format!("#{}", index + 1))
                        .text_size(px(9.0))
                        .font_family("Lilex")
                        .text_color(move || theme().muted_foreground),
                )
        }))
        .child(
            text(i18n::text(locale, TextKey::Plan))
                .text_size(px(9.0))
                .text_color(move || theme().muted_foreground),
        )
        .into_element()
}

fn tool_group(
    row: &TranscriptRow,
    group: &ToolGroupRow,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    depth: usize,
) -> AnyElement {
    let locale = snapshot.get().0.language;
    let group_id = row.id;
    let toggle_controller = controller.clone();
    let mut card = view()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .bg(move || theme().card)
        .border_1()
        .border_color(move || theme().border)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(group.summary.to_string())
                .on_press(move || {
                    let _ = toggle_controller.dispatch(DesktopIntent::ToggleGroup { id: group_id });
                })
                .child(
                    text(group.summary.to_string())
                        .flex_1()
                        .text_size(px(11.0))
                        .font_family("Lilex")
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(if group.collapsed { "▸" } else { "▾" })
                        .text_size(px(10.0))
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(i18n::text(locale, TextKey::ToolGroup))
                        .text_size(px(9.0))
                        .text_color(move || theme().muted_foreground),
                ),
        );
    if !group.collapsed {
        for item in group.items.iter() {
            if let ToolGroupItem::Tool(tool) = item {
                card = card.child(tool_card(
                    group_id,
                    tool,
                    snapshot,
                    controller.clone(),
                    depth,
                ));
            }
        }
    }
    card.into_element()
}

fn tool_card(
    group: RowId,
    tool: &ToolRow,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    depth: usize,
) -> AnyElement {
    let id = tool.id;
    let status = tool.status;
    let expanded = tool.expanded;
    let toggle_controller = controller.clone();
    let mut card = view()
        .flex_col()
        .gap_1()
        .px_2()
        .py(px(3.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(tool.name.to_string())
        .hover(|style| style.bg(move || theme().muted))
        .on_press(move || {
            let _ = toggle_controller.dispatch(DesktopIntent::ToggleTool { group, tool: id });
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(if expanded { "▾" } else { "▸" })
                        .text_size(px(9.0))
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(tool.name.to_string())
                        .text_size(px(11.5))
                        .font_family("Lilex")
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(tool.arg_summary.to_string())
                        .flex_1()
                        .number_of_lines(1)
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                )
                .children(tool.duration_ms.map(|duration| {
                    text(format_duration(duration))
                        .text_size(px(9.0))
                        .font_family("Lilex")
                        .text_color(move || theme().muted_foreground)
                }))
                .child(
                    text(status_label(status))
                        .text_size(px(9.0))
                        .text_color(move || status_color(status)),
                )
                .child(open_button(
                    controller.clone(),
                    ReaderRef::ToolOutput(tool.id),
                )),
        );
    if expanded {
        card = card.child(tool_body(&tool.body)).child(meta_line(tool));
        let output = tool
            .output
            .detail
            .clone()
            .unwrap_or_else(|| tool.output.preview.clone());
        if web_search::is_search_tool(&tool.name) {
            let hits = web_search::parse(&output);
            if !hits.is_empty() {
                card = card.child(view().flex_col().gap(px(1.0)).children(hits.iter().map(
                    |hit| {
                        view()
                            .flex_col()
                            .child(
                                text(hit.title.clone())
                                    .number_of_lines(1)
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM),
                            )
                            .child(
                                text(hit.domain.clone())
                                    .text_size(px(9.0))
                                    .text_color(move || theme().muted_foreground),
                            )
                    },
                )));
            } else if !output.is_empty() {
                card = card.child(ansi::element("tool-output", &ansi::parse(&output)));
            }
        } else if !output.is_empty() {
            card = card.child(ansi::element("tool-output", &ansi::parse(&output)));
        }
        if depth < 2
            && let Some(nested) = &tool.subagent
        {
            card = card.child(nested_transcript(nested, snapshot, controller, depth + 1));
        }
    }
    card.into_element()
}

fn tool_body(body: &ToolBody) -> AnyElement {
    match body {
        ToolBody::None => view().into_element(),
        ToolBody::Text(text) => text_view()
            .markdown(text.to_string())
            .selectable(true)
            .into_element(),
        ToolBody::Code { language, text } => text_view()
            .markdown(format!(
                "```{}\n{}\n```",
                language.as_deref().unwrap_or(""),
                text
            ))
            .selectable(true)
            .into_element(),
        ToolBody::Diff { path, patch } => view()
            .flex_col()
            .gap_1()
            .child(
                text(path.as_deref().unwrap_or("diff").to_owned())
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(
                diff_view()
                    .patch(patch.to_string())
                    .layout(DiffLayout::Unified)
                    .font_size(px(11.0)),
            )
            .into_element(),
    }
}

fn meta_line(tool: &ToolRow) -> AnyElement {
    let mut parts = Vec::new();
    if let Some(model) = &tool.meta.model {
        parts.push(model.to_string());
    }
    if let Some(usage) = &tool.meta.token_usage {
        parts.push(format!(
            "{} in / {} out",
            usage.prompt_tokens, usage.completion_tokens
        ));
    }
    if let Some(task) = &tool.meta.task_id {
        parts.push(format!("task {task}"));
    }
    if let Some(agent) = &tool.meta.subagent_id {
        parts.push(format!("agent {agent}"));
    }
    if parts.is_empty() {
        return view().into_element();
    }
    text(parts.join(" · "))
        .text_size(px(9.0))
        .font_family("Lilex")
        .text_color(move || theme().muted_foreground)
        .into_element()
}

fn nested_transcript(
    view_model: &TranscriptView,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
    depth: usize,
) -> AnyElement {
    view()
        .flex_col()
        .gap_2()
        .pl_3()
        .border_l_1()
        .border_l_color(move || theme().border)
        .children(
            view_model
                .iter()
                .map(|row| row_element(row.clone(), snapshot, controller.clone(), depth)),
        )
        .into_element()
}

fn diff_row(
    row: &TranscriptRow,
    diff: &tact_desktop_core::transcript::DiffRow,
    controller: DesktopController,
) -> AnyElement {
    view()
        .flex_col()
        .gap_1()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    text(diff.path.as_deref().unwrap_or("diff").to_owned())
                        .flex_1()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(open_button(controller, ReaderRef::Diff(row.id))),
        )
        .child(
            diff_view()
                .patch(diff.patch.to_string())
                .layout(DiffLayout::Unified)
                .font_size(px(11.0)),
        )
        .into_element()
}

fn gate_card(gate: &GateCard, controller: DesktopController) -> AnyElement {
    let multi = gate.kind == GateKind::MultiSelect;
    let pending = gate.status == GateState::Pending;
    let selection: Vec<usize> = match &gate.selection {
        GateSelection::Single(Some(index)) => vec![*index],
        GateSelection::Multiple(indices) => indices.clone(),
        _ => Vec::new(),
    };
    let mut options = view().flex_row().flex_wrap().gap_2();
    for (index, option) in gate.options.iter().enumerate() {
        let controller = controller.clone();
        let selected = selection.contains(&index);
        let session_epoch = gate.session_epoch;
        let request_id = gate.request_id;
        let next_selection = if multi {
            let mut next = selection.clone();
            if selected {
                next.retain(|value| *value != index);
            } else {
                next.push(index);
            }
            next
        } else {
            Vec::new()
        };
        let label = option.to_string();
        options = options.child(
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(label.clone())
                .bg(move || {
                    if selected {
                        theme().primary
                    } else {
                        theme().muted
                    }
                })
                .on_press(move || {
                    if !pending {
                        return;
                    }
                    if multi {
                        let _ = controller.dispatch(DesktopIntent::SetGateDraft {
                            session_epoch,
                            request_id: request_id.0,
                            selected: next_selection.clone(),
                        });
                    } else {
                        let _ = controller.dispatch(DesktopIntent::RespondGate {
                            session_epoch,
                            request_id: request_id.0,
                            response: GateResponse::Select(Some(index)),
                        });
                    }
                })
                .child(text(label).text_size(px(12.0)).text_color(move || {
                    if selected {
                        theme().primary_foreground
                    } else {
                        theme().foreground
                    }
                })),
        );
    }
    let submit = if multi {
        let controller = controller.clone();
        let session_epoch = gate.session_epoch;
        let request_id = gate.request_id;
        let selected = selection.clone();
        Some(
            view()
                .px_3()
                .py(px(5.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label("Submit")
                .bg(move || theme().primary)
                .on_press(move || {
                    let _ = controller.dispatch(DesktopIntent::RespondGate {
                        session_epoch,
                        request_id: request_id.0,
                        response: GateResponse::MultiSelect(Some(selected.clone())),
                    });
                })
                .child(
                    text("Submit")
                        .text_size(px(12.0))
                        .text_color(move || theme().primary_foreground),
                ),
        )
    } else {
        None
    };
    view()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .bg(move || theme().card)
        .border_1()
        .border_color(move || theme().border)
        .child(text(gate.prompt.to_string()).text_size(px(13.0)))
        .child(options)
        .children(submit)
        .into_element()
}

fn open_button(controller: DesktopController, reference: ReaderRef) -> impl IntoElement {
    view()
        .flex_shrink_0()
        .px_2()
        .py(px(2.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label("Open")
        .hover(|style| style.bg(move || theme().muted))
        .on_press(move || {
            let _ = controller.dispatch(DesktopIntent::OpenReader(reference.clone()));
        })
        .child(
            text("Open")
                .text_size(px(10.0))
                .text_color(move || theme().primary),
        )
}

fn plan_is_tool_execution(
    row: &TranscriptRow,
    plan: &PlanRow,
    transcript: &TranscriptView,
) -> bool {
    if plan.steps.is_empty() || plan.steps.iter().any(|step| step.tool_id.is_none()) {
        return false;
    }
    let ids: Vec<&str> = plan
        .steps
        .iter()
        .filter_map(|step| step.tool_id.as_deref())
        .collect();
    transcript
        .iter()
        .filter(|candidate| candidate.ordinal > row.ordinal)
        .any(|candidate| {
            let RowBody::ToolGroup(group) = &candidate.body else {
                return false;
            };
            group.items.iter().any(|item| {
                matches!(item, ToolGroupItem::Tool(tool) if ids.contains(&tool.tool_id.as_ref()))
            })
        })
}

fn status_label(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Pending => "pending",
        RowStatus::Running => "running",
        RowStatus::Success => "done",
        RowStatus::Failed => "failed",
        RowStatus::Cancelled => "cancelled",
        RowStatus::Resolved => "resolved",
        RowStatus::Unknown => "unknown",
    }
}

fn status_color(status: RowStatus) -> nsh_core::element::AdaptiveColor {
    match status {
        RowStatus::Failed => theme().color("danger").unwrap_or(theme().foreground),
        RowStatus::Success | RowStatus::Resolved => {
            theme().color("success").unwrap_or(theme().foreground)
        }
        RowStatus::Running => theme().primary,
        _ => theme().muted_foreground,
    }
}

fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        format!("{}m", ms / 60_000)
    }
}

fn elapsed_label(started: Option<chrono::DateTime<chrono::Utc>>) -> String {
    started
        .map(|started| {
            let elapsed = (chrono::Utc::now() - started).num_seconds().max(0);
            format_duration((elapsed as u64) * 1_000)
        })
        .unwrap_or_else(|| "0s".to_owned())
}

fn attachment_size(path: &std::path::Path) -> String {
    let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{} KB", bytes / 1_024)
    } else {
        format!("{bytes} B")
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or(text)
        .chars()
        .take(120)
        .collect()
}

fn user_initials() -> &'static str {
    "ME"
}

fn sticky_strip(snapshot: Signal<SnapshotRef>) -> AnyElement {
    dynamic(move || {
        let current = snapshot.get();
        let Some(task) = crate::ui::sticky::active_task(&current.0.inspector) else {
            return view().into_element();
        };
        let subject = task.subject.to_string();
        let status = task.status;
        let elapsed = crate::ui::sticky::elapsed_label(
            task.started_at,
            task.completed_at,
            chrono::Utc::now().timestamp_millis(),
        );
        view()
            .flex_row()
            .items_center()
            .gap_2()
            .px_6()
            .py_2()
            .bg(move || theme().muted)
            .child(
                text("TASK")
                    .text_size(px(9.0))
                    .font_family("Lilex")
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                text(subject)
                    .flex_1()
                    .number_of_lines(1)
                    .text_size(px(11.0)),
            )
            .children(elapsed.map(|elapsed| {
                text(elapsed)
                    .text_size(px(9.0))
                    .font_family("Lilex")
                    .text_color(move || theme().muted_foreground)
            }))
            .child(
                text(match status {
                    tact_desktop_core::TaskStatusSnapshot::InProgress => "running",
                    tact_desktop_core::TaskStatusSnapshot::Pending => "pending",
                    tact_desktop_core::TaskStatusSnapshot::Completed => "done",
                })
                .text_size(px(9.0))
                .text_color(move || theme().muted_foreground),
            )
            .into_element()
    })
}
