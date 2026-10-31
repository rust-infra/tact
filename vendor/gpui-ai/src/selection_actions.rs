//! Selection-anchored actions for readable Markdown content.

use crate::control::{PressReleaseExt as _, composed_button};
use crate::motion::swap_progress;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    AnyElement, App, AppContext as _, Axis, Bounds, Element, ElementId, Entity, EventEmitter,
    FocusHandle, GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels,
    Point, Render, Role, ScrollHandle, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled, Subscription, Window, div, point,
    prelude::FluentBuilder as _,
};
use gpui_base::InteractiveElementExt as _;
use gpui_base::StyledExt as _;
use gpui_base::{Align, Placement, Positioner};
use gpui_component::{
    ActiveTheme as _,
    scroll::ScrollableMask,
    text::{TextView, TextViewState},
};

/// One application-owned action offered for the current text selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionAction {
    id: SharedString,
    label: SharedString,
}

impl SelectionAction {
    /// Creates an action with stable application identity and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Returns the stable application identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible and accessible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// An interaction emitted by [`SelectionActions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionActionsEvent {
    /// The user invoked an action for a settled, non-empty selection.
    Invoked {
        /// Stable selection-surface identifier.
        id: SharedString,
        /// Stable application action identifier.
        action_id: SharedString,
        /// Trimmed selected text at activation time.
        selected_text: SharedString,
    },
}

fn invocation_event(
    id: impl Into<SharedString>,
    action: &SelectionAction,
    selected_text: impl Into<SharedString>,
) -> SelectionActionsEvent {
    SelectionActionsEvent::Invoked {
        id: id.into(),
        action_id: action.id.clone(),
        selected_text: selected_text.into(),
    }
}

fn settled_selection(text: &str) -> Option<&str> {
    let selected = text.trim();
    (!selected.is_empty()).then_some(selected)
}

fn selection_control(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> gpui_base::Button {
    let tokens = cx.theme().semantic_tokens();
    let label = label.into();
    let id = id.into();
    composed_button(id.clone(), label.clone())
        .flex()
        .items_center()
        .justify_center()
        .h(crate::sizing::control_md(cx))
        .px(tokens.spacing.sm)
        .border_1()
        .border_color(tokens.colors.border)
        .rounded(tokens.radius.sm)
        .bg(tokens.colors.surface)
        .text_token(tokens.typography.sm)
        .text_color(tokens.colors.surface_foreground)
        .hover(|style| style.bg(cx.theme().button_hover))
        .active(|style| style.bg(cx.theme().button_active))
        .focus_visible(|style| style.border_color(tokens.colors.ring))
        .press_release(ElementId::from((id, "press")), tokens.radius.sm, window, cx)
        .child(div().child(label))
}

fn selection_toolbar_frame(
    id: SharedString,
    maximum_size: Size<Pixels>,
    focus_handle: &FocusHandle,
    scroll_handle: &ScrollHandle,
    buttons: Vec<gpui_base::Button>,
    cx: &mut App,
) -> Stateful<gpui::Div> {
    let tokens = cx.theme().semantic_tokens();
    let scroll_id = id.clone();
    div()
        .id((ElementId::from(id), "toolbar"))
        .debug_selector(|| "selection-actions-toolbar".to_owned())
        .role(Role::Toolbar)
        .aria_label("Selection actions")
        .tab_group()
        .track_focus(focus_handle)
        .occlude()
        .max_w(maximum_size.width)
        .max_h(maximum_size.height)
        .p(tokens.spacing.xs)
        .border_1()
        .border_color(tokens.colors.border)
        .rounded(tokens.radius.md)
        .bg(tokens.colors.surface)
        .shadow(tokens.shadow.md.clone())
        .child(
            div()
                .id((ElementId::from(scroll_id.clone()), "toolbar-scroll"))
                .max_w_full()
                .min_w_0()
                .overflow_x_scroll()
                .lock_scroll_axis()
                .track_scroll(scroll_handle)
                .flex()
                .items_center()
                .gap(tokens.spacing.xs)
                .children(buttons),
        )
        .child(
            ScrollableMask::new(Axis::Horizontal, scroll_handle)
                .id((ElementId::from(scroll_id), "toolbar-scroll-mask")),
        )
}

fn selection_toolbar_positioner(
    anchor: Point<Pixels>,
    placement: Placement,
    toolbar: impl IntoElement,
    cx: &App,
) -> Positioner {
    let spacing = cx.theme().semantic_tokens().spacing;
    Positioner::side(Bounds::new(anchor, Size::default()))
        .placement(placement)
        .align(Align::Center)
        .offset(spacing.sm)
        .margin(spacing.sm)
        .child(toolbar)
}

/// Sample the resolved bounds, not an estimate of the positioner's flip.
fn entrance_travel(anchor_y: Pixels, bounds: Bounds<Pixels>, travel: Pixels) -> Pixels {
    if bounds.top() >= anchor_y {
        -travel
    } else if bounds.bottom() <= anchor_y {
        travel
    } else {
        Pixels::ZERO
    }
}

/// Positioner resolves the final layout before this wrapper prepaints. Move
/// hitboxes and paint together, without copying its private flip algorithm.
struct ToolbarEntrance {
    child: AnyElement,
    anchor_y: Pixels,
    travel: Pixels,
}

impl IntoElement for ToolbarEntrance {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ToolbarEntrance {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let offset = point(
            Pixels::ZERO,
            entrance_travel(self.anchor_y, bounds, self.travel),
        );
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

fn defer_selection_settle(
    entity: Entity<SelectionActions>,
    pointer: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    window.defer(cx, move |_, cx| {
        entity.update(cx, |this, cx| this.settle_selection(Some(pointer), cx));
    });
}

/// Selectable Markdown with application-owned actions anchored to a settled selection.
///
/// The entity owns one upstream [`TextViewState`], preserving native GPUI
/// selection, copy, and select-all behavior. It emits immutable action
/// requests; applications retain all durable results and asynchronous work.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let selection = cx.new(|cx| {
///     SelectionActions::new("answer", "Select part of this answer", window, cx)
/// });
/// selection.update(cx, |selection, cx| {
///     selection.set_actions([
///         SelectionAction::new("ask", "Ask"),
///         SelectionAction::new("explain", "Explain"),
///     ], cx);
/// });
/// # }
/// ```
pub struct SelectionActions {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    markdown: SharedString,
    text: Entity<TextViewState>,
    actions: Vec<SelectionAction>,
    selected_text: SharedString,
    drag_active: bool,
    pointer_anchor: Option<Point<Pixels>>,
    last_text_pointer: Option<Point<Pixels>>,
    text_focus_scope: FocusHandle,
    toolbar_focus: FocusHandle,
    toolbar_action_focus: FocusHandle,
    selection_focus: Option<FocusHandle>,
    toolbar_scroll: ScrollHandle,
    content_scroll: ScrollHandle,
    toolbar_pointer_active: bool,
    /// Bumped for each newly settled, distinct selection, so the toolbar's
    /// entrance replays for a replacement and never for a re-render.
    toolbar_generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl SelectionActions {
    /// Creates a selectable Markdown surface.
    pub fn new(
        id: impl Into<SharedString>,
        markdown: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let markdown = markdown.into();
        let text = cx.new(|cx| TextViewState::markdown(markdown.as_ref(), cx).selectable(true));
        let observation = cx.observe(&text, |this, text, cx| {
            if this.drag_active {
                return;
            }
            let raw = text.read(cx).selected_text();
            if settled_selection(&raw).is_some() {
                this.apply_selection(&raw, None, cx);
            } else {
                let selection = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = selection.update(cx, |this, cx| {
                        if this.drag_active || this.toolbar_pointer_active {
                            return;
                        }
                        let raw = this.text.read(cx).selected_text();
                        if settled_selection(&raw).is_none() {
                            this.apply_selection(&raw, None, cx);
                        }
                    });
                });
            }
        });
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            markdown,
            text,
            actions: Vec::new(),
            selected_text: SharedString::default(),
            drag_active: false,
            pointer_anchor: None,
            last_text_pointer: None,
            text_focus_scope: cx.focus_handle(),
            toolbar_focus: cx.focus_handle(),
            toolbar_action_focus: cx.focus_handle(),
            selection_focus: None,
            toolbar_scroll: ScrollHandle::new(),
            content_scroll: ScrollHandle::new(),
            toolbar_pointer_active: false,
            toolbar_generation: 0,
            _subscriptions: vec![observation],
        }
    }

    /// Replaces the Markdown and clears any stale selection and toolbar.
    pub fn set_markdown(
        &mut self,
        markdown: impl Into<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) {
        let markdown = markdown.into();
        if self.markdown == markdown {
            return;
        }
        self.restore_text_focus(cx);
        self.markdown = markdown.clone();
        self.text
            .update(cx, |text, cx| text.set_text(markdown.as_ref(), cx));
        self.selected_text = SharedString::default();
        self.pointer_anchor = None;
        self.last_text_pointer = None;
        self.drag_active = false;
        self.toolbar_pointer_active = false;
        cx.notify();
    }

    /// Replaces the application-owned action catalog.
    pub fn set_actions(
        &mut self,
        actions: impl IntoIterator<Item = SelectionAction>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.actions = actions.into_iter().collect();
        cx.notify();
    }

    /// Returns the current settled, trimmed selection.
    pub fn selected_text(&self) -> &SharedString {
        &self.selected_text
    }

    /// Clears native and composite selection state.
    pub fn clear_selection(&mut self, cx: &mut gpui::Context<Self>) {
        self.restore_text_focus(cx);
        self.text.update(cx, TextViewState::clear_selection);
        self.selected_text = SharedString::default();
        self.pointer_anchor = None;
        self.drag_active = false;
        self.toolbar_pointer_active = false;
        cx.notify();
    }

    fn settle_selection(&mut self, pointer: Option<Point<Pixels>>, cx: &mut gpui::Context<Self>) {
        self.drag_active = false;
        let raw = self.text.read(cx).selected_text();
        self.apply_selection(&raw, pointer, cx);
    }

    fn capture_text_focus(&self, cx: &mut gpui::Context<Self>) -> Option<FocusHandle> {
        let entity_id = cx.entity_id();
        let text_focus_scope = self.text_focus_scope.clone();
        cx.with_window(entity_id, move |window, cx| {
            text_focus_scope
                .contains_focused(window, cx)
                .then(|| window.focused(cx))
                .flatten()
        })
        .flatten()
    }

    fn restore_text_focus(&self, cx: &mut gpui::Context<Self>) {
        let Some(selection_focus) = self.selection_focus.clone() else {
            return;
        };
        let entity_id = cx.entity_id();
        let toolbar_focus = self.toolbar_focus.clone();
        let _ = cx.with_window(entity_id, move |window, cx| {
            if toolbar_focus.contains_focused(window, cx) {
                selection_focus.focus(window, cx);
            }
        });
    }

    fn apply_selection(
        &mut self,
        raw: &str,
        pointer: Option<Point<Pixels>>,
        cx: &mut gpui::Context<Self>,
    ) {
        let selected = settled_selection(raw).unwrap_or_default();
        let next: SharedString = selected.to_owned().into();
        let next_focus = (!next.is_empty())
            .then(|| self.capture_text_focus(cx))
            .flatten();
        if next.is_empty() {
            self.restore_text_focus(cx);
        }
        let changed = self.selected_text != next;
        if changed && !next.is_empty() {
            self.toolbar_generation = self.toolbar_generation.wrapping_add(1);
        }
        self.selected_text = next;
        if let Some(next_focus) = next_focus {
            self.selection_focus = Some(next_focus);
        }
        if self.selected_text.is_empty() {
            self.pointer_anchor = None;
        } else if let Some(pointer) = pointer {
            self.pointer_anchor = Some(pointer);
        }
        if changed || !self.selected_text.is_empty() {
            cx.notify();
        }
    }

    fn toolbar(&self, window: &mut Window, cx: &mut gpui::Context<Self>) -> Positioner {
        let root_id = self.id.clone();
        let selected_text = self.selected_text.clone();
        let actions = self.actions.clone();
        let tokens = cx.theme().semantic_tokens();
        let maximum_size = Size::new(tokens.spacing.xxl * 9., tokens.spacing.xxl * 2.);
        let maximum_control_width = (maximum_size.width - tokens.spacing.md).max(Pixels::default());
        let buttons = actions
            .into_iter()
            .enumerate()
            .map(|(action_ix, action)| {
                let event = invocation_event(root_id.clone(), &action, selected_text.clone());
                let action_id = action.id.clone();
                let label = action.label.clone();
                selection_control(
                    (ElementId::from(root_id.clone()), action_id.clone()),
                    label.clone(),
                    window,
                    cx,
                )
                .when(action_ix == 0, |button| {
                    button.track_focus(&self.toolbar_action_focus)
                })
                .max_w(maximum_control_width)
                .overflow_hidden()
                .debug_selector(move || format!("selection-action-{action_id}"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if event.button == MouseButton::Left {
                            this.toolbar_pointer_active = true;
                            cx.stop_propagation();
                        }
                    }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.emit(event.clone());
                    this.toolbar_pointer_active = false;
                }))
            })
            .collect::<Vec<_>>();

        let toolbar = selection_toolbar_frame(
            self.id.clone(),
            maximum_size,
            &self.toolbar_focus,
            &self.toolbar_scroll,
            buttons,
            cx,
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _, _| {
                this.toolbar_pointer_active = false;
            }),
        );

        // The toolbar enters from the selection's side: a quick fade with a
        // few pixels of travel out of the anchor, keyed by the selection
        // generation so a replaced selection replays the entrance from its
        // new anchor while a re-render replays nothing. Reduced motion
        // renders the settled toolbar in one frame.
        let entrance = swap_progress(
            ElementId::Name(SharedString::from(format!(
                "{}-toolbar-entrance-{}",
                self.id, self.toolbar_generation
            ))),
            window,
            cx,
        );
        let anchor = self.toolbar_anchor(window);
        let toolbar = ToolbarEntrance {
            child: div().opacity(entrance).child(toolbar).into_any_element(),
            anchor_y: anchor.y,
            travel: tokens.spacing.xxs * (1.0 - entrance) * crate::motion::travel(cx),
        };

        selection_toolbar_positioner(anchor, Placement::Bottom, toolbar, cx)
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

impl gpui::Styled for SelectionActions {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for SelectionActions {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = self.id.clone();
        let text_surface = div()
            .id((ElementId::from(root_id.clone()), "text"))
            .size_full()
            .track_focus(&self.text_focus_scope)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.toolbar_pointer_active {
                        return;
                    }
                    this.drag_active = true;
                    this.toolbar_pointer_active = false;
                    this.selected_text = SharedString::default();
                    this.pointer_anchor = None;
                    this.last_text_pointer = Some(event.position);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, event: &MouseUpEvent, window, cx| {
                    defer_selection_settle(cx.entity().clone(), event.position, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|_, event: &MouseUpEvent, window, cx| {
                    defer_selection_settle(cx.entity().clone(), event.position, window, cx);
                }),
            )
            .on_key_up(cx.listener(|_, _, window, cx| {
                let entity = cx.entity().clone();
                window.defer(cx, move |_, cx| {
                    entity.update(cx, |this, cx| this.settle_selection(None, cx));
                });
            }))
            .child(TextView::new(&self.text).selectable(true));

        div()
            .id(ElementId::from(root_id))
            .debug_selector(|| "selection-actions-surface".to_owned())
            .role(Role::Group)
            .aria_label("Selectable content with actions")
            .relative()
            .size_full()
            .min_h(tokens.spacing.xxl * 3.)
            .border_1()
            .border_color(tokens.colors.border)
            .rounded(tokens.radius.lg)
            .bg(tokens.colors.background)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.clear_selection(cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let toolbar_visible =
                    !this.drag_active && !this.selected_text.is_empty() && !this.actions.is_empty();
                match event.keystroke.key.as_str() {
                    "escape" if toolbar_visible => {
                        this.clear_selection(cx);
                        cx.stop_propagation();
                    }
                    "tab"
                        if toolbar_visible
                            && !event.keystroke.modifiers.shift
                            && this.text_focus_scope.contains_focused(window, cx) =>
                    {
                        this.toolbar_action_focus.focus(window, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .id((ElementId::from(self.id.clone()), "content-scroll"))
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.content_scroll)
                    .p(tokens.spacing.md)
                    .child(text_surface),
            )
            .child(
                ScrollableMask::new(Axis::Vertical, &self.content_scroll)
                    .id((ElementId::from(self.id.clone()), "content-scroll-mask")),
            )
            .when(
                !self.drag_active && !self.selected_text.is_empty() && !self.actions.is_empty(),
                |surface| surface.child(self.toolbar(window, cx)),
            )
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds, Modifiers, RenderOnce as _, ScrollDelta, ScrollWheelEvent, Size, TestAppContext,
        VisualTestContext, accesskit, canvas, point, px, rems, size,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn duplicate_labels_keep_stable_action_identity() {
        let actions = [
            SelectionAction::new("ask-short", "Ask"),
            SelectionAction::new("ask-deep", "Ask"),
        ];

        let event = invocation_event("answer", &actions[1], "selected words");

        assert_eq!(
            event,
            SelectionActionsEvent::Invoked {
                id: "answer".into(),
                action_id: "ask-deep".into(),
                selected_text: "selected words".into(),
            }
        );
    }

    #[test]
    fn whitespace_only_selection_is_suppressed() {
        assert_eq!(settled_selection("  \n\t  "), None);
        assert_eq!(settled_selection("  useful words  "), Some("useful words"));
    }

    #[derive(Clone, Copy)]
    enum ProbePopupSize {
        Pixels(f32, f32),
        Viewport(f32, f32),
        Rems(f32, f32),
    }

    #[derive(Clone, Copy)]
    struct PositionCase {
        selector: &'static str,
        anchor: (f32, f32),
        placement: gpui_base::Placement,
        popup_size: ProbePopupSize,
    }

    struct PositionerProbe {
        cases: Vec<PositionCase>,
    }

    impl Render for PositionerProbe {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let viewport = window.viewport_size();
            div()
                .relative()
                .size_full()
                .children(self.cases.iter().map(|case| {
                    let selector = case.selector;
                    let anchor = point(
                        viewport.width * case.anchor.0,
                        viewport.height * case.anchor.1,
                    );
                    let popup = div().debug_selector(move || selector.to_owned());
                    let popup = match case.popup_size {
                        ProbePopupSize::Pixels(width, height) => popup.w(px(width)).h(px(height)),
                        ProbePopupSize::Viewport(width, height) => {
                            popup.w(viewport.width * width).h(viewport.height * height)
                        }
                        ProbePopupSize::Rems(width, height) => popup.w(rems(width)).h(rems(height)),
                    };
                    selection_toolbar_positioner(anchor, case.placement, popup, cx)
                }))
        }
    }

    fn assert_on_screen(bounds: Bounds<Pixels>, viewport: Size<Pixels>) {
        assert!(
            bounds.left() >= Pixels::default(),
            "{bounds:?} vs {viewport:?}"
        );
        assert!(
            bounds.top() >= Pixels::default(),
            "{bounds:?} vs {viewport:?}"
        );
        assert!(
            bounds.right() <= viewport.width,
            "{bounds:?} vs {viewport:?}"
        );
        assert!(
            bounds.bottom() <= viewport.height,
            "{bounds:?} vs {viewport:?}"
        );
    }

    #[gpui::test]
    fn selection_positioner_resolves_all_window_corners_with_expected_vertical_flip(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let cases = vec![
            PositionCase {
                selector: "selection-position-top-left",
                anchor: (0.02, 0.02),
                placement: gpui_base::Placement::Bottom,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-top-right",
                anchor: (0.98, 0.02),
                placement: gpui_base::Placement::Bottom,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-bottom-left",
                anchor: (0.02, 0.98),
                placement: gpui_base::Placement::Bottom,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-bottom-right",
                anchor: (0.98, 0.98),
                placement: gpui_base::Placement::Bottom,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
        ];
        let (_, cx) = cx.add_window_view(move |_, _| PositionerProbe { cases });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let viewport = cx.update(|window, _| window.viewport_size());

        for selector in [
            "selection-position-top-left",
            "selection-position-top-right",
            "selection-position-bottom-left",
            "selection-position-bottom-right",
        ] {
            assert_on_screen(
                cx.debug_bounds(selector)
                    .expect("positioned selection toolbar should render"),
                viewport,
            );
        }

        let top_anchor = viewport.height * 0.02;
        let bottom_anchor = viewport.height * 0.98;
        assert!(
            cx.debug_bounds("selection-position-top-left")
                .expect("top-left toolbar should render")
                .top()
                >= top_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-top-right")
                .expect("top-right toolbar should render")
                .top()
                >= top_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-bottom-left")
                .expect("bottom-left toolbar should render")
                .bottom()
                <= bottom_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-bottom-right")
                .expect("bottom-right toolbar should render")
                .bottom()
                <= bottom_anchor
        );
    }

    #[gpui::test]
    fn selection_positioner_flips_each_preferred_side_when_space_is_insufficient(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let cases = vec![
            PositionCase {
                selector: "selection-position-flip-top",
                anchor: (0.5, 0.02),
                placement: gpui_base::Placement::Top,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-flip-bottom",
                anchor: (0.5, 0.98),
                placement: gpui_base::Placement::Bottom,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-flip-left",
                anchor: (0.02, 0.5),
                placement: gpui_base::Placement::Left,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
            PositionCase {
                selector: "selection-position-flip-right",
                anchor: (0.98, 0.5),
                placement: gpui_base::Placement::Right,
                popup_size: ProbePopupSize::Pixels(120., 48.),
            },
        ];
        let (_, cx) = cx.add_window_view(move |_, _| PositionerProbe { cases });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let viewport = cx.update(|window, _| window.viewport_size());

        let top_anchor = viewport.height * 0.02;
        let bottom_anchor = viewport.height * 0.98;
        let left_anchor = viewport.width * 0.02;
        let right_anchor = viewport.width * 0.98;
        assert!(
            cx.debug_bounds("selection-position-flip-top")
                .expect("top toolbar should render")
                .top()
                >= top_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-flip-bottom")
                .expect("bottom toolbar should render")
                .bottom()
                <= bottom_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-flip-left")
                .expect("left toolbar should render")
                .left()
                >= left_anchor
        );
        assert!(
            cx.debug_bounds("selection-position-flip-right")
                .expect("right toolbar should render")
                .right()
                <= right_anchor
        );
    }

    #[gpui::test]
    fn selection_positioner_uses_the_larger_side_and_clamps_an_oversized_side(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let cases = vec![PositionCase {
            selector: "selection-position-larger-side",
            anchor: (0.5, 0.4),
            placement: gpui_base::Placement::Bottom,
            popup_size: ProbePopupSize::Viewport(0.3, 0.7),
        }];
        let (_, cx) = cx.add_window_view(move |_, _| PositionerProbe { cases });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let viewport = cx.update(|window, _| window.viewport_size());
        let bounds = cx
            .debug_bounds("selection-position-larger-side")
            .expect("oversized-side toolbar should render");

        assert_on_screen(bounds, viewport);
        assert!(bounds.top() > Pixels::default(), "{bounds:?}");
        assert!(bounds.bottom() > viewport.height * 0.95, "{bounds:?}");
    }

    #[gpui::test]
    fn selection_positioner_keeps_a_rem_scaled_toolbar_reachable_at_two_hundred_percent(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let cases = vec![PositionCase {
            selector: "selection-position-double-rem",
            anchor: (0.98, 0.98),
            placement: gpui_base::Placement::Bottom,
            popup_size: ProbePopupSize::Rems(18., 4.),
        }];
        let (_, cx) = cx.add_window_view(move |window, _| {
            window.set_rem_size(px(32.));
            PositionerProbe { cases }
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let viewport = cx.update(|window, _| window.viewport_size());
        let bounds = cx
            .debug_bounds("selection-position-double-rem")
            .expect("double-rem toolbar should render");

        assert_on_screen(bounds, viewport);
        assert_eq!(bounds.size, size(px(576.), px(128.)));
    }

    #[gpui::test]
    fn a_replaced_selection_replays_the_entrance_and_a_re_render_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        let cx: &mut gpui::VisualTestContext = cx;
        let selection = probe.read_with(cx, |probe, _| probe.selection.clone());

        let generation =
            |cx: &mut gpui::VisualTestContext| selection.read_with(cx, |s, _| s.toolbar_generation);
        assert_eq!(generation(cx), 0, "no selection has settled yet");

        selection.update(cx, |this, cx| {
            this.apply_selection("chosen words", Some(point(px(40.), px(40.))), cx)
        });
        assert_eq!(generation(cx), 1, "a settled selection enters once");

        selection.update(cx, |this, cx| {
            this.apply_selection("chosen words", Some(point(px(48.), px(44.))), cx)
        });
        assert_eq!(generation(cx), 1, "the same selection must not re-enter");

        selection.update(cx, |this, cx| {
            this.apply_selection("other words", Some(point(px(60.), px(200.))), cx)
        });
        assert_eq!(generation(cx), 2, "a replacement replays the entrance");

        selection.update(cx, |this, cx| this.apply_selection("", None, cx));
        assert_eq!(generation(cx), 2, "clearing enters nothing");
    }

    #[test]
    fn the_entrance_travels_out_of_the_anchors_side() {
        // Room below: the toolbar sits under the anchor and drops into
        // place, so it starts a little above its settled spot.
        assert_eq!(
            entrance_travel(
                px(100.),
                Bounds::new(point(px(0.), px(108.)), size(px(100.), px(40.))),
                px(4.)
            ),
            px(-4.)
        );
        // Bottom edge: the positioner flips above, and the entrance rises.
        assert_eq!(
            entrance_travel(
                px(560.),
                Bounds::new(point(px(0.), px(512.)), size(px(100.), px(40.))),
                px(4.)
            ),
            px(4.)
        );
        // A short toolbar still fits below where a maximum-height estimate
        // would have predicted a flip to the top.
        assert_eq!(
            entrance_travel(
                px(510.),
                Bounds::new(point(px(0.), px(518.)), size(px(100.), px(40.))),
                px(4.)
            ),
            px(-4.)
        );
    }

    struct SelectionProbe {
        selection: Entity<SelectionActions>,
        host_scroll: ScrollHandle,
    }

    impl SelectionProbe {
        fn new(window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
            let markdown = (0..24)
                .map(|ix| format!("Selectable line {ix} for scrolling and dismissal tests."))
                .collect::<Vec<_>>()
                .join("\n\n");
            let selection =
                cx.new(|cx| SelectionActions::new("selection-probe", markdown, window, cx));
            selection.update(cx, |selection, cx| {
                selection.set_actions(
                    [
                        SelectionAction::new("ask", "Ask about this selection"),
                        SelectionAction::new("explain", "Explain this selected passage"),
                        SelectionAction::new("rewrite", "Rewrite this selected passage clearly"),
                        SelectionAction::new("compare", "Compare this passage with the source"),
                        SelectionAction::new("final", "Open the final long selection action"),
                    ],
                    cx,
                );
            });
            Self {
                selection,
                host_scroll: ScrollHandle::new(),
            }
        }
    }

    impl Render for SelectionProbe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("selection-probe-scroll")
                        .w(px(184.))
                        .h(px(144.))
                        .overflow_y_scroll()
                        .track_scroll(&self.host_scroll)
                        .child(
                            div()
                                .w(px(184.))
                                .h(px(620.))
                                .child(div().w(px(184.)).h(px(144.)).child(self.selection.clone())),
                        ),
                )
                .child(
                    div()
                        .debug_selector(|| "selection-probe-outside".to_owned())
                        .flex_1(),
                )
        }
    }

    fn select_all_probe_text(probe: &Entity<SelectionProbe>, cx: &mut VisualTestContext) {
        let text = probe.read_with(cx, |probe, cx| probe.selection.read(cx).text.clone());
        text.update(cx, |text, cx| text.select_all(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn selection_toolbar_stays_attached_when_content_scrolls(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        select_all_probe_text(&probe, cx);
        let surface = cx
            .debug_bounds("selection-actions-surface")
            .expect("selection surface should render");
        let selection = probe.read_with(cx, |probe, _| probe.selection.clone());
        selection.update(cx, |selection, cx| {
            selection.pointer_anchor = Some(surface.center());
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let before = cx
            .debug_bounds("selection-actions-toolbar")
            .expect("toolbar should render before scrolling");

        probe.update(cx, |probe, cx| {
            probe
                .host_scroll
                .set_offset(point(Pixels::default(), px(-48.)));
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let after = cx
            .debug_bounds("selection-actions-toolbar")
            .expect("toolbar should remain after scrolling");

        assert_eq!(
            probe.read_with(cx, |probe, _| probe.host_scroll.offset().y),
            px(-48.)
        );
        assert_eq!(before, after);
    }

    #[gpui::test]
    fn selection_horizontal_overflow_keeps_the_final_action_reachable_in_the_window(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        select_all_probe_text(&probe, cx);
        let surface = cx
            .debug_bounds("selection-actions-surface")
            .expect("selection surface should render");
        let selection = probe.read_with(cx, |probe, _| probe.selection.clone());
        selection.update(cx, |selection, cx| {
            selection.pointer_anchor = Some(surface.center());
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let toolbar = cx
            .debug_bounds("selection-actions-toolbar")
            .expect("toolbar should render after selection");

        for _ in 0..12 {
            cx.simulate_event(ScrollWheelEvent {
                position: toolbar.center(),
                delta: ScrollDelta::Pixels(point(px(-120.), Pixels::default())),
                ..Default::default()
            });
        }
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let final_action = cx
            .debug_bounds("selection-action-final")
            .expect("final action should remain rendered after horizontal scrolling");
        let viewport = cx.update(|window, _| window.viewport_size());
        assert!(
            final_action.left() >= toolbar.left() && final_action.right() <= toolbar.right(),
            "{final_action:?} vs {toolbar:?}"
        );
        assert_on_screen(final_action, viewport);
    }

    #[gpui::test]
    fn selection_tab_preserves_selection_and_escape_restores_text_focus(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let surface = cx
            .debug_bounds("selection-actions-surface")
            .expect("selection surface should render");
        cx.simulate_click(
            point(surface.left() + px(24.), surface.top() + px(24.)),
            Modifiers::default(),
        );
        select_all_probe_text(&probe, cx);
        let selection = probe.read_with(cx, |probe, _| probe.selection.clone());

        let (text_focused, toolbar_focused) = cx.update(|window, cx| {
            let selection = selection.read(cx);
            (
                selection.text_focus_scope.contains_focused(window, cx),
                selection.toolbar_focus.contains_focused(window, cx),
            )
        });
        assert!(text_focused);
        assert!(!toolbar_focused);

        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("selection-actions-toolbar").is_some());
        assert!(selection.read_with(cx, |selection, _| { !selection.selected_text().is_empty() }));
        assert!(cx.update(|window, cx| selection.read(cx).toolbar_action_focus.is_focused(window)));
        assert!(cx.update(|window, cx| {
            selection
                .read(cx)
                .toolbar_focus
                .contains_focused(window, cx)
        }));

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
        assert!(cx.update(|window, cx| {
            selection
                .read(cx)
                .text_focus_scope
                .contains_focused(window, cx)
        }));
    }

    #[gpui::test]
    fn selection_outside_pointer_down_dismisses_toolbar(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        select_all_probe_text(&probe, cx);
        assert!(cx.debug_bounds("selection-actions-toolbar").is_some());

        let outside = cx
            .debug_bounds("selection-probe-outside")
            .expect("outside target should render");
        cx.simulate_mouse_down(outside.center(), MouseButton::Left, Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
    }

    #[gpui::test]
    fn selection_clear_removes_toolbar_immediately(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(SelectionProbe::new);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        select_all_probe_text(&probe, cx);
        let selection = probe.read_with(cx, |probe, _| probe.selection.clone());
        assert!(cx.debug_bounds("selection-actions-toolbar").is_some());

        selection.update(cx, SelectionActions::clear_selection);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(selection.read_with(cx, |selection, _| { selection.selected_text().is_empty() }));
        assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
    }

    struct ControlProbe {
        captured: Arc<Mutex<Option<accesskit::Node>>>,
        toolbar: Arc<Mutex<Option<accesskit::Node>>>,
    }

    impl Render for ControlProbe {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let captured = self.captured.clone();
            let toolbar = self.toolbar.clone();
            canvas(
                move |_, window, cx| {
                    let mut node = accesskit::Node::new(Role::Button);
                    selection_control("ask", "Ask about selection", window, cx)
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element()
                        .write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") = Some(node);
                    let toolbar_focus = cx.focus_handle();
                    let toolbar_element = selection_toolbar_frame(
                        "probe".into(),
                        size(px(240.), px(64.)),
                        &toolbar_focus,
                        &ScrollHandle::new(),
                        Vec::new(),
                        cx,
                    )
                    .into_element();
                    let mut toolbar_node =
                        accesskit::Node::new(toolbar_element.a11y_role().unwrap_or(Role::Unknown));
                    toolbar_element.write_a11y_info(&mut toolbar_node);
                    *toolbar.lock().expect("toolbar mutex should be available") =
                        Some(toolbar_node);
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn production_action_control_is_named_and_keyboard_activatable(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let toolbar = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let toolbar_result = toolbar.clone();
        let (_, cx) = cx.add_window_view(move |_, _| ControlProbe { captured, toolbar });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let node = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("button node should be captured");
        assert_eq!(node.role(), Role::Button);
        assert_eq!(node.label(), Some("Ask about selection"));
        assert!(node.supports_action(accesskit::Action::Click));

        let toolbar = toolbar_result
            .lock()
            .expect("toolbar mutex should be available")
            .take()
            .expect("toolbar node should be captured");
        assert_eq!(toolbar.role(), Role::Toolbar);
        assert_eq!(toolbar.label(), Some("Selection actions"));
    }
}
