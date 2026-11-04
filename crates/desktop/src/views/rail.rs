//! The session rail: the project, its sessions and the rail's filters.

use chrono::Utc;
use nsh_base::{input, theme};
use nsh_core::element::{AccessibilityRole, ReadSignal};
use nsh_core::prelude::*;
use tact_desktop_core::{
    ArchiveFilter, DesktopController, DesktopIntent, SessionId, SessionSummaryView,
};

use crate::app::SnapshotRef;
use crate::ui::session_rail;

use super::token;

pub const WIDTH: f32 = 250.0;

pub fn rail(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    let sessions = move || snapshot.get().0.sessions.items.clone();
    view()
        .w(px(WIDTH))
        .h_full()
        .flex_col()
        .bg(move || token("sidebar"))
        .border_r_1()
        .border_r_color(move || token("sidebar-border"))
        .child(project_header(snapshot, controller.clone()))
        .child(filters(snapshot, controller.clone()))
        .child(
            scroll()
                .flex_1()
                .child(view().flex_col().gap(px(2.0)).px_2().pb_3().child(each(
                    sessions,
                    |session: &SessionSummaryView| session.id.clone(),
                    {
                        let controller = controller.clone();
                        move |session, _| session_row(session, snapshot, controller.clone())
                    },
                ))),
        )
}

fn project_header(
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap_2()
        .px_4()
        .pt_4()
        .pb_3()
        .child(
            view()
                .flex_1()
                .flex_col()
                .gap(px(1.0))
                .child(
                    text(move || snapshot.get().0.project.display_name.to_string())
                        .number_of_lines(1)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    text(move || {
                        snapshot
                            .get()
                            .0
                            .project
                            .branch
                            .as_deref()
                            .unwrap_or("")
                            .to_owned()
                    })
                    .number_of_lines(1)
                    .text_size(px(11.0))
                    .text_color(move || theme().muted_foreground),
                ),
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
                .on_press(move || {
                    let _ = controller.dispatch(DesktopIntent::NewSession);
                })
                .child(
                    text("+")
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().primary_foreground),
                ),
        )
}

fn filters(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> impl IntoElement {
    let filters = [
        ArchiveFilter::Active,
        ArchiveFilter::Archived,
        ArchiveFilter::All,
    ];
    view()
        .flex_col()
        .gap_2()
        .px_3()
        .pb_3()
        .child(
            view()
                .flex_row()
                .gap(px(4.0))
                .children(filters.map(|filter| {
                    let controller = controller.clone();
                    let active = move || snapshot.get().0.sessions.filter == filter;
                    view()
                        .flex_1()
                        .items_center()
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .role(AccessibilityRole::Button)
                        .label(session_rail::filter_label(filter))
                        .style_when(active, |style| style.bg(move || token("sidebar-accent")))
                        .hover(|style| style.bg(move || theme().muted))
                        .on_press(move || {
                            let _ = controller.dispatch(DesktopIntent::SetSessionFilter(filter));
                        })
                        .child(
                            text(session_rail::filter_label(filter))
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
                })),
        )
        .child(
            input()
                .value(move || snapshot.get().0.sessions.query.clone())
                .on_change(move |text: &str| {
                    let _ = controller.dispatch(DesktopIntent::SetSessionQuery(text.to_owned()));
                })
                .placeholder("Search sessions")
                .text_size(px(12.0))
                .px_3()
                .py(px(6.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(move || theme().border)
                .bg(move || theme().background),
        )
}

fn session_row(
    session: ReadSignal<SessionSummaryView>,
    snapshot: Signal<SnapshotRef>,
    controller: DesktopController,
) -> impl IntoElement {
    let selected = move || {
        snapshot
            .get()
            .0
            .sessions
            .selected
            .as_ref()
            .is_some_and(|id| id.0 == session.get().id)
    };
    view()
        .flex_col()
        .gap(px(2.0))
        .px_3()
        .py_2()
        .rounded(px(8.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(move || session.get().title.clone())
        .style_when(selected, |style| style.bg(move || token("sidebar-accent")))
        .hover(|style| style.bg(move || theme().muted))
        .on_press(move || {
            let _ = controller.dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId(session.get().id.clone()),
            });
        })
        .child(
            text(move || session.get().title.clone())
                .number_of_lines(1)
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM),
        )
        .child(
            text(move || {
                session_rail::session_detail(&session.get(), Utc::now(), snapshot.get().0.language)
            })
            .number_of_lines(1)
            .text_size(px(11.0))
            .text_color(move || theme().muted_foreground),
        )
}
