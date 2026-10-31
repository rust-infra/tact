//! Voice controls: dictation and speech as typed intent.
//!
//! gpui-ai does not capture audio or synthesize speech. [`VoiceControls`]
//! renders the dictate and speak controls for a [`VoiceState`] the
//! application owns, shows the live input level and interim transcript it
//! is handed, and reports every press as a [`VoiceEvent`] so the
//! application starts, stops, or cancels the real audio work.

use crate::{
    control::{outlined_control_with_icon, outlined_control_with_label},
    handlers::SharedHandler,
    motion::{MotionTokens, VisibleAnimationExt as _},
    surface::icon_button,
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, rems,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, spinner::Spinner, v_flex,
};
use std::rc::Rc;

/// Where the application's audio pipeline is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoiceState {
    /// Nothing is recording or playing.
    Idle,
    /// The microphone is open; `level` is the current input level (0–1).
    Listening {
        /// Normalized input level used for the meter.
        level: f32,
    },
    /// Audio stopped; speech-to-text is still finishing.
    Transcribing,
    /// Text-to-speech is playing.
    Speaking,
}

impl VoiceState {
    /// The short status word read to assistive technology.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Listening { .. } => "Listening",
            Self::Transcribing => "Transcribing",
            Self::Speaking => "Speaking",
        }
    }

    /// Whether the microphone is open.
    pub fn is_listening(self) -> bool {
        matches!(self, Self::Listening { .. })
    }
}

/// An interaction emitted by [`VoiceControls`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    /// The user asked to start dictating.
    DictationStarted,
    /// The user asked to stop and transcribe what was heard.
    DictationStopped,
    /// The user discarded the dictation in progress.
    DictationCancelled,
    /// The user asked for the latest response to be read aloud.
    SpeakRequested,
    /// The user stopped playback.
    SpeakStopped,
}

/// Dictate and speak controls with a live level meter and interim transcript.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// VoiceControls::new("voice", VoiceState::Listening { level: 0.6 })
///     .transcript("compare the three suppl…")
///     .speakable(true)
///     .on_event(|event, _, _| { /* VoiceEvent::DictationStopped … */ });
/// ```
#[derive(IntoElement)]
pub struct VoiceControls {
    id: SharedString,
    style: StyleRefinement,
    state: VoiceState,
    transcript: Option<SharedString>,
    speakable: bool,
    on_event: Option<SharedHandler<VoiceEvent>>,
}

impl VoiceControls {
    /// Creates the controls for the given state.
    pub fn new(id: impl Into<SharedString>, state: VoiceState) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            state,
            transcript: None,
            speakable: false,
            on_event: None,
        }
    }

    /// Shows the interim transcript under the controls.
    pub fn transcript(mut self, transcript: impl Into<SharedString>) -> Self {
        let transcript = transcript.into();
        self.transcript = (!transcript.is_empty()).then_some(transcript);
        self
    }

    /// Offers a Speak / Stop control for reading responses aloud.
    pub fn speakable(mut self, speakable: bool) -> Self {
        self.speakable = speakable;
        self
    }

    /// Handles typed interactions. Without a handler the controls are inert.
    pub fn on_event(
        mut self,
        handler: impl Fn(&VoiceEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for VoiceControls {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for VoiceControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = ElementId::from(self.id.clone());
        let debug_id = self.id.to_string();
        let handler = self.on_event;
        let state = self.state;

        let (dictate_name, dictate_label, dictate_event): (&str, &str, Option<VoiceEvent>) =
            match state {
                VoiceState::Idle | VoiceState::Speaking => (
                    "Start dictation",
                    "Dictate",
                    Some(VoiceEvent::DictationStarted),
                ),
                VoiceState::Listening { .. } => (
                    "Stop dictation",
                    "Listening",
                    Some(VoiceEvent::DictationStopped),
                ),
                VoiceState::Transcribing => ("Transcribing", "Transcribing", None),
            };
        let indicator: AnyElement = match state {
            VoiceState::Listening { level } => {
                let field_debug = debug_id.clone();
                div()
                    .debug_selector(move || format!("voice-signal-{field_debug}"))
                    .child(signal_field(
                        ElementId::from((root_id.clone(), "signal-field")),
                        level,
                        cx,
                    ))
                    .into_any_element()
            }
            VoiceState::Transcribing => Spinner::new().xsmall().into_any_element(),
            VoiceState::Idle | VoiceState::Speaking => div()
                .flex_none()
                .size_2()
                .rounded(tokens.radius.full)
                .bg(cx.theme().muted_foreground.opacity(0.6))
                .into_any_element(),
        };
        let dictate_debug = debug_id.clone();
        let dictate = {
            let handler = handler.clone();
            let event = dictate_event.clone();
            outlined_control_with_label(
                (root_id.clone(), "dictate"),
                dictate_name,
                dictate_label,
                window,
                cx,
            )
            .debug_selector(move || format!("voice-dictate-{dictate_debug}"))
            .disabled(dictate_event.is_none() || handler.is_none())
            .when(state.is_listening(), |this| {
                this.border_color(cx.theme().primary)
                    .text_color(cx.theme().primary)
            })
            .gap(tokens.spacing.xs)
            .child(indicator)
            .on_click(move |_: &ClickEvent, window, cx| {
                if let (Some(handler), Some(event)) = (&handler, &event) {
                    handler(event, window, cx)
                }
            })
        };
        // The visible label is the last child of the pill; put the indicator
        // before it by rebuilding the children order.
        let cancel = (state.is_listening() && handler.is_some()).then(|| {
            let handler = handler.clone();
            let cancel_debug = debug_id.clone();
            icon_button(
                (root_id.clone(), "cancel"),
                IconName::Close,
                "Cancel dictation",
                window,
                cx,
            )
            .debug_selector(move || format!("voice-cancel-{cancel_debug}"))
            .on_click(move |_: &ClickEvent, window, cx| {
                if let Some(handler) = &handler {
                    handler(&VoiceEvent::DictationCancelled, window, cx)
                }
            })
        });
        let speak = self.speakable.then(|| {
            let handler = handler.clone();
            let speak_debug = debug_id.clone();
            let speaking = state == VoiceState::Speaking;
            let (name, icon, event) = if speaking {
                ("Stop speaking", IconName::Pause, VoiceEvent::SpeakStopped)
            } else {
                (
                    "Read the latest response aloud",
                    IconName::Play,
                    VoiceEvent::SpeakRequested,
                )
            };
            // The library's own icon-and-label control, tinted while
            // speaking. It used to be hand-built with a padding-derived
            // height, so it was the one control in the crate that did not
            // move when an application replaced the size policy — and the
            // one without the shared press decay.
            outlined_control_with_icon(
                (root_id.clone(), "speak"),
                name,
                icon,
                if speaking { "Stop" } else { "Speak" },
                window,
                cx,
            )
            .debug_selector(move || format!("voice-speak-{speak_debug}"))
            .when(speaking, |button| {
                button
                    .border_color(cx.theme().primary)
                    .text_color(cx.theme().primary)
            })
            .disabled(handler.is_none())
            .on_click(move |_: &ClickEvent, window, cx| {
                if let Some(handler) = &handler {
                    handler(&event, window, cx)
                }
            })
        });
        let transcript_debug = debug_id.clone();
        let transcript = self.transcript.map(|text| {
            div()
                .id((root_id.clone(), "transcript"))
                .role(Role::Status)
                .aria_label(format!("Heard: {text}"))
                .debug_selector(move || format!("voice-transcript-{transcript_debug}"))
                .min_w_0()
                .truncate()
                .text_token(tokens.typography.sm)
                .italic()
                .text_color(cx.theme().muted_foreground)
                .child(text)
        });

        v_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(format!(
                "Voice controls, {}",
                state.label().to_ascii_lowercase()
            ))
            .debug_selector(move || format!("voice-{debug_id}"))
            .min_w_0()
            .gap(tokens.spacing.xs)
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.xs)
                    .child(dictate)
                    .children(cancel)
                    .children(speak),
            )
            .children(transcript)
            .refine_style(&self.style)
    }
}

/// Four bars that rise with the input level; heights stay in rems so the
/// meter scales with the UI.
/// The listening signal field: six lobes shaped by the controlled level,
/// pulsing on one shared clock.
///
/// Spike C's shipped outcome: plain layout primitives produce a clean
/// field with no shader, no overdraw pass, and no background job. Lobe
/// heights follow the application-owned level — sampled from the
/// controlled snapshot, never smoothed here — and a single phase-locked
/// animation drives a wave of opacity across the lobes, so the whole
/// field costs one scheduled clock. Reduced motion holds the phase at
/// zero, leaving a static graded envelope at the level's heights as the
/// meaningful mark; the status text remains the semantic carrier.
fn signal_field(id: ElementId, level: f32, cx: &App) -> impl IntoElement {
    let tokens = cx.theme().semantic_tokens();
    let color = cx.theme().primary;
    let level = level.clamp(0.0, 1.0);
    let gap = tokens.spacing.xxs;
    let radius = tokens.radius.full;
    let full = crate::motion::motion_is_full(cx);
    const WEIGHTS: [f32; 6] = [0.35, 0.65, 1.0, 0.85, 0.55, 0.3];
    h_flex()
        .flex_none()
        .items_end()
        .h(rems(0.9))
        .gap(gap)
        .with_visible_animation(
            id,
            // Frame demand: active while the state stays Listening — the
            // field is that state's indicator and settles by the state
            // changing, which unmounts it. Phase-locked to the shared
            // epoch, so a field remounted mid-session rejoins the beat.
            MotionTokens::read(cx).breathing().looping_synced(),
            move |field, delta| {
                let delta = if full { delta } else { 0.0 };
                field.children(WEIGHTS.iter().enumerate().map(|(index, weight)| {
                    let phase = index as f32 / WEIGHTS.len() as f32;
                    let wave = ((delta - phase).rem_euclid(1.0) * 2.0 - 1.0).abs();
                    div()
                        .w(rems(0.14))
                        .h(rems(0.25 + 0.65 * level * weight))
                        .rounded(radius)
                        .bg(color)
                        .opacity(0.35 + 0.65 * wave)
                }))
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, px};

    struct VoiceProbe {
        state: VoiceState,
    }

    impl Render for VoiceProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(420.))
                .h(px(160.))
                .child(VoiceControls::new("probe-voice", self.state).speakable(true))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn the_signal_field_exists_exactly_while_listening(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| VoiceProbe {
            state: VoiceState::Listening { level: 0.6 },
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        assert!(
            cx.debug_bounds("voice-signal-probe-voice").is_some(),
            "listening must show the signal field"
        );

        probe.update(cx, |probe, cx| {
            probe.state = VoiceState::Idle;
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.debug_bounds("voice-signal-probe-voice").is_none(),
            "leaving Listening must unmount the field, and its clock with it"
        );
    }

    #[gpui::test]
    fn reduced_motion_keeps_a_static_field(cx: &mut TestAppContext) {
        cx.update(crate::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (_, cx) = cx.add_window_view(|_, _| VoiceProbe {
            state: VoiceState::Listening { level: 0.6 },
        });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        assert!(
            cx.debug_bounds("voice-signal-probe-voice").is_some(),
            "reduced motion keeps the static envelope as the meaningful mark"
        );
    }

    #[test]
    fn states_describe_themselves() {
        assert_eq!(VoiceState::Listening { level: 0.2 }.label(), "Listening");
        assert!(VoiceState::Listening { level: 0.2 }.is_listening());
        assert!(!VoiceState::Transcribing.is_listening());
    }
}
