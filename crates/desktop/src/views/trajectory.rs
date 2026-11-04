//! The Trajectory page: the ledger of records and the selected record's
//! detail tabs.

use nsh_base::theme;
use nsh_core::element::AccessibilityRole;
use nsh_core::prelude::*;
use tact_desktop_core::{TrajectoryRecord, TrajectoryView};

use crate::app::SnapshotRef;
use crate::ui::shell::TrajectoryDetailTab;

use super::token;

pub fn page(snapshot: Signal<SnapshotRef>) -> impl IntoElement {
    let selected = signal(None::<usize>);
    let tab = signal(TrajectoryDetailTab::Summary);
    let trajectory = move || TrajectoryView::from_transcript(&snapshot.get().0.transcript);
    view()
        .flex_1()
        .flex_row()
        .child(
            view()
                .w(px(260.0))
                .h_full()
                .flex_col()
                .border_r_1()
                .border_r_color(move || theme().border)
                .child(header(trajectory))
                .child(scroll().flex_1().child(
                    view().flex_col().gap(px(1.0)).px_3().py_2().child(each(
                        move || trajectory().records().to_vec(),
                        |record: &TrajectoryRecord| record.index,
                        move |record, _| {
                            let record = record.get_untracked();
                            let index = record.index;
                            view()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py(px(3.0))
                                .rounded(px(5.0))
                                .cursor_pointer()
                                .role(AccessibilityRole::Button)
                                .label(record.text.to_string())
                                .style_when(
                                    move || selected.get() == Some(index),
                                    |style| style.bg(move || theme().muted),
                                )
                                .hover(|style| style.bg(move || theme().muted))
                                .on_press(move || selected.set(Some(index)))
                                .child(
                                    text(format!("#{}", record.index))
                                        .w(px(34.0))
                                        .text_size(px(10.0))
                                        .font_family("Lilex")
                                        .text_color(move || theme().muted_foreground),
                                )
                                .child(
                                    text(record.text.to_string())
                                        .flex_1()
                                        .number_of_lines(1)
                                        .text_size(px(11.0)),
                                )
                                .child(
                                    text(record.status_label())
                                        .text_size(px(9.0))
                                        .text_color(move || status_color(record.status)),
                                )
                        },
                    )),
                )),
        )
        .child(
            view()
                .flex_1()
                .h_full()
                .flex_col()
                .child(detail_tabs(tab))
                .child(
                    scroll().flex_1().child(
                        view()
                            .flex_col()
                            .gap_2()
                            .px_5()
                            .py_3()
                            .child(detail(trajectory, selected, tab)),
                    ),
                ),
        )
}

fn header(trajectory: impl Fn() -> TrajectoryView + Copy + 'static) -> impl IntoElement {
    view()
        .flex_col()
        .gap(px(1.0))
        .px_4()
        .py_3()
        .border_b_1()
        .border_b_color(move || theme().border)
        .child(
            text(move || {
                let summary = trajectory().summary();
                format!(
                    "{} records · {} turns · {} tools",
                    summary.records, summary.turns, summary.tool_calls
                )
            })
            .text_size(px(12.0))
            .font_weight(FontWeight::SEMIBOLD),
        )
        .child(
            text(move || {
                let summary = trajectory().summary();
                format!(
                    "{} measured · {} ms · {} in / {} out",
                    summary.measured,
                    summary.measured_ms,
                    summary.input_tokens,
                    summary.output_tokens
                )
            })
            .text_size(px(10.0))
            .text_color(move || theme().muted_foreground),
        )
}

fn detail_tabs(tab: Signal<TrajectoryDetailTab>) -> impl IntoElement {
    view()
        .flex_row()
        .gap_1()
        .px_4()
        .py_2()
        .border_b_1()
        .border_b_color(move || theme().border)
        .children(TrajectoryDetailTab::ALL.map(|value| {
            let active = move || tab.get() == value;
            view()
                .px_2()
                .py(px(3.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .role(AccessibilityRole::Button)
                .label(detail_label(value))
                .style_when(active, |style| style.bg(move || theme().muted))
                .on_press(move || tab.set(value))
                .child(
                    text(detail_label(value))
                        .text_size(px(10.0))
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

fn detail(
    trajectory: impl Fn() -> TrajectoryView + Copy + 'static,
    selected: Signal<Option<usize>>,
    tab: Signal<TrajectoryDetailTab>,
) -> impl IntoElement {
    dynamic(move || match selected.get() {
        None => text("Select a record")
            .text_size(px(12.0))
            .text_color(move || theme().muted_foreground)
            .into_element(),
        Some(index) => {
            let trajectory = trajectory();
            let Some(record) = trajectory.records().get(index) else {
                return text("Select a record")
                    .text_size(px(12.0))
                    .text_color(move || theme().muted_foreground)
                    .into_element();
            };
            let body = match tab.get() {
                TrajectoryDetailTab::Summary => format!(
                    "{}\n\n{}\n\n{}\n{}",
                    record.text,
                    record.status_label(),
                    record.tool_name.as_deref().unwrap_or(""),
                    record.at.as_deref().unwrap_or("")
                ),
                TrajectoryDetailTab::Payload => {
                    record.input.as_deref().unwrap_or("No payload").to_owned()
                }
                TrajectoryDetailTab::Result => {
                    record.output.as_deref().unwrap_or("No result").to_owned()
                }
                TrajectoryDetailTab::Schema => {
                    record.tool_name.as_deref().unwrap_or("No tool").to_owned()
                }
                TrajectoryDetailTab::Timing => format!(
                    "{}\n{}",
                    record
                        .duration_ms
                        .map(|ms| format!("{ms} ms"))
                        .unwrap_or_else(|| "No duration".to_owned()),
                    record
                        .usage
                        .as_ref()
                        .map(|usage| format!(
                            "{} prompt · {} completion",
                            usage.prompt_tokens, usage.completion_tokens
                        ))
                        .unwrap_or_default()
                ),
            };
            nsh_base::text_view()
                .markdown(body)
                .selectable(true)
                .into_element()
        }
    })
}

fn detail_label(tab: TrajectoryDetailTab) -> &'static str {
    match tab {
        TrajectoryDetailTab::Summary => "Summary",
        TrajectoryDetailTab::Payload => "Payload",
        TrajectoryDetailTab::Result => "Result",
        TrajectoryDetailTab::Schema => "Schema",
        TrajectoryDetailTab::Timing => "Timing",
    }
}

fn status_color(status: tact_desktop_core::RowStatus) -> nsh_core::element::AdaptiveColor {
    match status {
        tact_desktop_core::RowStatus::Failed => token("danger"),
        tact_desktop_core::RowStatus::Success | tact_desktop_core::RowStatus::Resolved => {
            token("success")
        }
        tact_desktop_core::RowStatus::Running => theme().primary,
        _ => theme().muted_foreground,
    }
}
