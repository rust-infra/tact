//! The controls a person answers with: one choice from a set, and one
//! switch between two states.
//!
//! Written in the library's own grammar rather than wrapped from upstream.
//! Upstream's checkbox and radio are `Styled` only on their outer frame —
//! the box, its radius, its fill, and its size are fixed inside `render` —
//! so a wrapped one arrives with a look that no amount of styling can bring
//! into line with the rows, chips, and cards around it. Composing them here
//! costs a `Role` and an `aria_*` per control, and buys the same selected
//! surface, the same press ramp, the same focus ring, and the same size
//! policy everything else in this crate already answers to.
//!
//! Both are controlled: the application owns which option is chosen and
//! whether a switch is on, and these report a typed event asking for the
//! change. Nothing here keeps durable state.

use crate::control::{PressReleaseExt as _, QuietSurfaceExt as _, composed_button};
use crate::handlers::SharedHandler;
use crate::motion::disclosure_progress;
use crate::surface::{hint, selection_surface};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, accesskit, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, v_flex};

/// One option in a single-choice set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    disabled: bool,
}

impl ChoiceOption {
    /// Creates an option with a stable identifier and its visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            disabled: false,
        }
    }

    /// A second line under the label, for what the option means.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Marks the option unavailable. It stays readable and stays announced.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// What a choice group reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceEvent {
    /// A person picked an option.
    Chosen {
        /// The group's identifier.
        group: SharedString,
        /// The chosen option's identifier.
        option: SharedString,
    },
}

/// A set of options where exactly one is chosen.
///
/// # Example
///
/// ```
/// use gpui_ai::prelude::{ChoiceGroup, ChoiceOption};
///
/// let flavours = ChoiceGroup::new("flavours", "How many flavours?")
///     .options([
///         ChoiceOption::new("three", "Three").description("The core line"),
///         ChoiceOption::new("five", "Five"),
///     ])
///     .selected("three");
/// ```
#[derive(IntoElement)]
pub struct ChoiceGroup {
    id: SharedString,
    label: Option<SharedString>,
    options: Vec<ChoiceOption>,
    selected: Option<SharedString>,
    on_event: Option<SharedHandler<ChoiceEvent>>,
    style: StyleRefinement,
}

impl ChoiceGroup {
    /// Creates a group with a stable identifier and the question it asks.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: Some(label.into()),
            options: Vec::new(),
            selected: None,
            on_event: None,
            style: StyleRefinement::default(),
        }
    }

    /// Creates a group with no visible question of its own, for a caller
    /// that has already asked one.
    pub fn unlabelled(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: None,
            options: Vec::new(),
            selected: None,
            on_event: None,
            style: StyleRefinement::default(),
        }
    }

    /// Sets the options, in the order they are offered.
    pub fn options(mut self, options: impl IntoIterator<Item = ChoiceOption>) -> Self {
        self.options = options.into_iter().collect();
        self
    }

    /// Selects an option by identifier. Nothing is selected by default.
    pub fn selected(mut self, option: impl Into<SharedString>) -> Self {
        self.selected = Some(option.into());
        self
    }

    /// Selects an option, or none.
    pub fn selection(mut self, option: Option<SharedString>) -> Self {
        self.selected = option;
        self
    }

    /// Handles the typed choice.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ChoiceEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(std::rc::Rc::new(handler));
        self
    }
}

impl Styled for ChoiceGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ChoiceGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = ElementId::from(self.id.clone());
        let group = self.id.clone();
        let handler = self.on_event;
        let selected = self.selected;
        let label = self.label.clone();
        let rows: Vec<AnyElement> = self
            .options
            .into_iter()
            .enumerate()
            .map(|(position, option)| {
                let chosen = selected.as_ref() == Some(&option.id);
                choice_row(
                    &root_id,
                    &group,
                    option,
                    chosen,
                    position,
                    handler.clone(),
                    window,
                    cx,
                )
            })
            .collect();
        let set_size = rows.len();

        v_flex()
            .id(root_id)
            .gap(tokens.spacing.xs)
            .role(Role::RadioGroup)
            .when_some(label.clone(), |group, label| group.aria_label(label))
            .aria_size_of_set(set_size)
            .refine_style(&self.style)
            .when_some(label, |group, label| {
                group.child(
                    div()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .child(label),
                )
            })
            .children(rows)
    }
}

/// The diameter of a choice's indicator ring, and the side of a check box.
///
/// A glyph slot rather than a fresh number, and deliberately not the seat it
/// sits in: the control is sixteen pixels, the seat is the label's own line,
/// and conflating the two put every indicator in this file two pixels above
/// the text it belongs to.
fn indicator_size(cx: &App) -> Pixels {
    crate::sizing::slot_sm(cx)
}

/// The seat every control in this file sits in.
///
/// As tall as the label's first line, so the control is centred on the words
/// rather than on the whole option - an option with a description is two lines
/// tall, and a control centred on both floats between them. As wide as the
/// widest control the file draws, which is the switch's track, so a column
/// mixing switches and boxes keeps one left edge and one text inset.
fn control_seat(control: impl IntoElement, cx: &App) -> gpui::Div {
    let tokens = cx.theme().semantic_tokens();
    crate::surface::leading_control_slot(
        tokens.typography.sm.line_height,
        switch_track_width(indicator_size(cx)),
        control,
    )
}

/// How wide a switch's track is, for the seat and for the switch itself.
fn switch_track_width(slot: Pixels) -> Pixels {
    slot * 1.6
}

/// One option: an indicator, its label, and whatever it says about itself.
#[allow(clippy::too_many_arguments)]
fn choice_row(
    root_id: &ElementId,
    group: &SharedString,
    option: ChoiceOption,
    chosen: bool,
    position: usize,
    handler: Option<SharedHandler<ChoiceEvent>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    let row_id = ElementId::from((root_id.clone(), option.id.clone()));
    let press_id = ElementId::from((row_id.clone(), "press"));
    // Built inside the selector, never for it: `debug_selector` drops its
    // closure unread in release, and a `SharedString` clone is a refcount bump.
    let debug_group = group.clone();
    let debug_option = option.id.clone();
    let event = ChoiceEvent::Chosen {
        group: group.clone(),
        option: option.id.clone(),
    };
    // The dot grows into the ring rather than appearing in it. Same channel
    // the disclosures use, so a reduced preference lands it in one frame.
    let fill = disclosure_progress(
        gpui_base::motion::TransitionId::from(ElementId::from((row_id.clone(), "chosen"))),
        chosen,
        window,
        cx,
    );
    let ring = indicator_size(cx);
    let accessibility_label = option.label.clone();

    let row = composed_button(row_id.clone(), accessibility_label)
        .w_full()
        .justify_start()
        .items_start()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xs)
        .role(Role::RadioButton)
        .aria_toggled(if chosen {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        })
        .aria_position_in_set(position + 1)
        .debug_selector(move || format!("choice-{debug_group}-{debug_option}"))
        .child(
            control_seat(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(ring)
                    .rounded_full()
                    .border_1()
                    .border_color(if chosen {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .child(
                        div()
                            .size(ring * 0.5 * fill)
                            .rounded_full()
                            .bg(cx.theme().primary),
                    ),
                cx,
            )
            .debug_selector({
                let (group, option) = (group.clone(), option.id.clone());
                move || format!("choice-seat-{group}-{option}")
            }),
        )
        .child(
            v_flex()
                .min_w_0()
                .gap(tokens.spacing.xxs)
                .child(
                    div()
                        .debug_selector({
                            let (group, option) = (group.clone(), option.id.clone());
                            move || format!("choice-label-{group}-{option}")
                        })
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .child(option.label.clone()),
                )
                .when_some(option.description.clone(), |body, description| {
                    body.child(hint(description, cx))
                }),
        );

    // The selected surface owns hover — GPUI accepts `.hover` once, so the
    // ramp added here is the press decay alone, not `quiet_press_surface`.
    let row = selection_surface(row, chosen, tokens.radius.lg, tokens.spacing.xs, None, cx)
        .border_1()
        .border_color(cx.theme().transparent)
        .focus_visible(|style| style.border_color(cx.theme().ring))
        .press_release(press_id, tokens.radius.md, window, cx);

    if option.disabled {
        return row.disabled(true).into_any_element();
    }

    match handler {
        Some(handler) => row
            .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
            .into_any_element(),
        None => row.into_any_element(),
    }
}

/// Whether a toggle reads as a box that is ticked or a switch that is thrown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToggleShape {
    /// A box: one of several things a person is choosing to include.
    #[default]
    Check,
    /// A switch: a setting that takes effect the moment it moves.
    Switch,
}

/// What a toggle reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleEvent {
    /// A person asked for the toggle to change.
    Toggled {
        /// The toggle's identifier.
        id: SharedString,
        /// The state being asked for.
        on: bool,
    },
}

/// A labelled control that is either on or off.
///
/// # Example
///
/// ```
/// use gpui_ai::prelude::{Toggle, ToggleShape};
///
/// let stream = Toggle::new("stream", "Stream the answer")
///     .shape(ToggleShape::Switch)
///     .on(true);
/// ```
#[derive(IntoElement)]
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    shape: ToggleShape,
    on: bool,
    disabled: bool,
    on_event: Option<SharedHandler<ToggleEvent>>,
    style: StyleRefinement,
}

impl Toggle {
    /// Creates a toggle with a stable identifier and its label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shape: ToggleShape::default(),
            on: false,
            disabled: false,
            on_event: None,
            style: StyleRefinement::default(),
        }
    }

    /// A second line under the label.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Chooses the box or the switch. The role follows the shape.
    pub fn shape(mut self, shape: ToggleShape) -> Self {
        self.shape = shape;
        self
    }

    /// Sets the state. The application owns it.
    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    /// Marks the toggle unavailable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Handles the typed change request.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ToggleEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(std::rc::Rc::new(handler));
        self
    }
}

impl Styled for Toggle {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Toggle {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        toggle_button(self, window, cx)
    }
}

/// The button a [`Toggle`] is, before it becomes an element.
///
/// Split out so its role and toggled state can be read directly — a
/// `RenderOnce` hands a caller an opaque `impl IntoElement`, and the role
/// belongs to the button underneath it.
fn toggle_button(toggle: Toggle, window: &mut Window, cx: &mut App) -> gpui_base::Button {
    let tokens = cx.theme().semantic_tokens();
    let row_id = ElementId::from(toggle.id.clone());
    let press_id = ElementId::from((row_id.clone(), "press"));
    let debug_id = toggle.id.clone();
    let event = ToggleEvent::Toggled {
        id: toggle.id.clone(),
        on: !toggle.on,
    };
    let travel = disclosure_progress(
        gpui_base::motion::TransitionId::from(ElementId::from((row_id.clone(), "on"))),
        toggle.on,
        window,
        cx,
    );
    let slot = indicator_size(cx);
    let indicator = match toggle.shape {
        ToggleShape::Check => check_indicator(slot, travel, cx),
        ToggleShape::Switch => switch_indicator(slot, travel, cx),
    };
    let role = match toggle.shape {
        ToggleShape::Check => Role::CheckBox,
        ToggleShape::Switch => Role::Switch,
    };

    let row = composed_button(row_id.clone(), toggle.label.clone())
        .w_full()
        .justify_start()
        .items_start()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xs)
        .role(role)
        .aria_toggled(if toggle.on {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        })
        .debug_selector(move || format!("toggle-{debug_id}"))
        .refine_style(&toggle.style)
        .child(control_seat(indicator, cx))
        .child(
            v_flex()
                .min_w_0()
                .gap(tokens.spacing.xxs)
                .child(
                    div()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .child(toggle.label.clone()),
                )
                .when_some(toggle.description.clone(), |body, description| {
                    body.child(hint(description, cx))
                }),
        )
        .quiet_press_surface(press_id, tokens.radius.md, window, cx);

    let handler = toggle.on_event.filter(|_| !toggle.disabled);
    row.when(toggle.disabled, |row| row.disabled(true))
        .when_some(handler, |row, handler| {
            row.on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
        })
}

/// A box that fills and shows its tick as the state arrives.
fn check_indicator(slot: Pixels, travel: f32, cx: &App) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(slot)
        .rounded(tokens.radius.sm)
        .border_1()
        .border_color(if travel > 0.5 {
            cx.theme().primary
        } else {
            cx.theme().border
        })
        .bg(cx.theme().primary.opacity(travel))
        .child(
            Icon::new(IconName::Check)
                .xsmall()
                .text_color(cx.theme().primary_foreground.opacity(travel)),
        )
        .into_any_element()
}

/// A track the thumb travels along, sized from the same slot as the box so
/// a column of mixed toggles keeps one text inset.
fn switch_indicator(slot: Pixels, travel: f32, cx: &App) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    // Every number here is the slot or a spacing token: a switch that grows
    // with the size policy rather than one drawn at a fixed size.
    let track_width = switch_track_width(slot);
    let track_height = slot * 0.75;
    let inset = tokens.spacing.xxs;
    let thumb = track_height - inset * 2.0;
    div()
        .relative()
        .flex()
        .items_center()
        .w(track_width)
        .h(track_height)
        .rounded_full()
        // The track carries the state: the border colour at rest, the
        // primary once thrown, crossed through as the thumb travels.
        .bg(cx.theme().border.opacity(1.0 - travel))
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .bg(cx.theme().primary.opacity(travel)),
        )
        .child(
            div()
                .absolute()
                .ml(inset + (track_width - thumb - inset * 2.0) * travel)
                .size(thumb)
                .rounded_full()
                .bg(cx.theme().background),
        )
        .into_any_element()
}
