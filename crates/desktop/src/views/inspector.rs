//! The inspector: Run, Goals, Subagents, Background and Context.

use nsh_base::theme;
use nsh_core::element::AccessibilityRole;
use nsh_core::prelude::*;
use tact_desktop_core::{
    BackgroundTaskView, DesktopController, DesktopIntent, InspectorTab, RowStatus,
};

use crate::app::SnapshotRef;
use crate::ui::inspector;

use super::token;

pub const WIDTH: f32 = 318.0;

pub fn inspector(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    view()
        .w(px(WIDTH))
        .h_full()
        .flex_col()
        .bg(move || theme().background)
        .border_l_1()
        .border_l_color(move || theme().border)
        .child(tabs(snapshot, controller.clone()))
        .child(
            scroll().flex_1().child(
                view()
                    .flex_col()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .child(dynamic(move || match snapshot.get().0.inspector.tab {
                        InspectorTab::Run => run_tab(snapshot).into_element(),
                        InspectorTab::Goals => goals_tab(snapshot).into_element(),
                        InspectorTab::Subagents => subagents_tab(snapshot).into_element(),
                        InspectorTab::Background => background_tab(snapshot).into_element(),
                        InspectorTab::Context => context_tab(snapshot).into_element(),
                    })),
            ),
        )
}

fn tabs(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    view()
        .flex_row()
        .flex_wrap()
        .gap_1()
        .px_3()
        .py_2()
        .border_b_1()
        .border_b_color(move || theme().border)
        .children(inspector::tabs().map(|(tab, label)| {
            let controller = controller.clone();
            let active = move || snapshot.get().0.inspector.tab == tab;
            view()
                .px_2()
                .py(px(4.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(label)
                .style_when(active, |style| style.bg(move || theme().muted))
                .on_press(move || {
                    let _ = controller.dispatch(DesktopIntent::SetInspector(tab));
                })
                .child(
                    text(label)
                        .text_size(px(11.0))
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

fn run_tab(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    view()
        .flex_col()
        .gap_1()
        .child(
            text(move || format!("{:?}", snapshot.get().0.runtime.current_phase))
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(move || {
                snapshot
                    .get()
                    .0
                    .runtime
                    .latest_tool
                    .as_deref()
                    .unwrap_or("No tool running")
                    .to_owned()
            })
            .text_size(px(12.0))
            .font_family("Lilex")
            .text_color(move || theme().muted_foreground),
        )
        .child(
            text(move || format!("{} turns", snapshot.get().0.runtime.turns_taken))
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn goals_tab(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    let rows = move || {
        snapshot
            .get()
            .0
            .inspector
            .goals
            .iter()
            .map(inspector::goal_row)
            .collect::<Vec<_>>()
    };
    row_list(rows)
}

fn subagents_tab(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    let rows = move || {
        snapshot
            .get()
            .0
            .inspector
            .subagents
            .iter()
            .map(inspector::subagent_row)
            .collect::<Vec<_>>()
    };
    row_list(rows)
}

fn background_tab(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        move || snapshot.get().0.inspector.background.iter().cloned().collect::<Vec<_>>(),
        |task: &BackgroundTaskView| task.id.clone(),
        |task, _| background_row(task),
    ))
}

fn background_row(task: nsh_core::element::ReadSignal<BackgroundTaskView>) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(1.0))
        .child(
            text(move || task.get().command.to_string())
                .number_of_lines(2)
                .text_size(px(12.0))
                .font_family("Lilex"),
        )
        .child(
            text(move || format!("{:?}", task.get().status))
                .text_size(px(11.0))
                .text_color(move || theme().muted_foreground),
        )
}

fn context_tab(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    view()
        .flex_col()
        .gap_1()
        .child(
            text(move || {
                let usage = &snapshot.get().0.runtime.usage;
                format!(
                    "{} prompt · {} completion · {} total",
                    usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
                )
            })
            .text_size(px(12.0)),
        )
        .child(
            text(move || {
                let usage = &snapshot.get().0.runtime.usage;
                match usage.context_window {
                    Some(window) => format!("{} of {window} context", usage.total_tokens),
                    None => "No context window".to_owned(),
                }
            })
            .text_size(px(11.0))
            .text_color(move || theme().muted_foreground),
        )
}

fn row_list(
    rows: impl Fn() -> Vec<inspector::InspectorRow> + 'static,
) -> impl IntoElement {
    view().flex_col().gap_2().child(each(
        rows,
        |row: &inspector::InspectorRow| row.id.clone(),
        |row, _| {
            let row = row.get_untracked();
            view()
                .flex_col()
                .gap(px(1.0))
                .child(
                    text(row.title)
                        .number_of_lines(1)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(row.detail)
                        .number_of_lines(2)
                        .text_size(px(11.0))
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(status_label(row.status))
                        .text_size(px(10.0))
                        .text_color(move || status_color(row.status)),
                )
        },
    ))
}

fn status_label(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Pending => "pending",
        RowStatus::Unknown => "unknown",
        RowStatus::Running => "running",
        RowStatus::Success => "done",
        RowStatus::Failed => "failed",
        RowStatus::Cancelled => "cancelled",
        RowStatus::Resolved => "resolved",
    }
}

fn status_color(status: RowStatus) -> nsh_core::element::AdaptiveColor {
    match status {
        RowStatus::Failed => token("danger"),
        RowStatus::Success | RowStatus::Resolved => token("success"),
        RowStatus::Running => theme().primary,
        RowStatus::Pending | RowStatus::Cancelled | RowStatus::Unknown => {
            theme().muted_foreground
        }
    }
}
