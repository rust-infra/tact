//! Pending gates shown above the composer, so a decision stays reachable
//! while the transcript is scrolled away.

use nsh_base::theme;
use nsh_core::element::{AccessibilityRole, AnyElement};
use nsh_core::prelude::*;
use tact_desktop_core::{DesktopController, DesktopIntent, GateKind, GateResponse, GateSelection};

use crate::app::SnapshotRef;

use super::RADIUS;

pub fn banner(snapshot: Signal<SnapshotRef>, controller: DesktopController) -> AnyElement {
    let pending = move || snapshot.get().0.pending_gates.clone();
    dynamic(move || {
        let gates = pending();
        let Some(gate) = gates.first() else {
            return view().into_element();
        };
        let count = gates.len();
        let multi = gate.kind == GateKind::MultiSelect;
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
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label(label.clone())
                    .bg(move || {
                        if selected {
                            theme().primary
                        } else {
                            theme().background
                        }
                    })
                    .on_press(move || {
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
                    .child(text(label).text_size(px(11.0)).text_color(move || {
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
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(move || theme().primary)
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label("Submit")
                    .on_press(move || {
                        let _ = controller.dispatch(DesktopIntent::RespondGate {
                            session_epoch,
                            request_id: request_id.0,
                            response: GateResponse::MultiSelect(Some(selected.clone())),
                        });
                    })
                    .child(
                        text("Submit")
                            .text_size(px(11.0))
                            .text_color(move || theme().primary_foreground),
                    ),
            )
        } else {
            None
        };
        view()
            .flex_col()
            .gap_2()
            .mx_6()
            .mb_2()
            .p_3()
            .rounded(px(RADIUS))
            .bg(move || theme().card)
            .border_1()
            .border_color(move || theme().color("warning").unwrap_or(theme().border))
            .child(
                text(move || {
                    if count > 1 {
                        format!("{count} pending decisions")
                    } else {
                        "Pending decision".to_owned()
                    }
                })
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
            )
            .child(text(gate.prompt.to_string()).text_size(px(12.0)))
            .child(options)
            .children(submit)
            .into_element()
    })
}
