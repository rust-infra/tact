use crate::motion::press_release_progress;
use crate::sizing::SizeTokens;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ElementId, InteractiveElement as _, MouseButton, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::Button;
use gpui_component::ActiveTheme as _;

pub(crate) fn composed_button(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
) -> Button {
    // Every composed control is a pointer target; the affordance is part
    // of the family, not a per-component choice.
    Button::new(id)
        .accessibility_label(accessibility_label)
        .cursor_pointer()
}

/// The pressed-state half of the interaction ramp, shared by every
/// composed control family.
///
/// Pressing responds on pointer-down through the caller's `active` style —
/// instantly, as a direct manipulation must. This extension stages only
/// the way back out: on release, a tint overlay starts at the pressed
/// color and decays over the press spring's response, so the control eases
/// back to rest instead of snapping. The overlay sits under the label
/// (apply this before adding label children), takes the control's own
/// radius, and paints nothing once settled. Reduced motion resolves the
/// decay instantly through the shared clock.
pub(crate) trait PressReleaseExt: Sized {
    fn press_release(
        self,
        key: ElementId,
        radius: gpui::Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self;
}

/// The pressed flag and release-decay sample of one control's press ramp.
///
/// Reads the same keyed state [`PressReleaseExt::press_release`] installs,
/// so a control can derive extra pressed presentation — the icon-button
/// glyph compression — from the identical clock, adding no frame demand
/// of its own.
pub(crate) fn press_release_state(
    key: &ElementId,
    window: &mut Window,
    cx: &mut App,
) -> (bool, f32) {
    let pressed = window.use_keyed_state((key.clone(), "pressed"), cx, |_, _| false);
    let pressed = *pressed.read(cx);
    (pressed, press_release_fade(key, window, cx))
}

/// How much of the release tint is still showing, and nothing else.
///
/// A control that has been pressed once keeps a non-zero generation for
/// the rest of its life, so a guard on the generation alone left every
/// later render formatting a clock id and sampling a settled channel.
///
/// The curve says when to stop, rather than a clock guessing: the frame
/// that samples a finished decay latches this flag, and every frame after
/// it takes the cheap path. Reading a wall clock instead risks cutting the
/// sampling off mid-decay, and the sample is what asks for the next frame
/// — the tint would then stay painted at whatever it last reached.
fn press_release_fade(key: &ElementId, window: &mut Window, cx: &mut App) -> f32 {
    let generation = window.use_keyed_state((key.clone(), "press-generation"), cx, |_, _| 0u64);
    let settled = press_fade_settled(key, window, cx);
    let generation = *generation.read(cx);
    if generation == 0 || *settled.read(cx) {
        return 0.0;
    }
    let clock = ElementId::Name(format!("press-release-{key:?}-{generation}").into());
    let progress = press_release_progress(clock, window, cx);
    if progress >= 1.0 {
        settled.update(cx, |settled, _| *settled = true);
    }
    1.0 - progress
}

/// Whether this control's release decay has finished playing.
///
/// Starts true: a control that has never been pressed has nothing to
/// decay. A press clears it and the settled frame sets it again, so the
/// generation stays monotonic — reusing a generation would reuse a clock
/// that has already run, and the next release would snap instead of ease.
fn press_fade_settled(key: &ElementId, window: &mut Window, cx: &mut App) -> gpui::Entity<bool> {
    window.use_keyed_state((key.clone(), "press-fade-settled"), cx, |_, _| true)
}

impl PressReleaseExt for Button {
    fn press_release(
        self,
        key: ElementId,
        radius: gpui::Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let pressed = window.use_keyed_state((key.clone(), "pressed"), cx, |_, _| false);
        let generation = window.use_keyed_state((key.clone(), "press-generation"), cx, |_, _| 0u64);
        let settled = press_fade_settled(&key, window, cx);
        let fade = press_release_fade(&key, window, cx);
        let arm = pressed.clone();
        let release = move |window: &mut gpui::Window, cx: &mut App| {
            let was_pressed = *arm.read(cx);
            if was_pressed {
                arm.update(cx, |pressed, _| *pressed = false);
                generation.update(cx, |generation, _| *generation += 1);
                settled.update(cx, |settled, _| *settled = false);
                window.refresh();
            }
        };
        let release_out = release.clone();
        self.on_mouse_down(MouseButton::Left, {
            let pressed = pressed.clone();
            move |_, window, cx| {
                pressed.update(cx, |pressed, _| *pressed = true);
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| release(window, cx))
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            release_out(window, cx)
        })
        .when(fade > 0.004, |this| {
            this.child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(radius)
                    .bg(cx.theme().button_active)
                    .opacity(fade),
            )
        })
    }
}

/// The quiet pressable surface every low-chrome control shares.
///
/// A transparent border reserving the focus ring, the accent ramp — hover
/// tints, press deepens to the full accent, release decays through the
/// shared spring — and the ring on keyboard focus alone. The radius is
/// stated once and reaches the fill, the ring, and the decay overlay,
/// where each site used to state it twice and could drift.
pub(crate) trait QuietSurfaceExt: Sized {
    fn quiet_press_surface(
        self,
        key: ElementId,
        radius: gpui::Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self;
}

impl QuietSurfaceExt for Button {
    fn quiet_press_surface(
        self,
        key: ElementId,
        radius: gpui::Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        self.border_1()
            .border_color(cx.theme().transparent)
            .rounded(radius)
            .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
            .active(|style| style.bg(cx.theme().accent))
            .focus_visible(|style| style.border_color(cx.theme().ring))
            .press_release(key, radius, window, cx)
    }
}

/// Applies the library's own control geometry to an upstream button.
///
/// Upstream sizes its buttons for its own density — a small button is
/// twenty-four pixels tall with eight of horizontal padding, which is
/// what the 0.4.0 feel review read as tight in the approval and plan
/// CTAs. This keeps the upstream variant's colours (primary, danger,
/// ghost, outline) and states, and states the geometry the rest of the
/// library uses: one height and one padding from the size policy, one
/// radius from the theme. An application that widens
/// [`SizeTokens::control_padding_sm`](crate::sizing::SizeTokens::control_padding_sm)
/// widens every one of them at once.
pub(crate) trait ControlMetricsExt: Sized {
    /// Restates this control's height, horizontal padding, and radius from
    /// the crate's own policy. Apply it last, after the variant.
    ///
    /// Nothing here nudges the label, and an optical correction was tried and
    /// removed. This height and the theme's own leading already centre the cap
    /// height over the baseline, which is the thing to centre; the test in
    /// `tests/approval.rs` says why, and measures it.
    fn control_metrics(self, cx: &App) -> Self;
}

impl<E: gpui::Styled + Sized> ControlMetricsExt for E {
    fn control_metrics(self, cx: &App) -> Self {
        let tokens = cx.theme().semantic_tokens();
        let sizes = SizeTokens::read(cx);
        self.h(sizes.control_md())
            .px(sizes.control_padding_md())
            .rounded(tokens.radius.md)
    }
}

pub(crate) fn outlined_control(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let label = accessibility_label.into();
    outlined_control_with_label(id, label.clone(), label, window, cx)
}

/// [`outlined_control_with_label`] with a leading glyph.
///
/// The icon joins the control's own padding and gap rather than an
/// upstream button's, so an icon-and-label control stands in the same
/// geometry as every other compact control in the library.
pub(crate) fn outlined_control_with_icon(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    icon: impl gpui_component::IconNamed,
    visible_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    outlined_control_bare(id, accessibility_label, window, cx)
        .gap(tokens.spacing.xs)
        .child(gpui_component::Sizable::xsmall(gpui_component::Icon::new(
            icon,
        )))
        .child(div().min_w_0().truncate().child(visible_label.into()))
}

pub(crate) fn outlined_control_with_label(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    visible_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    outlined_control_bare(id, accessibility_label, window, cx)
        // Compact controls are often placed in fixed-width table columns.
        // Keep their visible label inside the control instead of allowing a
        // long title to paint over the adjacent column; the full accessible
        // label remains on the Button.
        .child(div().min_w_0().truncate().child(visible_label.into()))
}

/// The same compact control, with a label that wraps rather than truncating.
///
/// [`outlined_control_with_label`] cuts a long title to keep it inside a
/// fixed-width column, which is right where the column cannot grow. A column
/// that takes its share of the table's width can grow, and there a plan named
/// "Enterprise with dedicated support" is a name the reader needs whole rather
/// than an ellipsis they have to hover to resolve.
///
/// So the label wraps onto a second line and the control grows with it: the
/// family's one height becomes a floor instead of a ceiling, and the row of
/// headers stretches to the tallest of them, which keeps the columns level.
pub(crate) fn outlined_control_with_wrapping_label(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    visible_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let height = SizeTokens::read(cx).control_sm();
    let padding = cx.theme().semantic_tokens().spacing.xxs;
    outlined_control_bare(id, accessibility_label, window, cx)
        .h(gpui::Length::Auto)
        .min_h(height)
        .py(padding)
        .text_center()
        .child(div().min_w_0().child(visible_label.into()))
}

/// The compact control frame every outlined control shares, without its
/// children: one height from the size policy, one radius, one text style,
/// the interaction ramp, and the disabled recipe.
fn outlined_control_bare(
    id: impl Into<ElementId>,
    accessibility_label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let tokens = cx.theme().semantic_tokens();
    let sizes = SizeTokens::read(cx);
    let id = id.into();
    composed_button(id.clone(), accessibility_label)
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            gpui_base::GlobalState::suppress_text_selection(cx);
        })
        .flex()
        .items_center()
        .justify_center()
        // Compact pill geometry shared by every small control in the
        // library — filter chips, row CTAs, toggles. One height from the
        // size policy, one radius, one text style, so controls look like
        // one family next to tables.
        .h(sizes.control_sm())
        .px(sizes.control_padding_sm())
        .border_1()
        .border_color(cx.theme().border)
        .rounded(tokens.radius.md)
        .bg(cx.theme().transparent)
        .text_token(tokens.typography.sm)
        .text_color(cx.theme().foreground)
        .hover(|style| style.bg(cx.theme().button_hover))
        .active(|style| style.bg(cx.theme().button_active))
        .focus_visible(|style| style.border_color(cx.theme().ring))
        // A control that cannot be pressed says so: muted label, softened
        // border, and no tint ramp. Defined once, so a disabled row action
        // stops rendering exactly like an enabled one.
        .styles(|styles| {
            styles.disabled(|style| {
                style
                    .text_color(cx.theme().muted_foreground)
                    .border_color(cx.theme().border.opacity(0.5))
                    .bg(cx.theme().transparent)
            })
        })
        .press_release(id, tokens.radius.md, window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Element as _, IntoElement as _, Render, RenderOnce as _, Role, TestAppContext, Window,
        accesskit, canvas,
    };
    use std::sync::{Arc, Mutex};

    mod control_metrics {
        use super::super::*;
        use crate::sizing::SizeTokens;
        use gpui::{
            Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, div, px,
        };
        use gpui_component::h_flex;

        struct MetricsProbe;

        impl Render for MetricsProbe {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div().w(px(420.)).h(px(160.)).child(
                    h_flex()
                        .items_start()
                        .gap(px(8.))
                        .child(
                            outlined_control("metrics-outlined", "Open", window, cx)
                                .debug_selector(|| "metrics-outlined".into()),
                        )
                        .child(
                            crate::surface::icon_button(
                                "metrics-icon",
                                gpui_component::IconName::Search,
                                "Search",
                                window,
                                cx,
                            )
                            .debug_selector(|| "metrics-icon".into()),
                        ),
                )
            }
        }

        /// Every composed control resolves its height through the size
        /// policy — the six coincidental heights the 0.4.0 audit found
        /// cannot come back without failing here.
        #[gpui::test]
        fn composed_controls_take_their_heights_from_the_size_policy(cx: &mut TestAppContext) {
            cx.update(crate::init);
            let (_, cx) = cx.add_window_view(|_, _| MetricsProbe);
            let cx: &mut VisualTestContext = cx;
            cx.update(|window, cx| window.draw(cx).clear(cx));

            let sizes = cx.update(|_, cx| *SizeTokens::read(cx));
            let outlined = cx
                .debug_bounds("metrics-outlined")
                .expect("the outlined control should render");
            assert_eq!(
                outlined.size.height,
                sizes.control_sm(),
                "outlined_control must stand exactly control_sm tall"
            );
            let icon = cx
                .debug_bounds("metrics-icon")
                .expect("the icon button should render");
            assert_eq!(
                icon.size.height,
                sizes.control_sm(),
                "icon buttons are control_sm"
            );
            assert_eq!(
                icon.size.width,
                sizes.control_sm(),
                "icon buttons are square"
            );
        }

        /// Pressing responds instantly through the active style; only the
        /// way back out is staged. A click therefore leaves a decaying
        /// tint that asks for frames until the press spring settles, and a
        /// settled control asks for nothing.
        #[gpui::test]
        fn a_released_press_fades_out_and_settles(cx: &mut TestAppContext) {
            cx.update(crate::init);
            let (_, cx) = cx.add_window_view(|_, _| MetricsProbe);
            let cx: &mut VisualTestContext = cx;
            cx.update(|window, cx| window.draw(cx).clear(cx));
            crate::motion::take_reveal_frame_requests();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(
                crate::motion::take_reveal_frame_requests(),
                0,
                "an untouched control is settled"
            );

            let bounds = cx
                .debug_bounds("metrics-outlined")
                .expect("the control should render");
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                crate::motion::take_reveal_frame_requests() > 0,
                "a released press must decay across frames"
            );

            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            crate::motion::take_reveal_frame_requests();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(
                crate::motion::take_reveal_frame_requests(),
                0,
                "the decay must settle and stop asking for frames"
            );
        }

        struct PressStateProbe {
            captured: std::rc::Rc<std::cell::Cell<(bool, f32)>>,
        }

        impl Render for PressStateProbe {
            fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let key = gpui::ElementId::from("metrics-icon");
                self.captured.set(press_release_state(&key, window, cx));
                div().w(px(420.)).h(px(160.)).child(
                    crate::surface::icon_button(
                        "metrics-icon",
                        gpui_component::IconName::Search,
                        "Search",
                        window,
                        cx,
                    )
                    .debug_selector(|| "metrics-icon".into()),
                )
            }
        }

        /// The shared press state feeds pressed presentation beyond the
        /// tint — the icon-button glyph compression rides it — so it must
        /// report the press on its frame and decay to rest after release.
        #[gpui::test]
        fn press_release_state_reports_the_press_and_its_decay(cx: &mut TestAppContext) {
            cx.update(crate::init);
            let captured = std::rc::Rc::new(std::cell::Cell::new((false, 0.0)));
            let probe = captured.clone();
            let (_, cx) = cx.add_window_view(move |_, _| PressStateProbe { captured: probe });
            let cx: &mut VisualTestContext = cx;
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(captured.get(), (false, 0.0), "untouched controls rest");

            let bounds = cx
                .debug_bounds("metrics-icon")
                .expect("the icon button should render");
            cx.simulate_mouse_down(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(
                captured.get().0,
                "the press must register on the press frame"
            );

            cx.simulate_mouse_up(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let (pressed, fade) = captured.get();
            assert!(!pressed, "release clears the press");
            assert!(fade > 0.0, "the decay starts at the release");

            cx.executor()
                .advance_clock(std::time::Duration::from_secs(1));
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let (_, fade) = captured.get();
            assert!(fade < 0.004, "the decay settles to rest: {fade}");
        }

        /// The policy is a policy: replacing it before init moves every
        /// composed control, which is the customization contract.
        #[gpui::test]
        fn a_replaced_size_policy_moves_the_composed_controls(cx: &mut TestAppContext) {
            cx.update(|cx| {
                SizeTokens::default().with_control_sm(px(30.)).set(cx);
                crate::init(cx);
            });
            let (_, cx) = cx.add_window_view(|_, _| MetricsProbe);
            let cx: &mut VisualTestContext = cx;
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let outlined = cx
                .debug_bounds("metrics-outlined")
                .expect("the outlined control should render");
            assert_eq!(outlined.size.height, px(30.));
        }
    }

    type CapturedNode = Arc<Mutex<Option<(Option<Role>, accesskit::Node)>>>;

    struct Probe {
        captured: CapturedNode,
    }

    impl Render for Probe {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let element = composed_button("probe", "Open pricing.md")
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element();
                    let role = element.a11y_role();
                    let mut node = accesskit::Node::new(Role::Unknown);
                    element.write_a11y_info(&mut node);
                    *captured.lock().expect("capture mutex should be available") =
                        Some((role, node));
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn composed_button_exposes_its_name_and_activation(cx: &mut TestAppContext) {
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Probe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let (role, node) = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("button node should be captured");
        assert_eq!(role, Some(Role::Button));
        assert_eq!(node.label(), Some("Open pricing.md"));
        assert!(node.supports_action(accesskit::Action::Click));
    }
}
