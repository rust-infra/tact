//! Controlled fine-tune inspector for design properties.

use gpui_base::StyledExt as _;
use std::sync::Arc;

use gpui::Hsla;
use gpui::{
    AccessibleAction, AppContext as _, Axis, Context, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, MouseButton, Orientation,
    ParentElement as _, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, relative,
};
use gpui_base::{
    Decrement, Increment, SliderIndicator, SliderThumb, SliderTrack, StepAction, step_value,
};
use gpui_component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Sizable as _,
    button::Button,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::ScrollableMask,
    slider::{SliderEvent, SliderState},
    v_flex,
};

use crate::{
    control::{outlined_control, outlined_control_with_label},
    theme::SemanticStyledExt as _,
};

const MIN_DIMENSION: f64 = 1.;
const MAX_DIMENSION: f64 = 4_096.;
const MIN_RADIUS: f64 = 0.;
const MAX_RADIUS: f64 = 2_048.;

fn finite_clamp(value: f64, min: f64, max: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        min
    }
}

fn normalize_opacity(opacity: f32) -> f32 {
    if opacity.is_finite() {
        opacity.clamp(0., 1.)
    } else {
        0.
    }
}

/// Consumer-owned design values displayed by a [`FineTuneCard`].
#[derive(Clone, Debug, PartialEq)]
pub struct FineTuneValues {
    width: f64,
    height: f64,
    radius: f64,
    opacity: f32,
    typeface_id: SharedString,
    accent: Option<Hsla>,
}

impl FineTuneValues {
    /// Create a normalized snapshot without an accent color.
    ///
    /// Dimensions clamp to `1..=4096`, radius to `0..=2048`, and opacity to
    /// the normalized `0..=1` range.
    pub fn new(
        width: f64,
        height: f64,
        radius: f64,
        opacity: f32,
        typeface_id: impl Into<SharedString>,
    ) -> Self {
        Self {
            width: finite_clamp(width, MIN_DIMENSION, MAX_DIMENSION),
            height: finite_clamp(height, MIN_DIMENSION, MAX_DIMENSION),
            radius: finite_clamp(radius, MIN_RADIUS, MAX_RADIUS),
            opacity: normalize_opacity(opacity),
            typeface_id: typeface_id.into(),
            accent: None,
        }
    }

    /// Set the optional accent color.
    pub fn accent(mut self, accent: Hsla) -> Self {
        self.accent = Some(accent);
        self
    }

    /// Return a snapshot with a different normalized width.
    pub fn with_width(mut self, width: f64) -> Self {
        self.width = finite_clamp(width, MIN_DIMENSION, MAX_DIMENSION);
        self
    }

    /// Return a snapshot with a different normalized height.
    pub fn with_height(mut self, height: f64) -> Self {
        self.height = finite_clamp(height, MIN_DIMENSION, MAX_DIMENSION);
        self
    }

    /// Return a snapshot with a different normalized radius.
    pub fn with_radius(mut self, radius: f64) -> Self {
        self.radius = finite_clamp(radius, MIN_RADIUS, MAX_RADIUS);
        self
    }

    /// Return a snapshot with a different normalized opacity.
    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = normalize_opacity(opacity);
        self
    }

    /// Return a snapshot with a different stable typeface identity.
    pub fn with_typeface(mut self, typeface_id: impl Into<SharedString>) -> Self {
        self.typeface_id = typeface_id.into();
        self
    }

    /// Return a snapshot with a different optional accent color.
    pub fn with_accent(mut self, accent: Option<Hsla>) -> Self {
        self.accent = accent;
        self
    }

    /// Return the clamped width in logical pixels.
    pub fn width(&self) -> f64 {
        self.width
    }

    /// Return the clamped height in logical pixels.
    pub fn height(&self) -> f64 {
        self.height
    }

    /// Return the clamped corner radius in logical pixels.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// Return normalized opacity in `0..=1`.
    pub fn opacity(&self) -> f32 {
        self.opacity
    }

    /// Return the stable selected typeface identity.
    pub fn typeface_id(&self) -> &SharedString {
        &self.typeface_id
    }

    /// Return the optional accent color.
    pub fn accent_color(&self) -> Option<Hsla> {
        self.accent
    }
}

/// One application-owned typeface choice.
///
/// IDs must be stable and unique in a snapshot. Labels may be duplicated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FineTuneTypeface {
    id: SharedString,
    label: SharedString,
}

impl FineTuneTypeface {
    /// Create a typeface choice with stable identity and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Return the stable application identity.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Return the visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// A typed application intent emitted by [`FineTuneCard`].
#[derive(Clone, Debug, PartialEq)]
pub enum FineTuneEvent {
    /// The width editor proposed a different clamped value.
    WidthChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// Width in logical pixels.
        width: f64,
    },
    /// The height editor proposed a different clamped value.
    HeightChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// Height in logical pixels.
        height: f64,
    },
    /// The radius editor proposed a different clamped value.
    RadiusChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// Corner radius in logical pixels.
        radius: f64,
    },
    /// The opacity editor or slider proposed a normalized value.
    OpacityChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// Normalized opacity in `0..=1`.
        opacity: f32,
    },
    /// The typeface menu proposed a different stable typeface identity.
    TypefaceChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// Stable application typeface identity.
        typeface_id: SharedString,
    },
    /// The accent picker proposed a different optional color.
    AccentChanged {
        /// Fine-tune card identity.
        id: SharedString,
        /// The selected accent, or `None` when cleared.
        accent: Option<Hsla>,
    },
    /// The named Reset control was activated.
    ResetRequested {
        /// Fine-tune card identity.
        id: SharedString,
    },
    /// The named Apply control was activated.
    ApplyRequested {
        /// Fine-tune card identity.
        id: SharedString,
    },
}

impl FineTuneEvent {
    /// Return the stable identity of the card that emitted this intent.
    pub fn id(&self) -> &SharedString {
        match self {
            Self::WidthChanged { id, .. }
            | Self::HeightChanged { id, .. }
            | Self::RadiusChanged { id, .. }
            | Self::OpacityChanged { id, .. }
            | Self::TypefaceChanged { id, .. }
            | Self::AccentChanged { id, .. }
            | Self::ResetRequested { id }
            | Self::ApplyRequested { id } => id,
        }
    }
}

#[derive(Clone, Copy)]
enum NumericProperty {
    Width,
    Height,
    Radius,
    Opacity,
}

/// A consumer-controlled design-property inspector.
pub struct FineTuneCard {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    values: FineTuneValues,
    typefaces: Arc<[FineTuneTypeface]>,
    /// Scroll position of the card body when its height is constrained.
    scroll_handle: ScrollHandle,
    width_input: Entity<InputState>,
    height_input: Entity<InputState>,
    radius_input: Entity<InputState>,
    opacity_input: Entity<InputState>,
    opacity_slider: Entity<SliderState>,
    opacity_slider_focus: FocusHandle,
    accent_picker: Entity<ColorPickerState>,
    local_width: f64,
    local_height: f64,
    local_radius: f64,
    local_opacity: f32,
    local_typeface_id: SharedString,
    local_accent: Option<Hsla>,
    _subscriptions: Vec<Subscription>,
}

impl FineTuneCard {
    /// Create a controlled card from a value snapshot and typeface catalog.
    pub fn new<I>(
        id: impl Into<SharedString>,
        values: FineTuneValues,
        typefaces: I,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        I: IntoIterator<Item = FineTuneTypeface>,
    {
        let typefaces = valid_typefaces(typefaces).unwrap_or_default();
        let width_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(values.width.to_string())
                .step(1.)
                .min(MIN_DIMENSION)
                .max(MAX_DIMENSION)
        });
        let height_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(values.height.to_string())
                .step(1.)
                .min(MIN_DIMENSION)
                .max(MAX_DIMENSION)
        });
        let radius_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(values.radius.to_string())
                .step(1.)
                .min(MIN_RADIUS)
                .max(MAX_RADIUS)
        });
        let opacity_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value((values.opacity * 100.).to_string())
                .step(1.)
                .min(0.)
                .max(100.)
        });
        let opacity_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(100.)
                .step(1.)
                .default_value(values.opacity * 100.)
        });
        let opacity_slider_focus = cx.focus_handle();
        let accent_picker = cx.new(|cx| {
            let state = ColorPickerState::new(window, cx);
            if let Some(accent) = values.accent {
                state.default_value(accent)
            } else {
                state
            }
        });
        let width_subscription =
            cx.subscribe_in(&width_input, window, |this, input, event, window, cx| {
                this.on_numeric_input_event(NumericProperty::Width, input, event, window, cx)
            });
        let height_subscription =
            cx.subscribe_in(&height_input, window, |this, input, event, window, cx| {
                this.on_numeric_input_event(NumericProperty::Height, input, event, window, cx)
            });
        let radius_subscription =
            cx.subscribe_in(&radius_input, window, |this, input, event, window, cx| {
                this.on_numeric_input_event(NumericProperty::Radius, input, event, window, cx)
            });
        let opacity_subscription =
            cx.subscribe_in(&opacity_input, window, |this, input, event, window, cx| {
                this.on_numeric_input_event(NumericProperty::Opacity, input, event, window, cx)
            });
        let slider_subscription = cx.subscribe_in(
            &opacity_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                if let SliderEvent::Change(value) = event {
                    this.change_opacity(value.end() / 100., window, cx);
                }
            },
        );
        let accent_subscription = cx.subscribe_in(
            &accent_picker,
            window,
            |this, _, event: &ColorPickerEvent, window, cx| match event {
                ColorPickerEvent::Change(accent) => {
                    this.change_accent(*accent, window, cx);
                }
            },
        );
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            local_width: values.width,
            local_height: values.height,
            local_radius: values.radius,
            local_opacity: values.opacity,
            local_typeface_id: values.typeface_id.clone(),
            local_accent: values.accent,
            values,
            typefaces,
            width_input,
            height_input,
            radius_input,
            opacity_input,
            opacity_slider,
            opacity_slider_focus,
            accent_picker,
            scroll_handle: ScrollHandle::new(),
            _subscriptions: vec![
                width_subscription,
                height_subscription,
                radius_subscription,
                opacity_subscription,
                slider_subscription,
                accent_subscription,
            ],
        }
    }

    /// Replace the consumer-controlled value snapshot without rebuilding editors.
    ///
    /// A numeric editor keeps its exact text when that text is valid and
    /// semantically equal to the incoming value. This preserves focused caret
    /// and undo state while other editors synchronize to the new snapshot.
    pub fn set_values(
        &mut self,
        values: FineTuneValues,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.values != values
            || self.local_width != values.width
            || self.local_height != values.height
            || self.local_radius != values.radius
            || self.local_opacity != values.opacity
            || self.local_typeface_id != values.typeface_id
            || self.local_accent != values.accent;
        self.local_width = values.width;
        self.local_height = values.height;
        self.local_radius = values.radius;
        self.local_opacity = values.opacity;
        self.local_typeface_id = values.typeface_id.clone();
        self.local_accent = values.accent;

        Self::sync_numeric_input(
            &self.width_input,
            NumericProperty::Width,
            values.width,
            window,
            cx,
        );
        Self::sync_numeric_input(
            &self.height_input,
            NumericProperty::Height,
            values.height,
            window,
            cx,
        );
        Self::sync_numeric_input(
            &self.radius_input,
            NumericProperty::Radius,
            values.radius,
            window,
            cx,
        );
        Self::sync_numeric_input(
            &self.opacity_input,
            NumericProperty::Opacity,
            f64::from(values.opacity * 100.),
            window,
            cx,
        );
        let slider_value = values.opacity * 100.;
        if self.opacity_slider.read(cx).value().end() != slider_value {
            self.opacity_slider
                .update(cx, |slider, cx| slider.set_value(slider_value, window, cx));
        }
        if self.accent_picker.read(cx).value() != values.accent {
            self.accent_picker.update(cx, |picker, cx| {
                if let Some(accent) = values.accent {
                    picker.set_value(accent, window, cx);
                } else {
                    picker.clear_value(window, cx);
                }
            });
        }

        self.values = values;
        if changed {
            cx.notify();
        }
    }

    /// Replace the application typeface catalog without rebuilding editor state.
    ///
    /// Catalogs containing an empty or duplicate stable ID are ignored as one
    /// malformed snapshot. Duplicate visible labels are supported.
    pub fn set_typefaces<I>(&mut self, typefaces: I, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = FineTuneTypeface>,
    {
        let Some(typefaces) = valid_typefaces(typefaces) else {
            return;
        };
        if self.typefaces != typefaces {
            self.typefaces = typefaces;
            cx.notify();
        }
    }

    fn sync_numeric_input(
        input: &Entity<InputState>,
        property: NumericProperty,
        target: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = input.read(cx).value();
        let current = text.trim().parse::<f64>().ok().filter(|value| {
            value.is_finite()
                && match property {
                    NumericProperty::Width | NumericProperty::Height => {
                        (MIN_DIMENSION..=MAX_DIMENSION).contains(value)
                    }
                    NumericProperty::Radius => (MIN_RADIUS..=MAX_RADIUS).contains(value),
                    NumericProperty::Opacity => (0.0..=100.0).contains(value),
                }
        });
        // Preserving every valid semantically equal textual form also covers
        // focused editors without needing to disturb their caret or undo state.
        if current == Some(target) {
            return;
        }
        input.update(cx, |input, cx| {
            input.set_value(target.to_string(), window, cx)
        });
    }

    fn on_numeric_input_event(
        &mut self,
        property: NumericProperty,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let text = input.read(cx).value();
                let Ok(value) = text.trim().parse::<f64>() else {
                    return;
                };
                self.change_numeric(property, value, window, cx);
            }
            InputEvent::Blur if input.read(cx).value().trim().parse::<f64>().is_err() => {
                let value = match property {
                    NumericProperty::Width => self.values.width,
                    NumericProperty::Height => self.values.height,
                    NumericProperty::Radius => self.values.radius,
                    NumericProperty::Opacity => f64::from(self.values.opacity * 100.),
                };
                input.update(cx, |input, cx| {
                    input.set_value(value.to_string(), window, cx)
                });
                match property {
                    NumericProperty::Width => self.local_width = self.values.width,
                    NumericProperty::Height => self.local_height = self.values.height,
                    NumericProperty::Radius => self.local_radius = self.values.radius,
                    NumericProperty::Opacity => self.local_opacity = self.values.opacity,
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } | InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    fn change_numeric(
        &mut self,
        property: NumericProperty,
        value: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match property {
            NumericProperty::Width => {
                let width = finite_clamp(value, MIN_DIMENSION, MAX_DIMENSION);
                if self.local_width != width {
                    self.local_width = width;
                    cx.notify();
                    cx.emit(FineTuneEvent::WidthChanged {
                        id: self.id.clone(),
                        width,
                    });
                }
            }
            NumericProperty::Height => {
                let height = finite_clamp(value, MIN_DIMENSION, MAX_DIMENSION);
                if self.local_height != height {
                    self.local_height = height;
                    cx.notify();
                    cx.emit(FineTuneEvent::HeightChanged {
                        id: self.id.clone(),
                        height,
                    });
                }
            }
            NumericProperty::Radius => {
                let radius = finite_clamp(value, MIN_RADIUS, MAX_RADIUS);
                if self.local_radius != radius {
                    self.local_radius = radius;
                    cx.notify();
                    cx.emit(FineTuneEvent::RadiusChanged {
                        id: self.id.clone(),
                        radius,
                    });
                }
            }
            NumericProperty::Opacity => {
                self.change_opacity((value / 100.) as f32, window, cx);
            }
        }
    }

    fn change_opacity(&mut self, opacity: f32, window: &mut Window, cx: &mut Context<Self>) {
        let opacity = normalize_opacity(opacity);
        if self.local_opacity == opacity {
            return;
        }
        self.local_opacity = opacity;
        let percentage = opacity * 100.;
        let input_percentage = self
            .opacity_input
            .read(cx)
            .value()
            .trim()
            .parse::<f32>()
            .ok();
        if input_percentage != Some(percentage) {
            self.opacity_input.update(cx, |input, cx| {
                input.set_value(percentage.to_string(), window, cx)
            });
        }
        if self.opacity_slider.read(cx).value().end() != percentage {
            self.opacity_slider
                .update(cx, |slider, cx| slider.set_value(percentage, window, cx));
        }
        cx.notify();
        cx.emit(FineTuneEvent::OpacityChanged {
            id: self.id.clone(),
            opacity,
        });
    }

    fn step_opacity(&mut self, step: f32, window: &mut Window, cx: &mut Context<Self>) {
        let percentage = (self.local_opacity * 100. + step).clamp(0., 100.);
        self.change_opacity(percentage / 100., window, cx);
    }

    fn select_typeface(&mut self, typeface_id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let typeface_id = typeface_id.into();
        if self.local_typeface_id == typeface_id
            || !self.typefaces.iter().any(|item| item.id == typeface_id)
        {
            return;
        }
        self.local_typeface_id = typeface_id.clone();
        cx.notify();
        cx.emit(FineTuneEvent::TypefaceChanged {
            id: self.id.clone(),
            typeface_id,
        });
    }

    fn change_accent(&mut self, accent: Option<Hsla>, window: &mut Window, cx: &mut Context<Self>) {
        if self.local_accent == accent {
            return;
        }
        self.local_accent = accent;
        if self.accent_picker.read(cx).value() != accent {
            self.accent_picker.update(cx, |picker, cx| {
                if let Some(accent) = accent {
                    picker.set_value(accent, window, cx);
                } else {
                    picker.clear_value(window, cx);
                }
            });
        }
        cx.notify();
        cx.emit(FineTuneEvent::AccentChanged {
            id: self.id.clone(),
            accent,
        });
    }
}

fn valid_typefaces<I>(typefaces: I) -> Option<Arc<[FineTuneTypeface]>>
where
    I: IntoIterator<Item = FineTuneTypeface>,
{
    let mut validated = Vec::new();
    for typeface in typefaces {
        if typeface.id.is_empty()
            || validated
                .iter()
                .any(|item: &FineTuneTypeface| item.id == typeface.id)
        {
            return None;
        }
        validated.push(typeface);
    }
    Some(validated.into())
}

impl EventEmitter<FineTuneEvent> for FineTuneCard {}

#[derive(Clone, Copy)]
struct NumericSemantics {
    value: f64,
    min: f64,
    max: f64,
}

fn step_numeric_editor(
    input: &Entity<InputState>,
    action: StepAction,
    semantics: NumericSemantics,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let text = input.read(cx).value();
    let Some(next) = step_value(&text, action, 1., Some(semantics.min), Some(semantics.max)) else {
        return;
    };
    input.update(cx, |input, cx| input.replace_all(next, window, cx));
}

fn named_number_input(
    card_id: &SharedString,
    key: &'static str,
    label: &'static str,
    semantics: NumericSemantics,
    input: &Entity<InputState>,
    suffix: &'static str,
    cx: &mut gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let tokens = cx.theme().semantic_tokens();
    let focus = input.focus_handle(cx);
    let decrement_input = input.clone();
    let increment_input = input.clone();
    let accessible_decrement_input = input.clone();
    let accessible_increment_input = input.clone();
    let accessible_value_input = input.clone();
    div()
        .id((gpui::ElementId::from(card_id.clone()), key))
        .accessibility_id(format!("fine-tune.{card_id}.{key}"))
        .debug_selector(move || format!("fine-tune-{key}-editor"))
        .role(Role::SpinButton)
        .aria_label(label)
        .aria_numeric_value(semantics.value)
        .aria_min_numeric_value(semantics.min)
        .aria_max_numeric_value(semantics.max)
        .aria_numeric_value_step(1.)
        .aria_value(format!("{} {suffix}", semantics.value))
        .track_focus(&focus)
        .key_context("NumberInput")
        .on_action(move |_: &Decrement, window, cx| {
            step_numeric_editor(
                &decrement_input,
                StepAction::Decrement,
                semantics,
                window,
                cx,
            );
        })
        .on_action(move |_: &Increment, window, cx| {
            step_numeric_editor(
                &increment_input,
                StepAction::Increment,
                semantics,
                window,
                cx,
            );
        })
        .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
            step_numeric_editor(
                &accessible_decrement_input,
                StepAction::Decrement,
                semantics,
                window,
                cx,
            );
        })
        .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
            step_numeric_editor(
                &accessible_increment_input,
                StepAction::Increment,
                semantics,
                window,
                cx,
            );
        })
        .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
            let Some(gpui::accesskit::ActionData::Value(value)) = data else {
                return;
            };
            accessible_value_input.update(cx, |input, cx| {
                input.replace_all(value.to_string(), window, cx)
            });
        })
        .rounded(tokens.radius.sm)
        .border_1()
        .border_color(cx.theme().transparent)
        .focus_visible(|style| style.border_color(cx.theme().ring))
        .child(
            Input::new(input)
                .small()
                .role(None::<Role>)
                .suffix(crate::surface::field_unit(suffix, cx)),
        )
}

fn opacity_slider_control(
    card_id: &SharedString,
    value: f32,
    state: &Entity<SliderState>,
    focus: &FocusHandle,
    weak_card: gpui::WeakEntity<FineTuneCard>,
    cx: &mut gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let tokens = cx.theme().semantic_tokens();
    let percentage = state.read(cx).percentage().end;
    let mouse_focus = focus.clone();
    let keyboard_card = weak_card.clone();
    let decrement_card = weak_card.clone();
    let increment_card = weak_card.clone();
    let set_value_card = weak_card;
    div()
        .id((gpui::ElementId::from(card_id.clone()), "opacity-slider"))
        .debug_selector(|| "fine-tune-opacity-slider".to_owned())
        .accessibility_id(format!("fine-tune.{card_id}.opacity-slider"))
        .role(Role::Slider)
        .aria_label("Opacity slider")
        .aria_numeric_value(f64::from(value * 100.))
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(100.)
        .aria_numeric_value_step(1.)
        .aria_value(format!("{} percent", value * 100.))
        .aria_orientation(Orientation::Horizontal)
        .track_focus(focus)
        .tab_index(0)
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            mouse_focus.focus(window, cx);
        })
        .on_key_down(move |event, window, cx| {
            let step = match event.keystroke.key.as_str() {
                "left" | "down" => -1.,
                "right" | "up" => 1.,
                _ => return,
            };
            let _ = keyboard_card.update(cx, |card, cx| {
                card.step_opacity(step, window, cx);
            });
            cx.stop_propagation();
        })
        .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
            let _ = decrement_card.update(cx, |card, cx| {
                card.step_opacity(-1., window, cx);
            });
        })
        .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
            let _ = increment_card.update(cx, |card, cx| {
                card.step_opacity(1., window, cx);
            });
        })
        .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
            let Some(gpui::accesskit::ActionData::Value(value)) = data else {
                return;
            };
            let Ok(value) = value.parse::<f32>() else {
                return;
            };
            let _ = set_value_card.update(cx, |card, cx| {
                card.change_opacity(value / 100., window, cx);
            });
        })
        .relative()
        .w_full()
        .py(tokens.spacing.xs)
        .border_1()
        .border_color(cx.theme().transparent)
        .rounded(tokens.radius.sm)
        .focus_visible(|style| style.border_color(cx.theme().ring))
        .child(
            SliderTrack::new(state)
                .relative()
                .w_full()
                .h(tokens.spacing.sm)
                .child(
                    SliderIndicator::new(state)
                        .absolute()
                        .top(tokens.spacing.xs)
                        .left_0()
                        .w_full()
                        .h(tokens.spacing.xxs)
                        .rounded(tokens.radius.sm)
                        .bg(cx.theme().muted)
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .right(relative(1. - percentage))
                                .rounded(tokens.radius.sm)
                                .bg(cx.theme().primary),
                        ),
                )
                .child(
                    SliderThumb::new(state)
                        .absolute()
                        .top_0()
                        .left(relative(percentage))
                        .ml(-tokens.spacing.xs)
                        .size(tokens.spacing.sm)
                        // A thumb is a dot, and dots are circles here.
                        .rounded(tokens.radius.full)
                        .border_1()
                        .border_color(cx.theme().primary)
                        .bg(cx.theme().background),
                ),
        )
}

fn numeric_field(
    card_id: &SharedString,
    key: &'static str,
    label: &'static str,
    semantics: NumericSemantics,
    input: &Entity<InputState>,
    suffix: &'static str,
    cx: &mut gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let tokens = cx.theme().semantic_tokens();
    v_flex()
        .id((gpui::ElementId::from(card_id.clone()), key))
        .debug_selector(move || format!("fine-tune-{key}-input"))
        .role(Role::Group)
        .aria_label(format!("{label} property"))
        .flex_1()
        .min_w_0()
        .gap(tokens.spacing.xs)
        .child(Label::new(label).text_token(tokens.typography.sm))
        .child(named_number_input(
            card_id, key, label, semantics, input, suffix, cx,
        ))
}

impl gpui::Styled for FineTuneCard {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for FineTuneCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = self.id.clone();
        let selected_typeface = self
            .typefaces
            .iter()
            .find(|typeface| typeface.id == self.local_typeface_id)
            .map(|typeface| typeface.label.clone());
        let typefaces_empty = self.typefaces.is_empty();
        let selected_typeface_value: SharedString =
            selected_typeface.clone().unwrap_or_else(|| {
                if typefaces_empty {
                    "No typefaces available".into()
                } else {
                    format!("Unavailable typeface: {}", self.local_typeface_id).into()
                }
            });
        let selected_typeface =
            selected_typeface.unwrap_or_else(|| selected_typeface_value.clone());
        let typefaces = self.typefaces.clone();
        let selected_typeface_id = self.local_typeface_id.clone();
        let weak_card = cx.weak_entity();
        let opacity_weak_card = cx.weak_entity();
        let typeface_button =
            Button::new((gpui::ElementId::from(self.id.clone()), "typeface-trigger"))
                .label(selected_typeface)
                .small()
                .outline()
                .disabled(typefaces_empty)
                .debug_selector(|| "fine-tune-typeface".to_owned())
                .dropdown_caret(!typefaces_empty);
        let typeface_control = if typefaces_empty {
            typeface_button.into_any_element()
        } else {
            typeface_button
                .dropdown_menu(move |menu, _, _| {
                    typefaces.iter().fold(menu, |menu, typeface| {
                        let typeface_id = typeface.id.clone();
                        let callback_id = typeface_id.clone();
                        let weak_card = weak_card.clone();
                        menu.item(
                            PopupMenuItem::new(typeface.label.clone())
                                .checked(typeface_id == selected_typeface_id)
                                .on_click(move |_, _, cx| {
                                    let _ = weak_card.update(cx, |card, cx| {
                                        card.select_typeface(callback_id.clone(), cx);
                                    });
                                }),
                        )
                    })
                })
                .into_any_element()
        };
        let accent_text: SharedString = self
            .local_accent
            .map(|accent| accent.to_hex().into())
            .unwrap_or_else(|| "No accent selected".into());
        let has_accent = self.local_accent.is_some();
        let card = v_flex()
            .id(root_id.clone())
            .debug_selector(move || format!("fine-tune-card-{root_id}"))
            .role(Role::Group)
            .aria_label("Fine-tune design properties")
            .min_h_0()
            .h_full()
            .max_h_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .gap(tokens.spacing.md)
            .p(tokens.spacing.md)
            .rounded(tokens.radius.lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(tokens.colors.surface)
            .child(
                h_flex()
                    .w_full()
                    .gap(tokens.spacing.sm)
                    .child(numeric_field(
                        &self.id,
                        "width",
                        "Width",
                        NumericSemantics {
                            value: self.local_width,
                            min: MIN_DIMENSION,
                            max: MAX_DIMENSION,
                        },
                        &self.width_input,
                        "px",
                        cx,
                    ))
                    .child(numeric_field(
                        &self.id,
                        "height",
                        "Height",
                        NumericSemantics {
                            value: self.local_height,
                            min: MIN_DIMENSION,
                            max: MAX_DIMENSION,
                        },
                        &self.height_input,
                        "px",
                        cx,
                    )),
            )
            .child(numeric_field(
                &self.id,
                "radius",
                "Radius",
                NumericSemantics {
                    value: self.local_radius,
                    min: MIN_RADIUS,
                    max: MAX_RADIUS,
                },
                &self.radius_input,
                "px",
                cx,
            ))
            .child(
                v_flex()
                    .id((gpui::ElementId::from(self.id.clone()), "opacity"))
                    .role(Role::Group)
                    .aria_label("Opacity property")
                    .gap(tokens.spacing.xs)
                    .child(
                        h_flex()
                            .items_end()
                            .gap(tokens.spacing.sm)
                            .child(
                                Label::new("Opacity")
                                    .text_token(tokens.typography.sm)
                                    .flex_1(),
                            )
                            .child(div().w(tokens.spacing.xxl * 2.).child(named_number_input(
                                &self.id,
                                "opacity-input",
                                "Opacity",
                                NumericSemantics {
                                    value: f64::from(self.local_opacity * 100.),
                                    min: 0.,
                                    max: 100.,
                                },
                                &self.opacity_input,
                                "%",
                                cx,
                            ))),
                    )
                    .child(opacity_slider_control(
                        &self.id,
                        self.local_opacity,
                        &self.opacity_slider,
                        &self.opacity_slider_focus,
                        opacity_weak_card,
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .id((gpui::ElementId::from(self.id.clone()), "typeface"))
                    .role(Role::Group)
                    .aria_label("Typeface selection")
                    .aria_value(selected_typeface_value)
                    .gap(tokens.spacing.xs)
                    .child(Label::new("Typeface").text_token(tokens.typography.sm))
                    .child(typeface_control),
            )
            .child(
                v_flex()
                    .id((gpui::ElementId::from(self.id.clone()), "accent"))
                    .role(Role::Group)
                    .aria_label("Accent color")
                    .aria_value(accent_text.clone())
                    .gap(tokens.spacing.xs)
                    .child(Label::new("Accent").text_token(tokens.typography.sm))
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap(tokens.spacing.sm)
                            .items_center()
                            .child(
                                ColorPicker::new(&self.accent_picker)
                                    .small()
                                    .label("Accent color"),
                            )
                            .child(
                                Label::new(accent_text)
                                    .text_token(tokens.typography.sm)
                                    .flex_1(),
                            )
                            .when(has_accent, |this| {
                                this.child(
                                    outlined_control(
                                        (gpui::ElementId::from(self.id.clone()), "clear-accent"),
                                        "Remove accent color",
                                        window,
                                        cx,
                                    )
                                    .debug_selector(|| "fine-tune-clear-accent".to_owned())
                                    .on_click(cx.listener(
                                        |card, _, window, cx| {
                                            card.change_accent(None, window, cx);
                                        },
                                    )),
                                )
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .justify_end()
                    .gap(tokens.spacing.xs)
                    .child(
                        outlined_control_with_label(
                            (gpui::ElementId::from(self.id.clone()), "reset"),
                            "Reset fine-tune values",
                            "Reset",
                            window,
                            cx,
                        )
                        .debug_selector(|| "fine-tune-reset".to_owned())
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(FineTuneEvent::ResetRequested {
                                id: this.id.clone(),
                            });
                        })),
                    )
                    .child(
                        outlined_control_with_label(
                            (gpui::ElementId::from(self.id.clone()), "apply"),
                            "Apply fine-tune values",
                            "Apply",
                            window,
                            cx,
                        )
                        .debug_selector(|| "fine-tune-apply".to_owned())
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(FineTuneEvent::ApplyRequested {
                                id: this.id.clone(),
                            });
                        })),
                    ),
            )
            .child(div().hidden());

        div()
            .relative()
            .size_full()
            .child(card)
            .child(
                ScrollableMask::new(Axis::Vertical, &self.scroll_handle).id((
                    gpui::ElementId::from(self.id.clone()),
                    "content-scroll-mask",
                )),
            )
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Element as _, TestAppContext, accesskit, canvas};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::{Arc, Mutex},
    };

    type CapturedSemanticNode = Arc<Mutex<Option<(Option<Role>, accesskit::Node)>>>;

    struct NumericSemanticProbe {
        input: Entity<InputState>,
        captured: CapturedSemanticNode,
    }

    struct SliderSemanticProbe {
        card: Entity<FineTuneCard>,
        captured: CapturedSemanticNode,
    }

    impl SliderSemanticProbe {
        fn new(
            captured: CapturedSemanticNode,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let card = cx.new(|cx| {
                FineTuneCard::new(
                    "fine-tune",
                    FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                    [FineTuneTypeface::new("inter", "Inter")],
                    window,
                    cx,
                )
            });
            Self { card, captured }
        }
    }

    impl Render for SliderSemanticProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let card = self.card.clone();
            let captured = self.captured.clone();
            canvas(
                move |_, _, cx| {
                    let (id, value, state, focus) = card.read_with(cx, |card, _| {
                        (
                            card.id.clone(),
                            card.local_opacity,
                            card.opacity_slider.clone(),
                            card.opacity_slider_focus.clone(),
                        )
                    });
                    let slider =
                        opacity_slider_control(&id, value, &state, &focus, card.downgrade(), cx)
                            .into_element();
                    let role = slider.a11y_role();
                    let mut node = accesskit::Node::new(Role::Unknown);
                    slider.write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") =
                        Some((role, node));
                },
                |_, _, _, _| {},
            )
        }
    }

    impl NumericSemanticProbe {
        fn new(
            captured: CapturedSemanticNode,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let input = cx.new(|cx| InputState::new(window, cx).default_value("320"));
            Self { input, captured }
        }
    }

    impl Render for NumericSemanticProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let input = self.input.clone();
            let captured = self.captured.clone();
            canvas(
                move |_, _, cx| {
                    let field = named_number_input(
                        &"fine-tune".into(),
                        "width",
                        "Width",
                        NumericSemantics {
                            value: 320.,
                            min: MIN_DIMENSION,
                            max: MAX_DIMENSION,
                        },
                        &input,
                        "px",
                        cx,
                    )
                    .into_element();
                    let role = field.a11y_role();
                    let mut node = accesskit::Node::new(Role::Unknown);
                    field.write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") =
                        Some((role, node));
                },
                |_, _, _, _| {},
            )
        }
    }

    #[test]
    fn controlled_values_clamp_numeric_properties_and_normalize_opacity() {
        let values = FineTuneValues::new(9_000., -20., 3_000., 1.4, "inter");

        assert_eq!(values.width(), 4_096.);
        assert_eq!(values.height(), 1.);
        assert_eq!(values.radius(), 2_048.);
        assert_eq!(values.opacity(), 1.);
        assert_eq!(values.typeface_id(), "inter");
    }

    #[gpui::test]
    fn numeric_editor_exposes_name_value_bounds_step_and_actions(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) =
            cx.add_window_view(move |window, cx| NumericSemanticProbe::new(captured, window, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let (role, node) = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("numeric property semantics should be captured");
        assert_eq!(role, Some(Role::SpinButton));
        assert_eq!(node.label(), Some("Width"));
        assert_eq!(node.value(), Some("320 px"));
        assert_eq!(node.numeric_value(), Some(320.));
        assert_eq!(node.min_numeric_value(), Some(MIN_DIMENSION));
        assert_eq!(node.max_numeric_value(), Some(MAX_DIMENSION));
        assert_eq!(node.numeric_value_step(), Some(1.));
        assert!(node.supports_action(AccessibleAction::Increment));
        assert!(node.supports_action(AccessibleAction::Decrement));
        assert!(node.supports_action(AccessibleAction::SetValue));
    }

    #[gpui::test]
    fn opacity_slider_exposes_name_value_bounds_step_and_actions(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (probe, cx) =
            cx.add_window_view(move |window, cx| SliderSemanticProbe::new(captured, window, cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let (role, node) = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("opacity slider semantics should be captured");
        assert_eq!(role, Some(Role::Slider));
        assert_eq!(node.label(), Some("Opacity slider"));
        assert_eq!(node.value(), Some("72 percent"));
        assert_eq!(node.numeric_value(), Some(72.));
        assert_eq!(node.min_numeric_value(), Some(0.));
        assert_eq!(node.max_numeric_value(), Some(100.));
        assert_eq!(node.numeric_value_step(), Some(1.));
        assert!(node.supports_action(AccessibleAction::Increment));
        assert!(node.supports_action(AccessibleAction::Decrement));
        assert!(node.supports_action(AccessibleAction::SetValue));

        let card = probe.read_with(cx, |probe, _| probe.card.clone());
        cx.update(|window, cx| {
            card.update(cx, |card, cx| card.step_opacity(1., window, cx));
            let (id, value, state, focus) = card.read_with(cx, |card, _| {
                (
                    card.id.clone(),
                    card.local_opacity,
                    card.opacity_slider.clone(),
                    card.opacity_slider_focus.clone(),
                )
            });
            let slider = opacity_slider_control(&id, value, &state, &focus, card.downgrade(), cx)
                .into_element();
            let role = slider.a11y_role();
            let mut node = accesskit::Node::new(Role::Unknown);
            slider.write_a11y_info(&mut node);
            *result.lock().expect("capture mutex should be available") = Some((role, node));
        });
        let (_, node) = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("locally edited opacity semantics should be captured before consumer echo");
        assert_eq!(node.value(), Some("73 percent"));
        assert_eq!(node.numeric_value(), Some(73.));
        assert_eq!(card.read_with(cx, |card, _| card.values.opacity), 0.72);
    }

    #[gpui::test]
    fn invalid_intermediate_number_does_not_emit_a_semantic_change(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&card, move |_, event, _| {
                captured.borrow_mut().push(event.clone());
            })
        });
        let input = card.read_with(cx, |card, _| card.width_input.clone());

        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.replace_all("-", window, cx));
        });
        cx.run_until_parked();

        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn numeric_editor_events_are_clamped_typed_and_card_identified(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&card, move |_, event, _| {
                captured.borrow_mut().push(event.clone());
            })
        });
        let (width, height, radius, opacity) = card.read_with(cx, |card, _| {
            (
                card.width_input.clone(),
                card.height_input.clone(),
                card.radius_input.clone(),
                card.opacity_input.clone(),
            )
        });

        cx.update(|window, cx| {
            width.update(cx, |input, cx| input.replace_all("9000", window, cx));
            height.update(cx, |input, cx| input.replace_all("-20", window, cx));
            radius.update(cx, |input, cx| input.replace_all("3000", window, cx));
            opacity.update(cx, |input, cx| input.replace_all("125", window, cx));
        });
        cx.run_until_parked();

        assert_eq!(
            events.borrow().as_slice(),
            [
                FineTuneEvent::WidthChanged {
                    id: "fine-tune".into(),
                    width: 4_096.,
                },
                FineTuneEvent::HeightChanged {
                    id: "fine-tune".into(),
                    height: 1.,
                },
                FineTuneEvent::RadiusChanged {
                    id: "fine-tune".into(),
                    radius: 2_048.,
                },
                FineTuneEvent::OpacityChanged {
                    id: "fine-tune".into(),
                    opacity: 1.,
                },
            ]
        );
    }

    #[gpui::test]
    fn controlled_replacement_preserves_valid_editor_text_and_identity(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        let (width, width_id, height_id, radius_id, opacity_id) = card.read_with(cx, |card, _| {
            (
                card.width_input.clone(),
                card.width_input.entity_id(),
                card.height_input.entity_id(),
                card.radius_input.entity_id(),
                card.opacity_input.entity_id(),
            )
        });
        cx.update(|window, cx| {
            width.update(cx, |input, cx| {
                input.replace_all("480.", window, cx);
            });
            card.update(cx, |card, cx| {
                card.set_values(
                    FineTuneValues::new(480., 240., 12., 0.5, "inter"),
                    window,
                    cx,
                );
            });
        });

        card.read_with(cx, |card, cx| {
            assert_eq!(card.width_input.entity_id(), width_id);
            assert_eq!(card.height_input.entity_id(), height_id);
            assert_eq!(card.radius_input.entity_id(), radius_id);
            assert_eq!(card.opacity_input.entity_id(), opacity_id);
            assert_eq!(card.width_input.read(cx).value(), "480.");
            assert_eq!(card.height_input.read(cx).value(), "240");
            assert_eq!(card.radius_input.read(cx).value(), "12");
            assert_eq!(card.opacity_input.read(cx).value(), "50");
            assert_eq!(card.opacity_slider.read(cx).value().end(), 50.);
        });

        cx.update(|window, cx| {
            width.update(cx, |input, cx| input.replace_all("9000", window, cx));
            card.update(cx, |card, cx| {
                card.set_values(
                    FineTuneValues::new(4_096., 240., 12., 0.5, "inter"),
                    window,
                    cx,
                );
            });
        });
        assert_eq!(width.read_with(cx, |input, _| input.value()), "4096");
    }

    #[gpui::test]
    fn typeface_and_accent_changes_emit_stable_identity_once(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter-regular"),
                [
                    FineTuneTypeface::new("inter-regular", "Inter"),
                    FineTuneTypeface::new("inter-display", "Inter"),
                ],
                window,
                cx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&card, move |_, event, _| {
                captured.borrow_mut().push(event.clone());
            })
        });
        let accent = gpui::hsla(0.58, 0.75, 0.52, 1.);

        cx.update(|window, cx| {
            card.update(cx, |card, cx| {
                card.select_typeface("inter-display", cx);
                card.select_typeface("inter-display", cx);
                card.change_accent(Some(accent), window, cx);
                card.change_accent(Some(accent), window, cx);
            });
        });

        assert_eq!(
            events.borrow().as_slice(),
            [
                FineTuneEvent::TypefaceChanged {
                    id: "fine-tune".into(),
                    typeface_id: "inter-display".into(),
                },
                FineTuneEvent::AccentChanged {
                    id: "fine-tune".into(),
                    accent: Some(accent),
                },
            ]
        );
    }

    #[gpui::test]
    fn unchanged_consumer_snapshot_rejects_local_typeface_and_notifies(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let snapshot = FineTuneValues::new(320., 180., 24., 0.72, "inter-regular");
        let (card, cx) = cx.add_window_view({
            let snapshot = snapshot.clone();
            move |window, cx| {
                FineTuneCard::new(
                    "fine-tune",
                    snapshot,
                    [
                        FineTuneTypeface::new("inter-regular", "Inter Regular"),
                        FineTuneTypeface::new("inter-display", "Inter Display"),
                    ],
                    window,
                    cx,
                )
            }
        });
        let notifications = Rc::new(Cell::new(0));
        let captured = notifications.clone();
        let _observation =
            cx.update(|_, cx| cx.observe(&card, move |_, _| captured.set(captured.get() + 1)));

        cx.update(|_, cx| {
            card.update(cx, |card, cx| card.select_typeface("inter-display", cx));
        });
        cx.run_until_parked();
        let before_rejection = notifications.get();

        cx.update(|window, cx| {
            card.update(cx, |card, cx| card.set_values(snapshot, window, cx));
        });
        cx.run_until_parked();

        assert_eq!(
            card.read_with(cx, |card, _| card.local_typeface_id.clone()),
            "inter-regular"
        );
        assert!(notifications.get() > before_rejection);
    }

    #[gpui::test]
    fn retained_slider_and_picker_subscriptions_emit_typed_changes_once(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&card, move |_, event, _| {
                captured.borrow_mut().push(event.clone());
            })
        });
        let (slider, picker) = card.read_with(cx, |card, _| {
            (card.opacity_slider.clone(), card.accent_picker.clone())
        });
        let accent = gpui::hsla(0.08, 0.82, 0.56, 1.);

        cx.update(|_, cx| {
            slider.update(cx, |_, cx| cx.emit(SliderEvent::Change(75_f32.into())));
            slider.update(cx, |_, cx| cx.emit(SliderEvent::Release(75_f32.into())));
            picker.update(cx, |_, cx| cx.emit(ColorPickerEvent::Change(Some(accent))));
        });

        assert_eq!(
            events.borrow().as_slice(),
            [
                FineTuneEvent::OpacityChanged {
                    id: "fine-tune".into(),
                    opacity: 0.75,
                },
                FineTuneEvent::AccentChanged {
                    id: "fine-tune".into(),
                    accent: Some(accent),
                },
            ]
        );
    }

    #[gpui::test]
    fn malformed_typeface_replacement_is_ignored(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (card, cx) = cx.add_window_view(|window, cx| {
            FineTuneCard::new(
                "fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });

        cx.update(|_, cx| {
            card.update(cx, |card, cx| {
                card.set_typefaces(
                    [
                        FineTuneTypeface::new("duplicate", "Inter"),
                        FineTuneTypeface::new("duplicate", "Display"),
                    ],
                    cx,
                );
            });
        });

        card.read_with(cx, |card, _| {
            assert_eq!(
                card.typefaces.as_ref(),
                [FineTuneTypeface::new("inter", "Inter")]
            );
        });
    }
}
