//! The run room: the conversation, its rows and the Trajectory page.

use std::sync::Arc;

use nsh_base::{text_view, theme};
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::{
    DesktopController, DesktopIntent, GateCard, GateKind, GateResponse, GateSelection, GateState,
    ReaderRef, RowBody, RowId, RowStatus, ToolGroupItem, ToolGroupRow, ToolRow, TranscriptRow,
};

use crate::app::SnapshotRef;
use crate::ui::shell::RunTab;
use crate::ui::{ansi, mermaid, transcript::visible_row, web_search};

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

fn chat(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    let row_controller = controller.clone();
    let rows = move || {
        snapshot
            .get()
            .0
            .transcript
            .iter()
            .filter(|row| visible_row(row, &snapshot.get().0))
            .cloned()
            .collect::<Vec<_>>()
    };
    view().flex_1().child(
        scroll()
            .flex_1()
            .child(view().flex_col().gap_3().px_6().py_4().child(each(
                rows,
                |row: &Arc<TranscriptRow>| row.ordinal,
                move |row, _| {
                    let controller = row_controller.clone();
                    dynamic(move || row_element(row.get(), snapshot, controller.clone()))
                },
            ))),
    )
}

fn row_element(
    row: Arc<TranscriptRow>,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> AnyElement {
    let locale = snapshot.get().0.language;
    let label = crate::ui::transcript_row::accessibility_label(&row, locale);
    let role = crate::ui::transcript_row::accessibility_role(&row);
    let body = match &row.body {
        RowBody::User(user) => text(user.text.to_string())
            .text_size(px(14.0))
            .into_element(),
        RowBody::Assistant(assistant) => assistant_body(assistant),
        RowBody::Thinking(thinking) => view()
            .flex_col()
            .gap_1()
            .child(
                text_view()
                    .markdown(thinking.markdown.to_string())
                    .selectable(true),
            )
            .child(open_button(controller.clone(), ReaderRef::Thinking(row.id)))
            .into_element(),
        RowBody::Plan(plan) => view()
            .flex_col()
            .gap_1()
            .child(
                text(plan.title.to_string())
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .children(plan.steps.iter().map(|step| {
                text(step.label.to_string())
                    .text_size(px(12.0))
                    .text_color(theme().muted_foreground)
            }))
            .into_element(),
        RowBody::ToolGroup(group) => tool_group(row.id, group, controller.clone()),
        RowBody::Diff(diff) => view()
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
                    .child(open_button(controller.clone(), ReaderRef::Diff(row.id))),
            )
            .child(
                nsh_base::diff_view()
                    .patch(diff.patch.to_string())
                    .layout(nsh_base::DiffLayout::Unified)
                    .font_size(px(11.0)),
            )
            .into_element(),
        RowBody::Gate(gate) => gate_card(gate, controller.clone()),
        RowBody::Notice(notice) => text(notice.text.to_string())
            .text_size(px(12.0))
            .text_color(theme().muted_foreground)
            .into_element(),
        RowBody::HookContext(context) => text_view()
            .markdown(context.markdown.to_string())
            .into_element(),
        RowBody::HookStatus(status) => text(status.message.to_string())
            .text_size(px(11.0))
            .text_color(theme().muted_foreground)
            .into_element(),
        RowBody::Error(error) => text(error.message.to_string())
            .text_size(px(13.0))
            .text_color(move || theme().color("danger").unwrap_or(theme().foreground))
            .into_element(),
    };
    view().role(role).label(label).child(body).into_element()
}

fn tool_group(row: RowId, group: &ToolGroupRow, controller: DesktopController) -> AnyElement {
    let mut card = view()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(RADIUS))
        .bg(move || theme().card)
        .border_1()
        .border_color(move || theme().border)
        .child(
            text(group.summary.to_string())
                .text_size(px(11.0))
                .font_family("Lilex")
                .text_color(move || theme().muted_foreground),
        );
    for item in group.items.iter() {
        if let ToolGroupItem::Tool(tool) = item {
            card = card.child(tool_card(row, tool, controller.clone()));
        }
    }
    card.into_element()
}

fn tool_card(row: RowId, tool: &ToolRow, controller: DesktopController) -> AnyElement {
    let id = tool.id;
    let status = tool.status;
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
            let _ = toggle_controller.dispatch(DesktopIntent::ToggleTool {
                group: row,
                tool: id,
            });
        })
        .child(
            view()
                .flex_row()
                .items_center()
                .gap_2()
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
    if tool.expanded {
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
                            .gap(px(0.0))
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
    }
    card.into_element()
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

fn assistant_body(assistant: &tact_desktop_core::transcript::AssistantRow) -> AnyElement {
    let markdown = assistant.markdown.to_string();
    let diagrams: Vec<mermaid::Diagram> = mermaid::blocks(&markdown)
        .iter()
        .filter_map(|block| mermaid::parse(block))
        .collect();
    let body = mermaid::strip_blocks(&markdown);
    view()
        .flex_col()
        .gap_2()
        .child(text_view().markdown(body).selectable(true))
        .children(
            diagrams
                .iter()
                .enumerate()
                .map(|(index, diagram)| mermaid::element(&format!("mermaid-{index}"), diagram)),
        )
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
