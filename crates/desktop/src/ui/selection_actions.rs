//! Selectable Markdown with the application's selection-action surface.

use std::path::PathBuf;

use gpui::{
    Anchor, App, AppContext as _, Context, ElementId, EventEmitter, InteractiveElement as _,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _,
    Pixels, Point, Render, Role, SharedString, StatefulInteractiveElement as _, Styled, Window,
    anchored, div, point, px,
};
use gpui_base::Button as ActionButton;
use gpui_component::text::{TextView, TextViewState, TextViewStyle};
use gpui_component::{ActiveTheme as _, h_flex};

fn settled_selection(text: &str) -> Option<&str> {
    let selected = text.trim();
    (!selected.is_empty()).then_some(selected)
}

fn is_plain_click(click_count: usize, end: Point<Pixels>, start: Option<Point<Pixels>>) -> bool {
    click_count == 1
        && start.is_some_and(|start| {
            (end.x - start.x).abs() <= px(3.) && (end.y - start.y).abs() <= px(3.)
        })
}

fn file_url_path(url: &str) -> Option<PathBuf> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

/// One application action offered for selected text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionAction {
    id: SharedString,
    label: SharedString,
}

impl SelectionAction {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// An interaction emitted by [`SelectionActions`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionActionsEvent {
    Invoked {
        id: SharedString,
        action_id: SharedString,
        selected_text: SharedString,
    },
}

/// A selectable Markdown surface.
pub struct SelectionActions {
    id: SharedString,
    markdown: SharedString,
    text: gpui::Entity<TextViewState>,
    text_style: Option<TextViewStyle>,
    actions: Vec<SelectionAction>,
    toolbar_label: SharedString,
    selected: SharedString,
    drag_active: bool,
    pointer_anchor: Option<Point<Pixels>>,
    last_text_pointer: Option<Point<Pixels>>,
    toolbar_pointer_active: bool,
    _observation: gpui::Subscription,
}

impl SelectionActions {
    pub fn new(
        id: impl Into<SharedString>,
        markdown: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let markdown = markdown.into();
        let text = cx.new(|cx| TextViewState::markdown(markdown.as_ref(), cx).selectable(true));
        let observation = cx.observe(&text, |this, text, cx| {
            if this.drag_active || this.toolbar_pointer_active {
                return;
            }
            let selected: SharedString = settled_selection(&text.read(cx).selected_text())
                .unwrap_or_default()
                .into();
            if this.selected != selected {
                this.selected = selected;
                if this.selected.is_empty() {
                    this.pointer_anchor = None;
                }
                cx.notify();
            }
        });
        Self {
            id: id.into(),
            markdown,
            text,
            text_style: None,
            actions: Vec::new(),
            toolbar_label: "Selection actions".into(),
            selected: SharedString::default(),
            drag_active: false,
            pointer_anchor: None,
            last_text_pointer: None,
            toolbar_pointer_active: false,
            _observation: observation,
        }
    }

    pub fn frameless(self) -> Self {
        self
    }

    pub fn natural_height(self) -> Self {
        self
    }

    pub fn text_style(mut self, style: TextViewStyle) -> Self {
        self.text_style = Some(style);
        self
    }

    pub fn toolbar_label(mut self, label: impl Into<SharedString>) -> Self {
        self.toolbar_label = label.into();
        self
    }

    pub fn set_text_style(&mut self, style: TextViewStyle, cx: &mut Context<Self>) {
        self.text_style = Some(style);
        cx.notify();
    }

    pub fn set_toolbar_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.toolbar_label != label {
            self.toolbar_label = label;
            cx.notify();
        }
    }

    pub fn set_markdown(&mut self, markdown: impl Into<SharedString>, cx: &mut Context<Self>) {
        let markdown = markdown.into();
        if self.markdown == markdown {
            return;
        }
        self.markdown = markdown.clone();
        self.text
            .update(cx, |text, cx| text.set_text(markdown.as_ref(), cx));
        self.selected = SharedString::default();
        self.pointer_anchor = None;
        self.last_text_pointer = None;
        self.drag_active = false;
        self.toolbar_pointer_active = false;
        cx.notify();
    }

    pub fn set_actions(
        &mut self,
        actions: impl IntoIterator<Item = SelectionAction>,
        cx: &mut Context<Self>,
    ) {
        let actions: Vec<SelectionAction> = actions.into_iter().collect();
        if self.actions == actions {
            return;
        }
        self.actions = actions;
        cx.notify();
    }

    pub fn selected_text(&self, cx: &App) -> SharedString {
        settled_selection(&self.text.read(cx).selected_text())
            .unwrap_or_default()
            .to_string()
            .into()
    }

    #[cfg(test)]
    pub(crate) fn action_labels(&self) -> Vec<SharedString> {
        self.actions
            .iter()
            .map(|action| action.label().clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn toolbar_label_text(&self) -> &SharedString {
        &self.toolbar_label
    }

    #[cfg(test)]
    pub(crate) fn text_is_dark(&self) -> bool {
        self.text_style.as_ref().is_some_and(|style| style.is_dark)
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.text.update(cx, |text, cx| text.clear_selection(cx));
        self.selected = SharedString::default();
        self.pointer_anchor = None;
        self.drag_active = false;
        self.toolbar_pointer_active = false;
        cx.notify();
    }

    fn settle_selection(&mut self, pointer: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.drag_active = false;
        let raw = self.text.read(cx).selected_text();
        let selected: SharedString = settled_selection(&raw).unwrap_or_default().into();
        let changed = self.selected != selected;
        self.selected = selected;
        if self.selected.is_empty() {
            self.pointer_anchor = None;
        } else if pointer.is_some() {
            self.pointer_anchor = pointer;
        }
        if changed || !self.selected.is_empty() {
            cx.notify();
        }
    }

    fn toolbar_anchor(&self, window: &Window) -> Point<Pixels> {
        self.pointer_anchor
            .or(self.last_text_pointer)
            .unwrap_or_else(|| {
                let viewport = window.viewport_size();
                point(viewport.width / 2., viewport.height / 2.)
            })
    }
}

impl EventEmitter<SelectionActionsEvent> for SelectionActions {}

impl Render for SelectionActions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut view = TextView::new(&self.text)
            .selectable(true)
            .on_link_click(|url, _, _, cx| {
                if let Some(path) = file_url_path(url) {
                    if path.exists() {
                        cx.open_with_system(&path);
                    }
                } else {
                    cx.open_url(url);
                }
            });
        if let Some(style) = self.text_style.clone() {
            view = view.style(style);
        }
        let mut root = div()
            .id(ElementId::Name(self.id.clone()))
            .debug_selector(|| "selection-actions-surface".into())
            .role(Role::Document)
            .relative()
            .w_full()
            .min_w_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.toolbar_pointer_active {
                        return;
                    }
                    this.drag_active = true;
                    this.last_text_pointer = Some(event.position);
                    this.selected = SharedString::default();
                    this.pointer_anchor = None;
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    if this.toolbar_pointer_active {
                        return;
                    }
                    if is_plain_click(event.click_count, event.position, this.last_text_pointer) {
                        let entity = cx.entity().clone();
                        window.defer(cx, move |_, cx| {
                            entity.update(cx, |this, cx| this.clear_selection(cx));
                        });
                    } else {
                        let entity = cx.entity().clone();
                        let position = event.position;
                        window.defer(cx, move |_, cx| {
                            entity.update(cx, |this, cx| {
                                this.settle_selection(Some(position), cx);
                            });
                        });
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    if this.toolbar_pointer_active {
                        return;
                    }
                    if is_plain_click(event.click_count, event.position, this.last_text_pointer) {
                        let entity = cx.entity().clone();
                        window.defer(cx, move |_, cx| {
                            entity.update(cx, |this, cx| this.clear_selection(cx));
                        });
                    } else {
                        let entity = cx.entity().clone();
                        let position = event.position;
                        window.defer(cx, move |_, cx| {
                            entity.update(cx, |this, cx| {
                                this.settle_selection(Some(position), cx);
                            });
                        });
                    }
                }),
            )
            .on_key_up(cx.listener(|_, _, window, cx| {
                let entity = cx.entity().clone();
                window.defer(cx, move |_, cx| {
                    entity.update(cx, |this, cx| this.settle_selection(None, cx));
                });
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !this.selected.is_empty() {
                    this.clear_selection(cx);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left
                    && !this.toolbar_pointer_active
                    && (!this.selected.is_empty() || this.drag_active)
                {
                    this.clear_selection(cx);
                }
            }));
        root = root.child(view);
        let toolbar_visible =
            !self.drag_active && !self.selected.is_empty() && !self.actions.is_empty();
        if toolbar_visible {
            let selected = self.selected.clone();
            let toolbar_id = ElementId::Name(format!("{}-toolbar", self.id).into());
            let mut toolbar = h_flex()
                .id(toolbar_id)
                .occlude()
                .role(Role::Toolbar)
                .aria_label(self.toolbar_label.clone())
                .gap_1()
                .rounded(px(7.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .p_1()
                .shadow_sm()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toolbar_pointer_active = true;
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.toolbar_pointer_active = false;
                    }),
                );
            for action in &self.actions {
                let action_id = action.id().clone();
                let label = action.label().clone();
                let id = self.id.clone();
                let selected = selected.clone();
                let selector = format!("selection-action-{id}-{action_id}");
                toolbar = toolbar.child(
                    ActionButton::new(ElementId::Name(selector.clone().into()))
                        .debug_selector(move || selector)
                        .accessibility_label(label.clone())
                        .cursor_pointer()
                        .rounded(px(5.))
                        .px_2()
                        .py(px(3.))
                        .text_size(px(10.))
                        .hover(|style| style.bg(cx.theme().muted))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            this.clear_selection(cx);
                            cx.emit(SelectionActionsEvent::Invoked {
                                id: id.clone(),
                                action_id: action_id.clone(),
                                selected_text: selected.clone(),
                            });
                        })),
                );
            }
            let anchor = self.toolbar_anchor(window);
            root = root.child(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .position(anchor)
                    .offset(point(px(4.), px(-6.)))
                    .child(toolbar),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{Render, Subscription, TestAppContext, div, point, px, size};

    use super::*;

    #[test]
    fn file_urls_resolve_to_local_paths() {
        let path = std::env::temp_dir().join("tact file link.png");
        let url = url::Url::from_file_path(&path).expect("absolute path");

        assert_eq!(file_url_path(url.as_str()), Some(path));
        assert_eq!(file_url_path("https://example.com"), None);
    }

    #[gpui::test]
    fn a_code_label_link_receives_the_click(cx: &mut TestAppContext) {
        struct LinkProbe {
            selection: gpui::Entity<SelectionActions>,
        }

        impl Render for LinkProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().w(px(420.)).child(self.selection.clone())
            }
        }

        cx.update(gpui_component::init);
        let (_probe, window) = cx.add_window_view(|window, cx| LinkProbe {
            selection: cx.new(|cx| {
                SelectionActions::new(
                    "link-probe",
                    "[`charts/markets_outlook.svg`](https://example.com)",
                    window,
                    cx,
                )
            }),
        });
        window.simulate_resize(size(px(420.), px(200.)));
        window.update(|window, cx| window.draw(cx).clear(cx));

        window.simulate_click(point(px(10.), px(10.)), gpui::Modifiers::default());

        assert_eq!(
            window.opened_url(),
            Some("https://example.com".to_string()),
            "a code label inside a Markdown link must still be clickable"
        );
    }

    struct SelectionProbe {
        selection: gpui::Entity<SelectionActions>,
        _subscription: Option<Subscription>,
        event: Rc<RefCell<Option<SelectionActionsEvent>>>,
    }

    impl Render for SelectionProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(gpui_base::TextSelectionLayer).child(
                div()
                    .debug_selector(|| "selection-probe".into())
                    .w(px(420.))
                    .child(self.selection.clone()),
            )
        }
    }

    fn selection_window(
        id: &'static str,
        actions: Vec<SelectionAction>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<SelectionProbe>,
    ) -> SelectionProbe {
        let event = Rc::new(RefCell::new(None));
        let event_for_window = event.clone();
        let selection = cx.new(|cx| {
            let mut selection =
                SelectionActions::new(id, "Alpha beta gamma delta epsilon", window, cx);
            selection.set_actions(actions, cx);
            selection
        });
        let subscription = cx.subscribe_in(
            &selection,
            window,
            move |_, _, event: &SelectionActionsEvent, _, _| {
                *event_for_window.borrow_mut() = Some(event.clone());
            },
        );
        SelectionProbe {
            selection,
            _subscription: Some(subscription),
            event,
        }
    }

    fn selection_probe<'a>(
        cx: &'a mut TestAppContext,
        id: &'static str,
        actions: Vec<SelectionAction>,
    ) -> (
        gpui::Entity<SelectionProbe>,
        &'a mut gpui::VisualTestContext,
    ) {
        cx.update(gpui_component::init);
        let (probe, window) =
            cx.add_window_view(move |window, cx| selection_window(id, actions, window, cx));
        window.simulate_resize(size(px(800.), px(600.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        (probe, window)
    }

    #[gpui::test]
    fn selection_toolbar_floats_without_changing_layout(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "test-selection",
            vec![SelectionAction::new("explain", "Explain")],
        );
        let before = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface before selecting");
        let position = point(before.origin.x + px(30.), before.origin.y + px(10.));
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(!selected.is_empty(), "double click must select a word");
        let after = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface after selecting");
        assert_eq!(
            before.size.height, after.size.height,
            "the floating toolbar must not add a row to the message layout"
        );
        assert!(
            window
                .debug_bounds("selection-action-test-selection-explain")
                .is_some(),
            "the floating toolbar must be mounted"
        );
    }

    #[gpui::test]
    fn drag_selection_survives_mouse_up(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "drag-selection",
            vec![SelectionAction::new("ask", "Ask")],
        );
        let surface = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface");
        let start = point(surface.origin.x + px(12.), surface.origin.y + px(10.));
        let end = point(start.x + px(150.), start.y);
        window.simulate_event(gpui::MouseDownEvent {
            position: start,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: true,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let middle = point(start.x + px(75.), start.y);
        window.simulate_event(gpui::MouseMoveEvent {
            position: middle,
            pressed_button: Some(MouseButton::Left),
            modifiers: gpui::Modifiers::default(),
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_event(gpui::MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: gpui::Modifiers::default(),
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_event(MouseUpEvent {
            position: end,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(
            !selected.is_empty(),
            "a drag selection must survive the mouse release"
        );
    }

    #[gpui::test]
    fn unchanged_markdown_keeps_selection(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "stable-selection",
            vec![SelectionAction::new("ask", "Ask")],
        );
        let surface = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface");
        let position = point(surface.origin.x + px(30.), surface.origin.y + px(10.));
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        let before = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(!before.is_empty());
        window.update(|_, cx| {
            let selection = probe.read(cx).selection.clone();
            selection.update(cx, |selection, cx| {
                selection.set_markdown("Alpha beta gamma delta epsilon", cx);
            });
        });
        window.run_until_parked();
        let after = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert_eq!(
            before, after,
            "an unchanged Markdown refresh must not drop the selection"
        );
    }

    #[gpui::test]
    fn action_click_emits_and_clears_selection(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "action-selection",
            vec![SelectionAction::new("explain", "Explain")],
        );
        let surface = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface");
        let position = point(surface.origin.x + px(30.), surface.origin.y + px(10.));
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let action = window
            .debug_bounds("selection-action-action-selection-explain")
            .expect("selection action");
        window.simulate_click(action.center(), gpui::Modifiers::default());
        window.run_until_parked();
        let (event, selected) = window.update(|_, cx| {
            (
                probe.read(cx).event.borrow().clone(),
                probe
                    .read(cx)
                    .selection
                    .read(cx)
                    .selected_text(cx)
                    .to_string(),
            )
        });
        assert_eq!(
            event,
            Some(SelectionActionsEvent::Invoked {
                id: "action-selection".into(),
                action_id: "explain".into(),
                selected_text: "Alpha".into(),
            })
        );
        assert!(selected.is_empty(), "the action click clears the selection");
    }

    #[gpui::test]
    fn keyboard_selection_and_escape_control_the_toolbar(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "keyboard-selection",
            vec![SelectionAction::new("ask", "Ask")],
        );
        let surface = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface");
        let position = point(surface.origin.x + px(30.), surface.origin.y + px(10.));
        window.simulate_click(position, gpui::Modifiers::default());
        window.run_until_parked();
        window.simulate_keystrokes("secondary-a");
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(
            !selected.is_empty(),
            "keyboard selection must reach the toolbar"
        );
        assert!(
            window
                .debug_bounds("selection-action-keyboard-selection-ask")
                .is_some(),
            "the toolbar follows a keyboard selection"
        );
        window.simulate_keystrokes("escape");
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        let selected = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(selected.is_empty(), "Escape clears the selection");
        assert!(
            window
                .debug_bounds("selection-action-keyboard-selection-ask")
                .is_none(),
            "Escape hides the toolbar"
        );
    }

    #[gpui::test]
    fn outside_click_dismisses_the_toolbar(cx: &mut TestAppContext) {
        let (probe, window) = selection_probe(
            cx,
            "outside-selection",
            vec![SelectionAction::new("ask", "Ask")],
        );
        let surface = window
            .debug_bounds("selection-actions-surface")
            .expect("selection surface");
        let position = point(surface.origin.x + px(30.), surface.origin.y + px(10.));
        window.simulate_event(gpui::MouseDownEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            position,
            modifiers: gpui::Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_click(point(px(600.), px(500.)), gpui::Modifiers::default());
        window.run_until_parked();
        let selected = window.update(|_, cx| {
            probe
                .read(cx)
                .selection
                .read(cx)
                .selected_text(cx)
                .to_string()
        });
        assert!(selected.is_empty(), "clicking outside clears the selection");
    }
}
