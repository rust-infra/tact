//! Prompt behavior, overlay, accessibility, and input regression coverage.
#![cfg(test)]

use super::model_picker::{
    apply_model_option_state, model_groups, prompt_model_control, prompt_option_bare,
};
use super::suggestions::{
    PromptTokenKind, SuggestionKey, active_prompt_token, build_submission, retain_active_suggestion,
};
use super::{
    ProgressState, PromptAttachment, PromptBar, PromptBarEvent, PromptCommand, PromptMention,
    PromptModel, prompt_control, prompt_frame, prompt_listbox, prompt_status,
    stable_ids_are_unique,
};
use gpui::{
    AppContext as _, Element as _, Focusable as _, IntoElement as _, ParentElement as _, Render,
    RenderOnce as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, Window, accesskit, canvas, div, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex},
};

type CapturedControl = Arc<Mutex<Option<(Option<Role>, accesskit::Node)>>>;

struct ControlProbe {
    captured: CapturedControl,
    selected_option: bool,
    disabled: bool,
}

struct ModelControlProbe {
    captured: CapturedControl,
}

struct ModelOptionProbe {
    captured: CapturedControl,
}

struct PromptHarness {
    prompt: gpui::Entity<PromptBar>,
    bottom_aligned: bool,
    _subscription: gpui::Subscription,
}

impl PromptHarness {
    fn new(
        events: Rc<RefCell<Vec<PromptBarEvent>>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let prompt = cx.new(|cx| {
            let mut prompt = PromptBar::new("keyboard-prompt", window, cx);
            prompt.set_mentions(
                [
                    PromptMention::new("creamery", "Creamery"),
                    PromptMention::new("suppliers", "Suppliers"),
                ],
                cx,
            );
            prompt.set_draft("@", window, cx);
            prompt
        });
        let _subscription = cx.subscribe(&prompt, move |_, _, event, _| {
            events.borrow_mut().push(event.clone());
        });
        Self {
            prompt,
            bottom_aligned: false,
            _subscription,
        }
    }

    fn with_models(
        events: Rc<RefCell<Vec<PromptBarEvent>>>,
        bottom_aligned: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let prompt = cx.new(|cx| {
            let mut prompt = PromptBar::new("model-prompt", window, cx);
            prompt.set_models(
                [
                    PromptModel::new("fast", "Fast")
                        .provider("Lab")
                        .description("Fast responses")
                        .context_window(64_000),
                    PromptModel::new("disabled", "Disabled")
                        .provider("Lab")
                        .disabled(true),
                    PromptModel::new("balanced", "Balanced")
                        .provider("Cloud")
                        .description("Everyday work")
                        .context_window(128_000),
                    PromptModel::new("precise", "Precise")
                        .provider("Cloud")
                        .description("Detailed reasoning")
                        .context_window(200_000),
                ],
                cx,
            );
            prompt
        });
        let _subscription = cx.subscribe(&prompt, move |_, _, event, _| {
            events.borrow_mut().push(event.clone());
        });
        Self {
            prompt,
            bottom_aligned,
            _subscription,
        }
    }
}

impl Render for PromptHarness {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        if self.bottom_aligned {
            div()
                .size_full()
                .flex()
                .flex_col()
                .justify_end()
                .child(self.prompt.clone())
                .into_any_element()
        } else {
            self.prompt.clone().into_any_element()
        }
    }
}

fn draw(cx: &mut gpui::VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open_model_picker(cx: &mut gpui::VisualTestContext) {
    let trigger = cx
        .debug_bounds("prompt-bar-model-trigger")
        .expect("the model trigger should render");
    cx.simulate_click(trigger.center(), Default::default());
    draw(cx);
}

impl Render for ControlProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        let selected_option = self.selected_option;
        let disabled = self.disabled;
        canvas(
            move |_, window, cx| {
                let control = prompt_control("prompt-send", "Send prompt", cx)
                    .disabled(disabled)
                    .on_click(|_, _, _| {});
                let control = if selected_option {
                    control.role(Role::ListBoxOption).aria_selected(true)
                } else {
                    control
                };
                let element = control.render(window, cx).into_element();
                let role = element.a11y_role();
                let mut node = accesskit::Node::new(role.unwrap_or(Role::Unknown));
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") = Some((role, node));
            },
            |_, _, _, _| {},
        )
    }
}

impl Render for ModelControlProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let control = prompt_model_control("prompt-model", "Model: Balanced", true, cx)
                    .on_click(|_, _, _| {});
                let element = control.render(window, cx).into_element();
                let role = element.a11y_role();
                let mut node = accesskit::Node::new(role.unwrap_or(Role::Unknown));
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") = Some((role, node));
            },
            |_, _, _, _| {},
        )
    }
}

impl Render for ModelOptionProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let option = prompt_option_bare(
                    "prompt-model-option",
                    "Balanced",
                    div().child("Balanced"),
                    cx,
                )
                .role(Role::ListBoxOption)
                .on_click(|_, _, _| {});
                let element = apply_model_option_state(option, true, true, 2, 4, None, cx)
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                let mut node = accesskit::Node::new(role.unwrap_or(Role::Unknown));
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") = Some((role, node));
            },
            |_, _, _, _| {},
        )
    }
}

#[test]
fn models_group_by_provider_in_first_appearance_order() {
    let models = [
        PromptModel::new("a", "A").provider("Anthropic"),
        PromptModel::new("local", "Local"),
        PromptModel::new("b", "B").provider("Anthropic"),
        PromptModel::new("o", "O").provider("OpenAI"),
    ];
    let groups = model_groups(&models);
    let providers: Vec<Option<&str>> = groups
        .iter()
        .map(|(provider, _)| provider.as_deref())
        .collect();
    assert_eq!(providers, [Some("Anthropic"), None, Some("OpenAI")]);
    let anthropic: Vec<&str> = groups[0].1.iter().map(|model| model.id.as_ref()).collect();
    assert_eq!(anthropic, ["a", "b"]);
    assert_eq!(
        PromptModel::new("x", "X")
            .provider("Anthropic")
            .description("Everyday")
            .context_window(200_000)
            .context_window_tokens(),
        Some(200_000)
    );
}

#[test]
fn utf8_cursor_token_extraction_uses_byte_offsets() {
    let draft = "plan 🍦 @crème today";
    let cursor = draft.find(" today").expect("suffix should exist");
    let token = active_prompt_token(draft, cursor).expect("mention token should be active");

    assert_eq!(token.kind, PromptTokenKind::Mention);
    assert_eq!(&draft[token.range], "@crème");
    assert_eq!(token.query, "crème");
}

#[test]
fn mid_token_range_includes_the_untyped_suffix_for_replacement() {
    let draft = "Ask @Creamery about pricing";
    let cursor = draft.find("amery").expect("mention suffix should exist");
    let token = active_prompt_token(draft, cursor).expect("mention token should be active");

    assert_eq!(&draft[token.range], "@Creamery");
    assert_eq!(token.query, "Cre");
}

#[test]
fn stable_duplicate_label_suggestions_retain_identity() {
    let first = SuggestionKey::Mention("first".into());
    let second = SuggestionKey::Mention("second".into());
    let filtered = vec![first.clone(), second.clone()];

    assert_eq!(
        retain_active_suggestion(Some(second.clone()), &filtered),
        Some(second)
    );
    assert_ne!(
        PromptMention::new("first", "Sam").id(),
        PromptMention::new("second", "Sam").id()
    );
}

#[gpui::test]
fn visible_suggestion_label_changes_notify_the_prompt_entity(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (prompt, cx) = cx.add_window_view(|window, cx| PromptBar::new("prompt", window, cx));
    prompt.update(cx, |prompt, cx| {
        prompt.set_mentions([PromptMention::new("creamery", "Creamery")], cx);
    });
    cx.update(|window, cx| {
        prompt.update(cx, |prompt, cx| prompt.set_draft("@cr", window, cx));
    });
    assert!(prompt.read_with(cx, |prompt, _| prompt.token.is_some()));

    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&prompt, move |_, _| observed.set(observed.get() + 1)));

    prompt.update(cx, |prompt, cx| {
        prompt.set_mentions([PromptMention::new("creamery", "Creamery team")], cx);
    });

    assert_eq!(notifications.get(), 1);
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn cursor_only_keyboard_change_refreshes_the_active_token(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (prompt, cx) = cx.add_window_view(|window, cx| {
        let mut prompt = PromptBar::new("prompt", window, cx);
        prompt.set_mentions(
            [
                PromptMention::new("creamery", "Creamery"),
                PromptMention::new("suppliers", "Suppliers"),
            ],
            cx,
        );
        prompt.set_draft("Ask @Creamery then @Suppliers", window, cx);
        prompt
    });
    cx.update(|window, cx| {
        prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
        window.draw(cx).clear(cx);
    });

    cx.simulate_keystrokes("left left left left left left left left");

    assert_eq!(
        prompt.read_with(cx, |prompt, _| prompt
            .token
            .as_ref()
            .map(|token| token.query.clone())),
        Some("S".to_owned())
    );
    assert_eq!(
        prompt.read_with(cx, |prompt, _| prompt.filtered.clone()),
        vec![SuggestionKey::Mention("suppliers".into())]
    );
}

#[gpui::test]
fn cursor_only_mouse_equivalent_refreshes_and_replaces_the_whole_token(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(captured_events, window, cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_draft("Ask @Creamery now", window, cx);
            let cursor = "Ask @Cre".len();
            prompt.editor.update(cx, |editor, cx| {
                editor.set_selected_range(cursor..cursor, cx)
            });
        });
        window.draw(cx).clear(cx);
    });

    let prompt = harness.read_with(cx, |harness, _| harness.prompt.clone());
    cx.update(|window, cx| {
        prompt.update(cx, |prompt, cx| prompt.insert_active_suggestion(window, cx));
    });

    assert_eq!(
        prompt.read_with(cx, |prompt, cx| prompt.draft(cx)),
        "Ask @Creamery now"
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn unmatched_token_does_not_capture_native_multiline_up(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(events, window, cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_draft("first line\n@unmatched", window, cx);
            prompt.focus(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    let before = harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).editor.read(cx).cursor()
    });

    cx.simulate_keystrokes("up");

    let after = harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).editor.read(cx).cursor()
    });
    assert!(after < before, "native multiline up should move the caret");
}

/// The hover highlight sits on the row, not on the row's text.
///
/// It did not. The highlight is placed by subtracting one measured box from
/// another, and the two were being measured differently: `on_prepaint` reports
/// a `size_full` child, which is the **content** box - inside the border and
/// the padding both. A model option carries eight pixels of left padding, so
/// the highlight landed nine pixels in and ten short, clipping the group
/// heading above it and missing the row's own selection fill entirely.
///
/// The picker is the case that catches it because its rows are padded and its
/// frame is padded too; a list whose rows have neither looked fine throughout.
#[gpui::test]
fn the_glide_highlight_covers_the_row_it_is_on(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    draw(cx);
    harness.update(cx, |harness, cx| {
        harness.prompt.update(cx, |prompt, cx| {
            prompt.model_menu_open = true;
            cx.notify();
        });
    });
    draw(cx);
    draw(cx);

    let row = cx
        .debug_bounds("prompt-bar-model-option-balanced")
        .expect("the menu renders its rows");
    cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
    draw(cx);
    draw(cx);

    let glide = cx
        .debug_bounds("prompt-bar-model-glide")
        .expect("hovering a row mounts the highlight");

    // One border width in on every side, which is what "on the row" means for
    // a fill that must not paint over the frame around it.
    let inset = px(1.);
    assert_eq!(glide.origin.y, row.origin.y + inset, "highlight top");
    assert_eq!(glide.origin.x, row.origin.x + inset, "highlight left");
    assert_eq!(
        glide.size.height,
        row.size.height - inset * 2.,
        "highlight height"
    );
    assert_eq!(
        glide.size.width,
        row.size.width - inset * 2.,
        "highlight width"
    );
}

#[gpui::test]
fn empty_model_catalog_closes_the_menu(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (prompt, cx) = cx.add_window_view(|window, cx| PromptBar::new("prompt", window, cx));
    prompt.update(cx, |prompt, cx| {
        prompt.set_models([super::PromptModel::new("balanced", "Balanced")], cx);
        prompt.model_menu_open = true;
        prompt.set_models([], cx);
    });

    assert!(!prompt.read_with(cx, |prompt, _| prompt.model_menu_open));
    assert!(prompt.read_with(cx, |prompt, _| prompt.models.is_empty()));
}

#[gpui::test]
fn closed_model_picker_constructs_no_model_options(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    draw(cx);

    assert!(cx.debug_bounds("prompt-bar-model-trigger").is_some());
    assert!(cx.debug_bounds("prompt-bar-model-picker").is_none());
    for selector in [
        "prompt-bar-model-option-fast",
        "prompt-bar-model-option-disabled",
        "prompt-bar-model-option-balanced",
        "prompt-bar-model-option-precise",
    ] {
        assert!(cx.debug_bounds(selector).is_none());
    }
    assert!(!harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).model_menu_open
    }));
}

#[gpui::test]
fn opening_model_picker_keeps_composer_height_and_floats(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    draw(cx);
    let attach_before = cx
        .debug_bounds("prompt-bar-attach-control")
        .expect("attach should render before opening");

    open_model_picker(cx);

    let attach_after = cx
        .debug_bounds("prompt-bar-attach-control")
        .expect("attach should remain rendered");
    let trigger = cx
        .debug_bounds("prompt-bar-model-trigger")
        .expect("model trigger should remain rendered");
    let picker = cx
        .debug_bounds("prompt-bar-model-picker")
        .expect("open picker should render");
    assert_eq!(attach_after.origin.y, attach_before.origin.y);
    assert!(
        picker.top() >= trigger.bottom() || picker.bottom() <= trigger.top(),
        "picker {picker:?} must float outside trigger {trigger:?}"
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn model_picker_keys_move_by_stable_id_and_skip_disabled_models(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    draw(cx);
    open_model_picker(cx);
    let active = |cx: &mut gpui::VisualTestContext| {
        harness.read_with(cx, |harness, cx| {
            harness.prompt.read(cx).active_model.clone()
        })
    };

    assert_eq!(active(cx), Some("fast".into()));
    cx.simulate_keystrokes("down");
    assert_eq!(active(cx), Some("balanced".into()));
    cx.simulate_keystrokes("down up");
    assert_eq!(active(cx), Some("balanced".into()));
    cx.simulate_keystrokes("home");
    assert_eq!(active(cx), Some("fast".into()));
    cx.simulate_keystrokes("end");
    assert_eq!(active(cx), Some("precise".into()));
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn model_picker_enter_emits_the_active_id_once_and_closes(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        PromptHarness::with_models(captured_events, false, window, cx)
    });
    draw(cx);
    open_model_picker(cx);
    events.borrow_mut().clear();

    cx.simulate_keystrokes("down enter");
    draw(cx);

    assert!(!harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).model_menu_open
    }));
    assert!(cx.debug_bounds("prompt-bar-model-picker").is_none());
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| matches!(
                event,
                PromptBarEvent::ModelChanged { model_id, .. } if model_id == "balanced"
            ))
            .count(),
        1
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn model_picker_escape_closes_without_emitting(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) = cx.add_window_view(move |window, cx| {
        PromptHarness::with_models(captured_events, false, window, cx)
    });
    draw(cx);
    open_model_picker(cx);
    events.borrow_mut().clear();

    cx.simulate_keystrokes("escape");
    draw(cx);

    assert!(!harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).model_menu_open
    }));
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn outside_click_dismisses_the_model_picker(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    cx.simulate_resize(size(px(800.), px(600.)));
    draw(cx);
    open_model_picker(cx);

    cx.simulate_click(point(px(790.), px(590.)), Default::default());
    draw(cx);

    assert!(!harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).model_menu_open
    }));
    assert!(cx.debug_bounds("prompt-bar-model-picker").is_none());
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn closing_model_picker_restores_editor_for_immediate_typing(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, false, window, cx));
    draw(cx);
    open_model_picker(cx);

    cx.simulate_keystrokes("down enter x");

    assert_eq!(
        harness.read_with(cx, |harness, cx| harness.prompt.read(cx).draft(cx)),
        "x"
    );
}

#[gpui::test]
fn bottom_docked_model_picker_flips_above_its_trigger(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::with_models(events, true, window, cx));
    cx.simulate_resize(size(px(500.), px(320.)));
    draw(cx);
    open_model_picker(cx);

    let trigger = cx
        .debug_bounds("prompt-bar-model-trigger")
        .expect("bottom trigger should render");
    let picker = cx
        .debug_bounds("prompt-bar-model-picker")
        .expect("bottom picker should render");
    assert!(
        picker.bottom() <= trigger.top(),
        "bottom picker {picker:?} should flip above trigger {trigger:?}"
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn keyboard_navigation_inserts_the_active_stable_suggestion(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(captured_events, window, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
    });

    cx.simulate_keystrokes("down");
    assert_eq!(
        harness.read_with(cx, |harness, cx| {
            harness.prompt.read(cx).active_suggestion.clone()
        }),
        Some(SuggestionKey::Mention("suppliers".into()))
    );
    cx.simulate_keystrokes("enter");

    let draft = harness.read_with(cx, |harness, cx| harness.prompt.read(cx).draft(cx));
    assert_eq!(draft, "@Suppliers ");
    assert!(events.borrow().iter().any(|event| matches!(
        event,
        PromptBarEvent::MentionSelected { mention_id, .. } if mention_id == "suppliers"
    )));
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn shift_enter_preserves_multiline_editing_without_submitting(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(captured_events, window, cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| prompt.set_draft("first", window, cx));
        window.draw(cx).clear(cx);
    });
    events.borrow_mut().clear();
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
    });

    cx.simulate_keystrokes("shift-enter");

    let draft = harness.read_with(cx, |harness, cx| harness.prompt.read(cx).draft(cx));
    assert_eq!(draft, "first\n");
    assert!(
        !events
            .borrow()
            .iter()
            .any(|event| matches!(event, PromptBarEvent::Submit { .. }))
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn plain_enter_submits_once_and_leaves_no_newline(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(captured_events, window, cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| prompt.set_draft("send this", window, cx));
        window.draw(cx).clear(cx);
    });
    events.borrow_mut().clear();
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| prompt.focus(window, cx));
    });

    cx.simulate_keystrokes("enter");

    let draft = harness.read_with(cx, |harness, cx| harness.prompt.read(cx).draft(cx));
    assert_eq!(draft, "");
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| matches!(event, PromptBarEvent::Submit { .. }))
            .count(),
        1
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn escape_closes_the_model_menu_without_dropping_editor_focus(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(events, window, cx));
    cx.update(|window, cx| {
        let prompt = harness.read(cx).prompt.clone();
        prompt.update(cx, |prompt, cx| {
            prompt.set_draft("compare suppliers", window, cx);
            prompt.set_models([super::PromptModel::new("balanced", "Balanced")], cx);
            prompt.model_menu_open = true;
            prompt.focus(window, cx);
            cx.notify();
        });
        window.draw(cx).clear(cx);
    });

    cx.simulate_keystrokes("escape");

    assert!(!harness.read_with(cx, |harness, cx| {
        harness.prompt.read(cx).model_menu_open
    }));
    assert!(cx.update(|window, cx| {
        harness
            .read(cx)
            .prompt
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }));
}

#[gpui::test]
fn constrained_width_keeps_the_primary_action_reachable(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(events, window, cx));
    harness.update(cx, |harness, cx| {
        harness.prompt.update(cx, |prompt, cx| {
            prompt.set_progress(ProgressState::Running, cx)
        });
    });
    cx.simulate_resize(size(px(300.), px(350.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let primary = cx
        .debug_bounds("prompt-bar-cancel-control")
        .expect("the primary action should remain rendered");
    assert!(
        primary.size.width > px(0.),
        "primary action {primary:?} must retain a visible width"
    );
    assert!(
        primary.right() <= px(300.),
        "primary action {primary:?} must remain within the 300px viewport"
    );
}

#[gpui::test]
fn the_submit_slot_holds_one_width_across_send_and_cancel(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(events, window, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let send = cx
        .debug_bounds("prompt-bar-send-control")
        .expect("the send control should render")
        .size
        .width;

    harness.update(cx, |harness, cx| {
        harness.prompt.update(cx, |prompt, cx| {
            prompt.set_progress(ProgressState::Running, cx)
        });
    });
    cx.executor()
        .advance_clock(crate::motion::MotionTokens::DEFAULT.quick() * 2);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let cancel = cx
        .debug_bounds("prompt-bar-cancel-control")
        .expect("the cancel control should render")
        .size
        .width;
    assert_eq!(
        cancel, send,
        "Send and Cancel must swap inside one fixed slot, not resize the control"
    );
}

#[test]
fn active_suggestion_falls_back_safely_after_catalog_change() {
    let available = vec![SuggestionKey::Mention("remaining".into())];
    assert_eq!(
        retain_active_suggestion(Some(SuggestionKey::Mention("removed".into())), &available),
        available.first().cloned()
    );
    assert_eq!(retain_active_suggestion(None, &[]), None);
}

#[test]
fn empty_submission_is_rejected_and_attachment_identity_is_preserved() {
    assert!(build_submission("  \n ", None, &[]).is_none());
    let attachments = [
        PromptAttachment::new("sales", "sales.csv"),
        PromptAttachment::new("brief", "brief.md"),
    ];
    let submission = build_submission("  summarize these  ", Some("fast".into()), &attachments)
        .expect("non-empty draft should submit");

    assert_eq!(submission.text(), "summarize these");
    assert_eq!(submission.model_id(), Some(&"fast".into()));
    assert_eq!(submission.attachment_ids(), &["sales", "brief"]);
}

#[test]
fn production_frames_expose_named_group_and_listbox_semantics() {
    let root = prompt_frame(&"prompt".into(), "Prompt composer".into()).into_element();
    let mut root_node = accesskit::Node::new(Role::Unknown);
    root.write_a11y_info(&mut root_node);
    assert_eq!(root.a11y_role(), Some(Role::Group));
    assert_eq!(root_node.label(), Some("Prompt composer"));

    let listbox = prompt_listbox("suggestions".into(), "Prompt suggestions").into_element();
    let mut listbox_node = accesskit::Node::new(Role::Unknown);
    listbox.write_a11y_info(&mut listbox_node);
    assert_eq!(listbox.a11y_role(), Some(Role::ListBox));
    assert_eq!(listbox_node.label(), Some("Prompt suggestions"));

    let status =
        prompt_status("progress".into(), "Running; cancel is available".into()).into_element();
    let mut status_node = accesskit::Node::new(Role::Unknown);
    status.write_a11y_info(&mut status_node);
    assert_eq!(status.a11y_role(), Some(Role::Status));
    assert_eq!(status_node.label(), Some("Running; cancel is available"));
}

#[gpui::test]
fn production_controls_are_named_keyboard_activatable_buttons(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ControlProbe {
        captured,
        selected_option: false,
        disabled: false,
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (role, node) = result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("control semantics should be captured");
    assert_eq!(role, Some(Role::Button));
    assert_eq!(node.label(), Some("Send prompt"));
    assert!(node.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn disabled_production_control_exposes_no_click_action(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ControlProbe {
        captured,
        selected_option: false,
        disabled: true,
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (_, node) = result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("disabled control semantics should be captured");
    assert!(!node.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn suggestion_options_expose_selection_and_activation(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ControlProbe {
        captured,
        selected_option: true,
        disabled: false,
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (role, node) = result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("option semantics should be captured");
    assert_eq!(role, Some(Role::ListBoxOption));
    assert_eq!(node.label(), Some("Send prompt"));
    assert_eq!(node.is_selected(), Some(true));
    assert!(node.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn model_trigger_exposes_its_expanded_state(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ModelControlProbe { captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (role, node) = result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("model trigger semantics should be captured");
    assert_eq!(role, Some(Role::Button));
    assert_eq!(node.label(), Some("Model: Balanced"));
    assert_eq!(node.is_expanded(), Some(true));
    assert!(node.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn model_options_expose_selection_position_and_set_size(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ModelOptionProbe { captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (role, node) = result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("model option semantics should be captured");
    assert_eq!(role, Some(Role::ListBoxOption));
    assert_eq!(node.label(), Some("Balanced"));
    assert_eq!(node.is_selected(), Some(true));
    assert_eq!(node.position_in_set(), Some(2));
    assert_eq!(node.size_of_set(), Some(4));
    assert!(node.supports_action(accesskit::Action::Click));
}

#[test]
fn repeated_stable_ids_are_rejected() {
    let unique = [
        SharedString::from("balanced"),
        SharedString::from("precise"),
    ];
    assert!(stable_ids_are_unique(unique.iter()));

    let repeated = [
        SharedString::from("balanced"),
        SharedString::from("precise"),
        SharedString::from("balanced"),
    ];
    assert!(!stable_ids_are_unique(repeated.iter()));
    assert!(stable_ids_are_unique(std::iter::empty()));
}

#[gpui::test]
fn catalogs_repeating_a_stable_id_are_ignored_atomically(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (prompt, cx) = cx.add_window_view(|window, cx| PromptBar::new("duplicates", window, cx));

    let valid = || {
        [
            PromptModel::new("balanced", "Balanced"),
            PromptModel::new("precise", "Precise"),
        ]
    };
    cx.update(|_, cx| {
        prompt.update(cx, |prompt, cx| {
            prompt.set_models(valid(), cx);
            prompt.set_mentions([PromptMention::new("docs", "docs")], cx);
            prompt.set_commands([PromptCommand::new("summarize", "summarize")], cx);
        });
    });

    cx.update(|_, cx| {
        prompt.update(cx, |prompt, cx| {
            // Each of these repeats an ID, so each must be refused whole
            // rather than installing a catalog with aliased ElementIds.
            prompt.set_models(
                [
                    PromptModel::new("fast", "Fast"),
                    PromptModel::new("fast", "Fast again"),
                ],
                cx,
            );
            prompt.set_mentions(
                [
                    PromptMention::new("specs", "specs"),
                    PromptMention::new("specs", "specs again"),
                ],
                cx,
            );
            prompt.set_commands(
                [
                    PromptCommand::new("explain", "explain"),
                    PromptCommand::new("explain", "explain again"),
                ],
                cx,
            );
        });
    });

    prompt.read_with(cx, |prompt, _| {
        assert_eq!(
            prompt.models,
            valid().to_vec(),
            "a malformed model catalog must leave the previous one untouched"
        );
        assert_eq!(prompt.mentions.len(), 1, "mentions must be unchanged");
        assert_eq!(prompt.commands.len(), 1, "commands must be unchanged");
    });
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
#[gpui::test]
fn composer_grows_per_line_and_stops_at_its_auto_grow_cap(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured_events = events.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| PromptHarness::new(captured_events, window, cx));
    let measure = |cx: &mut gpui::VisualTestContext, draft: String| {
        cx.update(|window, cx| {
            let prompt = harness.read(cx).prompt.clone();
            prompt.update(cx, |prompt, cx| prompt.set_draft(draft, window, cx));
            window.draw(cx).clear(cx);
        });
        cx.debug_bounds("prompt-bar-editor")
            .expect("the composer should render")
            .size
            .height
    };
    let one = measure(cx, "first".into());
    let three = measure(cx, "first\nsecond\nthird".into());
    let five = measure(cx, ["l1", "l2", "l3", "l4", "l5"].join("\n"));
    let nine = measure(
        cx,
        (1..=9)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    // The composer is deliberately multi-line: each added line grows it
    // until the auto-grow cap, and past the cap it scrolls instead of
    // growing — the half of the upstream input contract the single-line
    // fields must not have.
    assert!(three > one, "{three:?} vs {one:?}");
    assert!(five > three, "{five:?} vs {three:?}");
    assert_eq!(nine, five);
}
